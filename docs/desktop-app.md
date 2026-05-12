# Barnstormer Desktop App

## Overview

The desktop wrapper is implemented in `crates/barnstormer-tauri/` and targets macOS first. It starts Barnstormer's existing Axum app in-process through `barnstormer-runtime` and opens a Tauri window against the embedded loopback URL.

## Data And Settings

- Desktop data lives under the macOS application support directory returned by Tauri for `Barnstormer`.
- Provider settings are stored in `desktop-settings.json` in that app data directory.
- Saved settings are projected into process environment variables so existing provider detection and LLM client code can be reused.
- If no provider key is available from saved settings or the process environment, the app opens `settings.html` first instead of launching a broken main window.

## Verification Matrix

- `cargo test -p barnstormer-runtime --tests`
- `cargo test -p barnstormer-tauri --lib`
- `cargo check -p barnstormer-tauri`
- `cargo tauri build --bundles app --config crates/barnstormer-tauri/tauri.conf.json`

## Build The macOS App

```bash
cargo tauri build --bundles app --config crates/barnstormer-tauri/tauri.conf.json
```

The resulting `.app` bundle should be emitted under Tauri's target output for the desktop crate.

## Release Artifacts

- Tagged `v*` GitHub Releases build a signed and notarized macOS desktop artifact through `.github/workflows/release-gui.yml`.
- That release flow requires the repo-level GitHub secrets documented in `docs/release-signing-secrets.md`.
- The preferred end-user download is `Barnstormer.dmg`, which is attached to the GitHub release beside the existing CLI binaries.

## Deferred Work

- Production icon set polish
- Auto-update
- A richer native settings UI
- Replacing HTTP and SSE with direct Tauri IPC
