# Blaze native application

Blaze is a native Rust desktop application built with eframe/egui and OpenGL. It does not use a browser engine, WebView, React, HTML, CSS, JavaScript, or Tauri.

## Playback

Embedded playback is provided by libmpv through the mpv render API. Video is rendered directly into the existing egui preview; audio is handled by mpv and remains the source's original audio stream. FFmpeg remains responsible for media probing and the editing/export operations built on the existing `blaze-engine`.

The player uses MPV's `vaapi-copy` hardware-decoding path for broad compatibility with Intel and AMD Linux graphics. On the Haswell target, hardware decoding is limited to H.264, MPEG-2 and VC-1; HEVC and other codecs fall back to software decoding. The player also enables decoder-level frame dropping to protect audio/video sync when decoding falls behind. The audio output buffer is set to 300 ms to provide a little more underrun tolerance on low-power systems.

## Fedora playback dependency

Install the libmpv runtime library:

```bash
sudo dnf install mpv-libs
```

Blaze uses the hand-written Rust bindings without requiring mpv headers or pkg-config metadata. Its build script locates the versioned runtime library (`libmpv.so.2`) and creates a temporary linker-name symlink inside Cargo's build directory. No system files are modified, and the application remains dynamically linked to libmpv at runtime.

## Run

```bash
cargo run --manifest-path native-app/Cargo.toml
```

The app expects the existing FFmpeg/FFprobe tools in the repository's `BIN/` directory or in `~/bin`, matching the media engine's existing lookup behaviour.

## Current scope

- Native application window and resizable preview / tools / timeline layout.
- Native file picker inside the preview.
- Embedded video and audio playback through libmpv.
- A dedicated transport strip with vector play/pause and import icons, a wide seek bar and right-aligned time display.
- Play resumes from the beginning after the current clip reaches its end.
- Background FFprobe metadata inspection through `blaze-engine`.
- A single continuous, mirrored waveform generated asynchronously from the video's original audio.
- During generation, the timeline becomes a left-to-right progress bar labelled “Generating Waveform Data”, with percentage updates from FFmpeg's decoded PCM output. The waveform replaces the progress bar when ready.
- A playback-synchronised playhead that follows libmpv's video position; clicking or dragging the waveform seeks the video.
- Waveform peaks are generated at 200 blocks per second (5 ms resolution), with all source channels merged into one envelope. Peak data is compact and rendered at screen-pixel resolution.
- Editing, cuts, export, and the Tools workflow are not implemented yet.

Audio playback is independent of the waveform. The waveform is a visual envelope only; it is never used as the playback signal. Videos without an audio stream do not generate a waveform.
