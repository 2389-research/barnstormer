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
}
