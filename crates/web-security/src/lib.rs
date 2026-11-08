//! What every `Maya2C` HTTP service sends, defined once.
//!
//! [`secure`] puts the browser-hardening headers on every response a router
//! produces. [`cors_from_env`] is the CORS policy for the public read-only
//! services, overridable with [`CORS_ENV`]. [`security_txt`] serves the
//! RFC 9116 disclosure file.

use axum::Router;
use axum::http::{HeaderName, HeaderValue, Method, header};
use axum::routing::get;
use tower_http::cors::{AllowOrigin, CorsLayer};
use tower_http::set_header::SetResponseHeaderLayer;

/// Environment variable naming the browser origins allowed to read a service,
/// comma-separated. Unset or empty means any origin.
pub const CORS_ENV: &str = "MAYA2C_CORS_ALLOWED_ORIGINS";

/// The disclosure file body. Points at GitHub's private advisory form so no
/// mailbox address has to be published. Expires one year after it was written;
/// RFC 9116 requires a renewal before then.
pub const SECURITY_TXT: &str = "Contact: https://github.com/EricWijesinghe/Maya2C/security/advisories/new\n\
Expires: 2027-11-08T00:00:00.000Z\n\
Preferred-Languages: en\n";

/// Wraps `router` so every response carries the hardening headers.
///
/// Headers a handler sets itself are left alone (`if_not_present`), so a route
/// that needs a different policy can still have one. `Server` is overwritten
/// rather than removed: hyper adds none, and a generic value keeps a proxy
/// from filling it in with the software name.
pub fn secure<S>(router: Router<S>, content_security_policy: &'static str) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    router
        .layer(if_absent("x-content-type-options", "nosniff"))
        .layer(if_absent("x-frame-options", "DENY"))
        .layer(if_absent("referrer-policy", "no-referrer"))
        .layer(if_absent("cross-origin-opener-policy", "same-origin"))
        .layer(if_absent(
            "permissions-policy",
            "geolocation=(), microphone=(), camera=(), payment=(), usb=()",
        ))
        .layer(if_absent(
            "content-security-policy",
            content_security_policy,
        ))
        .layer(SetResponseHeaderLayer::overriding(
            header::SERVER,
            HeaderValue::from_static("maya2c"),
        ))
}

fn if_absent(name: &'static str, value: &'static str) -> SetResponseHeaderLayer<HeaderValue> {
    SetResponseHeaderLayer::if_not_present(
        HeaderName::from_static(name),
        HeaderValue::from_static(value),
    )
}

/// Serves `/.well-known/security.txt` as plain text.
pub fn security_txt<S>() -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    Router::new().route(
        "/.well-known/security.txt",
        get(|| async {
            (
                [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
                SECURITY_TXT,
            )
        }),
    )
}

/// CORS for a public, read-only service: the origins listed in [`CORS_ENV`],
/// or any origin when that is unset or empty.
///
/// `Any` is acceptable only for data anyone may read. Credentials stay
/// disallowed (the default), so a hostile page gets nothing it could not
/// already fetch itself. An invalid listed origin is dropped with a warning;
/// if none remain, no origin is allowed (fail closed, never fall back to `Any`).
pub fn cors_from_env(methods: impl IntoIterator<Item = Method>) -> CorsLayer {
    let raw = std::env::var(CORS_ENV).unwrap_or_default();
    let listed: Vec<&str> = raw
        .split(',')
        .map(str::trim)
        .filter(|origin| !origin.is_empty())
        .collect();
    let origins = if listed.is_empty() {
        AllowOrigin::any()
    } else {
        AllowOrigin::list(listed.into_iter().filter_map(|origin| {
            HeaderValue::from_str(origin)
                .inspect_err(|_| tracing::warn!(%origin, "ignoring invalid CORS origin"))
                .ok()
        }))
    };
    CorsLayer::new()
        .allow_origin(origins)
        .allow_methods(methods.into_iter().collect::<Vec<_>>())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::routing::get;
    use tower::ServiceExt;

    async fn headers_of(app: Router) -> axum::http::HeaderMap {
        app.oneshot(Request::get("/x").body(Body::empty()).unwrap())
            .await
            .unwrap()
            .headers()
            .clone()
    }

    #[tokio::test]
    async fn every_response_carries_the_hardening_headers() {
        let app = secure(
            Router::new().route("/x", get(|| async { "ok" })),
            "default-src 'none'",
        );
        let headers = headers_of(app).await;
        assert_eq!(headers["x-content-type-options"], "nosniff");
        assert_eq!(headers["x-frame-options"], "DENY");
        assert_eq!(headers["referrer-policy"], "no-referrer");
        assert_eq!(headers["cross-origin-opener-policy"], "same-origin");
        assert_eq!(headers["content-security-policy"], "default-src 'none'");
        assert!(
            headers["permissions-policy"]
                .to_str()
                .unwrap()
                .contains("camera=()")
        );
    }

    #[tokio::test]
    async fn the_fallback_answer_is_covered_too() {
        let app = secure(Router::new(), "default-src 'none'");
        let response = app
            .oneshot(Request::get("/missing").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(response.headers()["x-frame-options"], "DENY");
    }

    #[tokio::test]
    async fn server_name_is_replaced_not_leaked() {
        let app = secure(
            Router::new().route(
                "/x",
                get(|| async { ([(header::SERVER, "nginx/1.27.0")], "ok") }),
            ),
            "default-src 'none'",
        );
        assert_eq!(headers_of(app).await["server"], "maya2c");
    }

    #[tokio::test]
    async fn a_handler_may_set_its_own_policy() {
        let app = secure(
            Router::new().route(
                "/x",
                get(|| async { ([("content-security-policy", "script-src 'none'")], "ok") }),
            ),
            "default-src 'none'",
        );
        assert_eq!(
            headers_of(app).await["content-security-policy"],
            "script-src 'none'"
        );
    }

    #[tokio::test]
    async fn security_txt_is_served_as_plain_text() {
        let response = security_txt::<()>()
            .oneshot(
                Request::get("/.well-known/security.txt")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()["content-type"],
            "text/plain; charset=utf-8"
        );
        let body = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        assert_eq!(body.as_ref(), SECURITY_TXT.as_bytes());
    }

    #[test]
    fn security_txt_has_the_required_fields() {
        assert!(SECURITY_TXT.contains("Contact: https://"));
        assert!(SECURITY_TXT.contains("Expires: 2027-"));
        assert!(
            !SECURITY_TXT.contains('@'),
            "no mailbox address in the file"
        );
    }
}
