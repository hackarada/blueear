// swift-tools-version: 5.10
import PackageDescription

let package = Package(
    name: "BlueEarAudio",
    platforms: [.macOS(.v14)],
    products: [
        // Built as a dynamic library so Cargo's build.rs can link the Rust
        // binary against it without embedding a second copy of the Swift
        // runtime. See src-tauri/build.rs for how this gets located.
        .library(name: "BlueEarAudio", type: .dynamic, targets: ["BlueEarAudio"])
    ],
    dependencies: [
        // Pinned to an exact tag rather than a range: FluidAudio is pre-1.0
        // and its ASR surface moves between minor releases. Re-run
        // spike/fluidaudio-spike before bumping this.
        .package(url: "https://github.com/FluidInference/FluidAudio.git", exact: "0.15.5")
    ],
    targets: [
        .target(
            name: "BlueEarAudio",
            dependencies: [.product(name: "FluidAudio", package: "FluidAudio")],
            path: "Sources/BlueEarAudio",
            swiftSettings: [
                // Pins the precise minor deployment target Core Audio
                // process taps need for stable TCC behavior (SPM's
                // `platforms` enum only expresses major macOS versions).
                .unsafeFlags(["-target", "arm64-apple-macos14.4"])
            ],
            linkerSettings: [
                .unsafeFlags(["-target", "arm64-apple-macos14.4"])
            ]
        )
    ]
)
