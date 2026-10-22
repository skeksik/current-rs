// Observable lists from the core (ADR 0004): batches of diffs, applied in order on the main actor. Each exported list gets a
// generated diff enum; its listener conformance maps it to `ListDiff` and calls `ListForwarder.forward`.

import CurrentFFIBindings
import Observation

/// One change to a list. Indices refer to the list after the diffs before it in the same batch.
public enum ListDiff<Item: Sendable>: Sendable {
    case append([Item])
    case insert(index: Int, item: Item)
    case set(index: Int, item: Item)
    case remove(index: Int)
    case reset([Item])
}

/// A diff whose index does not exist in the list: the mirror is out of step with the core.
public struct ListDiffOutOfRange: Error, Equatable {
    public let index: Int
    public let count: Int
}

extension ListDiff {
    /// Applies the diff to a mirror of the list. On error the list is unchanged.
    public func apply(to items: inout [Item]) throws(ListDiffOutOfRange) {
        switch self {
        case .append(let values):
            items.append(contentsOf: values)
        case .insert(let index, let item):
            guard index >= 0, index <= items.count else { throw ListDiffOutOfRange(index: index, count: items.count) }
            items.insert(item, at: index)
        case .set(let index, let item):
            guard items.indices.contains(index) else { throw ListDiffOutOfRange(index: index, count: items.count) }
            items[index] = item
        case .remove(let index):
            guard items.indices.contains(index) else { throw ListDiffOutOfRange(index: index, count: items.count) }
            items.remove(at: index)
        case .reset(let values):
            items = values
        }
    }
}

/// The listener the core holds for a list: forwards each batch into an AsyncStream, keeping nothing else.
public final class ListForwarder<Item: Sendable>: Sendable {
    private let continuation: AsyncStream<[ListDiff<Item>]>.Continuation

    init(_ continuation: AsyncStream<[ListDiff<Item>]>.Continuation) {
        self.continuation = continuation
    }

    /// Called on a core worker thread: hands the batch on and returns.
    public func forward(_ batch: [ListDiff<Item>]) {
        continuation.yield(batch)
    }

    deinit { continuation.finish() }
}

/// A list the core owns, mirrored on the main actor. Every batch is kept (unlike values, diffs cannot be skipped).
@MainActor
@Observable
public final class ObservedList<Item: Sendable> {
    public private(set) var items: [Item] = []

    private let handle: TaskHandle
    private let pump: Task<Void, Never>

    public init(subscribe: (ListForwarder<Item>) -> TaskHandle) {
        let (stream, continuation) = AsyncStream.makeStream(of: [ListDiff<Item>].self)
        let target = WeakTarget<ObservedList<Item>>()
        handle = subscribe(ListForwarder(continuation))
        pump = Task { @MainActor in
            for await batch in stream {
                target.object?.apply(batch)
            }
        }
        target.object = self
    }

    /// Applies a batch in order. A diff out of range means the mirror is broken: it stops applying the batch and waits for the next
    /// reset rather than crashing. The core never sends one; a test would catch it first.
    func apply(_ batch: [ListDiff<Item>]) {
        var next = items
        for diff in batch {
            do throws(ListDiffOutOfRange) {
                try diff.apply(to: &next)
            } catch {
                assertionFailure("list diff out of range: \(error)")
                return
            }
        }
        items = next
    }

    public func cancel() {
        handle.cancel()
        pump.cancel()
    }

    deinit {
        handle.cancel()
        pump.cancel()
    }
}
