# Blaze

Blaze is a lightweight native desktop video-cutting application.

## Technology

- Rust
- egui / eframe for the native desktop interface
- `blaze-engine` for media probing and FFmpeg-backed media operations
- MPV for embedded preview playback

The application does not use a browser engine, WebView, React, or Tauri at runtime.

## Current capabilities

- Native video preview and audio playback
- Waveform generation and timeline navigation
- Blaze Mode with keyframe-aligned cut boundaries
- Precise Mode with unrestricted cut boundaries
- Silence detection with preset and expert controls
- Skip Cuts playback
- Asynchronous media analysis and export jobs

## Export

The Export panel groups rendered video output and NLE timeline interchange.

- **Blaze Export:** stream-copy export for Blaze Mode edits. It extracts kept ranges from the original source and concatenates them without re-encoding. The source container is preserved.
- **DaVinci Resolve XML:** Final Cut Pro 7 XML interchange, importable by DaVinci Resolve.
- **EDL:** CMX 3600 edit decision list.
- **FCPXML:** Final Cut Pro XML 1.10 timeline.
- **Premiere XML:** Final Cut Pro 7 XML interchange, importable by Adobe Premiere Pro.
- **Precision Export:** deferred; currently marked Coming Soon.

Timestamp interchange currently models one source media file with one linked video track and, when present, one linked audio track. Effects, transitions, multiple source clips, and multi-track timelines are outside the current edit model.

## Run

```bash
cargo run --manifest-path native-app/Cargo.toml
```

Development builds and testing are performed locally. Automated GitHub workflows are intentionally not used.
