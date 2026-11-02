import AppKit
import Foundation
import ServiceManagement
import UserNotifications

struct SavedScan: Identifiable, Codable, Equatable {
    var id = UUID()
    var path: String
    var title: String
    var pages: Int
    var date = Date()
}

@MainActor
final class AppModel: ObservableObject {
    enum State: String {
        case searching, connecting, ready, scanning, notPaired = "not_paired", offline, stopped
    }

    @Published private(set) var state: State = .searching
    @Published private(set) var message = "Starting…"
    @Published private(set) var scannerName: String?
    @Published private(set) var activity: String?
    @Published private(set) var recent: [SavedScan] = []
    @Published var config: CoreConfig

    private var core: CoreBridge!
    private let notifier = Notifier()

    init() {
        config = CoreConfig.load()
        if let data = UserDefaults.standard.data(forKey: "recent"),
           let r = try? JSONDecoder().decode([SavedScan].self, from: data) {
            recent = r
        }
        core = CoreBridge { [weak self] event in self?.handle(event) }
        notifier.setUp()
        if !core.start(configJSON: config.json(apiKey: Keychain.apiKey())) {
            state = .offline
            message = "Settings are invalid; open Settings to fix them."
        }
    }

    @Published private(set) var menuBarIcon = FeatherIcon.still(dimmed: true)
    private var iconTimer: Timer?
    private var iconFrame = 0

    /// A scan is feeding paper or its PDF is being made.
    private var isWorking: Bool { state == .scanning || activity != nil }

    /// Animates the feather while a scan runs; dims it while the scanner is unreachable.
    private func updateIcon() {
        if isWorking {
            guard iconTimer == nil else { return }
            iconTimer = Timer.scheduledTimer(withTimeInterval: 1.0 / 12, repeats: true) { [weak self] _ in
                MainActor.assumeIsolated {
                    guard let self else { return }
                    let frames = FeatherIcon.scanningFrames
                    self.iconFrame = (self.iconFrame + 1) % frames.count
                    self.menuBarIcon = frames[self.iconFrame]
                }
            }
        } else {
            iconTimer?.invalidate()
            iconTimer = nil
            iconFrame = 0
            menuBarIcon = FeatherIcon.still(dimmed: state != .ready)
        }
    }

    var isReady: Bool { state == .ready }

    func scanNow() { core.scanNow() }

    /// Pairs this computer with the scanner without ScanSnap Home.
    func pair() {
        isPairing = true
        core.pair()
    }

    @Published private(set) var isPairing = false

    /// Saves settings and hands them to the running core.
    func apply(_ newConfig: CoreConfig, apiKey: String?) {
        config = newConfig
        config.save()
        Keychain.setAPIKey(apiKey)
        core.setConfig(config.json(apiKey: apiKey))
    }

    func openOutputFolder() {
        let url = URL(fileURLWithPath: config.outputDir)
        try? FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
        NSWorkspace.shared.open(url)
    }

    func open(_ scan: SavedScan) {
        NSWorkspace.shared.open(URL(fileURLWithPath: scan.path))
    }

    func reveal(_ scan: SavedScan) {
        NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: scan.path)])
    }

    var launchAtLogin: Bool {
        get { SMAppService.mainApp.status == .enabled }
        set {
            do {
                if newValue { try SMAppService.mainApp.register() } else { try SMAppService.mainApp.unregister() }
            } catch {
                NSLog("Feather Scan: launch at login: \(error)")
            }
            objectWillChange.send()
        }
    }

    private func handle(_ e: [String: Any]) {
        defer { updateIcon() }
        switch e["type"] as? String {
        case "status":
            state = State(rawValue: e["state"] as? String ?? "") ?? state
            message = e["message"] as? String ?? ""
            if let s = e["scanner"] as? [String: Any], let name = s["name"] as? String { scannerName = name }
            if state == .ready || state == .offline { activity = activity?.hasPrefix("Saving") == true ? activity : nil }
        case "scan_started":
            activity = "Scanning…"
        case "page":
            let n = e["sheets"] as? Int ?? 0
            activity = "Scanning… \(n) sheet\(n == 1 ? "" : "s")"
        case "processing":
            activity = "Saving PDF…"
        case "saved":
            activity = nil
            let scan = SavedScan(path: e["path"] as? String ?? "", title: e["title"] as? String ?? "Scan",
                                 pages: e["pages"] as? Int ?? 0)
            recent.insert(scan, at: 0)
            recent = Array(recent.prefix(8))
            if let data = try? JSONEncoder().encode(recent) { UserDefaults.standard.set(data, forKey: "recent") }
            var body = "\(scan.pages) page\(scan.pages == 1 ? "" : "s")"
            if let err = e["title_error"] as? String { body += " · no smart title (\(err))" }
            notifier.post(title: scan.title, body: body, path: scan.path)
        case "paired":
            isPairing = false
            notifier.post(title: "Paired with the scanner", body: e["message"] as? String ?? "", path: nil)
        case "error":
            activity = nil
            isPairing = false
            notifier.post(title: "Feather Scan", body: e["message"] as? String ?? "Something went wrong", path: nil)
        case "identity":
            if let raw = e["identity"],
               let data = try? JSONSerialization.data(withJSONObject: raw),
               let id = try? CoreConfig.decoder.decode(CoreConfig.Identity.self, from: data) {
                config.identity = id
                config.save()
            }
        default:
            break
        }
    }
}

/// Posts "scan saved" notifications; clicking one opens the PDF.
final class Notifier: NSObject, UNUserNotificationCenterDelegate {
    func setUp() {
        let c = UNUserNotificationCenter.current()
        c.delegate = self
        c.requestAuthorization(options: [.alert, .sound]) { _, _ in }
    }

    func post(title: String, body: String, path: String?) {
        let content = UNMutableNotificationContent()
        content.title = title
        content.body = body
        if let path { content.userInfo = ["path": path] }
        UNUserNotificationCenter.current().add(UNNotificationRequest(identifier: UUID().uuidString, content: content, trigger: nil))
    }

    func userNotificationCenter(_ center: UNUserNotificationCenter, didReceive response: UNNotificationResponse,
                                withCompletionHandler done: @escaping () -> Void) {
        if let path = response.notification.request.content.userInfo["path"] as? String {
            NSWorkspace.shared.open(URL(fileURLWithPath: path))
        }
        done()
    }

    func userNotificationCenter(_ center: UNUserNotificationCenter, willPresent notification: UNNotification,
                                withCompletionHandler done: @escaping (UNNotificationPresentationOptions) -> Void) {
        done([.banner, .sound])
    }
}
