<p align="center">
  <img src="assets/brand/logo.svg" alt="Blue Ear — local meeting audio capture" width="420">
</p>

<p align="center">
  <strong>Local-first audio capture for Teams and Zoom</strong><br>
  Isolate meeting audio, optionally record your microphone, and save synchronized WAV tracks — nothing leaves your computer.
</p>

<p align="center">
  <a href="https://github.com/hackarada/blueear">GitHub</a>
  ·
  <a href="./LICENSE">MIT License</a>
  ·
  <a href="./CONTRIBUTING.md">Contributing</a>
  ·
  <a href="./SECURITY.md">Security</a>
</p>

---

A local-first desktop app that isolates native Microsoft Teams or Zoom meeting
audio (Core Audio process taps on macOS; WASAPI process loopback on Windows),
optionally records your microphone, and produces synchronized `meeting.wav`,
`microphone.wav`, and `mixed.wav` tracks for every session. Nothing is uploaded.

## Requirements

### macOS

- macOS 14.4 or later on Apple Silicon or Intel.
- Xcode / the Swift toolchain (`swift build` must work) and the Rust toolchain.
- Node.js 18+ for the frontend.

### Windows

- Windows 10 **build 20348** or later (process loopback API).
- MSVC toolchain, Windows SDK, WebView2, Rust, Node.js 18+.
- CMake (for the optional Whisper.cpp provider).

### Meeting apps (both platforms)

- Native Microsoft Teams or Zoom desktop apps only. Browser / PWA versions are
  not supported: their audio belongs to the browser process.

## Project layout

```
blueear/
├── assets/brand/               Logo SVGs, app/tray icon sources, PNG exports
├── src/                        React + TypeScript UI (Vite)
└── src-tauri/
    ├── src/
    │   ├── audio/              ring buffers, DSP, PlatformAudioEngine (macOS FFI / Windows WASAPI)
    │   ├── session/            recording lifecycle
    │   ├── storage/            WAV writer, session metadata, crash recovery
    │   ├── transcription/      providers (Apple Speech, FluidAudio, Whisper), jobs, merge
    │   └── paths.rs            portable recordings + app-support roots
    └── native/BlueEarAudio/    macOS Swift package: process taps + mic + Apple ASR adapters
```

Rust owns session state, synchronization, output files, recovery, and the
shared Whisper provider. Swift owns Apple audio object lifecycles and Apple
ASR adapters on macOS. Windows capture lives in Rust (`audio/windows.rs`).

## Running in development

```bash
npm install
npm run tauri dev
```

### macOS

The first run needs Teams or Zoom open, and will prompt for Screen & System
Audio Recording permission the first time recording starts.

### Windows

Teams or Zoom must be running. Microphone privacy may prompt when the mic
track is enabled. Process loopback does not use a macOS-style system-audio TCC
prompt.

## Building

### macOS `.app`

```bash
npm run tauri build -- --bundles app
```

Produces `src-tauri/target/release/bundle/macos/Blue Ear.app`. macOS builds are
ad-hoc signed by default (`signingIdentity: "-"` in `tauri.conf.json`). See
`Entitlements.plist` for the Screen & System Audio Recording entitlement;
notarized Developer ID distribution is optional and environment-specific.

### Windows installer

```bash
npm run tauri build -- --bundles nsis
# or: --bundles msi
```

Requires the MSVC build tools and WebView2 on the build machine.

## Transcription

Optional and off by default (`none`). Providers:

| Provider | Platforms | Setup |
|---|---|---|
| Apple Speech | macOS 26+ | System language assets |
| FluidAudio | macOS 14.4+ | Manual `fluidaudio-v1` bundle import |
| Whisper | macOS + Windows | Manual `whisper-v1` ggml bundle import |

Whisper links Whisper.cpp via `whisper-rs` (Cargo feature `whisper`, on by
default and also listed in `tauri.conf.json` `build.features` because
`tauri dev`/`build` pass `--no-default-features`). Building it needs CMake and
a C++ compiler. Disable with `--no-default-features` (and clear
`build.features`) if you only need the recorder.

## Menu bar and recordings library

Blue Ear keeps a tray icon alongside its normal window. Closing the window
hides it instead of quitting. Recordings live under
`~/Music/BlueEar/Recordings` (or `%USERPROFILE%\Music\BlueEar\Recordings` on
Windows).

## Testing

```bash
npm test                 # frontend unit tests
cd src-tauri
cargo test               # DSP, ring buffer, WAV crash-safety, recovery, transcription contracts
cargo test -- --ignored --nocapture live_   # requires a real, running Teams or Zoom
```

## Output layout

```
~/Music/BlueEar/Recordings/          (or %USERPROFILE%\Music\BlueEar\Recordings)
└── 2026-08-07_10-30-00/
    ├── meeting.wav
    ├── microphone.wav   (omitted if microphone capture was off)
    ├── mixed.wav
    └── session.json
```

## Known limitations

- Native Teams or Zoom desktop apps only (no browser/PWA).
- One meeting app per recording session.
- Optional local transcription; off by default.
- No calendar integration or automatic start/stop.
- No compression, custom output folders, or editing.
- macOS builds are ad-hoc signed / not notarized out of the box.

## License

Blue Ear is released under the [MIT License](./LICENSE).

## Contributing

See [CONTRIBUTING.md](./CONTRIBUTING.md) for development setup, tests, and PR
guidelines. To report a vulnerability, see [SECURITY.md](./SECURITY.md).
