import AppKit
import BridgeSpikeBindings
import SwiftUI

@main
struct BridgeSpikeApp: App {
    // A bare executable (swift run) is not an app bundle: make it a regular app so it gets a Dock icon and focus.
    @NSApplicationDelegateAdaptor private var delegate: AppDelegate

    var body: some Scene {
        WindowGroup("Bridge spike") {
            RootView()
        }
    }
}

final class AppDelegate: NSObject, NSApplicationDelegate {
    func applicationDidFinishLaunching(_ notification: Notification) {
        NSApp.setActivationPolicy(.regular)
        NSApp.activate()
    }
}

/// Creates the core once. Starting its runtime can fail with BridgeError.Runtime.
struct RootView: View {
    @State private var models = Result { try Models() }

    var body: some View {
        switch models {
        case .success(let models): ContentView(models: models)
        case .failure(let error): Text("Core failed to start: \(error.localizedDescription)").padding()
        }
    }
}

@MainActor
final class Models {
    let value: ValueModel
    let items: ItemsModel
    let operation: OperationModel

    init() throws {
        let core = try Core()
        value = ValueModel(core: core)
        items = ItemsModel(core: core)
        operation = OperationModel(core: core)
    }
}
