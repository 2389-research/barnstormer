# Barnstormer Tauri Wrapper Design

**Date:** 2026-05-08
**Status:** Approved
**Scope:** macOS-only first pass

## Goal

Wrap Barnstormer's existing web UI and Rust backend in a native macOS Tauri app so users can install and launch it like a desktop application.

## Recommendation

Build a Tauri shell that owns the Barnstormer server lifecycle in-process. The app should start the existing Axum server inside the Tauri process, wait for it to become ready on loopback, then open a native window pointed at the local web UI.

This preserves the current web stack and avoids a large rewrite while still delivering an installed app experience.

## Alternatives Considered

### 1. In-process local server inside Tauri

**Recommendation**

- Reuse the existing Axum, HTMX, and SSE architecture with minimal UI churn.
- Refactor the current CLI startup path into a shared runtime entrypoint that both the CLI and Tauri wrapper can call.
- Bind only to loopback and let the native shell fully manage startup and shutdown.

### 2. Custom Tauri protocol and direct native command integration

- Would reduce visible reliance on localhost.
- Conflicts with the current HTTP-first architecture and would require substantial rework around routing, HTMX behavior, and SSE updates.

### 3. Full application-service split before adding Tauri

- Cleanest long-term abstraction boundary.
- Too much refactor for the first installed-app pass.

## Architecture

Create a new workspace crate for the native app, likely `crates/barnstormer-tauri`, plus a shared runtime startup function extracted from the current `Cli::Start` path in [`src/main.rs`](../../src/main.rs).

The shared runtime should:

- Resolve configuration and storage paths.
- Initialize storage and recover specs.
- Build `AppState`.
- Create the Axum router.
- Bind a loopback listener.
- Report the resolved local URL back to the caller.
- Support graceful shutdown.

The CLI binary should continue to use the same runtime path, preserving current behavior. The Tauri app should call that runtime directly instead of spawning the CLI as a child process.

## Native App Lifecycle

On app launch:

- Resolve Barnstormer app data under macOS application support.
- Load any persisted native settings.
- Start the embedded Barnstormer server with browser auto-open disabled.
- Bind to loopback, preferably on an ephemeral port.
- Wait for readiness.
- Open the main Tauri window to the resolved local URL.

On app quit:

- Signal the embedded server to shut down.
- Wait briefly for graceful cleanup.
- Exit cleanly even if shutdown times out.

## Configuration

The installed app should stop depending on terminal-first assumptions.

- Default app data should live under `~/Library/Application Support/Barnstormer`.
- `.env` loading may remain available for local development, but should not be the installed-app contract.
- Provider API keys should be persisted through the native app rather than expected from a shell environment.
- The first pass may use a minimal native settings surface rather than a polished preferences experience.
- Auth should remain disabled for the embedded loopback server.

## Packaging

First-pass packaging goals:

- Produce a working `.app` bundle through Tauri.
- Keep the existing CLI binary intact for terminal users.
- Defer notarization, auto-update, and DMG polish unless distribution requirements expand.

## Failure Handling

The Tauri app should treat the embedded server as a managed subsystem.

- If startup fails, show a native error state instead of opening a broken window.
- If the server exits unexpectedly after launch, surface the failure clearly and offer restart or quit.
- Write logs into the macOS app data area for post-failure debugging.

## Testing Strategy

Keep the first pass focused:

- Add unit coverage around the extracted shared runtime where practical.
- Add an integration-style test for bind, readiness, URL reporting, and graceful shutdown.
- Add a Tauri smoke path that proves the wrapper launches and targets the embedded server correctly.
- Manually verify first launch, relaunch with persisted data, quit/reopen, and provider-key persistence on macOS.

## Explicit Deferrals

- Full native settings UX polish
- Auto-updater
- Notarization and release-signing automation
- Replacing HTTP and SSE with direct Tauri IPC
