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
- Direct Gumroad Monthly and Permanent licence verification in the native Rust application

## Licensing

Blaze verifies licence keys directly with Gumroad from the native Rust application. No separate licensing server, Node.js runtime, or `BLAZE_LICENSE_API_URL` configuration is required.

Two separate Gumroad products are supported:

| Product | Gumroad product ID | Entitlement |
|---|---|---|
| Blaze Monthly | `Niyme89XCcFC44ShhEPPrQ==` | Monthly subscription |
| Blaze Permanent | `ksPYCeAJW8TtmOjsBRQ0OA==` | Permanent licence |

The app checks the Monthly product first and then the Permanent product. A successful activation increments Gumroad's Uses counter once, after the key has matched and passed entitlement checks. Periodic revalidation and export-time checks use `increment_uses_count=false`.

Monthly keys must report a monthly recurrence. Gumroad's subscription cancellation, end, and failed-renewal timestamps are treated as the end of paid access: access remains valid until a future timestamp and is denied once that timestamp has passed. Permanent keys do not have a subscription expiry. Refunds, chargebacks, and unresolved disputes invalidate either product.

## Export

The Export panel groups rendered video output and NLE timeline interchange.

- **Blaze Export:** stream-copy export for Blaze Mode edits. It extracts kept ranges from the original source and concatenates them without re-encoding. The source container is preserved.
- **Adobe Premiere Pro (FCP7 XML):** exports Final Cut Pro 7 XML interchange for import into Premiere Pro.
- **DaVinci Resolve (FCP7 XML):** exports the same Final Cut Pro 7 XML interchange for import into DaVinci Resolve.
- **Final Cut Pro XML:** exports FCPXML 1.14, introduced with Final Cut Pro 12.0.
- **OpenTimelineIO:** exports a .otio JSON timeline with source-media references and linked video/audio tracks when audio is present. Premiere Pro and DaVinci Resolve support OTIO interchange.
- **Edit Decision List:** exports a CMX 3600-style EDL.
- **Precision Export:** deferred; currently marked Coming Soon.

Timestamp interchange currently models one source media file with one linked video track and, when present, one linked audio track. Effects, transitions, multiple source clips, and multi-track timelines are outside the current edit model. The OTIO export follows the same scope and references the original media file; it does not bundle media.

## Roadmap / TODO

This is the working product roadmap. It lists unfinished work only; completed items are removed.

### 1. Product design and UX

- [ ] Review and improve the overall application design.
- [ ] Add standard right-click Cut, Copy, and Paste actions to editable text fields.
- [ ] Add timeline right-click actions to set an "Add starting point" and an "Add ending point" (mouse-driven equivalents of the existing start/end-point workflow).
- [ ] Review responsive behaviour at the supported window sizes.
- [ ] Revisit the application colour system and decide whether to adopt a true-black interface. (Deferred decision.)

### 2. Bugs, reliability, and performance

- [ ] Triage the bug inventory and prioritise data-loss, export, playback, and editing defects.
- [ ] Fix confirmed functional bugs.
- [ ] Fix confirmed UI and interaction bugs.
- [ ] Test media import, probing, waveform generation, seeking, and playback across varied files.
- [ ] Test cut-boundary placement, dragging, snapping, and Skip Cuts behaviour.
- [ ] Test silence detection presets, expert settings, padding, and minimum-duration handling.
- [ ] Test asynchronous analysis and export cancellation/stale-result handling.
- [ ] Profile performance and resource use on the target low-spec hardware.
- [ ] Run regression testing after significant changes and maintain a record of verified results.

### 3. Mix

- [ ] Website integration.
- [ ] Auto-Update Functionality
- [ ] Use on a windows build & Github's mac

### 8. Release readiness

- [ ] Linux
- [ ] Windows
- [ ] MacOS
- [ ] Auto-Update
- [ ] Publish the first stable public release.

## Run

```bash
cargo run -p blaze-native
```

Development builds and Linux testing are performed locally. The Windows x64 distribution is assembled by GitHub Actions with the application executable, MPV runtime, and bundled LGPL FFmpeg/FFprobe tools.
