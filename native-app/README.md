# Blaze native application

Blaze is a native Rust desktop application built with eframe/egui and OpenGL. It does not use a browser engine, WebView, React, HTML, CSS, JavaScript, or Tauri.

## Architecture

- `native-app` contains the desktop application, interface, playback integration, and licensing.
- `crates/blaze-engine` contains UI-independent media probing, keyframe analysis, waveform generation, silence detection, and export operations.
- FFmpeg and FFprobe are external executables. The media engine looks for them in `Bin` and `Bin/FFmpeg` beside the executable, then (for development builds run from Cargo's `target` folder) the repository-level `BIN` folder, then `~/bin`.

The root README is the canonical reference for current application capabilities, licence behaviour, export formats, and the product roadmap.

## Playback

Embedded playback is provided by libmpv through its render API. Video is rendered into the egui preview and audio playback is handled by mpv.

On Linux, the application links against the `libmpv.so.2` runtime library. On Fedora, install it with:

```bash
sudo dnf install mpv-libs
```

On Windows, the release package keeps `libmpv-2.dll` beside `Blaze.exe`; do not move it into `Bin`, because the executable imports it at load time and Windows only searches the executable's folder. FFmpeg and its DLLs live in `Bin\FFmpeg`. See the root README for the full package layout.

The Linux build script locates the installed runtime library and creates a linker-name symlink inside Cargo's build directory. It does not modify system files.

## Development

Run Blaze from the repository root:

```bash
cargo run -p blaze-native
```
