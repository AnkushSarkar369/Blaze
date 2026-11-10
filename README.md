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
- Gumroad license-key activation with periodic entitlement verification

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

### 1. Product audit and planning

- [ ] Audit the existing product, architecture, workflows, and feature set.
- [ ] Audit the Tools Tab: information architecture, controls, grouping, and editing-mode workflows.
- [ ] Audit the complete interface: hierarchy, layout, visual consistency, accessibility, and interaction design.
- [ ] Create a consolidated bug and regression inventory, with reproducible steps and severity.
- [ ] Run an independent, read-only repository audit with Claude (no code changes during the initial review).
- [ ] Compare the independent audit with our findings; accept or reject recommendations deliberately.
- [ ] Prioritise the work into an implementation plan before major redesign.

### 2. Product design and UX

- [ ] Review and improve the overall application design.
- [ ] Add standard right-click Cut, Copy, and Paste actions to editable text fields.
- [ ] Add timeline right-click actions to set an "Add starting point" and an "Add ending point" (mouse-driven equivalents of the existing start/end-point workflow).
- [ ] Review responsive behaviour at the supported window sizes.
- [ ] Revisit the application colour system and decide whether to adopt a true-black interface. (Deferred decision.)

### 3. Bugs, reliability, and performance

- [ ] Triage the bug inventory and prioritise data-loss, export, playback, and editing defects.
- [ ] Fix confirmed functional bugs.
- [ ] Fix confirmed UI and interaction bugs.
- [ ] Test media import, probing, waveform generation, seeking, and playback across varied files.
- [ ] Test cut-boundary placement, dragging, snapping, and Skip Cuts behaviour.
- [ ] Test silence detection presets, expert settings, padding, and minimum-duration handling.
- [ ] Test asynchronous analysis and export cancellation/stale-result handling.
- [ ] Profile performance and resource use on the target low-spec hardware.
- [ ] Run regression testing after significant changes and maintain a record of verified results.

### 4. Export and NLE compatibility

- [ ] Validate generated FCPXML 1.14 against the current Apple DTD and import it into Final Cut Pro 12.x.
- [ ] Validate FCP7 XML import in DaVinci Resolve and Adobe Premiere Pro.
- [ ] Validate OpenTimelineIO import in DaVinci Resolve and Adobe Premiere Pro.
- [ ] Validate CMX 3600 EDL imports in supported NLEs.
- [ ] Test Blaze stream-copy export with varied codecs, containers, frame rates, keyframe spacing, audio layouts, and cut positions.
- [ ] Document known export limitations and compatibility.

### 5. Cross-platform readiness and packaging

- [ ] Define supported Linux distributions and minimum system requirements.
- [ ] Create and verify distributable Linux packages.
- [ ] Establish a reproducible Windows build.
- [ ] Test the application on a physical Windows device.
- [ ] Verify Windows media-tool discovery, playback, import/export paths, permissions, and file dialogs.
- [ ] Resolve platform-specific bugs and document installation requirements.
- [ ] Define release versioning and a repeatable release process.

### 6. Licensing and commercial model

- [ ] Finalise and publish pricing and the free/paid feature split.
- [ ] Test reinstall, device changes, revoked/refunded licenses, and verification-service outages.
- [ ] Document privacy, data collection, refunds, and support policies.

### 7. Website and sales

- [ ] Add pricing and plan comparison.
- [ ] Integrate checkout and payment processing.
- [ ] Implement license delivery and account/order workflows.
- [ ] Add downloads, installation instructions, documentation, and support contact.
- [ ] Test the complete purchase-to-activation journey.

### 8. Release readiness

- [ ] Complete Linux release testing.
- [ ] Complete Windows release testing.
- [ ] Verify clean installation, upgrade, and uninstall paths.
- [ ] Prepare release notes and known-issues documentation.
- [ ] Publish the first stable public release.

## Run

```bash
cargo run -p blaze-native
```

Development builds and testing are performed locally. Automated GitHub workflows are intentionally not used.
