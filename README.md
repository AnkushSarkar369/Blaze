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
- **FCPXML:** currently generates Final Cut Pro XML 1.10. Updating and validating newer-version support is tracked below.
- **Premiere XML:** Final Cut Pro 7 XML interchange, importable by Adobe Premiere Pro.
- **Precision Export:** deferred; currently marked Coming Soon.

Timestamp interchange currently models one source media file with one linked video track and, when present, one linked audio track. Effects, transitions, multiple source clips, and multi-track timelines are outside the current edit model.

## Roadmap / TODO

This is the working product roadmap. Check items off here as they are completed and verified. Keep unfinished work visible; do not mark an item complete merely because implementation has started.

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

- [ ] Review the FCPXML exporter against the current FCPXML specification.
- [ ] Add and validate FCPXML 1.14 support; determine whether older-version output should remain available.
- [ ] Validate FCPXML imports in the intended Final Cut Pro version(s).
- [ ] Validate DaVinci Resolve XML imports in DaVinci Resolve.
- [ ] Validate Premiere XML imports in Adobe Premiere Pro.
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

- [ ] Define the free and paid feature split.
- [ ] Decide pricing, license duration, activation limits, and device-transfer policy.
- [ ] Decide whether activation and ongoing use require an internet connection.
- [ ] Design the license and entitlement system before implementing DRM.
- [ ] Implement proportionate licensing/DRM protections without compromising legitimate offline use.
- [ ] Test activation, deactivation, reinstall, device changes, expired licenses, and service outages.
- [ ] Document privacy, data collection, refunds, and support policies.

### 7. Website and sales

- [ ] Define the website structure and product messaging.
- [ ] Build the Blaze product website.
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
cargo run --manifest-path native-app/Cargo.toml
```

Development builds and testing are performed locally. Automated GitHub workflows are intentionally not used.
