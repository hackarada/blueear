use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=icons/tray-icon.png");
    tauri_build::build();

    #[cfg(target_os = "macos")]
    build_native_audio();
}

/// Builds the linked `BlueEarAudio` Swift package (Core Audio process-tap
/// capture + microphone capture) and links the resulting dynamic library
/// into this crate. See `src/audio/ffi.rs` for the Rust side of the C ABI
/// this produces, and `native/BlueEarAudio/` for the Swift side.
#[cfg(target_os = "macos")]
fn build_native_audio() {
    let manifest_dir =
        PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let package_dir = manifest_dir.join("native").join("BlueEarAudio");

    let profile = env::var("PROFILE").unwrap_or_else(|_| "debug".to_string());
    let swift_config = if profile == "release" { "release" } else { "debug" };

    let status = Command::new("swift")
        .args(["build", "-c", swift_config, "--package-path"])
        .arg(&package_dir)
        .status()
        .expect(
            "failed to invoke `swift build` for native/BlueEarAudio -- \
             is Xcode / the Swift toolchain installed?",
        );
    if !status.success() {
        panic!("swift build failed for native/BlueEarAudio");
    }

    let lib_dir = package_dir.join(".build").join(swift_config);
    let built_dylib = lib_dir.join("libBlueEarAudio.dylib");
    if !built_dylib.is_file() {
        panic!(
            "swift build succeeded but {} is missing",
            built_dylib.display()
        );
    }

    // Stage a profile-matched copy for Tauri's `bundle.macOS.frameworks`.
    // `tauri dev` / bundling copy that path into `target/Frameworks`, and
    // dyld searches `@executable_path/../Frameworks` *before* our absolute
    // `.build` rpath — so a stale release dylib there (old Teams ABI)
    // crashes on the first meeting-app FFI call (jump to null).
    let staged_dir = package_dir.join(".build").join("bundled");
    fs::create_dir_all(&staged_dir).expect("create BlueEarAudio .build/bundled");
    let staged_dylib = staged_dir.join("libBlueEarAudio.dylib");
    fs::copy(&built_dylib, &staged_dylib).unwrap_or_else(|e| {
        panic!(
            "failed to stage {} -> {}: {e}",
            built_dylib.display(),
            staged_dylib.display()
        );
    });

    // Keep `target/Frameworks` in sync for already-running / next `cargo run`
    // without waiting for Tauri to re-copy frameworks.
    let target_dir = env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| manifest_dir.join("target"));
    let frameworks_dir = target_dir.join("Frameworks");
    fs::create_dir_all(&frameworks_dir).expect("create target/Frameworks");
    let frameworks_dylib = frameworks_dir.join("libBlueEarAudio.dylib");
    fs::copy(&built_dylib, &frameworks_dylib).unwrap_or_else(|e| {
        panic!(
            "failed to copy {} -> {}: {e}",
            built_dylib.display(),
            frameworks_dylib.display()
        );
    });

    println!("cargo:rustc-link-search=native={}", lib_dir.display());
    println!("cargo:rustc-link-lib=dylib=BlueEarAudio");
    // Absolute `.build` rpath for `cargo run` / `cargo test`. Tauri already
    // adds `@executable_path/../Frameworks` — do not add it again (duplicate
    // -rpath warning) or put it first ahead of the fresh build.
    println!("cargo:rustc-link-arg=-Wl,-rpath,{}", lib_dir.display());

    println!(
        "cargo:rerun-if-changed={}",
        package_dir.join("Sources").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        package_dir.join("Package.swift").display()
    );
}
