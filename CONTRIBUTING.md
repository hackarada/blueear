# Contributing to Blue Ear

Thanks for your interest in contributing. Blue Ear is a local-first desktop
app: React UI, Rust core, and a Swift native audio package on macOS.

## Prerequisites

### macOS

- macOS 14.4 or later
- Xcode / Swift toolchain (`swift build` must work)
- Rust stable toolchain
- Node.js 18+

### Windows

- Windows 10 build 20348 or later
- MSVC toolchain, Windows SDK, WebView2
- Rust stable toolchain
- Node.js 18+
- CMake (needed when building with the default Whisper feature)

## Development

```bash
npm install
npm run tauri dev
```

See [README.md](./README.md) for build, transcription, and output layout details.

## Tests

```bash
npm test
cd src-tauri && cargo test
```

Live capture tests need a running native Teams or Zoom desktop app:

```bash
cd src-tauri
cargo test -- --ignored --nocapture live_
```

## Pull requests

- Keep changes focused; prefer small PRs over mixed refactors.
- Match existing naming and module boundaries (Rust owns session/storage/DSP;
  Swift owns Core Audio lifecycles on macOS; Windows capture lives in Rust).
- Do not commit recordings, model bundles, build artifacts, or local `docs/`.
- Run `npm test` and `cargo test` before opening a PR when your change touches
  those areas.
- CI runs frontend tests plus `cargo test` on macOS and Windows
  (`.github/workflows/ci.yml`).

## Scope reminders

- Native Microsoft Teams or Zoom desktop apps only (no browser/PWA).
- No cloud upload path; keep that invariant unless a change is explicitly
  designed and reviewed for network use.
- Recording consent is the user's responsibility; do not add features that
  imply Blue Ear verifies participant consent.
