//! === EMBEDDED LOCALHOST WEB CANVAS SERVER ===
//!
//! PURPOSE:
//! Serves the embedded `frontend/dist` Web Canvas SPA (`rust-embed`) and
//! `GET /api/health` exclusively on `127.0.0.1:<port>` (`imgforge ui`).
//!
//! THREAT MODEL / DESIGN RATIONALE ("WHY"):
//! 1. Binding strictly to `127.0.0.1:<port>` prevents exposing the local asset
//!    workspace and interactive tools to LAN or public interfaces.
//! 2. `/api/*` misses return structured 404 JSON rather than falling back to
//!    `index.html`, ensuring client API callers fail closed on unknown routes.
//! 3. `GET /api/health` checks local `auth.json` provisioning without spawning
//!    a subprocess so UI polling remains lightweight and deterministic.
//!
//! EXECUTION LIFECYCLE / DATA FLOW:
//! 1. `build_router(state)` wires `GET /api/health`, `/api/{*rest}`, and SPA fallback.
//! 2. `serve_ui(state, port)` binds `127.0.0.1:<port>` and runs `axum::serve`
//!    until Ctrl+C shutdown.

use std::net::SocketAddr;
use std::path::PathBuf;

use axum::Json;
use axum::Router;
use axum::extract::{Request, State};
use axum::http::{HeaderValue, StatusCode, Uri, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get};
use imgforge_core::{AuthManager, BackendKind, DEFAULT_CODEX_BIN};
use rust_embed::RustEmbed;
use serde::{Deserialize, Serialize};
use tokio::net::TcpListener;

/// Strict Content-Security-Policy applied to all responses from the localhost UI server.
pub const CONTENT_SECURITY_POLICY: &str = "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; connect-src 'self'; object-src 'none'; base-uri 'self'; frame-ancestors 'none'";

/// Embedded `frontend/dist` static bundle compiled into the single Rust binary.
#[derive(RustEmbed)]
#[folder = "../../frontend/dist"]
pub struct EmbeddedSpaAssets;

/// Health status enum matching `frontend/src/types/domain.ts` (`'ok' | 'degraded'`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HealthStatusKind {
    /// Local server and tool-local Codex OAuth credentials are ready.
    Ok,
    /// Local server is running, but tool-local Codex OAuth credentials are missing.
    Degraded,
}

/// JSON contract returned by `GET /api/health` (verified by `frontend/src/api/health.ts`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthResponse {
    /// Overall server readiness (`"ok"` or `"degraded"`).
    pub status: HealthStatusKind,
    /// `imgforge` binary version string.
    pub version: String,
    /// Active backend identifier (`"codex-native"`).
    pub backend: String,
    /// True when tool-local `auth.json` is provisioned and valid.
    pub auth_provisioned: bool,
}

/// Shared state injected into `axum` handlers.
#[derive(Debug, Clone)]
pub struct AppState {
    /// Tool-local Codex auth directory checked by `GET /api/health`.
    pub auth_dir: PathBuf,
    /// Binary semantic version.
    pub version: String,
    /// Active backend identifier.
    pub backend: String,
}

impl AppState {
    /// Construct default `AppState` for the compiled `imgforge` version and `codex-native` backend.
    #[must_use]
    pub fn new(auth_dir: PathBuf) -> Self {
        Self {
            auth_dir,
            version: env!("CARGO_PKG_VERSION").to_string(),
            backend: BackendKind::CodexNative.as_str().to_string(),
        }
    }
}

fn is_allowed_host_header(raw_host: &str) -> bool {
    let trimmed = raw_host.trim();
    if trimmed.is_empty() {
        return false;
    }

    let host_only = if let Some(rest) = trimmed.strip_prefix('[') {
        let Some((addr, after)) = rest.split_once(']') else {
            return false;
        };
        if !after.is_empty() {
            let Some(port_str) = after.strip_prefix(':') else {
                return false;
            };
            if !port_str.parse::<u16>().is_ok_and(|port| port > 0) {
                return false;
            }
        }
        return addr == "::1";
    } else if let Some((host, port_str)) = trimmed.rsplit_once(':') {
        if !port_str.parse::<u16>().is_ok_and(|port| port > 0) {
            return false;
        }
        host
    } else {
        trimmed
    };

    host_only.eq_ignore_ascii_case("localhost") || host_only == "127.0.0.1"
}

