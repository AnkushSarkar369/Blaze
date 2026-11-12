# Blaze native application

Blaze is a native Rust desktop application built with eframe/egui and OpenGL. It does not use a browser engine, WebView, React, HTML, CSS, JavaScript, or Tauri.

## Architecture

- `native-app` contains the desktop application, interface, playback integration, and licensing.
- `crates/blaze-engine` contains UI-independent media probing, keyframe analysis, waveform generation, silence detection, and export operations.
- FFmpeg and FFprobe are external executables. The media engine looks for them beside the application in `Bin` and `Bin/FFmpeg`; macOS app bundles may also use `Contents/Resources/Bin`. Development builds can use the repository-level `BIN` folder. Explicit tool directories, the system `PATH`, and finally `~/bin` are fallback locations.

The root README is the canonical reference for current application capabilities, licence behaviour, export formats, and the product roadmap.

## Playback

Embedded playback is provided by libmpv through its render API. Video is rendered into the egui preview and audio playback is handled by mpv.

### Linux

Blaze links to the system `libmpv.so.2` runtime. Install the MPV development/runtime package before building; for Fedora:

```bash
sudo dnf install mpv-libs
```

On Debian/Ubuntu, install `libmpv-dev` (which brings the runtime dependency) and `pkg-config`. FFmpeg and FFprobe may be installed by the distribution package manager and discovered through `PATH`, or placed in the application's `Bin` folder.

### macOS

Install the native dependencies with Homebrew:

```bash
brew install mpv ffmpeg
cargo run -p blaze-native
```

The build script locates `libmpv.dylib` through Homebrew or `MPV_LIBRARY_DIR`, then creates a linker alias in Cargo's build directory and adds runtime search paths. Set `MPV_LIBRARY_DIR` to the directory containing `libmpv.dylib` when using a non-Homebrew installation.

The macOS app-bundle resource lookup is supported by the media engine, but this repository does not yet produce a distributable signed/notarized `.app` bundle. The CI job validates a native build and integration tests, not app-bundle packaging or Gatekeeper launch.

### Windows

Keep `libmpv-2.dll` and the other MPV runtime DLLs beside `Blaze.exe`; do not move any of them into `Bin`, because the executable imports the MPV runtime at load time and Windows resolves those DLLs beside the executable. FFmpeg and its DLLs live in `Bin\\FFmpeg`. See the root README for the full package layout.

The Windows release build has no console window. Diagnostic output is written to `%APPDATA%\\Blaze\\blaze.log`.

## Development

Run Blaze from the repository root:

```bash
cargo run -p blaze-native
```
