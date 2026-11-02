import SwiftUI

@main
struct FeatherScanApp: App {
    @StateObject private var model = AppModel()

    var body: some Scene {
        MenuBarExtra {
            MenuContent(model: model)
        } label: {
            Image(nsImage: model.menuBarIcon)
        }
        .menuBarExtraStyle(.menu)

        Settings {
            SettingsView(model: model)
        }
    }
}

struct MenuContent: View {
    @ObservedObject var model: AppModel

    var body: some View {
        Text(model.activity ?? model.message)
        if let name = model.scannerName, model.isReady {
            Text(name).font(.caption)
        }
        Divider()
        Button("Scan Now") { model.scanNow() }
            .keyboardShortcut("s")
            .disabled(!model.isReady)
        Button("Open Scans Folder") { model.openOutputFolder() }
            .keyboardShortcut("o")
        if !model.recent.isEmpty {
            Menu("Recent Scans") {
                ForEach(model.recent) { scan in
                    Button(scan.title) { model.open(scan) }
                }
                if let last = model.recent.first {
                    Divider()
                    Button("Show Latest in Finder") { model.reveal(last) }
                }
            }
        }
        Divider()
        SettingsLink { Text("Settings…") }
            .keyboardShortcut(",")
        Button("Quit Feather Scan") { NSApplication.shared.terminate(nil) }
            .keyboardShortcut("q")
    }
}