async fn security_headers_and_host_guard(request: Request, next: Next) -> Response {
    let mut host_values = request.headers().get_all(header::HOST).iter();
    let allowed = match (host_values.next(), host_values.next()) {
        (Some(val), None) => val.to_str().ok().is_some_and(is_allowed_host_header),
        _ => false,
    };

    let mut response = if allowed {
        next.run(request).await
    } else {
        (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": "forbidden_host" })),
        )
            .into_response()
    };

    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(CONTENT_SECURITY_POLICY),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    response
}

/// Construct the `axum::Router` for `GET /api/health` and embedded SPA static serving.
pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/api/health", get(health_handler))
        .route("/api", any(api_not_found))
        .route("/api/", any(api_not_found))
        .route("/api/{*rest}", any(api_not_found))
        .fallback(serve_spa)
        .layer(middleware::from_fn(security_headers_and_host_guard))
        .with_state(state)
}

async fn health_handler(State(state): State<AppState>) -> Json<HealthResponse> {
    let auth_manager = AuthManager::new(state.auth_dir.clone(), DEFAULT_CODEX_BIN);
    let auth_provisioned = auth_manager.is_provisioned_locally();
    let status = if auth_provisioned {
        HealthStatusKind::Ok
    } else {
        HealthStatusKind::Degraded
    };

    Json(HealthResponse {
        status,
        version: state.version,
        backend: state.backend,
        auth_provisioned,
    })
}

async fn api_not_found() -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({ "error": "not_found" })),
    )
}

async fn serve_spa(uri: Uri) -> Response {
    let trimmed = uri.path().trim_start_matches('/');
    let requested_path = if trimmed.is_empty() {
        "index.html"
    } else {
        trimmed
    };

    if let Some(asset) = EmbeddedSpaAssets::get(requested_path) {
        return asset_response(requested_path, asset.data.into_owned());
    }

    // Client-side SPA route fallback for extensionless paths; explicit missing assets return 404.
    let has_extension = requested_path
        .rsplit('/')
        .next()
        .is_some_and(|segment| segment.contains('.'));
    if !has_extension && let Some(index) = EmbeddedSpaAssets::get("index.html") {
        return asset_response("index.html", index.data.into_owned());
    }

    (StatusCode::NOT_FOUND, "Not Found").into_response()
}

fn asset_response(path: &str, bytes: Vec<u8>) -> Response {
    let mime = mime_guess::from_path(path).first_or_octet_stream();
    let mut response = (StatusCode::OK, bytes).into_response();
    if let Ok(content_type) = HeaderValue::from_str(mime.as_ref()) {
        response
            .headers_mut()
            .insert(header::CONTENT_TYPE, content_type);
    }
    response.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    response
}

