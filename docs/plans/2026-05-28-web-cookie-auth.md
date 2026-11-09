# Web Cookie Auth Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add cookie-based auth on `/web/*` (and `/`) so the browser-rendered UI — including the SSE EventSource — can authenticate when `BARNSTORMER_AUTH_TOKEN` is set, without breaking Tauri or local-no-auth dev.

**Architecture:** New `WebAuthLayer` tower middleware sibling to the existing `AuthLayer`. Layer matches only `/` and `/web/*`. Two entry paths: (1) a one-time `?token=<value>` query param mints an HttpOnly cookie via 303 + Set-Cookie + token-stripped Location; (2) subsequent requests are admitted by a matching cookie. `/api/*` continues to require Bearer. Both layers attach only when `auth_token` is `Some`. Raw token is the cookie value (single-user threat model — same posture as token at rest in `.env`).

**Tech Stack:** axum 0.8, tower 0.5, http 1. No new dependencies. Cookie header and query string parsing are done in a handful of lines each — adding a cookie crate would be premature for a single named cookie.

**Design source:** `docs/plans/2026-05-28-web-cookie-auth-design.md`.

**Branch:** Already on `feature/web-cookie-auth`. The SSE route move landed as `e49813a`; this plan picks up from there.

---

## Task 1: Cookie header parser helper

**Files:**
- Modify: `crates/barnstormer-server/src/auth.rs` (append a new module-private function + tests at the bottom of the existing test module)

