import AppKit
import Foundation
import Security

/// Checks GitHub Releases for a newer version and installs it in place.
///
/// A release is a `vX.Y.Z` tag with a `Feather-Scan-*.zip` asset holding the
/// notarized "Feather Scan.app". A download is only installed when it is signed
/// with a Developer ID from the same team as the running app, so builds signed
/// any other way (development, ad-hoc, a fork's own certificate) only point to
/// the download page.
///
/// Updates never post notifications. With automatic install on, an update is
/// installed once the scanner has been idle for a while; otherwise it waits in
/// the menu.
@MainActor
final class Updater: ObservableObject {
    static let shared = Updater()

    struct Release {
        var version: String
        var zipURL: URL
        var pageURL: URL
    }

    enum Phase: Equatable {
        case idle, checking, downloading, installing
    }

    @Published private(set) var available: Release?
    @Published private(set) var phase: Phase = .idle
    /// Result of a manual check, shown in the menu for a minute ("Up to date").
    @Published private(set) var note: String?
    @Published var checkAutomatically: Bool {
        didSet { UserDefaults.standard.set(checkAutomatically, forKey: Self.checkKey) }
    }
    @Published var installAutomatically: Bool {
        didSet {
            UserDefaults.standard.set(installAutomatically, forKey: Self.installKey)
            installIfIdle()
        }
    }

    /// Installing in place needs the running app to be Developer ID signed.
    let canInstall: Bool
    let currentVersion = Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "0"

    /// Whether a scan is running; set by the app model.
    var isBusy: () -> Bool = { false }
    /// When a scan last ran; updates wait a while after it.
    var lastBusy = Date.distantPast

    private static let checkKey = "checkForUpdates"
    private static let installKey = "installUpdates"
    private static let lastCheckKey = "lastUpdateCheck"
    /// The GitHub repo whose releases are checked ("owner/name"), from Info.plist.
    private static let repo = Bundle.main.object(forInfoDictionaryKey: "FeatherUpdateRepo") as? String ?? "aortbals/feather-scan"
    /// `defaults write app.featherscan.FeatherScan updateFeedURL <url>` points the check elsewhere, for testing.
    private static let api = UserDefaults.standard.string(forKey: "updateFeedURL").flatMap(URL.init(string:))
        ?? URL(string: "https://api.github.com/repos/\(repo)/releases/latest")!
    private static let checkInterval: TimeInterval = 24 * 60 * 60
    private static let idleDelay: TimeInterval = 2 * 60
    private let requirement: String?
    /// A version whose automatic install failed; it's left to the menu after that.
    private var failedVersion: String?
    private var timer: Timer?

    private init() {
        checkAutomatically = UserDefaults.standard.object(forKey: Self.checkKey) as? Bool ?? true
        installAutomatically = UserDefaults.standard.object(forKey: Self.installKey) as? Bool ?? true
        requirement = Self.ownRequirement()
        canInstall = requirement != nil
    }

    /// Checks shortly after launch, then about once a day; installs when idle.
    func start() {
        timer = Timer.scheduledTimer(withTimeInterval: 60, repeats: true) { _ in
            Task { @MainActor in
                Updater.shared.checkIfDue()
                Updater.shared.installIfIdle()
            }
        }
        DispatchQueue.main.asyncAfter(deadline: .now() + 15) { self.checkIfDue() }
    }

    private func checkIfDue() {
        let last = UserDefaults.standard.double(forKey: Self.lastCheckKey)
        guard checkAutomatically, Date().timeIntervalSince1970 - last > Self.checkInterval else { return }
        Task { await check(manual: false) }
    }

