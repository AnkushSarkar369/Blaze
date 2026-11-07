# Blaze

**Clean your footage up.**

Blaze is a local-first desktop video utility. This branch contains the first desktop application shell, built with Tauri 2, Rust, React, TypeScript and Vite.

## Current status

The v0.1 build establishes the desktop window and initial visual workspace. Video import and processing are not implemented yet.

## Development

Requirements: Node.js 22, Rust stable, and the Linux dependencies listed in `.github/workflows/build-linux.yml`.

```sh
npm install
npm run tauri -- dev
```

Build a Linux AppImage:

```sh
npm run tauri -- build --bundles appimage
```

The AppImage is generated under `src-tauri/target/release/bundle/appimage/`.
