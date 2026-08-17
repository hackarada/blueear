# Contributing to Blue Ear

Thanks for your interest in contributing. Blue Ear is a local-first desktop
app: React UI, Rust core, and a Swift native audio package on macOS.

## Prerequisites

### macOS

- macOS 14.4 or later
- Xcode / Swift toolchain (`swift build` must work)
- Rust stable toolchain
- Node.js 22+
- CMake when building with Whisper (the default). `brew install cmake`.

### Windows

- Windows 10 build 20348 or later
- MSVC toolchain, Windows SDK, WebView2
- Rust stable toolchain
- Node.js 22+
- CMake when building with Whisper (the default)

CMake is needed on both platforms for the default Whisper feature. Skip it
with `make dev-no-whisper` / `make test-rust-no-whisper`, or by building Cargo
with `--no-default-features` and clearing `build.features` in
`src-tauri/tauri.conf.json`.

## Development

```bash
make install && make dev
# or: npm install && npm run tauri dev
```

See [README.md](./README.md) for build, transcription, and output layout details.
`make help` lists install, dev, build, and test targets, including
`*-no-whisper` variants that skip Whisper.cpp (no CMake).

## Tests

```bash
make test
# or: npm test && cargo test --manifest-path src-tauri/Cargo.toml
```

Live capture tests need a running native Teams or Zoom desktop app:

```bash
make live-test
# or: cargo test --manifest-path src-tauri/Cargo.toml -- --ignored --nocapture live_
```

## Pull requests

- Keep changes focused; prefer small PRs over mixed refactors.
- Match existing naming and module boundaries (Rust owns session/storage/DSP;
  Swift owns Core Audio lifecycles on macOS; Windows capture lives in Rust).
- Do not commit recordings, model bundles, build artifacts, or local `docs/`.
- Run `make test` (or `npm test` and `cargo test`) before opening a PR when
  your change touches those areas.
- CI runs frontend tests plus `cargo test` on macOS and Windows
  (`.github/workflows/ci.yml`).

## Scope reminders

- Native Microsoft Teams or Zoom desktop apps only (no browser/PWA).
- No cloud upload path; keep that invariant unless a change is explicitly
  designed and reviewed for network use.
- Recording consent is the user's responsibility; do not add features that
  imply Blue Ear verifies participant consent.
