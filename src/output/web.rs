//! Built-in web dashboard (`web/`), embedded in the binary at compile time.
//!
//! Static assets are public; the page reads data from `/api/v1/*` and sends
//! the bearer token itself, so the API's protection is unchanged. The strict
//! CSP allows no inline script, no remote origins and no framing.

use axum::{
    Router,
    http::{HeaderValue, header},
    response::{IntoResponse, Response},
    routing::get,
};

const INDEX: &str = include_str!("../../web/index.html");
const CSS: &str = include_str!("../../web/app.css");
const JS: &str = include_str!("../../web/app.js");
const ICON: &str = include_str!("../../web/icon.svg");

const CSP: &str = "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self' data:; \
                   connect-src 'self'; font-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'";

fn asset(body: &'static str, content_type: &'static str) -> Response {
    (
        [
            (header::CONTENT_TYPE, HeaderValue::from_static(content_type)),
            (
                header::CONTENT_SECURITY_POLICY,
                HeaderValue::from_static(CSP),
            ),
            (header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY")),
            (
                header::REFERRER_POLICY,
                HeaderValue::from_static("no-referrer"),
            ),
        ],
        body,
    )
        .into_response()
}

pub fn router<S: Clone + Send + Sync + 'static>() -> Router<S> {
    Router::new()
        .route(
            "/",
            get(|| async { asset(INDEX, "text/html; charset=utf-8") }),
        )
        .route(
            "/app.css",
            get(|| async { asset(CSS, "text/css; charset=utf-8") }),
        )
        .route(
            "/app.js",
            get(|| async { asset(JS, "text/javascript; charset=utf-8") }),
        )
        .route("/icon.svg", get(|| async { asset(ICON, "image/svg+xml") }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    #[tokio::test]
    async fn serves_assets_with_csp() {
        let app: Router = router();
        for (path, ct) in [
            ("/", "text/html"),
            ("/app.js", "text/javascript"),
            ("/app.css", "text/css"),
        ] {
            let res = app
                .clone()
                .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(res.status(), StatusCode::OK, "{path}");
            assert!(
                res.headers()[header::CONTENT_TYPE]
                    .to_str()
                    .unwrap()
                    .starts_with(ct)
            );
            assert!(
                res.headers()[header::CONTENT_SECURITY_POLICY]
                    .to_str()
                    .unwrap()
                    .contains("script-src 'self'")
            );
        }
    }

    #[test]
    fn page_has_no_inline_script_or_remote_resources() {
        assert!(
            !INDEX.contains("<script>"),
            "inline scripts are blocked by CSP"
        );
        for text in [INDEX, CSS, JS] {
            assert!(
                !text.contains("http://") && !text.contains("https://"),
                "no remote resources"
            );
        }
        assert!(
            !JS.contains("innerHTML"),
            "render untrusted text with textContent only"
        );
    }
}
