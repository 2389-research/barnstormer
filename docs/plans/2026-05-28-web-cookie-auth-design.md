# Web cookie auth for remote browser access

**Date:** 2026-05-28
**Status:** Approved

## Context

`barnstormer` ships a CLI server (`cargo run -- start`) and an embedded Tauri desktop wrapper. The CLI server is used in two modes:

- **Local dev:** `BARNSTORMER_BIND=127.0.0.1:7331`, no `BARNSTORMER_AUTH_TOKEN`. Open access on loopback.
- **LAN-remote:** `BARNSTORMER_BIND=0.0.0.0:7331`, `BARNSTORMER_AUTH_TOKEN=<hex>`, `BARNSTORMER_ALLOW_REMOTE=true`. Single user (Dyl-Dawg) accessing from another box on the LAN.

The existing `AuthLayer` gates `/api/*` with `Authorization: Bearer <token>`. Browsers' `EventSource` API cannot send custom request headers, so the SSE endpoint — previously at `/api/specs/{id}/events/stream` — silently returned 401 in remote+auth mode. Live updates only worked after a hard refresh.

That has been mitigated in the prior change by moving the SSE route to `/web/specs/{id}/events/stream`, where `/web/*` is currently unauthenticated. As a side effect, the entire web UI is now reachable from the LAN with no auth. This design closes that gap.

## Goal

Add cookie-based session auth on `/web/*` (and `/`) so that:

1. The web UI requires auth when `BARNSTORMER_AUTH_TOKEN` is set.
2. The browser can authenticate the SSE EventSource (it sends cookies on same-origin connections; it cannot send headers).
3. Local-no-auth dev and the Tauri desktop wrapper are unaffected.

## Non-goals

- Multi-user accounts, login forms, logout endpoints, password reset, token rotation.
- CSRF protection on `/web/*` POSTs. Threat model is single LAN-local user; revisit if it broadens.
- Replacing the `/api/*` Bearer auth. `/api/*` continues to require Bearer for programmatic access (curl, scripts, future MCP integrations).
- HTTPS. Operator's responsibility (reverse proxy if desired).

## Design

### New middleware

A new `WebAuthLayer` lives alongside the existing `AuthLayer` in `crates/barnstormer-server/src/auth.rs`. Both layers share a small constant-time-comparison helper.

`WebAuthLayer` only matches paths under `/web/*` and the bare `/`. `/static/*` is served via a separate `nest_service` and is not wrapped (CSS/JS, no secrets). `/health` stays open.

`routes.rs` wires the layer in `create_router` immediately after the existing `AuthLayer` block:

```rust
if let Some(token) = auth_token {
    router
        .layer(AuthLayer::new(token.clone()))
        .layer(WebAuthLayer::new(token))
} else {
    router
}
```

Both layers attach only when `auth_token` is `Some`. Tauri (`auth_token: None` + `disable_auth_fallback: true`) and bare local dev (no token in env) get neither layer.

### Request handling

For each `/` or `/web/*` request, `WebAuthLayer`:

1. **Query-param entry:** if the URL contains `?token=<value>`:
   - Compare `<value>` to the expected token in constant time.
   - On match: respond `303 See Other`, `Location:` is the same path with the `token` query param stripped (other params preserved), `Set-Cookie: barnstormer_session=<token>; HttpOnly; SameSite=Strict; Path=/; Max-Age=2592000`.
   - On mismatch: `401` with body `invalid token`.
2. **Cookie:** else if the `Cookie` header contains `barnstormer_session=<value>` and the value matches the expected token (constant time): pass the request through.
3. **Unauthenticated:** else: `401` with a small HTML body:
   > Authentication required. Visit `<host>/?token=YOUR_TOKEN` once to log in.

### Cookie content

The cookie value is the raw `BARNSTORMER_AUTH_TOKEN`. No server-side session storage, no expiry tracking beyond the cookie `Max-Age`. Same security posture as the token sitting in `.env` — accepted for the threat model. If the threat model expands later (multiple users, public exposure), upgrade to a signed/HMAC'd session token without changing the surface.

