# Blaze native application shell

This is the first native application stage. It is deliberately isolated from the existing Tauri application so it can be built and evaluated without replacing the current working branch or changing the existing frontend.

## What this stage provides

- A native desktop window created by eframe/winit and rendered with OpenGL through the Glow backend.
- A resizable preview / tools / timeline layout.
- Native file selection through rfd.
- Background FFprobe metadata inspection through the existing `blaze-engine` crate.
- The existing single-lane waveform requirement is documented in the timeline shell, but waveform data is not yet connected.

## What is deliberately not implemented

- Video playback, audio output, seeking, waveform rendering, editing, cuts, export, and the Tools workflow.
- No browser engine, WebView, React, HTML, CSS, JavaScript, or Tauri runtime is used by this application.
- The preview placeholder is not a simulated video player.

## Run on Fedora

Install the native development packages required by eframe/winit and the OpenGL backend, then run:

```bash
cargo run --manifest-path native-app/Cargo.toml
```

The app expects the existing media tools to be available in the repository's `BIN/` directory or in `~/bin`, matching the media engine's existing lookup behaviour.

The isolated workspace generates its own `Cargo.lock`. It will be folded into the root workspace after the native foundation is validated.

## Playback architecture gate

The next stage is a small, separately testable libmpv render-API integration against eframe's Glow context. mpv's current documentation recommends the render API over embedding a foreign native child window. The render API requires careful OpenGL context ownership and thread coordination; the mpv update callback must only wake the render thread, and render calls must run with the GL context current. No playback dependency is added until that integration is proven on the target Linux graphics stack.

Audio must remain the original stereo stream. The waveform is only a merged visual envelope and must never be used as playback audio.