/// Bind strictly to `127.0.0.1:<port>` and serve the embedded Web Canvas SPA.
pub async fn serve_ui(state: AppState, port: u16) -> anyhow::Result<()> {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let listener = TcpListener::bind(addr).await?;
    let bound_addr = listener.local_addr()?;
    tracing::info!("imgforge Web Canvas UI listening on http://{bound_addr}");
    println!("imgforge Web Canvas UI listening on http://{bound_addr}");

    let app = build_router(state);
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    use axum::body::Body;
    use axum::http::{Method, Request};
    use http_body_util::BodyExt;
    use rstest::rstest;
    use tempfile::TempDir;
    use tower::ServiceExt;

    use super::*;

    #[rstest]
    #[case::auth_provisioned_returns_ok(true, HealthStatusKind::Ok)]
    #[case::auth_missing_returns_degraded(false, HealthStatusKind::Degraded)]
    #[tokio::test]
    async fn api_health_matches_frontend_contract(
        #[case] provision_auth: bool,
        #[case] expected_status: HealthStatusKind,
    ) {
        let dir = TempDir::new().expect("tempdir");
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
        if provision_auth {
            let auth_file = dir.path().join("auth.json");
            fs::write(&auth_file, r#"{"token":"ok"}"#).unwrap();
            fs::set_permissions(&auth_file, fs::Permissions::from_mode(0o600)).unwrap();
        }

        let app = build_router(AppState::new(dir.path().to_path_buf()));
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/health")
                    .header(header::HOST, "127.0.0.1:8080")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("router response");

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get(header::CONTENT_SECURITY_POLICY)
                .and_then(|v| v.to_str().ok()),
            Some(CONTENT_SECURITY_POLICY)
        );
        let body_bytes = response.into_body().collect().await.unwrap().to_bytes();
        let parsed: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();

        assert_eq!(parsed["authProvisioned"], serde_json::json!(provision_auth));
        assert_eq!(parsed["backend"], serde_json::json!("codex-native"));
        assert!(parsed["version"].as_str().is_some_and(|v| !v.is_empty()));

        let decoded: HealthResponse = serde_json::from_value(parsed).unwrap();
        assert_eq!(decoded.status, expected_status);
    }

    #[tokio::test]
    async fn embedded_spa_serves_index_and_rejects_unknown_api_and_missing_assets() {
        let dir = TempDir::new().expect("tempdir");
        let app = build_router(AppState::new(dir.path().to_path_buf()));

        let root_resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/")
                    .header(header::HOST, "127.0.0.1:8080")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(root_resp.status(), StatusCode::OK);
        let content_type = root_resp
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        assert!(content_type.contains("text/html"));
        assert_eq!(
            root_resp
                .headers()
                .get(header::CONTENT_SECURITY_POLICY)
                .and_then(|v| v.to_str().ok()),
            Some(CONTENT_SECURITY_POLICY)
        );
        let html = root_resp.into_body().collect().await.unwrap().to_bytes();
        let html_str = String::from_utf8_lossy(&html);
        assert!(html_str.contains("<!doctype html>") || html_str.contains("<!DOCTYPE html>"));

        let asset_miss = app
            .oneshot(
                Request::builder()
                    .uri("/assets/nonexistent-bundle.js")
                    .header(header::HOST, "127.0.0.1:8080")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(asset_miss.status(), StatusCode::NOT_FOUND);
    }

    #[rstest]
    #[case::get_api_root(Method::GET, "/api")]
    #[case::post_api_root(Method::POST, "/api")]
    #[case::get_api_trailing_slash(Method::GET, "/api/")]
    #[case::get_api_unknown(Method::GET, "/api/unknown")]
    #[case::post_api_unknown(Method::POST, "/api/unknown/nested")]
    #[case::delete_api_unknown(Method::DELETE, "/api/runs/1")]
    #[tokio::test]
    async fn api_unknown_routes_return_json_404_across_methods(
        #[case] method: Method,
        #[case] path: &str,
    ) {
        let dir = TempDir::new().expect("tempdir");
        let app = build_router(AppState::new(dir.path().to_path_buf()));

        let resp = app
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .header(header::HOST, "localhost:8080")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed["error"], serde_json::json!("not_found"));
    }

    #[rstest]
    #[case::loopback_ipv4_with_port(Some("127.0.0.1:8080"), StatusCode::OK)]
    #[case::loopback_ipv4_without_port(Some("127.0.0.1"), StatusCode::OK)]
    #[case::localhost_with_port(Some("localhost:3000"), StatusCode::OK)]
    #[case::localhost_case_insensitive(Some("LOCALHOST"), StatusCode::OK)]
    #[case::loopback_ipv6_with_port(Some("[::1]:8080"), StatusCode::OK)]
    #[case::loopback_ipv6_without_port(Some("[::1]"), StatusCode::OK)]
    #[case::missing_host_rejected(None, StatusCode::FORBIDDEN)]
    #[case::dns_rebinding_host_rejected(Some("evil.example:8080"), StatusCode::FORBIDDEN)]
    #[case::suffix_spoof_host_rejected(Some("localhost.evil.example:8080"), StatusCode::FORBIDDEN)]
    #[case::lan_ip_rejected(Some("192.168.1.50:8080"), StatusCode::FORBIDDEN)]
    #[case::other_ipv6_rejected(Some("[::2]:8080"), StatusCode::FORBIDDEN)]
    #[case::zero_port_rejected(Some("127.0.0.1:0"), StatusCode::FORBIDDEN)]
    #[case::invalid_port_rejected(Some("127.0.0.1:notaport"), StatusCode::FORBIDDEN)]
    #[tokio::test]
    async fn enforces_loopback_host_header_allowlist(
        #[case] host_header: Option<&str>,
        #[case] expected_status: StatusCode,
    ) {
        let dir = TempDir::new().expect("tempdir");
        let app = build_router(AppState::new(dir.path().to_path_buf()));

        let mut builder = Request::builder().uri("/api/health");
        if let Some(host) = host_header {
            builder = builder.header(header::HOST, host);
        }
        let resp = app
            .oneshot(builder.body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), expected_status);
    }
}