Attributes:

- `HttpOnly` — not readable from page JS.
- `SameSite=Strict` — no cross-site submission.
- `Path=/` — same cookie covers `/web/*` and the SSE endpoint.
- `Max-Age=2592000` (30 days) — fresh cookie on each magic-URL visit.
- No `Secure` flag — operator runs on plain HTTP over LAN; setting `Secure` would prevent the cookie from being saved.

### SSE

Already moved to `/web/specs/{id}/events/stream`. `EventSource` is same-origin (page was served from the same host:port), so the browser attaches the cookie to the SSE GET. `WebAuthLayer` accepts it. Live updates work.

### Tauri

No change required. `crates/barnstormer-tauri/src/lib.rs:29-40` already passes `auth_token: None` + `disable_auth_fallback: true`, so neither layer is applied. The embedded webview hits `127.0.0.1:<ephemeral>` directly and is loopback-only by `bind`.

## Failure modes & UX

| Scenario | Response |
|---|---|
| `/web/foo?token=GOOD` | 303 → `/web/foo` with `Set-Cookie` |
| `/web/foo?token=BAD` | 401 `invalid token` |
| `/web/foo` + valid cookie | 200, normal response |
| `/web/foo` + stale/wrong cookie | 401 with re-auth hint |
| `/web/foo` + no cookie | 401 with re-auth hint |
| `/api/foo` + cookie only | 401 (unchanged — `/api/*` still requires Bearer) |
| `/static/foo.css` (any) | 200 (layer doesn't match) |
| `/health` | 200 (no auth) |

The 401 body is plain HTML, not a redirect, to avoid loops (e.g. EventSource auto-retrying through redirects).

## Testing

Unit tests in `auth.rs`:

1. `web_layer_strips_token_and_sets_cookie` — request with `?token=<good>` → 303, `Location` header has token removed, `Set-Cookie` present.
2. `web_layer_preserves_other_query_params` — `?token=<good>&foo=bar` → `Location: …?foo=bar`.
3. `web_layer_rejects_wrong_token` — `?token=<bad>` → 401.
4. `web_layer_accepts_cookie` — request with `Cookie: barnstormer_session=<good>` → passes through.
5. `web_layer_rejects_wrong_cookie` — wrong cookie value → 401.
6. `web_layer_rejects_no_auth` — no cookie, no token param → 401.
7. `web_layer_exempts_static` — `/static/foo.css` → passes through.
8. `web_layer_exempts_health` — `/health` → passes through.
9. `web_layer_exempts_api` — `/api/foo` → passes through (`AuthLayer` covers it).
10. `web_layer_constant_time_comparison` — compile-time/use of constant-time eq helper (sanity check, not a real timing test).
11. `router_attaches_neither_layer_without_token` — `create_router(..., None)` exposes `/web/foo` open and `/api/foo` open.
12. `router_attaches_both_layers_with_token` — `create_router(..., Some(t))` requires Bearer on `/api/foo` and cookie/query on `/web/foo`.

Integration sanity: existing `context_summarize_failed_sse.rs` test continues to use `/web/specs/{id}/events/stream` without auth (the test constructs the router via `test_state()` which provides no token).

## Migration

None. The change is additive when `BARNSTORMER_AUTH_TOKEN` is set. Existing local-no-auth deployments are unaffected.

The user's bookmark on the remote box becomes `http://2389-mini:7331/?token=<token>` (visit once), then any `http://2389-mini:7331/web/specs/...` URL works for 30 days.

## Open follow-ups (not part of this change)

- **CSRF.** If we ever expose this to anyone but Dyl-Dawg, `/web/*` POSTs need a CSRF token (e.g. double-submit cookie or per-form HMAC).
- **HTTPS.** Operator concern. If we ever ship a hosted version, terminate TLS at a proxy and add `Secure` to the cookie.
- **Session model.** If the threat model broadens, replace the raw-token cookie with a server-side or HMAC'd session as outlined in the brainstorming alternatives.
