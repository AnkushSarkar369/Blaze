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

### Activation and Gumroad Uses

Blaze checks the Monthly product first and then the Permanent product. During a manual activation it first verifies the key without incrementing Uses. Only after the key and entitlement pass does it make a second verification request with `increment_uses_count=true`. This increments Gumroad's Uses counter once for that activation.

Periodic licence revalidation and export-time checks use `increment_uses_count=false`; they do not intentionally add Uses events. The Gumroad dashboard associates the activation event with the relevant purchase/key record, which gives the publisher an audit trail for investigating repeated or suspicious activations. Uses is an activation-event counter, not a list of hardware IDs or a unique-device registry.

### Local device binding

A successful activation is bound to the current installation's operating-system machine identifier:

| Platform | Identifier source |
|---|---|
| Windows | Registry `HKLM\\SOFTWARE\\Microsoft\\Cryptography\\MachineGuid` |
| Linux | `/etc/machine-id`, falling back to `/var/lib/dbus/machine-id` |
| macOS | `IOPlatformUUID` from `ioreg` |

Blaze hashes the identifier into a deterministic HID. The same identifier produces the same HID on subsequent launches. A second SHA-256 value is then derived from an application-specific domain separator, the HID, and the Gumroad licence key. The local state file stores the licence key and this verifier. At startup Blaze recomputes the HID and verifier and compares the result with the saved value.

Consequently, copying the application and its saved activation state to a computer with a different machine identifier does not copy the activation. The key must be entered and activated on that computer. The HID and verifier are not sent to Gumroad.

The state file is stored at the platform's normal application-configuration location:

- Windows: `%APPDATA%\\Blaze\\license-state`
- macOS: `~/Library/Application Support/Blaze/license-state`
- Linux: `$XDG_CONFIG_HOME/Blaze/license-state`, or `~/.config/Blaze/license-state` when `XDG_CONFIG_HOME` is unset

The local state file is not encrypted; it contains the Gumroad key in plaintext. Device binding is intended to deter casual copying of an activation-state file, not a determined attacker who can inspect or patch the executable or modify local state. The embedded application salt is not a server-side secret, and the SHA-256 verifier is not a cryptographic DRM boundary.

These identifiers are operating-system/platform identifiers, not guaranteed immutable hardware serial numbers. Reinstalling or cloning an operating system, regenerating its machine ID, or changing the platform identifier can cause Blaze to request activation again. Existing `gumroad-v1` state files require a one-time reactivation so the new device binding can be written.

### Entitlement checks

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
