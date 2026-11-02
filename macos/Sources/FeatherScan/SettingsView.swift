import AppKit
import SwiftUI

struct SettingsView: View {
    @ObservedObject var model: AppModel

    var body: some View {
        TabView {
            GeneralSettings(model: model)
                .tabItem { Label("General", systemImage: "gearshape") }
            TitleSettings(model: model)
                .tabItem { Label("Smart Titles", systemImage: "sparkles") }
            ScannerSettings(model: model)
                .tabItem { Label("Scanner", systemImage: "scanner") }
        }
        .frame(width: 500)
        .onAppear { NSApp.activate(ignoringOtherApps: true) }
    }
}

private struct GeneralSettings: View {
    @ObservedObject var model: AppModel
    @State private var launchAtLogin = false

    var body: some View {
        Form {
            LabeledContent("Save scans to") {
                HStack {
                    Text(abbreviate(model.config.outputDir))
                        .lineLimit(1)
                        .truncationMode(.middle)
                        .foregroundStyle(.secondary)
                    Button("Choose…", action: chooseFolder)
                }
            }
            Toggle("Remove blank back sides", isOn: binding(\.removeBlankPages))
            Toggle("Clean up page images", isOn: cleanBinding)
                .help("Straighten, crop and whiten pages for crisper text. Off keeps the scanner's original images.")
            Toggle("Open at login", isOn: $launchAtLogin)
                .onChange(of: launchAtLogin) { _, v in model.launchAtLogin = v }
        }
        .formStyle(.grouped)
        .onAppear { launchAtLogin = model.launchAtLogin }
    }

    private func chooseFolder() {
        let panel = NSOpenPanel()
        panel.canChooseDirectories = true
        panel.canChooseFiles = false
        panel.canCreateDirectories = true
        panel.directoryURL = URL(fileURLWithPath: model.config.outputDir)
        if panel.runModal() == .OK, let url = panel.url {
            var c = model.config
            c.outputDir = url.path
            model.apply(c, apiKey: Keychain.apiKey())
        }
    }

    private var cleanBinding: Binding<Bool> {
        Binding(get: { model.config.cleanPages ?? true }, set: { v in
            var c = model.config
            c.cleanPages = v
            model.apply(c, apiKey: Keychain.apiKey())
        })
    }

    private func binding(_ path: WritableKeyPath<CoreConfig, Bool>) -> Binding<Bool> {
        Binding(get: { model.config[keyPath: path] }, set: { v in
            var c = model.config
            c[keyPath: path] = v
            model.apply(c, apiKey: Keychain.apiKey())
        })
    }
}

private struct TitleSettings: View {
    @ObservedObject var model: AppModel
    @State private var enabled = true
    @State private var baseURL = ""
    @State private var model_ = ""
    @State private var apiKey = ""

    var body: some View {
        Form {
            Toggle("Name scans with an LLM", isOn: $enabled)
            TextField("API base URL", text: $baseURL, prompt: Text("https://api.openai.com/v1"))
            SecureField("API key", text: $apiKey, prompt: Text("sk-…"))
            TextField("Model", text: $model_)
            Text("Any OpenAI-compatible endpoint works. Recognized text from each scan is sent to it. The key is stored in your Keychain.")
                .font(.caption)
                .foregroundStyle(.secondary)
            HStack {
                Spacer()
                Button("Save", action: save).keyboardShortcut(.defaultAction)
            }
        }
        .formStyle(.grouped)
        .onAppear {
            enabled = model.config.llm.enabled
            baseURL = model.config.llm.baseUrl
            model_ = model.config.llm.model
            apiKey = Keychain.apiKey() ?? ""
        }
    }

    private func save() {
        var c = model.config
        c.llm.enabled = enabled
        c.llm.baseUrl = baseURL.trimmingCharacters(in: .whitespaces).isEmpty
            ? "https://api.openai.com/v1" : baseURL.trimmingCharacters(in: .whitespaces)
        c.llm.model = model_.trimmingCharacters(in: .whitespaces)
        model.apply(c, apiKey: apiKey.trimmingCharacters(in: .whitespacesAndNewlines))
    }
}

private struct ScannerSettings: View {
    @ObservedObject var model: AppModel
    @State private var ip = ""
    @State private var found: [CoreBridge.Found] = []
    @State private var searching = false

    var body: some View {
        Form {
            LabeledContent("Status", value: model.message)
            HStack {
                TextField("Scanner IP", text: $ip, prompt: Text("Automatic"))
                Button(searching ? "Searching…" : "Find") { find() }.disabled(searching)
                Button("Use") { saveIP() }
            }
            ForEach(found) { s in
                Button("\(s.name) — \(s.ip)") { ip = s.ip; saveIP() }
                    .buttonStyle(.link)
            }
            Picker("Resolution", selection: dpiBinding) {
                Text("150 dpi").tag(150)
                Text("200 dpi").tag(200)
                Text("300 dpi").tag(300)
                Text("600 dpi").tag(600)
            }
            Toggle("Scan both sides", isOn: duplexBinding)
            Toggle("Use the settings of the profile tapped on the scanner", isOn: panelBinding)
            if let id = model.config.identity {
                LabeledContent("Paired as", value: id.userName)
            }
            Text("Quit ScanSnap Home while using Feather Scan: only one app can hold the scanner. Feather Scan uses this computer's existing ScanSnap pairing, so pick this computer on the scanner's screen.")
                .font(.caption)
                .foregroundStyle(.secondary)
        }
        .formStyle(.grouped)
        .onAppear { ip = model.config.scannerIp ?? "" }
    }

    private var dpiBinding: Binding<Int> {
        Binding(get: { model.config.scan.dpi }, set: { v in update { $0.scan.dpi = v } })
    }

    private var duplexBinding: Binding<Bool> {
        Binding(get: { model.config.scan.duplex }, set: { v in update { $0.scan.duplex = v } })
    }

    private var panelBinding: Binding<Bool> {
        Binding(get: { model.config.usePanelSettings }, set: { v in update { $0.usePanelSettings = v } })
    }

    private func update(_ f: (inout CoreConfig) -> Void) {
        var c = model.config
        f(&c)
        model.apply(c, apiKey: Keychain.apiKey())
    }

    private func saveIP() {
        let v = ip.trimmingCharacters(in: .whitespaces)
        update { $0.scannerIp = v.isEmpty ? nil : v }
    }

    private func find() {
        searching = true
        let target = ip
        Task.detached {
            let r = CoreBridge.discover(ip: target.isEmpty ? nil : target)
            await MainActor.run {
                found = r
                searching = false
            }
        }
    }
}

private func abbreviate(_ path: String) -> String {
    let home = NSHomeDirectory()
    return path.hasPrefix(home) ? "~" + path.dropFirst(home.count) : path
}
