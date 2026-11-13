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
use axum::extract::State;
use axum::http::{HeaderValue, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use imgforge_core::{AuthManager, DEFAULT_CODEX_BIN};
use rust_embed::RustEmbed;
use serde::{Deserialize, Serialize};
use tokio::net::TcpListener;

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
            backend: "codex-native".to_string(),
        }
    }
}

/// Construct the `axum::Router` for `GET /api/health` and embedded SPA static serving.
pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/api/health", get(health_handler))
        .route("/api/{*rest}", get(api_not_found))
        .fallback(serve_spa)
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

    use axum::body::Body;
    use axum::http::Request;
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
        if provision_auth {
            fs::write(dir.path().join("auth.json"), r#"{"token":"ok"}"#).unwrap();
        }

        let app = build_router(AppState::new(dir.path().to_path_buf()));
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("router response");

        assert_eq!(response.status(), StatusCode::OK);
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
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(root_resp.status(), StatusCode::OK);
        let content_type = root_resp
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        assert!(content_type.contains("text/html"));
        let html = root_resp.into_body().collect().await.unwrap().to_bytes();
        let html_str = String::from_utf8_lossy(&html);
        assert!(html_str.contains("<!doctype html>") || html_str.contains("<!DOCTYPE html>"));

        let api_miss = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/unknown")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(api_miss.status(), StatusCode::NOT_FOUND);

        let asset_miss = app
            .oneshot(
                Request::builder()
                    .uri("/assets/nonexistent-bundle.js")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(asset_miss.status(), StatusCode::NOT_FOUND);
    }
}