Pull a named cookie value out of a `Cookie` header value (single-value comma-less syntax per RFC 6265 §5.4: `name=value; name=value`). Tolerate leading/trailing whitespace around names and values; do not URL-decode (cookie values may include `=` and `%`, but for our use case the value is a hex token so this doesn't matter).

**Step 1: Write the failing test**

Append to the `#[cfg(test)] mod tests { ... }` block in `crates/barnstormer-server/src/auth.rs`:

```rust
    #[test]
    fn parse_named_cookie_finds_value() {
        assert_eq!(
            super::parse_named_cookie("barnstormer_session=abc123", "barnstormer_session"),
            Some("abc123".to_string())
        );
    }

    #[test]
    fn parse_named_cookie_finds_value_among_others() {
        assert_eq!(
            super::parse_named_cookie(
                "foo=bar; barnstormer_session=abc123; baz=qux",
                "barnstormer_session"
            ),
            Some("abc123".to_string())
        );
    }

    #[test]
    fn parse_named_cookie_returns_none_when_missing() {
        assert_eq!(
            super::parse_named_cookie("foo=bar; baz=qux", "barnstormer_session"),
            None
        );
    }

    #[test]
    fn parse_named_cookie_tolerates_whitespace() {
        assert_eq!(
            super::parse_named_cookie("  barnstormer_session  =  abc123  ", "barnstormer_session"),
            Some("abc123".to_string())
        );
    }

    #[test]
    fn parse_named_cookie_empty_header_returns_none() {
        assert_eq!(super::parse_named_cookie("", "barnstormer_session"), None);
    }
```

**Step 2: Run tests, verify they fail**

```bash
cargo test -p barnstormer-server --lib auth::tests::parse_named_cookie -- --nocapture
```

Expected: 5 tests, all fail with `cannot find function 'parse_named_cookie'`.

**Step 3: Write minimal implementation**

Add (above the `#[cfg(test)]` block, after the existing `AuthMiddleware` impl):

```rust
/// Find a named cookie's value in a `Cookie` header value.
/// Returns the first matching value; tolerates surrounding whitespace.
fn parse_named_cookie(header: &str, name: &str) -> Option<String> {
    for part in header.split(';') {
        let (k, v) = part.split_once('=')?;
        if k.trim() == name {
            return Some(v.trim().to_string());
        }
    }
    None
}
```

**Step 4: Run tests, verify they pass**

```bash
cargo test -p barnstormer-server --lib auth::tests::parse_named_cookie
```

Expected: `5 passed`.

**Step 5: Commit**

```bash
git add crates/barnstormer-server/src/auth.rs
git commit -m "feat(auth): parse_named_cookie helper for Cookie header lookup"
```

---

## Task 2: Query token extractor + stripper

**Files:**
- Modify: `crates/barnstormer-server/src/auth.rs` (helper + tests in the existing tests module)

Pull `token=<value>` out of a query string, returning the value AND a rebuilt query string with that param removed (preserving order of remaining params). If no `token=` param exists, the value is `None` and the rebuilt string equals the input.

Edge cases the implementation must handle:
- `token=abc` alone → `("", Some("abc"))`
- `foo=bar&token=abc` → `("foo=bar", Some("abc"))`
- `token=abc&foo=bar` → `("foo=bar", Some("abc"))`
- `foo=bar&token=abc&baz=qux` → `("foo=bar&baz=qux", Some("abc"))`
- `foo=bar` → `("foo=bar", None)`
- `""` → `("", None)`
- `token=` (empty) → `("", Some(""))`

**Step 1: Write the failing test**

Append to the tests module:

```rust
    #[test]
    fn extract_token_param_finds_value() {
        let (rest, token) = super::extract_token_param("token=abc");
        assert_eq!(token, Some("abc".to_string()));
        assert_eq!(rest, "");
    }

    #[test]
    fn extract_token_param_preserves_other_params_before() {
        let (rest, token) = super::extract_token_param("foo=bar&token=abc");
        assert_eq!(token, Some("abc".to_string()));
        assert_eq!(rest, "foo=bar");
    }

    #[test]
    fn extract_token_param_preserves_other_params_after() {
        let (rest, token) = super::extract_token_param("token=abc&foo=bar");
        assert_eq!(token, Some("abc".to_string()));
        assert_eq!(rest, "foo=bar");
    }

    #[test]
    fn extract_token_param_preserves_other_params_around() {
        let (rest, token) = super::extract_token_param("foo=bar&token=abc&baz=qux");
        assert_eq!(token, Some("abc".to_string()));
        assert_eq!(rest, "foo=bar&baz=qux");
    }

    #[test]
    fn extract_token_param_no_token_passes_through() {
        let (rest, token) = super::extract_token_param("foo=bar&baz=qux");
        assert_eq!(token, None);
        assert_eq!(rest, "foo=bar&baz=qux");
    }

    #[test]
    fn extract_token_param_empty_query() {
        let (rest, token) = super::extract_token_param("");
        assert_eq!(token, None);
        assert_eq!(rest, "");
    }

    #[test]
    fn extract_token_param_empty_value() {
        let (rest, token) = super::extract_token_param("token=");
        assert_eq!(token, Some("".to_string()));
        assert_eq!(rest, "");
    }
```

**Step 2: Run tests, verify they fail**

```bash
cargo test -p barnstormer-server --lib auth::tests::extract_token_param
```

Expected: 7 tests, all fail (function undefined).

**Step 3: Write minimal implementation**

Add alongside `parse_named_cookie`:

```rust
/// Extract `token=<value>` from a query string, returning the value (if any)
/// and the query string with the `token` param removed. Other params keep
/// their original order.
fn extract_token_param(query: &str) -> (String, Option<String>) {
    let mut token: Option<String> = None;
    let mut kept: Vec<&str> = Vec::new();
    for part in query.split('&').filter(|p| !p.is_empty()) {
        let (k, v) = part.split_once('=').unwrap_or((part, ""));
        if k == "token" && token.is_none() {
            token = Some(v.to_string());
        } else {
            kept.push(part);
        }
    }
    (kept.join("&"), token)
}
```

Note: we don't URL-decode. The `BARNSTORMER_AUTH_TOKEN` is hex, so this is fine; if the operator ever puts non-hex bytes in the token they'll need to URL-encode them in the magic URL, but that's a non-issue today.

**Step 4: Run tests, verify they pass**

```bash
cargo test -p barnstormer-server --lib auth::tests::extract_token_param
```

Expected: `7 passed`.

**Step 5: Commit**

```bash
git add crates/barnstormer-server/src/auth.rs
git commit -m "feat(auth): extract_token_param helper to lift token query param"
```

---

## Task 3: WebAuthLayer scaffold + path-match logic

**Files:**
- Modify: `crates/barnstormer-server/src/auth.rs`

Add the `WebAuthLayer` and `WebAuthMiddleware` types. The middleware should only intercept requests whose path is `/` or starts with `/web/`. Everything else (`/api/*`, `/static/*`, `/health`) passes through untouched. At this stage, requests that DO match the path still pass through — we wire auth logic in subsequent tasks.

The structure mirrors the existing `AuthLayer`/`AuthMiddleware` pair (see `crates/barnstormer-server/src/auth.rs:12-43`).

**Step 1: Write the failing test**

Append to the tests module:

```rust
    fn web_test_router() -> Router {
        Router::new()
            .route("/", get(|| async { "root" }))
            .route("/web/specs", get(|| async { "web specs" }))
            .route("/web/specs/{id}/events/stream", get(|| async { "sse" }))
            .route("/api/specs", get(|| async { "api specs" }))
            .route("/static/style.css", get(|| async { "css" }))
            .route("/health", get(|| async { "ok" }))
            .layer(WebAuthLayer::new("test-token-123".to_string()))
    }

    #[tokio::test]
    async fn web_layer_passes_static_through() {
        let app = web_test_router();
        let resp = app
            .oneshot(Request::get("/static/style.css").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn web_layer_passes_health_through() {
        let app = web_test_router();
        let resp = app
            .oneshot(Request::get("/health").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn web_layer_passes_api_through() {
        let app = web_test_router();
        // /api/* is the AuthLayer's job; WebAuthLayer must not touch it.
        let resp = app
            .oneshot(Request::get("/api/specs").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }
```

**Step 2: Run tests, verify they fail to compile**

```bash
cargo test -p barnstormer-server --lib auth::tests::web_layer
```

Expected: build error, `WebAuthLayer` not defined.

**Step 3: Write minimal implementation**

After the existing `AuthMiddleware` `Service` impl, before `parse_named_cookie`, add:

```rust
/// A tower Layer that applies cookie-based session auth to `/` and `/web/*`.
/// Browsers cannot attach a Bearer header to EventSource and similar
/// same-origin GETs, so the web UI is gated by a cookie that the user
/// obtains via a one-time `?token=<value>` magic URL.
#[derive(Clone)]
pub struct WebAuthLayer {
    token: Arc<String>,
}

impl WebAuthLayer {
    pub fn new(token: String) -> Self {
        Self {
            token: Arc::new(token),
        }
    }
}

impl<S> Layer<S> for WebAuthLayer {
    type Service = WebAuthMiddleware<S>;

    fn layer(&self, inner: S) -> Self::Service {
        WebAuthMiddleware {
            inner,
            token: Arc::clone(&self.token),
        }
    }
}

#[derive(Clone)]
pub struct WebAuthMiddleware<S> {
    inner: S,
    token: Arc<String>,
}

fn path_is_web(path: &str) -> bool {
    path == "/" || path.starts_with("/web/")
}

impl<S> Service<Request<Body>> for WebAuthMiddleware<S>
where
    S: Service<Request<Body>, Response = Response<Body>> + Clone + Send + 'static,
    S::Future: Send + 'static,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Request<Body>) -> Self::Future {
        let path = req.uri().path().to_string();
        if !path_is_web(&path) {
            let mut inner = self.inner.clone();
            return Box::pin(async move { inner.call(req).await });
        }

        // Subsequent tasks fill in cookie + token-param logic. For now, pass through.
        let mut inner = self.inner.clone();
        Box::pin(async move { inner.call(req).await })
    }
}
```

The pass-through-while-matching behavior is temporary; later tasks replace the second `pass through` block.

**Step 4: Run tests, verify they pass**

```bash
cargo test -p barnstormer-server --lib auth::tests::web_layer
```

Expected: `3 passed`.

**Step 5: Commit**

```bash
git add crates/barnstormer-server/src/auth.rs
git commit -m "feat(auth): WebAuthLayer scaffold matching / and /web/*"
```

---

## Task 4: Reject unauthenticated `/web/*` requests with helpful 401

**Files:**
- Modify: `crates/barnstormer-server/src/auth.rs`

Requests to `/` or `/web/*` with no cookie and no token query param should return `401 Unauthorized` with a small HTML body telling the user how to authenticate. We need this to fail closed BEFORE we add the happy paths, so the rest of the suite has the right behavior to assert against.

**Step 1: Write the failing test**

Append to the tests module:

```rust
    #[tokio::test]
    async fn web_layer_rejects_no_auth_on_root() {
        let app = web_test_router();
        let resp = app
            .oneshot(Request::get("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn web_layer_rejects_no_auth_on_web_path() {
        let app = web_test_router();
        let resp = app
            .oneshot(Request::get("/web/specs").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn web_layer_401_body_mentions_token_param() {
        let app = web_test_router();
        let resp = app
            .oneshot(Request::get("/web/specs").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let body_str = std::str::from_utf8(&body).unwrap();
        assert!(
            body_str.contains("?token="),
            "401 body should mention ?token=, got: {}",
            body_str
        );
    }
```

**Step 2: Run tests, verify they fail**

```bash
cargo test -p barnstormer-server --lib auth::tests::web_layer
```

Expected: the three new tests fail (currently the layer passes through to a 200).

**Step 3: Write minimal implementation**

Replace the trailing pass-through in `WebAuthMiddleware::call` (the `Subsequent tasks fill in...` block) with:

```rust
        // Path matches /web/* — enforce auth. (Cookie + query-token paths land in later tasks.)
        let body = "<html><body><h1>Authentication required</h1>\
            <p>Visit <code>?token=YOUR_TOKEN</code> on any page once to log in.</p>\
            </body></html>";
        let resp = Response::builder()
            .status(StatusCode::UNAUTHORIZED)
            .header("content-type", "text/html; charset=utf-8")
            .body(Body::from(body))
            .unwrap();
        Box::pin(async move { Ok(resp) })
```

**Step 4: Run tests, verify they pass**

```bash
cargo test -p barnstormer-server --lib auth::tests::web_layer
```

Expected: all `web_layer_*` tests pass.

**Step 5: Commit**

```bash
git add crates/barnstormer-server/src/auth.rs
git commit -m "feat(auth): WebAuthLayer fails closed with helpful 401"
```

---

## Task 5: Admit requests with a valid session cookie

**Files:**
- Modify: `crates/barnstormer-server/src/auth.rs`

Add cookie-based admittance: if the `Cookie` header contains `barnstormer_session=<token>` and the value matches the configured token, pass the request through. Wrong/missing cookie still falls through to the 401 from Task 4.

**Step 1: Write the failing tests**

Append to the tests module:

```rust
    #[tokio::test]
    async fn web_layer_accepts_valid_cookie() {
        let app = web_test_router();
        let resp = app
            .oneshot(
                Request::get("/web/specs")
                    .header("cookie", "barnstormer_session=test-token-123")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn web_layer_accepts_valid_cookie_among_others() {
        let app = web_test_router();
        let resp = app
            .oneshot(
                Request::get("/web/specs")
                    .header("cookie", "foo=bar; barnstormer_session=test-token-123; baz=qux")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn web_layer_rejects_wrong_cookie() {
        let app = web_test_router();
        let resp = app
            .oneshot(
                Request::get("/web/specs")
                    .header("cookie", "barnstormer_session=wrong-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }
```

**Step 2: Run tests, verify they fail**

```bash
cargo test -p barnstormer-server --lib auth::tests::web_layer
```

Expected: the new "accepts" tests fail (the layer still 401s). `rejects_wrong_cookie` passes already.

**Step 3: Write minimal implementation**

In `WebAuthMiddleware::call`, just before the "Path matches /web/* — enforce auth" comment, insert:

```rust
        // Cookie-based admittance.
        if let Some(cookie_header) = req
            .headers()
            .get("cookie")
            .and_then(|v| v.to_str().ok())
            && let Some(value) = parse_named_cookie(cookie_header, "barnstormer_session")
            && value == *self.token
        {
            let mut inner = self.inner.clone();
            return Box::pin(async move { inner.call(req).await });
        }
```

**Step 4: Run tests, verify they pass**

```bash
cargo test -p barnstormer-server --lib auth::tests::web_layer
```

Expected: all `web_layer_*` tests pass.

**Step 5: Commit**

```bash
git add crates/barnstormer-server/src/auth.rs
git commit -m "feat(auth): WebAuthLayer admits requests with valid session cookie"
```

---

## Task 6: Magic-URL entry — set cookie and 303 with token stripped

**Files:**
- Modify: `crates/barnstormer-server/src/auth.rs`

If the request URL contains `?token=<value>`:

- Value matches expected token → respond `303 See Other`, `Set-Cookie: barnstormer_session=<token>; HttpOnly; SameSite=Strict; Path=/; Max-Age=2592000`, `Location:` is the same path with the `token` query param removed (other params preserved). The 303 (not 302) ensures the redirected GET is a fresh idempotent request.
- Value mismatches → 401 (no cookie set).

**Step 1: Write the failing tests**

Append to the tests module:

```rust
    #[tokio::test]
    async fn web_layer_query_token_sets_cookie_and_redirects() {
        let app = web_test_router();
        let resp = app
            .oneshot(
                Request::get("/web/specs?token=test-token-123")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
        let location = resp.headers().get("location").unwrap().to_str().unwrap();
        assert_eq!(location, "/web/specs");
        let set_cookie = resp.headers().get("set-cookie").unwrap().to_str().unwrap();
        assert!(set_cookie.starts_with("barnstormer_session=test-token-123"));
        assert!(set_cookie.contains("HttpOnly"));
        assert!(set_cookie.contains("SameSite=Strict"));
        assert!(set_cookie.contains("Path=/"));
        assert!(set_cookie.contains("Max-Age=2592000"));
    }

    #[tokio::test]
    async fn web_layer_query_token_preserves_other_params() {
        let app = web_test_router();
        let resp = app
            .oneshot(
                Request::get("/web/specs?foo=bar&token=test-token-123&baz=qux")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
        let location = resp.headers().get("location").unwrap().to_str().unwrap();
        assert_eq!(location, "/web/specs?foo=bar&baz=qux");
    }

    #[tokio::test]
    async fn web_layer_query_token_on_root() {
        let app = web_test_router();
        let resp = app
            .oneshot(
                Request::get("/?token=test-token-123")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
        let location = resp.headers().get("location").unwrap().to_str().unwrap();
        assert_eq!(location, "/");
    }

    #[tokio::test]
    async fn web_layer_query_token_wrong_value_rejected() {
        let app = web_test_router();
        let resp = app
            .oneshot(
                Request::get("/web/specs?token=wrong-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert!(resp.headers().get("set-cookie").is_none());
    }
```

**Step 2: Run tests, verify they fail**

```bash
cargo test -p barnstormer-server --lib auth::tests::web_layer
```

Expected: four new failures (currently the layer 401s on every query path).

**Step 3: Write minimal implementation**

In `WebAuthMiddleware::call`, AFTER the cookie-admittance block and BEFORE the 401 block, insert:

```rust
        // Magic-URL entry: ?token=<value> mints a cookie.
        let query = req.uri().query().unwrap_or("").to_string();
        let (remaining_query, token_param) = extract_token_param(&query);
        if let Some(provided) = token_param {
            if provided == *self.token {
                let location = if remaining_query.is_empty() {
                    path.clone()
                } else {
                    format!("{}?{}", path, remaining_query)
                };
                let set_cookie = format!(
                    "barnstormer_session={}; HttpOnly; SameSite=Strict; Path=/; Max-Age=2592000",
                    *self.token
                );
                let resp = Response::builder()
                    .status(StatusCode::SEE_OTHER)
                    .header("location", location)
                    .header("set-cookie", set_cookie)
                    .body(Body::empty())
                    .unwrap();
                return Box::pin(async move { Ok(resp) });
            }
            // Wrong token in query → fall through to the 401 below.
        }
```

**Step 4: Run tests, verify they pass**

```bash
cargo test -p barnstormer-server --lib auth::tests::web_layer
```

Expected: all `web_layer_*` tests pass.

**Step 5: Commit**

```bash
git add crates/barnstormer-server/src/auth.rs
git commit -m "feat(auth): WebAuthLayer mints cookie on ?token=<value> entry"
```

---

## Task 7: Wire WebAuthLayer into the router

**Files:**
- Modify: `crates/barnstormer-server/src/routes.rs:125-131` (the `if let Some(token) = auth_token` block)
- Modify: `crates/barnstormer-server/src/routes.rs` (`pub use` exports if needed)

When `auth_token` is `Some`, attach `WebAuthLayer` in addition to the existing `AuthLayer`. When `None`, attach neither (preserves Tauri and local-dev behavior).

**Step 1: Write the failing tests**

The existing tests in `routes.rs` (lines ~182-234) cover the no-auth and with-auth cases for `/api/*`. Add web-side coverage alongside them.

Append to the `mod tests` block in `routes.rs`:

```rust
    #[tokio::test]
    async fn web_route_requires_cookie_when_token_provided() {
        let app =
            create_router(test_state(), Some("secret-token".to_string())).into_service::<Body>();
        let resp = app
            .oneshot(Request::get("/web/specs").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "/web/* should require cookie when auth_token configured"
        );
    }

    #[tokio::test]
    async fn web_route_accepts_valid_cookie_when_token_provided() {
        let app =
            create_router(test_state(), Some("secret-token".to_string())).into_service::<Body>();
        let resp = app
            .oneshot(
                Request::get("/web/specs")
                    .header("cookie", "barnstormer_session=secret-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_ne!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "valid cookie should bypass WebAuthLayer (actual handler may 200 or something else)"
        );
    }

    #[tokio::test]
    async fn web_route_open_when_no_token_configured() {
        let app = create_router(test_state(), None).into_service::<Body>();
        let resp = app
            .oneshot(Request::get("/web/specs").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_ne!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "/web/* should be open without auth_token"
        );
    }

    #[tokio::test]
    async fn static_assets_open_when_token_provided() {
        let app =
            create_router(test_state(), Some("secret-token".to_string())).into_service::<Body>();
        // The /static service serves from a real directory; missing files yield
        // 404 (open), the point is the layer doesn't 401 them.
        let resp = app
            .oneshot(
                Request::get("/static/style.css")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_ne!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "/static/* must stay open"
        );
    }

    #[tokio::test]
    async fn health_open_when_token_provided() {
        let app =
            create_router(test_state(), Some("secret-token".to_string())).into_service::<Body>();
        let resp = app
            .oneshot(Request::get("/health").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }
```

**Step 2: Run tests, verify they fail**

```bash
cargo test -p barnstormer-server --lib routes::tests::web_route
cargo test -p barnstormer-server --lib routes::tests::static_assets_open
cargo test -p barnstormer-server --lib routes::tests::health_open
```

Expected: `web_route_requires_cookie_when_token_provided` fails (currently 200 because /web/* is unguarded). Other "passes-through" tests already pass.

**Step 3: Wire the layer**

In `crates/barnstormer-server/src/routes.rs`, update the import line near the top to include `WebAuthLayer`:

```rust
use crate::auth::{AuthLayer, WebAuthLayer};
```

Then change the trailing block at lines 126-130:

```rust
    if let Some(token) = auth_token {
        router.layer(AuthLayer::new(token))
    } else {
        router
    }
```

to:

```rust
    if let Some(token) = auth_token {
        router
            .layer(AuthLayer::new(token.clone()))
            .layer(WebAuthLayer::new(token))
    } else {
        router
    }
```

**Step 4: Run tests, verify they pass**

```bash
cargo test -p barnstormer-server --lib
```

Expected: all `routes::tests::*` pass; no regression elsewhere in `barnstormer-server`.

**Step 5: Commit**

```bash
git add crates/barnstormer-server/src/auth.rs crates/barnstormer-server/src/routes.rs
git commit -m "feat(server): gate /web/* with WebAuthLayer when auth_token configured"
```

---

## Task 8: Manual end-to-end verification

**Files:** none modified — verification only.

Spin up the real server with the `.env` config (auth token set, bind 0.0.0.0) and walk through the user-visible flow: cold visit, magic-URL entry, bookmarked URL, SSE live update.

**Step 1: Run the full suite**

```bash
cargo test --all 2>&1 | grep -E "(test result|FAILED)" | grep -v "0 failed"
```

Expected: no output (no FAILED lines).

**Step 2: Clippy and fmt**

```bash
cargo clippy --all-targets -- -D warnings
cargo fmt --all -- --check
```

Expected: both pass cleanly. If `cargo fmt --check` fails, run `cargo fmt --all` and stage the fixup.

**Step 3: Start the server**

```bash
cargo run -- start --no-open &
```

Wait for `barnstormer listening on http://0.0.0.0:7331`.

**Step 4: Walk the cold-visit path**

```bash
curl -i http://127.0.0.1:7331/web/specs | head -10
```

Expected: `HTTP/1.1 401 Unauthorized`, body contains `?token=`.

**Step 5: Walk the magic-URL path**

```bash
TOKEN=$(grep BARNSTORMER_AUTH_TOKEN .env | cut -d= -f2)
curl -i "http://127.0.0.1:7331/?token=$TOKEN" | head -10
```

Expected: `HTTP/1.1 303 See Other`, `Location: /`, `Set-Cookie: barnstormer_session=<token>; HttpOnly; SameSite=Strict; Path=/; Max-Age=2592000`.

**Step 6: Walk the cookie path**

```bash
curl -i -H "Cookie: barnstormer_session=$TOKEN" http://127.0.0.1:7331/web/specs | head -10
```

Expected: `HTTP/1.1 200 OK`, real content.

**Step 7: SSE through the cookie**

```bash
curl -i -H "Cookie: barnstormer_session=$TOKEN" --max-time 3 \
  "http://127.0.0.1:7331/web/specs/$(curl -s -H "Authorization: Bearer $TOKEN" \
  http://127.0.0.1:7331/api/specs | python3 -c 'import json,sys; print(json.load(sys.stdin)["specs"][0]["spec_id"])')/events/stream" \
  | head -5
```

Expected: `HTTP/1.1 200 OK`, `content-type: text/event-stream`. Timeout after 3s is the test passing — the stream is open and idle.

**Step 8: Verify /api still wants Bearer (no regression)**

```bash
curl -i http://127.0.0.1:7331/api/specs | head -5
curl -i -H "Cookie: barnstormer_session=$TOKEN" http://127.0.0.1:7331/api/specs | head -5
curl -i -H "Authorization: Bearer $TOKEN" http://127.0.0.1:7331/api/specs | head -5
```

Expected: 401, 401, 200 — `/api/*` accepts only Bearer.

**Step 9: Kill the server**

```bash
kill %1
```

**Step 10: (Optional) Remote browser smoke**

On your other box: hit `http://2389-mini:7331/?token=<token>` once → bookmark `http://2389-mini:7331/web/specs/<id>` → open it cold → submit a chat → confirm the manager's response appears without a hard refresh (this is the original bug).

**Step 11: Commit any fmt fixups**

If `cargo fmt --all` produced changes during Step 2:

```bash
git add -u
git commit -m "style: cargo fmt"
```

---

## Done criteria

- All unit tests pass.
- `cargo clippy --all-targets -- -D warnings` is clean.
- Manual curl walkthrough in Task 8 matches expected output at every step.
- The original symptom — submit a chat → updates only appear on hard refresh — is gone when accessed remotely via cookie.

## Follow-ups (explicitly NOT in this plan)

- CSRF on `/web/*` POSTs.
- Cookie attribute audit if/when we add HTTPS (`Secure` flag).
- Replacing raw-token cookie with HMAC-signed session if the threat model broadens.