    /// Looks up the latest release. A manual check also shows its result in the menu.
    func check(manual: Bool) async {
        guard phase == .idle else { return }
        phase = .checking
        note = nil
        do {
            var request = URLRequest(url: Self.api)
            request.setValue("application/vnd.github+json", forHTTPHeaderField: "Accept")
            request.setValue("FeatherScan/\(currentVersion)", forHTTPHeaderField: "User-Agent")
            let (data, response) = try await URLSession.shared.data(for: request)
            guard (response as? HTTPURLResponse)?.statusCode == 200 else { throw UpdateError("GitHub returned an error") }
            let release = try JSONDecoder().decode(GitHubRelease.self, from: data)
            UserDefaults.standard.set(Date().timeIntervalSince1970, forKey: Self.lastCheckKey)

            let version = release.tagName.hasPrefix("v") ? String(release.tagName.dropFirst()) : release.tagName
            if Self.isNewer(version, than: currentVersion),
               let asset = release.assets.first(where: { $0.name.hasPrefix("Feather-Scan") && $0.name.hasSuffix(".zip") }) {
                available = Release(version: version, zipURL: asset.browserDownloadUrl, pageURL: release.htmlUrl)
                appLog("update available: \(version)")
            } else {
                available = nil
                if manual { show("Feather Scan Is Up to Date") }
            }
        } catch {
            appLog("update check failed: \(error)")
            if manual { show("Couldn't Check for Updates") }
        }
        phase = .idle
        installIfIdle()
    }

    private func show(_ text: String) {
        note = text
        DispatchQueue.main.asyncAfter(deadline: .now() + 60) { if self.note == text { self.note = nil } }
    }

    /// Installs a waiting update when automatic install is on and no scan has run for a while.
    private func installIfIdle() {
        guard installAutomatically, canInstall, phase == .idle, let release = available,
              release.version != failedVersion, !isBusy(),
              Date().timeIntervalSince(lastBusy) > Self.idleDelay
        else { return }
        install(automatic: true)
    }

    /// Downloads, verifies and installs the available update, then relaunches.
    func install(automatic: Bool = false) {
        guard let release = available, phase == .idle else { return }
        guard canInstall else { NSWorkspace.shared.open(release.pageURL); return }
        phase = .downloading
        Task {
            do {
                let app = Bundle.main.bundleURL
                // A scratch folder on the app's volume, so the final swap is a rename.
                let work = try FileManager.default.url(for: .itemReplacementDirectory, in: .userDomainMask,
                                                       appropriateFor: app, create: true)
                defer { try? FileManager.default.removeItem(at: work) }

                let (download, _) = try await URLSession.shared.download(from: release.zipURL)
                let zip = work.appendingPathComponent("update.zip")
                try FileManager.default.moveItem(at: download, to: zip)

                phase = .installing
                try await Self.run("/usr/bin/ditto", ["-x", "-k", zip.path, work.path])
                let newApp = work.appendingPathComponent("Feather Scan.app")
                try verify(newApp, version: release.version)

                // A scan may have started during the download; try again once it's done.
                if automatic, isBusy() {
                    phase = .idle
                    return
                }
                _ = try FileManager.default.replaceItemAt(app, withItemAt: newApp)
                appLog("updated \(currentVersion) → \(release.version)\(automatic ? " automatically" : "")")
                relaunch(app)
            } catch {
                appLog("update failed: \(error)")
                phase = .idle
                if automatic {
                    failedVersion = release.version
                } else {
                    NSWorkspace.shared.open(release.pageURL)
                }
            }
        }
    }

    /// The new app must carry a valid Developer ID signature from our team, and the expected version.
    private func verify(_ app: URL, version: String) throws {
        guard let requirement else { throw UpdateError("This copy can't verify updates.") }
        var code: SecStaticCode?
        var req: SecRequirement?
        guard SecStaticCodeCreateWithPath(app as CFURL, [], &code) == errSecSuccess, let code,
              SecRequirementCreateWithString(requirement as CFString, [], &req) == errSecSuccess, let req
        else { throw UpdateError("The download isn't a Feather Scan app.") }
        let flags = SecCSFlags(rawValue: kSecCSCheckAllArchitectures | kSecCSStrictValidate | kSecCSCheckNestedCode)
        guard SecStaticCodeCheckValidity(code, flags, req) == errSecSuccess else {
            throw UpdateError("The download's signature doesn't match.")
        }
        let info = NSDictionary(contentsOf: app.appendingPathComponent("Contents/Info.plist"))
        guard info?["CFBundleShortVersionString"] as? String == version else {
            throw UpdateError("The download has an unexpected version.")
        }
    }

