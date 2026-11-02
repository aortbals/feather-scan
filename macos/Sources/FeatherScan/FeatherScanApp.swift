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
    @Environment(\.openSettings) private var openSettings

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
        Button("Settings…") { showSettings() }
            .keyboardShortcut(",")
        Button("Quit Feather Scan") { NSApplication.shared.terminate(nil) }
            .keyboardShortcut("q")
    }
}

extension MenuContent {
    /// A menu bar app isn't active when its menu is used, so a newly opened (or
    /// already open) Settings window would sit behind other apps' windows.
    /// Open it, then bring the app and the window to the front once the menu has closed.
    func showSettings() {
        openSettings()
        DispatchQueue.main.async {
            NSApp.activate()
            let window = NSApp.windows.first { $0.identifier?.rawValue == "com_apple_SwiftUI_Settings_window" }
            window?.makeKeyAndOrderFront(nil)
            window?.orderFrontRegardless()
        }
    }
}
