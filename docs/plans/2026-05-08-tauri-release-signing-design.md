# Barnstormer Tauri Release Signing Design

**Date:** 2026-05-08
**Status:** Approved
**Scope:** release-grade macOS signing and notarization on version tags only

## Goal

Extend Barnstormer's GitHub release workflow so version tags produce a signed and notarized macOS Tauri desktop artifact for distribution under the 2389 Apple developer account.

## Recommendation

Keep `.github/workflows/release.yml` as the tagged-release workflow for CLI artifacts, and add a parallel `.github/workflows/release-gui.yml` that publishes the signed and notarized macOS Tauri desktop artifact onto the same tag.

This preserves the existing CLI release contract while keeping the desktop signing/notarization plumbing — Apple keychains, App Store Connect keys, `cargo tauri build` — isolated in a workflow that only macOS runners ever touch. The desktop job uploads its artifact into the GitHub Release created by `release.yml` so end users see one consolidated release.

> **Note:** an earlier draft of this document recommended adding the desktop job directly to `release.yml`. The implementation ultimately split the two so the desktop pipeline can be edited and re-run independently of the CLI pipeline.

## Alternatives Considered

### 1. Extend `release.yml` with a macOS Tauri release job

- Keeps all `v*` release outputs in one workflow.
- Fits the repo's existing tag-driven release model.
- Avoids replacing the current CLI release path.

### 2. Create a separate desktop-only tagged workflow

**Recommendation** (implemented as `.github/workflows/release-gui.yml`)

- Cleaner separation for desktop packaging.
- Apple signing/notarization steps don't leak into the CLI release path.
- Adds coordination overhead because one tag now drives multiple release workflows.

### 3. Replace the release flow with a Tauri-centric release action

- Potentially less custom shell scripting.
- Poorer fit for a repo that still releases non-desktop Rust CLI artifacts.

## Workflow Shape

Keep release triggering on:

- `push.tags: v*`

Add a new macOS desktop release job that:

- Runs on `macos-latest`
- Checks out the repository
- Installs stable Rust
- Installs the Tauri CLI
- Imports the Apple Developer ID certificate into a temporary keychain
- Writes the App Store Connect API key to a temporary `.p8` file
- Runs `cargo tauri build --bundles app,dmg --config crates/barnstormer-tauri/tauri.conf.json`
- Signs the app and DMG
- Notarizes the release artifact
- Staples the notarization ticket
- Uploads the signed desktop artifact into the same GitHub release as the existing CLI assets

The existing Linux, Windows, and CLI artifact jobs should remain intact.

## Secret Contract

Use **repo-level GitHub secrets** on `2389-research/barnstormer`.

The desktop release path should use Apple signing materials sourced from the team's secure secret storage (e.g. a shared password manager, vault, or CI secret mount) — not from a maintainer's local filesystem.

Recommended GitHub secret mapping:

- `APPLE_CERTIFICATE_BASE64` — base64 of the chosen Developer ID `.p12`
- `APPLE_CERTIFICATE_PASSWORD` — password for the certificate archive
- `APPLE_SIGNING_IDENTITY` — exact `codesign` identity string
- `APPLE_API_KEY_BASE64` — base64 of the selected `AuthKey_*.p8`
- `APPLE_API_KEY_ID` — App Store Connect API key ID
- `APPLE_API_ISSUER_ID` — App Store Connect issuer ID
- `APPLE_TEAM_ID` — Apple team ID

Provisioning profiles are not required for this flow because the target is Developer ID distribution, not App Store submission.

## Signing Pattern

Follow the same operational pattern already used in `buddy-app`:

- Create a temporary keychain on the runner
- Unlock it and make it available to signing tools
- Import the base64-decoded `.p12`
- Set the key partition list so `codesign` can use it non-interactively
- Write the App Store Connect `.p8` to a temporary file
- Expose signing/notarization variables only for the lifetime of the job
- Remove temporary key material before the job exits

## Artifact Strategy

The desktop artifact should be published as a distinct release asset, not confused with the raw CLI binary.

Examples:

- `Barnstormer.app`
- `Barnstormer.dmg`

Prefer attaching the DMG to the GitHub release as the primary end-user download.

## Failure Handling

The workflow should fail hard on:

- Certificate import failure
- Missing or invalid signing identity
- Build failure
- Notarization rejection
- Stapling failure

On notarization failure, capture the submission output and retrieve the notarization log in CI so rejection causes are visible in the GitHub Actions logs.

## Verification

The desktop release job should verify:

- `cargo test -p barnstormer-runtime --tests`
- `cargo test -p barnstormer-tauri --lib`
- `cargo check -p barnstormer-tauri`
- `cargo tauri build --bundles app,dmg --config crates/barnstormer-tauri/tauri.conf.json`

## Explicit Deferrals

- PR-time desktop packaging
- App Store release packaging
- Auto-update signing/plumbing
- Environment-scoped GitHub secrets or release environments
- Non-tag desktop CI smoke builds