    /// Opens the app again once this process has exited.
    private func relaunch(_ app: URL) {
        let pid = ProcessInfo.processInfo.processIdentifier
        let p = Process()
        p.executableURL = URL(fileURLWithPath: "/bin/sh")
        p.arguments = ["-c", "while kill -0 \(pid) 2>/dev/null; do sleep 0.2; done; /usr/bin/open \"$0\"", app.path]
        try? p.run()
        NSApp.terminate(nil)
    }

    /// The code requirement an update must meet: Developer ID, our bundle ID and team.
    /// Nil when the running app isn't Developer ID signed itself.
    private static func ownRequirement() -> String? {
        var me: SecCode?
        var staticMe: SecStaticCode?
        var info: CFDictionary?
        guard SecCodeCopySelf([], &me) == errSecSuccess, let me,
              SecCodeCopyStaticCode(me, [], &staticMe) == errSecSuccess, let staticMe,
              SecCodeCopySigningInformation(staticMe, SecCSFlags(rawValue: kSecCSSigningInformation), &info) == errSecSuccess,
              let dict = info as? [String: Any],
              let team = dict[kSecCodeInfoTeamIdentifier as String] as? String,
              let id = Bundle.main.bundleIdentifier
        else { return nil }
        let text = """
        anchor apple generic and identifier "\(id)" \
        and certificate 1[field.1.2.840.113635.100.6.2.6] exists \
        and certificate leaf[field.1.2.840.113635.100.6.1.13] exists \
        and certificate leaf[subject.OU] = "\(team)"
        """
        var req: SecRequirement?
        guard SecRequirementCreateWithString(text as CFString, [], &req) == errSecSuccess, let req,
              SecStaticCodeCheckValidity(staticMe, [], req) == errSecSuccess
        else { return nil }
        return text
    }

    /// Compares dotted versions numerically ("0.1.10" > "0.1.9", "0.2" > "0.1.1").
    nonisolated static func isNewer(_ a: String, than b: String) -> Bool {
        let x = a.split(separator: ".").map { Int($0) ?? 0 }
        let y = b.split(separator: ".").map { Int($0) ?? 0 }
        for i in 0..<max(x.count, y.count) {
            let l = i < x.count ? x[i] : 0, r = i < y.count ? y[i] : 0
            if l != r { return l > r }
        }
        return false
    }

    private static func run(_ tool: String, _ args: [String]) async throws {
        try await withCheckedThrowingContinuation { (c: CheckedContinuation<Void, Error>) in
            let p = Process()
            p.executableURL = URL(fileURLWithPath: tool)
            p.arguments = args
            p.terminationHandler = { p in
                p.terminationStatus == 0 ? c.resume() : c.resume(throwing: UpdateError("Unpacking the download failed."))
            }
            do { try p.run() } catch { c.resume(throwing: error) }
        }
    }
}

private struct GitHubRelease: Decodable {
    struct Asset: Decodable {
        var name: String
        var browserDownloadUrl: URL
        enum CodingKeys: String, CodingKey { case name, browserDownloadUrl = "browser_download_url" }
    }
    var tagName: String
    var htmlUrl: URL
    var assets: [Asset]
    enum CodingKeys: String, CodingKey { case tagName = "tag_name", htmlUrl = "html_url", assets }
}

struct UpdateError: Error, CustomStringConvertible {
    var description: String
    init(_ d: String) { description = d }
}
