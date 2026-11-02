import Foundation

/// Mirrors `feather_core::config::Config` (JSON, snake_case on the wire).
struct CoreConfig: Codable, Equatable {
    var outputDir: String
    var scannerIp: String?
    var scan: Scan
    var usePanelSettings: Bool
    var removeBlankPages: Bool
    var llm: LLM
    var identity: Identity?

    struct Scan: Codable, Equatable {
        var dpi: Int
        var duplex: Bool
        var color: String
    }

    struct LLM: Codable, Equatable {
        var enabled: Bool
        var baseUrl: String
        var apiKey: String?
        var model: String
    }

    struct Identity: Codable, Equatable {
        var hostId: String
        var userId: String
        var userName: String
    }

    static let defaultsKey = "coreConfig"

    static var encoder: JSONEncoder {
        let e = JSONEncoder()
        e.keyEncodingStrategy = .convertToSnakeCase
        return e
    }

    static var decoder: JSONDecoder {
        let d = JSONDecoder()
        d.keyDecodingStrategy = .convertFromSnakeCase
        return d
    }

    /// Saved settings, or the core's defaults on first run. The API key lives in the Keychain.
    static func load() -> CoreConfig {
        if let data = UserDefaults.standard.data(forKey: defaultsKey),
           let cfg = try? decoder.decode(CoreConfig.self, from: data) {
            return cfg
        }
        return CoreBridge.defaultConfig()
    }

    func save() {
        var stored = self
        stored.llm.apiKey = nil
        if let data = try? Self.encoder.encode(stored) {
            UserDefaults.standard.set(data, forKey: Self.defaultsKey)
        }
    }

    /// JSON for the core, with the API key filled in from the Keychain.
    func json(apiKey: String?) -> String {
        var c = self
        c.llm.apiKey = apiKey
        let data = (try? Self.encoder.encode(c)) ?? Data("{}".utf8)
        return String(decoding: data, as: UTF8.self)
    }
}
