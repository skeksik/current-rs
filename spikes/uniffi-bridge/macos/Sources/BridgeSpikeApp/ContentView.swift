import SwiftUI

struct ContentView: View {
    let models: Models

    var body: some View {
        Form {
            ValueSection(model: models.value)
            ItemsSection(model: models.items)
            OperationSection(model: models.operation)
        }
        .formStyle(.grouped)
        .frame(minWidth: 560, minHeight: 640)
    }
}

struct ValueSection: View {
    let model: ValueModel

    var body: some View {
        Section("Observable value") {
            Text("\(model.count)").font(.largeTitle).monospacedDigit()
            Text(model.threads).font(.caption).foregroundStyle(.secondary)
            HStack {
                Button("Increment", action: model.increment)
                Button("Start ticker", action: model.startTicker)
                Button("Stop ticker", action: model.stopTicker)
            }
            Toggle(
                "Subscribed",
                isOn: Binding(get: { model.isSubscribed }, set: { $0 ? model.subscribe() : model.unsubscribe() }))
        }
    }
}

struct ItemsSection: View {
    let model: ItemsModel

    var body: some View {
        Section("Observable list (diffs): \(model.batches) batches applied") {
            HStack {
                Button("Step", action: model.step)
                Button("Start feed", action: model.startFeed)
                Button("Stop feed", action: model.stopFeed)
            }
            List(model.rows) { row in Text(row.title) }
                .frame(height: 220)
                .animation(.default, value: model.rows)
        }
    }
}

struct OperationSection: View {
    @Bindable var model: OperationModel

    var body: some View {
        Section("Async call with cancellation") {
            TextField("Text to reverse (3 s)", text: $model.input)
            HStack {
                Button("Run", action: model.run).keyboardShortcut(.defaultAction)
                Button("Cancel", action: model.cancel).disabled(!model.isRunning)
                Button("Run with empty text", action: model.runEmpty)
                Button("Run and abandon", action: model.runAndAbandon)
            }
            Text(model.result)
            Text(model.coreState).font(.caption).foregroundStyle(.secondary)
        }
    }
}
