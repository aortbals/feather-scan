import CFeather
import Foundation

/// Thin wrapper over the Rust core's C API. Events arrive on a background
/// thread and are handed to `onEvent` on the main thread.
final class CoreBridge {
    private var handle: UnsafeMutableRawPointer?
    private let onEvent: ([String: Any]) -> Void

    init(onEvent: @escaping ([String: Any]) -> Void) {
        self.onEvent = onEvent
    }

    deinit { stop() }

    func start(configJSON: String) -> Bool {
        stop()
        let ctx = Unmanaged.passUnretained(self).toOpaque()
        handle = configJSON.withCString { feather_start($0, coreEventCallback, ctx) }
        return handle != nil
    }

    func setConfig(_ json: String) {
        guard let handle else { return }
        _ = json.withCString { feather_set_config(handle, $0) }
    }

    func scanNow() {
        guard let handle else { return }
        feather_scan_now(handle)
    }

    func stop() {
        guard let h = handle else { return }
        handle = nil
        feather_stop(h)
    }

    fileprivate func deliver(_ json: String) {
        guard let data = json.data(using: .utf8),
              let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else { return }
        DispatchQueue.main.async { [onEvent] in onEvent(obj) }
    }

    static func defaultConfig() -> CoreConfig {
        guard let p = feather_default_config() else { fatalError("core returned no default config") }
        defer { feather_free_string(p) }
        let data = Data(String(cString: p).utf8)
        return try! CoreConfig.decoder.decode(CoreConfig.self, from: data)
    }

    struct Found: Decodable, Identifiable {
        var ip: String
        var name: String
        var model: String
        var id: String { ip }
    }

    /// Blocking; call off the main thread.
    static func discover(ip: String? = nil, timeoutMs: UInt32 = 2000) -> [Found] {
        let p: UnsafeMutablePointer<CChar>? = if let ip, !ip.isEmpty {
            ip.withCString { feather_discover($0, timeoutMs) }
        } else {
            feather_discover(nil, timeoutMs)
        }
        guard let p else { return [] }
        defer { feather_free_string(p) }
        return (try? JSONDecoder().decode([Found].self, from: Data(String(cString: p).utf8))) ?? []
    }
}

private func coreEventCallback(_ json: UnsafePointer<CChar>?, _ ctx: UnsafeMutableRawPointer?) {
    guard let json, let ctx else { return }
    Unmanaged<CoreBridge>.fromOpaque(ctx).takeUnretainedValue().deliver(String(cString: json))
}
