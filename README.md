# Blaze

Blaze is a lightweight native desktop video-cutting application.

## Technology

- Rust
- egui / eframe for the native desktop interface
- `blaze-engine` for media probing and media-processing foundations

The application does not use a browser engine, WebView, React, or Tauri at runtime.

## Current state

The native application is an early shell. It currently provides the desktop window, file selection, and media metadata probing. Native video playback, audio playback, waveform rendering, timeline interaction, editing, and export are not yet implemented.

## Run

```bash
cargo run --manifest-path native-app/Cargo.toml
```

See [`native-app/README.md`](native-app/README.md) for the current implementation notes and playback architecture gate.

Development builds and testing are performed locally. Automated GitHub workflows are intentionally not used.
