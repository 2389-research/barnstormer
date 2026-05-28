// ABOUTME: Bearer token authentication middleware for the barnstormer API.
// ABOUTME: Checks Authorization header on /api/* routes, exempts web UI and static routes.

use axum::body::Body;
use axum::http::{Request, Response, StatusCode};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use tower::{Layer, Service};

/// A tower Layer that applies bearer token authentication to API routes.
#[derive(Clone)]
pub struct AuthLayer {
    token: Arc<String>,
}

impl AuthLayer {
    /// Create a new AuthLayer with the expected bearer token.
    pub fn new(token: String) -> Self {
        Self {
            token: Arc::new(token),
        }
    }
}

impl<S> Layer<S> for AuthLayer {
    type Service = AuthMiddleware<S>;

    fn layer(&self, inner: S) -> Self::Service {
        AuthMiddleware {
            inner,
            token: Arc::clone(&self.token),
        }
    }
}

/// The middleware service that checks bearer tokens on /api/* routes.
#[derive(Clone)]
pub struct AuthMiddleware<S> {
    inner: S,
    token: Arc<String>,
}

impl<S> Service<Request<Body>> for AuthMiddleware<S>
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

        // Only authenticate /api and /api/* routes
        if !(path == "/api" || path.starts_with("/api/")) {
            let mut inner = self.inner.clone();
            return Box::pin(async move { inner.call(req).await });
        }

        // Check for Authorization: Bearer <token>
        let auth_header = req
            .headers()
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .map(|v| v.to_string());

        let expected = format!("Bearer {}", self.token);

        match auth_header {
            Some(ref header) if *header == expected => {
                let mut inner = self.inner.clone();
                Box::pin(async move { inner.call(req).await })
            }
            _ => Box::pin(async move {
                let body = serde_json::json!({ "error": "unauthorized" });
                let resp = Response::builder()
                    .status(StatusCode::UNAUTHORIZED)
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap();
                Ok(resp)
            }),
        }
    }
}

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

        // Magic-URL entry: ?token=<value> mints a cookie.
        // Wrong token in query → fall through to the 401 below.
        let query = req.uri().query().unwrap_or("").to_string();
        let (remaining_query, token_param) = extract_token_param(&query);
        if let Some(provided) = token_param
            && provided == *self.token
        {
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
    }
}

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

/// Extract `token=<value>` from a query string, returning the value (if any)
/// and the query string with every `token` param removed. If `token=` appears
/// more than once, the first value is returned and all occurrences are
/// stripped. Other params keep their original order.
fn extract_token_param(query: &str) -> (String, Option<String>) {
    let mut token: Option<String> = None;
    let mut kept: Vec<&str> = Vec::new();
    for part in query.split('&').filter(|p| !p.is_empty()) {
        let (k, v) = part.split_once('=').unwrap_or((part, ""));
        if k == "token" {
            if token.is_none() {
                token = Some(v.to_string());
            }
        } else {
            kept.push(part);
        }
    }
    (kept.join("&"), token)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Router;
    use axum::routing::get;
    use http::Request;
    use tower::ServiceExt;

    fn test_router() -> Router {
        Router::new()
            .route("/api/specs", get(|| async { "specs" }))
            .route("/", get(|| async { "index" }))
            .route("/web/specs", get(|| async { "web specs" }))
            .route("/health", get(|| async { "ok" }))
            .layer(AuthLayer::new("test-token-123".to_string()))
    }

    #[tokio::test]
    async fn auth_middleware_rejects_without_token() {
        let app = test_router();

        let resp = app
            .oneshot(Request::get("/api/specs").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn auth_middleware_allows_with_valid_token() {
        let app = test_router();

        let resp = app
            .oneshot(
                Request::get("/api/specs")
                    .header("authorization", "Bearer test-token-123")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn auth_middleware_rejects_with_wrong_token() {
        let app = test_router();

        let resp = app
            .oneshot(
                Request::get("/api/specs")
                    .header("authorization", "Bearer wrong-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn auth_middleware_exempts_web_routes() {
        let app = test_router();

        let resp = app
            .oneshot(Request::get("/web/specs").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn auth_middleware_exempts_index() {
        let app = test_router();

        let resp = app
            .oneshot(Request::get("/").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn auth_middleware_protects_api_without_trailing_slash() {
        let app = Router::new()
            .route("/api", get(|| async { "api root" }))
            .route("/api/specs", get(|| async { "specs" }))
            .layer(AuthLayer::new("test-token-123".to_string()));

        let resp = app
            .oneshot(Request::get("/api").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "/api should be protected by auth"
        );
    }

    #[tokio::test]
    async fn auth_middleware_exempts_health() {
        let app = test_router();

        let resp = app
            .oneshot(Request::get("/health").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
    }

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

    #[tokio::test]
    async fn web_layer_rejects_cookie_header_without_session() {
        let app = web_test_router();
        let resp = app
            .oneshot(
                Request::get("/web/specs")
                    .header("cookie", "foo=bar; baz=qux")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn web_layer_accepts_cookie_on_sse_route() {
        // SSE is the original motivation for cookie auth: browsers' EventSource
        // can't attach a Bearer header but does send cookies on same-origin
        // connections. Pin that the layer admits the SSE GET with a valid cookie.
        let app = web_test_router();
        let resp = app
            .oneshot(
                Request::get("/web/specs/abc/events/stream")
                    .header("cookie", "barnstormer_session=test-token-123")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

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

    #[test]
    fn extract_token_param_skips_empty_leading_segment() {
        let (rest, token) = super::extract_token_param("&token=abc");
        assert_eq!(token, Some("abc".to_string()));
        assert_eq!(rest, "");
    }

    #[test]
    fn extract_token_param_passes_through_bare_key() {
        let (rest, token) = super::extract_token_param("flag&token=abc");
        assert_eq!(token, Some("abc".to_string()));
        assert_eq!(rest, "flag");
    }

    #[test]
    fn extract_token_param_duplicate_token_strips_all_first_wins() {
        let (rest, token) = super::extract_token_param("token=abc&token=def&foo=bar");
        assert_eq!(
            token,
            Some("abc".to_string()),
            "first token value should win"
        );
        assert_eq!(
            rest, "foo=bar",
            "all token params should be stripped from output"
        );
    }

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
}
