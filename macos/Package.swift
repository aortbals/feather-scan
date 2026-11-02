// swift-tools-version:5.10
import PackageDescription

let package = Package(
    name: "FeatherScan",
    platforms: [.macOS(.v14)],
    targets: [
        .target(
            name: "CFeather",
            path: "Sources/CFeather",
            linkerSettings: [
                .unsafeFlags(["-L../target/release"]),
                .linkedLibrary("feather_core"),
                .linkedLibrary("iconv"),
                .linkedFramework("Security"),
                .linkedFramework("Vision"),
                .linkedFramework("ImageIO"),
                .linkedFramework("CoreFoundation"),
                .linkedFramework("Foundation"),
            ]
        ),
        .executableTarget(
            name: "FeatherScan",
            dependencies: ["CFeather"],
            path: "Sources/FeatherScan",
            swiftSettings: [.unsafeFlags(["-Osize"])],
            linkerSettings: [.unsafeFlags(["-Xlinker", "-dead_strip"])]
        ),
    ]
)
