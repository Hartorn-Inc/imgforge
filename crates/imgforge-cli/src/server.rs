//! === EMBEDDED LOCALHOST WEB CANVAS REST & WEBSOCKET SERVER ===
//!
//! PURPOSE:
//! Serves the embedded `frontend/dist` Web Canvas SPA (`rust-embed`) and the
//! local REST + WebSocket API (`/api/*`, `/api/ws`) exclusively on
//! `127.0.0.1:<port>` (`imgforge ui`).
//!
//! THREAT MODEL / DESIGN RATIONALE ("WHY"):
//! 1. Binding strictly to `127.0.0.1:<port>` prevents exposing the local asset
//!    workspace and interactive tools to LAN or public interfaces.
//! 2. Strict `Host` allowlist (`127.0.0.1`, `localhost`, `[::1]`) blocks DNS
//!    rebinding attacks, and strict `Origin` validation on all mutating routes
//!    (`POST`, `PUT`, `PATCH`, `DELETE`) and `/api/ws` WebSocket upgrades blocks
//!    cross-origin browser requests (`403 Forbidden`).
//! 3. Every workspace path/asset parameter passes `validate_no_traversal_or_nul`,
//!    `verify_contained_path_no_symlinks`, and `O_NOFOLLOW` regular-file reads;
//!    `GET /api/assets/{*path}` additionally restricts extensions to raster images
//!    and refuses `auth.json` or non-image files.
//! 4. `Lifecycle::Canonical` immutability and `Visibility` boundaries are
//!    enforced via `imgforge-core` before any mutation or `Export` execution.
//!
//! EXECUTION LIFECYCLE / DATA FLOW:
//! 1. `build_router(state)` wires security middleware, bounded body limits, all
//!    REST + WS `/api/*` endpoints, structured JSON 404 for unknown `/api/*`,
//!    and SPA fallback.
//! 2. Mutating/lifecycle handlers delegate domain math and persistence to
//!    `imgforge-core` and broadcast `WorkspaceEvent`s to connected `/api/ws`
//!    subscribers.

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::net::SocketAddr;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use axum::Json;
use axum::Router;
use axum::body::Bytes;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{DefaultBodyLimit, Path as AxumPath, Request, State};
use axum::http::{HeaderValue, Method, StatusCode, Uri, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get, post};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use imgforge_core::{
    Atmosphere, AuthManager, BackendKind, CARD_FILENAME, CodexNativeBackend, ColorQaEngine,
    ColorQaReport, CoreError, DEFAULT_CDF_DAMP_STRENGTH, DEFAULT_COLLAR_WIDTH_PX,
    DEFAULT_TOKEN_RING_HEX, DEFAULT_VTT_MAP_GRID_PX, DEFAULT_VTT_TOKEN_SIZE_PX, ExportProfile,
    ExportRequest, ExportSource, ExposureFloor, ExposureQaConfig, GeneratedSheetRecord,
    GuideRenderer, GuideSpec, HeatmapKind, ImageBackend, LibraryRegistry, LibrarySource, Lifecycle,
    MAX_STYLE_EXEMPLARS, MIN_STYLE_EXEMPLARS, ManifestEntry, ManifestEntryKind, ManifestLock,
    ManifestLockStore, MatchGradeOptions, OverrideMode, OwnedSubjectRoi, PNG_SIGNATURE,
    PaletteQaConfig, PaletteSwatch, PatchEditConfig, PatchEditRequest, PatchPrepMode,
    PatchTargetMask, PixelRect, RectOverride, ReferencePackManifest, ReferenceSheetRole,
    ResolvedAtmosphere, ResolvedStyle, ResolvedSubject, RunMetadata, RunStore, SliceRecord,
    SlicesManifest, SparkleConfig, Style, Subject, SubjectRoiBinding, TraitCategory,
    TraitCheckInput, UntouchedDriftConfig, ValidateReferencePackRequest, ValidateRunRequest,
    Visibility, VisionCritic, VttMapOptions, VttTokenOptions, WorkspacePathsConfig,
    check_visibility_compatibility, compute_l_star_histogram_and_vectorscope, decode_png_checked,
    execute_export, execute_patch_edit, inspect_workspace_status, list_workspace_reference_packs,
    load_and_verify_style_exemplars, now_utc_iso, promote_reference_pack, promote_run,
    read_regular_file_nofollow, reslice_reference_pack_sheet, resolve_user_config_dir_from_env,
    resolve_workspace, sha256_hex, validate_reference_pack, validate_run, validate_slug,
    verify_contained_path_no_symlinks, verify_existing_dir_prefix_no_symlinks,
    verify_optional_contained_dir_no_symlinks, write_atomic,
};
use rust_embed::RustEmbed;
use serde::{Deserialize, Serialize};
use tempfile::TempDir;
use tokio::net::TcpListener;
use tokio::sync::broadcast;

/// Strict Content-Security-Policy applied to all responses from the localhost UI server.
pub const CONTENT_SECURITY_POLICY: &str = "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; connect-src 'self' ws://127.0.0.1:* ws://localhost:* ws://[::1]:*; object-src 'none'; base-uri 'self'; frame-ancestors 'none'";

/// Maximum request body size accepted by `/api/*` endpoints (`24 MiB`).
pub const MAX_API_BODY_BYTES: usize = 24 * 1024 * 1024;

/// Maximum raw PNG bytes accepted for uploaded base64 exemplars or PatchEdit masks (`16 MiB`).
pub const MAX_UPLOAD_PNG_BYTES: usize = 16 * 1024 * 1024;

/// Default maximum vectorscope sample count returned by `/api/runs/{slug}/{version}/inspect`.
pub const DEFAULT_VECTORSCOPE_SAMPLES: usize = 2048;

/// Hard upper bound on vectorscope sample count to keep UI payloads snappy.
pub const MAX_VECTORSCOPE_SAMPLES: usize = 8192;

/// Embedded `frontend/dist` static bundle compiled into the single Rust binary.
#[derive(RustEmbed)]
#[folder = "../../frontend/dist"]
pub struct EmbeddedSpaAssets;

// === 1. Shared AppState, Events & Health DTOs ===

/// Live workspace mutation/lifecycle event broadcast over `GET /api/ws`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceEvent {
    /// Event verb (`"created"`, `"updated"`, `"validated"`, `"promoted"`, `"exported"`).
    pub event_type: String,
    /// Resource category (`"style"`, `"atmosphere"`, `"subject"`, `"referencePack"`, `"run"`, `"export"`).
    pub resource_kind: String,
    /// Stable identifier of the affected resource (slug, `subject/style`, or `runId`).
    pub id: String,
    /// Current `Lifecycle` state after the operation, when applicable.
    pub lifecycle: Option<Lifecycle>,
    /// Short human-readable summary.
    pub detail: String,
    /// RFC 3339 UTC timestamp.
    pub timestamp_iso: String,
}

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
#[derive(Clone)]
pub struct AppState {
    /// Optional resolved workspace root directory (`None` when `imgforge ui` runs outside a workspace).
    pub workspace_root: Option<PathBuf>,
    /// Tool-local Codex auth directory checked by `GET /api/health`.
    pub auth_dir: PathBuf,
    /// Codex CLI executable name or explicit `--codex-bin` override.
    pub codex_bin: String,
    /// Optional injected `ImageBackend` seam (`CodexNativeBackend` by default when started via `imgforge ui`).
    pub backend: Option<Arc<dyn ImageBackend>>,
    /// Broadcast sender streaming `WorkspaceEvent`s to `/api/ws` WebSocket subscribers.
    pub events_tx: broadcast::Sender<WorkspaceEvent>,
    /// Binary semantic version.
    pub version: String,
    /// Active backend identifier (`"codex-native"` or injected stub name).
    pub backend_name: String,
}

impl fmt::Debug for AppState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AppState")
            .field("workspace_root", &self.workspace_root)
            .field("auth_dir", &self.auth_dir)
            .field("codex_bin", &self.codex_bin)
            .field("backend", &self.backend.as_ref().map(|b| b.name()))
            .field("version", &self.version)
            .field("backend_name", &self.backend_name)
            .finish()
    }
}

impl AppState {
    /// Construct standalone `AppState` (without a workspace) using `CodexNativeBackend`.
    #[cfg_attr(not(test), allow(dead_code))]
    #[must_use]
    pub fn new(auth_dir: PathBuf) -> Self {
        let codex_bin = imgforge_core::DEFAULT_CODEX_BIN.to_string();
        let backend: Arc<dyn ImageBackend> = Arc::new(CodexNativeBackend::with_codex_bin(
            auth_dir.clone(),
            codex_bin.clone(),
        ));
        Self::with_workspace_and_backend(None, auth_dir, codex_bin, Some(backend))
    }

    /// Construct `AppState` with explicit optional workspace root, `codex_bin`, and `ImageBackend` seam.
    #[must_use]
    pub fn with_workspace_and_backend(
        workspace_root: Option<PathBuf>,
        auth_dir: PathBuf,
        codex_bin: String,
        backend: Option<Arc<dyn ImageBackend>>,
    ) -> Self {
        let (events_tx, _) = broadcast::channel(128);
        let backend_name = backend
            .as_ref()
            .map_or_else(|| BackendKind::CodexNative.as_str(), |b| b.name())
            .to_string();
        Self {
            workspace_root,
            auth_dir,
            codex_bin,
            backend,
            events_tx,
            version: env!("IMGFORGE_VERSION").to_string(),
            backend_name,
        }
    }

    fn emit_event(
        &self,
        event_type: &str,
        resource_kind: &str,
        id: impl Into<String>,
        lifecycle: Option<Lifecycle>,
        detail: impl Into<String>,
    ) {
        let _ = self.events_tx.send(WorkspaceEvent {
            event_type: event_type.to_string(),
            resource_kind: resource_kind.to_string(),
            id: id.into(),
            lifecycle,
            detail: detail.into(),
            timestamp_iso: now_utc_iso(),
        });
    }
}

// === 2. API DTOs (camelCase matching frontend/src/types/) ===

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PaletteSwatchDto {
    label: String,
    hex: String,
    surface_slot: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SubjectDto {
    slug: String,
    name: String,
    category: String,
    height_meters: f64,
    palette_lock: Vec<PaletteSwatchDto>,
    visibility: Visibility,
    lifecycle: Lifecycle,
    source: LibrarySource,
    asymmetric_markers: Vec<String>,
    positive_weapon_geometry: Vec<String>,
    tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StyleDto {
    slug: String,
    name: String,
    medium: String,
    exemplar_count: usize,
    lifecycle: Lifecycle,
    source: LibrarySource,
    linework: String,
    shading_and_texture: String,
    palette_profile: String,
    framing_and_border: String,
    style_guardrails: Vec<String>,
    exemplar_urls: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AtmosphereDto {
    slug: String,
    name: String,
    light_direction: String,
    color_temperature_kelvin: u32,
    exposure_floor: f64,
    max_ambient_tint_blend: f64,
    particles: String,
    source: LibrarySource,
    secondary_light_direction: String,
    min_mean_l_star: f64,
    min_p10_l_star: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SheetDto {
    role: ReferenceSheetRole,
    filename: String,
    image_url: String,
    sha256: String,
    width: u32,
    height: u32,
    bg_coverage: f64,
    border_bg_coverage: f64,
    foreground_pixels: u64,
    strip_y: u32,
    strip_height: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SliceDto {
    id: String,
    file: String,
    image_url: String,
    rect: PixelRect,
    source: imgforge_core::SliceSource,
    label: Option<String>,
    sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReferencePackDto {
    subject_slug: String,
    style_slug: String,
    sheet_kind: ReferenceSheetRole,
    slice_count: usize,
    lifecycle: Lifecycle,
    version: u32,
    visibility: Visibility,
    sheets: Vec<SheetDto>,
    slices: Vec<SliceDto>,
    created_at_iso: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RunDto {
    run_id: String,
    slug: String,
    version: u32,
    version_label: String,
    lifecycle: Lifecycle,
    visibility: Visibility,
    width_px: u32,
    height_px: u32,
    vision_critic_passed: Option<bool>,
    color_qa_max_delta_e2000: Option<f64>,
    created_at_iso: String,
    image_url: String,
    compiled_prompt: String,
    backend_name: String,
    structural_checks: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManifestLockDto {
    schema_version: u32,
    canonical_count: usize,
    superseded_count: usize,
    canonical: BTreeMap<String, ManifestEntry>,
    superseded: Vec<ManifestEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WorkspaceSummaryResponse {
    has_workspace: bool,
    workspace_root: Option<String>,
    workspace_name: Option<String>,
    auth_provisioned: bool,
    subjects: Vec<SubjectDto>,
    styles: Vec<StyleDto>,
    atmospheres: Vec<AtmosphereDto>,
    reference_packs: Vec<ReferencePackDto>,
    runs: Vec<RunDto>,
    manifest_lock: ManifestLockDto,
}

// === 3. Request Payloads ===

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CreateStyleRequest {
    slug: String,
    display_name: String,
    #[serde(default)]
    medium: Option<String>,
    #[serde(default)]
    linework: Option<String>,
    #[serde(default)]
    shading_and_texture: Option<String>,
    #[serde(default)]
    palette_profile: Option<String>,
    #[serde(default)]
    framing_and_border: Option<String>,
    #[serde(default)]
    style_guardrails: Option<Vec<String>>,
    #[serde(default)]
    clone_from_slug: Option<String>,
    #[serde(default)]
    exemplars_base64: Option<Vec<String>>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UpdateStyleRequest {
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    medium: Option<String>,
    #[serde(default)]
    linework: Option<String>,
    #[serde(default)]
    shading_and_texture: Option<String>,
    #[serde(default)]
    palette_profile: Option<String>,
    #[serde(default)]
    framing_and_border: Option<String>,
    #[serde(default)]
    style_guardrails: Option<Vec<String>>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UploadExemplarsRequest {
    exemplars_base64: Vec<String>,
    #[serde(default = "default_true")]
    replace: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PromoteStyleRequest {
    #[serde(default)]
    target_lifecycle: Option<Lifecycle>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CreateAtmosphereRequest {
    slug: String,
    display_name: String,
    key_light_direction: String,
    secondary_light_direction: String,
    color_temperature_k: u32,
    max_ambient_tint_blend: f64,
    min_mean_l_star: f64,
    min_p10_l_star: f64,
    weather_and_particles: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UpdateAtmosphereRequest {
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    key_light_direction: Option<String>,
    #[serde(default)]
    secondary_light_direction: Option<String>,
    #[serde(default)]
    color_temperature_k: Option<u32>,
    #[serde(default)]
    max_ambient_tint_blend: Option<f64>,
    #[serde(default)]
    min_mean_l_star: Option<f64>,
    #[serde(default)]
    min_p10_l_star: Option<f64>,
    #[serde(default)]
    weather_and_particles: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UpdatePaletteLockRequest {
    name: String,
    color_hex: String,
    #[serde(default)]
    surface_slot: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum GuideSpecRequestPayload {
    Wrapped {
        #[serde(rename = "guideSpec")]
        guide_spec: serde_json::Value,
    },
    Direct(serde_json::Value),
}

impl GuideSpecRequestPayload {
    fn into_value(self) -> serde_json::Value {
        match self {
            Self::Wrapped { guide_spec } => guide_spec,
            Self::Direct(val) => val,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct GuideValidationErrorDto {
    path: String,
    message: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReslicePackRequest {
    sheet_role: ReferenceSheetRole,
    #[serde(default)]
    mode: OverrideMode,
    overrides: Vec<RectOverride>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PatchEditApiRequest {
    slug: String,
    #[serde(default)]
    target_image_path: Option<String>,
    #[serde(default)]
    run_slug: Option<String>,
    #[serde(default)]
    run_version: Option<u32>,
    #[serde(default)]
    roi: Option<PixelRect>,
    #[serde(default)]
    mask_png_base64: Option<String>,
    #[serde(default)]
    prep_mode: Option<PatchPrepMode>,
    #[serde(default)]
    collar_width_px: Option<u32>,
    #[serde(default)]
    feather_px: Option<u32>,
    prompt: String,
    #[serde(default)]
    visibility: Option<Visibility>,
    #[serde(default)]
    reference_images: Option<Vec<String>>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct InspectRunRequest {
    #[serde(default)]
    atmosphere_slug: Option<String>,
    #[serde(default)]
    subject_slugs: Vec<String>,
    #[serde(default)]
    before_image_path: Option<String>,
    #[serde(default)]
    before_run_version: Option<u32>,
    #[serde(default)]
    max_samples: Option<usize>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ValidateRunApiRequest {
    #[serde(default)]
    atmosphere_slug: Option<String>,
    #[serde(default)]
    subject_slugs: Vec<String>,
    #[serde(default)]
    reference_images: Vec<String>,
    #[serde(default)]
    traits: Vec<ValidateTraitDto>,
    #[serde(default)]
    run_critic: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ValidatePackApiRequest {
    #[serde(default)]
    traits: Vec<ValidateTraitDto>,
    #[serde(default)]
    run_critic: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ValidateTraitDto {
    trait_id: String,
    category: String,
    criterion: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ExportMatchGradeDto {
    #[serde(default = "default_true")]
    enabled: bool,
    #[serde(default)]
    grade_ref_path: Option<String>,
    #[serde(default)]
    grade_strength: Option<f64>,
    #[serde(default)]
    cube_lut_path: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ExportApiRequest {
    run_slug: String,
    run_version: u32,
    profile: String,
    #[serde(default)]
    visibility: Option<Visibility>,
    #[serde(default)]
    output_slug: Option<String>,
    #[serde(default)]
    out_dir: Option<String>,
    #[serde(default)]
    token_size: Option<u32>,
    #[serde(default)]
    ring_color_hex: Option<String>,
    #[serde(default)]
    jpeg_backdrop_hex: Option<String>,
    #[serde(default)]
    cols: Option<u32>,
    #[serde(default)]
    rows: Option<u32>,
    #[serde(default)]
    grid_px: Option<u32>,
    #[serde(default)]
    scale_factor: Option<f64>,
    #[serde(default)]
    jpeg_quality: Option<u8>,
    #[serde(default)]
    match_grade: Option<ExportMatchGradeDto>,
}

// === 4. Error Mapping ===

struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: String,
}

impl ApiError {
    fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }

    fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "bad_request", message)
    }

    fn forbidden(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(StatusCode::FORBIDDEN, code, message)
    }

    fn not_found(message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, "not_found", message)
    }

    fn conflict(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(StatusCode::CONFLICT, code, message)
    }
}

impl From<CoreError> for ApiError {
    fn from(err: CoreError) -> Self {
        match &err {
            CoreError::CanonicalImmutable { .. } => {
                Self::conflict("canonical_immutable", err.to_string())
            }
            CoreError::LifecycleTransitionInvalid { .. } => {
                Self::conflict("lifecycle_transition_invalid", err.to_string())
            }
            CoreError::VisibilityViolation { .. } => {
                Self::forbidden("visibility_violation", err.to_string())
            }
            CoreError::RegistryResolutionFailed { .. } => Self::not_found(err.to_string()),
            CoreError::ColorQaFailed(_) | CoreError::StructuredOutputInvalid { .. } => Self::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "qa_failed",
                err.to_string(),
            ),
            CoreError::AuthMissing { .. }
            | CoreError::AuthInvalid { .. }
            | CoreError::AuthHomeUnavailable => Self::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "auth_unavailable",
                err.to_string(),
            ),
            CoreError::WorkspaceNotFound { .. } | CoreError::WorkspaceInvalid { .. } => {
                Self::bad_request(err.to_string())
            }
            CoreError::Io { source, .. } => match source.kind() {
                std::io::ErrorKind::NotFound => Self::not_found(err.to_string()),
                std::io::ErrorKind::InvalidInput => {
                    Self::forbidden("invalid_path", err.to_string())
                }
                _ => Self::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "io_error",
                    err.to_string(),
                ),
            },
            CoreError::SubprocessFailed { .. }
            | CoreError::GeneratedImageNotFound { .. }
            | CoreError::AspectRatioExceeded { .. } => {
                Self::new(StatusCode::BAD_GATEWAY, "backend_failed", err.to_string())
            }
            _ => Self::bad_request(err.to_string()),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        if self.status.is_server_error() {
            if self.status == StatusCode::INTERNAL_SERVER_ERROR {
                tracing::error!(
                    status = self.status.as_u16(),
                    code = self.code,
                    message = %self.message,
                    "imgforge API request failed with internal server error"
                );
            } else {
                tracing::warn!(
                    status = self.status.as_u16(),
                    code = self.code,
                    message = %self.message,
                    "imgforge API request failed with 5xx server error"
                );
            }
        }
        (
            self.status,
            Json(serde_json::json!({
                "error": self.code,
                "message": self.message,
            })),
        )
            .into_response()
    }
}

// === 5. Security Middleware (Host + Origin Allowlists + CSP) ===

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

fn is_allowed_origin_header(raw_origin: &str) -> bool {
    let trimmed = raw_origin.trim();
    let Some(rest) = trimmed.strip_prefix("http://") else {
        return false;
    };
    is_allowed_host_header(rest)
}

fn requires_origin_check(request: &Request) -> bool {
    let is_mutating = matches!(
        *request.method(),
        Method::POST | Method::PUT | Method::PATCH | Method::DELETE
    );
    let is_ws =
        request.uri().path() == "/api/ws" || request.headers().contains_key(header::UPGRADE);
    is_mutating || is_ws
}

async fn security_headers_and_host_guard(request: Request, next: Next) -> Response {
    let mut host_values = request.headers().get_all(header::HOST).iter();
    let host_allowed = match (host_values.next(), host_values.next()) {
        (Some(val), None) => val.to_str().ok().is_some_and(is_allowed_host_header),
        _ => false,
    };

    let origin_allowed = if requires_origin_check(&request) {
        let mut origin_values = request.headers().get_all(header::ORIGIN).iter();
        match (origin_values.next(), origin_values.next()) {
            (None, None) => true,
            (Some(val), None) => val.to_str().ok().is_some_and(is_allowed_origin_header),
            _ => false,
        }
    } else {
        true
    };

    let mut response = if !host_allowed {
        (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": "forbidden_host" })),
        )
            .into_response()
    } else if !origin_allowed {
        (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": "forbidden_origin" })),
        )
            .into_response()
    } else {
        next.run(request).await
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

// === 6. Router & Startup ===

/// Construct the `axum::Router` for `/api/*`, `/api/ws`, and embedded SPA static serving.
pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/api/health", get(health_handler))
        .route("/api/workspace", get(workspace_handler))
        .route("/api/assets/{*path}", get(serve_workspace_asset_handler))
        .route(
            "/api/presets/styles/{slug}/exemplars/{index}",
            get(serve_style_exemplar_handler),
        )
        .route("/api/styles", post(create_style_handler))
        .route(
            "/api/styles/{slug}",
            axum::routing::put(update_style_handler),
        )
        .route(
            "/api/styles/{slug}/exemplars",
            post(upload_style_exemplars_handler),
        )
        .route("/api/styles/{slug}/promote", post(promote_style_handler))
        .route("/api/atmospheres", post(create_atmosphere_handler))
        .route(
            "/api/atmospheres/{slug}",
            axum::routing::put(update_atmosphere_handler),
        )
        .route(
            "/api/subjects/{slug}/palette-lock",
            post(update_subject_palette_lock_handler),
        )
        .route("/api/guide/validate", post(validate_guide_handler))
        .route("/api/guide/render", post(render_guide_handler))
        .route(
            "/api/packs/{subject}/{style}/reslice",
            post(reslice_pack_handler),
        )
        .route(
            "/api/packs/{subject}/{style}/validate",
            post(validate_pack_handler),
        )
        .route(
            "/api/packs/{subject}/{style}/promote",
            post(promote_pack_handler),
        )
        .route("/api/edit/patch", post(patch_edit_handler))
        .route(
            "/api/runs/{slug}/{version}/inspect",
            post(inspect_run_handler),
        )
        .route(
            "/api/runs/{slug}/{version}/validate",
            post(validate_run_handler),
        )
        .route(
            "/api/runs/{slug}/{version}/promote",
            post(promote_run_handler),
        )
        .route("/api/export", post(export_handler))
        .route("/api/ws", get(ws_handler))
        .route("/api", any(api_not_found))
        .route("/api/", any(api_not_found))
        .route("/api/{*rest}", any(api_not_found))
        .fallback(serve_spa)
        .layer(DefaultBodyLimit::max(MAX_API_BODY_BYTES))
        .layer(middleware::from_fn(security_headers_and_host_guard))
        .with_state(state)
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

// === 7. Workspace & Containment Helpers ===

struct WorkspaceContext {
    root: PathBuf,
    paths: WorkspacePathsConfig,
}

impl WorkspaceContext {
    fn library_dir(&self) -> PathBuf {
        self.root.join(&self.paths.library_dir)
    }

    fn runs_dir(&self) -> PathBuf {
        self.root.join(&self.paths.runs_dir)
    }

    fn registry(&self) -> LibraryRegistry {
        LibraryRegistry::new(Some(self.library_dir()), resolve_user_config_dir_from_env())
    }

    fn run_store(&self) -> RunStore {
        RunStore::new(self.runs_dir())
    }

    fn lock_store(&self) -> ManifestLockStore {
        ManifestLockStore::new(&self.library_dir())
    }

    fn asset_url_for_path(&self, path: &Path) -> Option<String> {
        let rel = path.strip_prefix(&self.root).ok()?;
        let norm = rel.to_string_lossy().replace('\\', "/");
        let trimmed = norm.trim_start_matches('/');
        if trimmed.is_empty() {
            None
        } else {
            Some(format!("/api/assets/{trimmed}"))
        }
    }
}

fn resolve_workspace_ctx(state: &AppState) -> Result<WorkspaceContext, ApiError> {
    let Some(ref root) = state.workspace_root else {
        return Err(ApiError::new(
            StatusCode::PRECONDITION_REQUIRED,
            "workspace_unconfigured",
            "no imgforge workspace is open; launch 'imgforge ui --workspace <PATH>'",
        ));
    };

    if root.join("imgforge.toml").exists() {
        let resolved =
            resolve_workspace(Some(root.as_path()), None, root).map_err(ApiError::from)?;
        Ok(WorkspaceContext {
            root: resolved.root,
            paths: resolved.config.paths,
        })
    } else if root.is_dir() {
        let canon = fs::canonicalize(root).map_err(|err| CoreError::Io {
            context: format!("canonicalizing workspace root {}", root.display()),
            source: err,
        })?;
        Ok(WorkspaceContext {
            root: canon,
            paths: WorkspacePathsConfig::default(),
        })
    } else {
        Err(ApiError::bad_request(format!(
            "configured workspace '{}' does not exist",
            root.display()
        )))
    }
}

fn validate_no_traversal_or_nul(raw: &str, label: &str) -> Result<PathBuf, ApiError> {
    if raw.trim().is_empty()
        || raw != raw.trim()
        || raw.contains('\0')
        || raw.contains('\\')
        || raw.chars().any(char::is_control)
    {
        return Err(ApiError::forbidden(
            "invalid_path",
            format!("{label} '{raw}' is empty or contains invalid characters"),
        ));
    }
    let path = Path::new(raw);
    if path.is_absolute() {
        return Err(ApiError::forbidden(
            "invalid_path",
            format!("{label} '{raw}' must be relative to the workspace root"),
        ));
    }
    let mut normal_count = 0usize;
    for comp in path.components() {
        match comp {
            Component::Normal(seg) if !seg.is_empty() => normal_count += 1,
            Component::CurDir => {}
            _ => {
                return Err(ApiError::forbidden(
                    "path_traversal",
                    format!("{label} '{raw}' must not traverse outside the workspace"),
                ));
            }
        }
    }
    if normal_count == 0 {
        return Err(ApiError::forbidden(
            "invalid_path",
            format!("{label} must point to a file or directory inside the workspace"),
        ));
    }
    Ok(path.to_path_buf())
}

fn resolve_contained_workspace_file(
    ws_root: &Path,
    raw: &str,
    label: &str,
) -> Result<PathBuf, ApiError> {
    let rel = validate_no_traversal_or_nul(raw, label)?;
    verify_contained_path_no_symlinks(ws_root, &rel, false, label).map_err(ApiError::from)
}

fn resolve_contained_workspace_dir_for_write(
    ws_root: &Path,
    raw: &str,
    label: &str,
) -> Result<PathBuf, ApiError> {
    let rel = validate_no_traversal_or_nul(raw, label)?;
    verify_existing_dir_prefix_no_symlinks(ws_root, &rel, label).map_err(ApiError::from)?;
    Ok(ws_root.join(rel))
}

fn decode_base64_png_bounded(raw: &str, label: &str) -> Result<Vec<u8>, ApiError> {
    let trimmed = raw.trim();
    let payload = if let Some((_, b64)) = trimmed.split_once(";base64,") {
        b64.trim()
    } else {
        trimmed
    };
    if payload.is_empty() {
        return Err(ApiError::bad_request(format!(
            "{label} base64 payload must not be empty"
        )));
    }
    let max_b64_len = MAX_UPLOAD_PNG_BYTES.div_ceil(3) * 4 + 16;
    if payload.len() > max_b64_len {
        return Err(ApiError::bad_request(format!(
            "{label} exceeds maximum upload size ({MAX_UPLOAD_PNG_BYTES} bytes)"
        )));
    }
    let bytes = BASE64_STANDARD
        .decode(payload)
        .map_err(|err| ApiError::bad_request(format!("{label} contains invalid base64: {err}")))?;
    if bytes.len() > MAX_UPLOAD_PNG_BYTES {
        return Err(ApiError::bad_request(format!(
            "{label} decoded size {} exceeds maximum {MAX_UPLOAD_PNG_BYTES} bytes",
            bytes.len()
        )));
    }
    if bytes.len() < PNG_SIGNATURE.len() || bytes[..PNG_SIGNATURE.len()] != PNG_SIGNATURE {
        return Err(ApiError::bad_request(format!(
            "{label} does not start with a valid 8-byte PNG signature"
        )));
    }
    let _ = decode_png_checked(Path::new(label), &bytes).map_err(ApiError::from)?;
    Ok(bytes)
}

fn parse_optional_json_body<T: serde::de::DeserializeOwned + Default>(
    body: &Bytes,
) -> Result<T, ApiError> {
    if body.iter().all(|b| b.is_ascii_whitespace()) {
        return Ok(T::default());
    }
    serde_json::from_slice(body)
        .map_err(|err| ApiError::bad_request(format!("invalid JSON body: {err}")))
}

// === 8. DTO Conversion Helpers ===

fn subject_to_dto(resolved: &ResolvedSubject) -> SubjectDto {
    let s = &resolved.subject;
    SubjectDto {
        slug: s.slug.clone(),
        name: s.display_name.clone(),
        category: s.kind.as_str().to_string(),
        height_meters: s.height_m.unwrap_or(0.0),
        palette_lock: s
            .palette_lock
            .swatches
            .iter()
            .map(|sw| PaletteSwatchDto {
                label: sw.name.clone(),
                hex: sw.color_hex.clone(),
                surface_slot: sw.surface_slot.clone().unwrap_or_default(),
            })
            .collect(),
        visibility: s.visibility,
        lifecycle: s.lifecycle,
        source: resolved.source,
        asymmetric_markers: s.asymmetric_markers.clone(),
        positive_weapon_geometry: s.positive_weapon_geometry.clone(),
        tags: s.tags.clone(),
    }
}

fn style_to_dto(resolved: &ResolvedStyle) -> StyleDto {
    let st = &resolved.style;
    let exemplar_urls = (0..st.exemplars.len())
        .map(|i| format!("/api/presets/styles/{}/exemplars/{i}", st.slug))
        .collect();
    StyleDto {
        slug: st.slug.clone(),
        name: st.display_name.clone(),
        medium: st.medium.clone(),
        exemplar_count: st.exemplars.len(),
        lifecycle: st.lifecycle,
        source: resolved.source,
        linework: st.linework.clone(),
        shading_and_texture: st.shading_and_texture.clone(),
        palette_profile: st.palette_profile.clone(),
        framing_and_border: st.framing_and_border.clone(),
        style_guardrails: st.style_guardrails.clone(),
        exemplar_urls,
    }
}

fn atmosphere_to_dto(resolved: &ResolvedAtmosphere) -> AtmosphereDto {
    let a = &resolved.atmosphere;
    AtmosphereDto {
        slug: a.slug.clone(),
        name: a.display_name.clone(),
        light_direction: a.key_light_direction.clone(),
        color_temperature_kelvin: a.color_temperature_k,
        exposure_floor: a.exposure_floor.min_mean_l_star,
        max_ambient_tint_blend: a.max_ambient_tint_blend,
        particles: a.weather_and_particles.clone(),
        source: resolved.source,
        secondary_light_direction: a.secondary_light_direction.clone(),
        min_mean_l_star: a.exposure_floor.min_mean_l_star,
        min_p10_l_star: a.exposure_floor.min_p10_l_star,
    }
}

fn pack_to_dto(
    ctx: &WorkspaceContext,
    pack_dir: &Path,
    manifest: &ReferencePackManifest,
    slices_override: Option<&[SliceRecord]>,
) -> ReferencePackDto {
    let slices_slice = slices_override.unwrap_or(&manifest.slices);
    let sheets = manifest
        .sheets
        .iter()
        .map(|sh: &GeneratedSheetRecord| {
            let image_url = ctx
                .asset_url_for_path(&pack_dir.join(&sh.filename))
                .unwrap_or_default();
            SheetDto {
                role: sh.role,
                filename: sh.filename.clone(),
                image_url,
                sha256: sh.sha256.clone(),
                width: sh.structural_report.width,
                height: sh.structural_report.height,
                bg_coverage: sh.structural_report.bg_coverage,
                border_bg_coverage: sh.structural_report.border_bg_coverage,
                foreground_pixels: sh.structural_report.foreground_pixels,
                strip_y: sh.calibration_strip.strip_y,
                strip_height: sh.calibration_strip.strip_height,
            }
        })
        .collect();

    let slices = slices_slice
        .iter()
        .map(|sl| {
            let image_url = ctx
                .asset_url_for_path(&pack_dir.join(&sl.file))
                .unwrap_or_default();
            SliceDto {
                id: sl.id.clone(),
                file: sl.file.clone(),
                image_url,
                rect: sl.rect,
                source: sl.source,
                label: sl.label.clone(),
                sha256: sl.sha256.clone(),
            }
        })
        .collect();

    let sheet_kind = manifest
        .sheets
        .first()
        .map_or(ReferenceSheetRole::SingleSheet, |s| s.role);

    ReferencePackDto {
        subject_slug: manifest.subject_slug.clone(),
        style_slug: manifest.style_slug.clone(),
        sheet_kind,
        slice_count: slices_slice.len(),
        lifecycle: manifest.lifecycle,
        version: manifest.version,
        visibility: manifest.visibility,
        sheets,
        slices,
        created_at_iso: manifest.created_at_iso.clone(),
    }
}

fn run_to_dto(ctx: &WorkspaceContext, meta: &RunMetadata) -> RunDto {
    let image_path = ctx.run_store().run_image_path(&meta.slug, meta.version);
    let image_url = ctx.asset_url_for_path(&image_path).unwrap_or_default();
    RunDto {
        run_id: meta.run_id.clone(),
        slug: meta.slug.clone(),
        version: meta.version,
        version_label: meta.version_label.clone(),
        lifecycle: meta.lifecycle,
        visibility: meta.visibility,
        width_px: meta.backend.width,
        height_px: meta.backend.height,
        vision_critic_passed: meta.qa_reports.critic_report.as_ref().map(|c| c.passed),
        color_qa_max_delta_e2000: meta.qa_reports.color_qa_max_delta_e2000,
        created_at_iso: meta.created_at_iso.clone(),
        image_url,
        compiled_prompt: meta.compiled_prompt.clone(),
        backend_name: meta.backend.name.clone(),
        structural_checks: meta.qa_reports.structural_checks.clone(),
    }
}

fn manifest_lock_to_dto(lock: ManifestLock) -> ManifestLockDto {
    ManifestLockDto {
        schema_version: lock.schema_version,
        canonical_count: lock.canonical.len(),
        superseded_count: lock.superseded.len(),
        canonical: lock.canonical,
        superseded: lock.superseded,
    }
}

// === 9. Endpoint Handlers ===

async fn health_handler(State(state): State<AppState>) -> Json<HealthResponse> {
    let auth_manager = AuthManager::new(state.auth_dir.clone(), state.codex_bin.clone());
    let auth_provisioned = auth_manager.is_provisioned_locally();
    let status = if auth_provisioned {
        HealthStatusKind::Ok
    } else {
        HealthStatusKind::Degraded
    };

    Json(HealthResponse {
        status,
        version: state.version,
        backend: state.backend_name,
        auth_provisioned,
    })
}

async fn workspace_handler(
    State(state): State<AppState>,
) -> Result<Json<WorkspaceSummaryResponse>, ApiError> {
    let auth_manager = AuthManager::new(state.auth_dir.clone(), state.codex_bin.clone());
    let auth_provisioned = auth_manager.is_provisioned_locally();

    if state.workspace_root.is_none() {
        let registry = LibraryRegistry::new(None, resolve_user_config_dir_from_env());
        let styles = registry.list_styles()?.iter().map(style_to_dto).collect();
        let atmospheres = registry
            .list_atmospheres()?
            .iter()
            .map(atmosphere_to_dto)
            .collect();
        return Ok(Json(WorkspaceSummaryResponse {
            has_workspace: false,
            workspace_root: None,
            workspace_name: None,
            auth_provisioned,
            subjects: Vec::new(),
            styles,
            atmospheres,
            reference_packs: Vec::new(),
            runs: Vec::new(),
            manifest_lock: manifest_lock_to_dto(ManifestLock::default()),
        }));
    }

    let ctx = resolve_workspace_ctx(&state)?;
    let workspace_name = if ctx.root.join("imgforge.toml").exists() {
        resolve_workspace(Some(ctx.root.as_path()), None, &ctx.root)
            .ok()
            .and_then(|w| w.config.name)
    } else {
        None
    };

    let registry = ctx.registry();
    let subjects = registry
        .list_subjects()?
        .iter()
        .map(subject_to_dto)
        .collect();
    let styles = registry.list_styles()?.iter().map(style_to_dto).collect();
    let atmospheres = registry
        .list_atmospheres()?
        .iter()
        .map(atmosphere_to_dto)
        .collect();

    let discovered_packs = list_workspace_reference_packs(&ctx.library_dir())?;
    let reference_packs = discovered_packs
        .iter()
        .map(|p| {
            let slices_ref = p.slices_manifest.as_ref().map(|sm| sm.slices.as_slice());
            pack_to_dto(&ctx, &p.pack_dir, &p.manifest, slices_ref)
        })
        .collect();

    let lock_store = ctx.lock_store();
    let run_store = ctx.run_store();
    let status = inspect_workspace_status(&lock_store, &run_store, None)?;
    let mut runs = Vec::new();
    for versions in status.runs.values() {
        for meta in versions {
            runs.push(run_to_dto(&ctx, meta));
        }
    }

    let lock = lock_store.load_or_init()?;

    Ok(Json(WorkspaceSummaryResponse {
        has_workspace: true,
        workspace_root: Some(ctx.root.display().to_string()),
        workspace_name,
        auth_provisioned,
        subjects,
        styles,
        atmospheres,
        reference_packs,
        runs,
        manifest_lock: manifest_lock_to_dto(lock),
    }))
}

async fn serve_workspace_asset_handler(
    State(state): State<AppState>,
    AxumPath(raw_path): AxumPath<String>,
) -> Result<Response, ApiError> {
    let ctx = resolve_workspace_ctx(&state)?;
    let rel = validate_no_traversal_or_nul(&raw_path, "asset path")?;

    // Explicitly block credential files, dotfiles, and non-image extensions.
    for comp in rel.components() {
        if let Component::Normal(seg) = comp {
            let s = seg.to_string_lossy();
            if s.eq_ignore_ascii_case("auth.json") || s.starts_with('.') {
                return Err(ApiError::forbidden(
                    "forbidden_asset",
                    "access to hidden or credential files is forbidden",
                ));
            }
        }
    }

    let ext = rel
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    let content_type = match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        _ => {
            return Err(ApiError::forbidden(
                "forbidden_asset_type",
                "only .png, .jpg, .jpeg, and .webp workspace assets may be served",
            ));
        }
    };

    let full_path = verify_contained_path_no_symlinks(&ctx.root, &rel, false, "workspace asset")
        .map_err(ApiError::from)?;
    let bytes =
        read_regular_file_nofollow(&full_path, "workspace asset").map_err(ApiError::from)?;
    if ext == "png" {
        let _ = decode_png_checked(&full_path, &bytes).map_err(ApiError::from)?;
    }

    let mut response = (StatusCode::OK, bytes).into_response();
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    response.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    Ok(response)
}

async fn serve_style_exemplar_handler(
    State(state): State<AppState>,
    AxumPath((slug, index)): AxumPath<(String, usize)>,
) -> Result<Response, ApiError> {
    validate_slug(&slug, "Style").map_err(ApiError::bad_request)?;
    let registry = match state.workspace_root {
        Some(_) => resolve_workspace_ctx(&state)?.registry(),
        None => LibraryRegistry::new(None, resolve_user_config_dir_from_env()),
    };
    let resolved = registry.resolve_style(&slug)?;
    let Some((_, bytes)) = resolved.exemplar_pngs.get(index) else {
        return Err(ApiError::not_found(format!(
            "Style '{slug}' has no exemplar at index {index}"
        )));
    };

    let mut response = (StatusCode::OK, bytes.clone()).into_response();
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static("image/png"));
    response.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    Ok(response)
}

async fn create_style_handler(
    State(state): State<AppState>,
    Json(req): Json<CreateStyleRequest>,
) -> Result<(StatusCode, Json<StyleDto>), ApiError> {
    let ctx = resolve_workspace_ctx(&state)?;
    validate_slug(&req.slug, "Style").map_err(ApiError::bad_request)?;

    let library_dir = ctx.library_dir();
    let rel_style_dir = Path::new("styles").join(&req.slug);
    verify_existing_dir_prefix_no_symlinks(&library_dir, &rel_style_dir, "Style directory")?;
    let style_dir = library_dir.join(&rel_style_dir);
    if style_dir.join(CARD_FILENAME).exists() {
        return Err(ApiError::conflict(
            "style_exists",
            format!("Style '{}' already exists in project library", req.slug),
        ));
    }

    let registry = ctx.registry();
    let cloned_source = match req.clone_from_slug.as_deref() {
        Some(src_slug) => {
            validate_slug(src_slug, "Style").map_err(ApiError::bad_request)?;
            Some(registry.resolve_style(src_slug)?)
        }
        None => None,
    };

    let exemplar_bytes_list: Vec<Vec<u8>> = if let Some(ref b64_list) = req.exemplars_base64 {
        if b64_list.len() < MIN_STYLE_EXEMPLARS || b64_list.len() > MAX_STYLE_EXEMPLARS {
            return Err(ApiError::bad_request(format!(
                "exemplarsBase64 must contain {MIN_STYLE_EXEMPLARS}..={MAX_STYLE_EXEMPLARS} PNG images"
            )));
        }
        let mut decoded = Vec::with_capacity(b64_list.len());
        for (idx, b64) in b64_list.iter().enumerate() {
            decoded.push(decode_base64_png_bounded(
                b64,
                &format!("exemplarsBase64[{idx}]"),
            )?);
        }
        decoded
    } else if let Some(ref src) = cloned_source {
        src.exemplar_pngs
            .iter()
            .map(|(_, bytes)| bytes.clone())
            .collect()
    } else {
        return Err(ApiError::bad_request(
            "provide either exemplarsBase64 (1..=3 PNGs) or cloneFromSlug",
        ));
    };

    let exemplar_rels: Vec<PathBuf> = (0..exemplar_bytes_list.len())
        .map(|i| PathBuf::from(format!("exemplars/exemplar_{}.png", i + 1)))
        .collect();

    let pick_str =
        |opt: Option<String>, fallback: Option<&str>, field: &str| -> Result<String, ApiError> {
            if let Some(val) = opt {
                Ok(val)
            } else if let Some(fb) = fallback {
                Ok(fb.to_string())
            } else {
                Err(ApiError::bad_request(format!(
                    "{field} is required when not cloning from an existing Style"
                )))
            }
        };

    let style = Style {
        schema_version: 1,
        slug: req.slug.clone(),
        display_name: req.display_name,
        medium: pick_str(
            req.medium,
            cloned_source.as_ref().map(|c| c.style.medium.as_str()),
            "medium",
        )?,
        linework: pick_str(
            req.linework,
            cloned_source.as_ref().map(|c| c.style.linework.as_str()),
            "linework",
        )?,
        shading_and_texture: pick_str(
            req.shading_and_texture,
            cloned_source
                .as_ref()
                .map(|c| c.style.shading_and_texture.as_str()),
            "shadingAndTexture",
        )?,
        palette_profile: pick_str(
            req.palette_profile,
            cloned_source
                .as_ref()
                .map(|c| c.style.palette_profile.as_str()),
            "paletteProfile",
        )?,
        framing_and_border: pick_str(
            req.framing_and_border,
            cloned_source
                .as_ref()
                .map(|c| c.style.framing_and_border.as_str()),
            "framingAndBorder",
        )?,
        style_guardrails: match req.style_guardrails {
            Some(rules) => rules,
            None => cloned_source
                .as_ref()
                .map(|c| c.style.style_guardrails.clone())
                .ok_or_else(|| ApiError::bad_request("styleGuardrails is required"))?,
        },
        exemplars: exemplar_rels.clone(),
        lifecycle: Lifecycle::Draft,
    };
    style.validate()?;
    let yaml_str = style.to_yaml_string()?;

    let rel_exemplars_dir = rel_style_dir.join("exemplars");
    verify_existing_dir_prefix_no_symlinks(
        &library_dir,
        &rel_exemplars_dir,
        "Style exemplars dir",
    )?;
    fs::create_dir_all(style_dir.join("exemplars")).map_err(|err| CoreError::Io {
        context: format!("creating Style exemplars dir {}", style_dir.display()),
        source: err,
    })?;
    verify_contained_path_no_symlinks(
        &library_dir,
        &rel_exemplars_dir,
        true,
        "Style exemplars dir",
    )?;

    for (rel, bytes) in exemplar_rels.iter().zip(exemplar_bytes_list.iter()) {
        write_atomic(&style_dir.join(rel), bytes, "Style exemplar PNG")?;
    }
    write_atomic(
        &style_dir.join(CARD_FILENAME),
        yaml_str.as_bytes(),
        "Style card.yaml",
    )?;

    let resolved = ctx.registry().resolve_style(&req.slug)?;
    state.emit_event(
        "created",
        "style",
        &req.slug,
        Some(resolved.style.lifecycle),
        format!("Created Style '{}'", req.slug),
    );
    Ok((StatusCode::CREATED, Json(style_to_dto(&resolved))))
}

fn load_project_style_for_mutation(
    ctx: &WorkspaceContext,
    slug: &str,
) -> Result<(PathBuf, PathBuf, Style), ApiError> {
    validate_slug(slug, "Style").map_err(ApiError::bad_request)?;
    let library_dir = ctx.library_dir();
    let rel_style_dir = Path::new("styles").join(slug);
    let Some(style_dir) =
        verify_optional_contained_dir_no_symlinks(&library_dir, &rel_style_dir, "Style directory")?
    else {
        return Err(ApiError::not_found(format!(
            "Style '{slug}' does not exist in project library (clone built-in styles via POST /api/styles first)"
        )));
    };
    let card_path = verify_contained_path_no_symlinks(
        &style_dir,
        Path::new(CARD_FILENAME),
        false,
        "Style card.yaml",
    )?;
    let bytes = read_regular_file_nofollow(&card_path, "Style card.yaml")?;
    let raw = String::from_utf8(bytes).map_err(|err| {
        ApiError::bad_request(format!("Style card.yaml is not valid UTF-8: {err}"))
    })?;
    let style = Style::from_yaml_str(&raw)?;
    if style.slug != slug {
        return Err(ApiError::bad_request(format!(
            "Style card slug '{}' mismatches path slug '{slug}'",
            style.slug
        )));
    }
    Ok((style_dir, card_path, style))
}

async fn update_style_handler(
    State(state): State<AppState>,
    AxumPath(slug): AxumPath<String>,
    Json(req): Json<UpdateStyleRequest>,
) -> Result<Json<StyleDto>, ApiError> {
    let ctx = resolve_workspace_ctx(&state)?;
    let (_style_dir, card_path, mut style) = load_project_style_for_mutation(&ctx, &slug)?;
    style.lifecycle.ensure_mutable(&format!("style:{slug}"))?;

    if let Some(v) = req.display_name {
        style.display_name = v;
    }
    if let Some(v) = req.medium {
        style.medium = v;
    }
    if let Some(v) = req.linework {
        style.linework = v;
    }
    if let Some(v) = req.shading_and_texture {
        style.shading_and_texture = v;
    }
    if let Some(v) = req.palette_profile {
        style.palette_profile = v;
    }
    if let Some(v) = req.framing_and_border {
        style.framing_and_border = v;
    }
    if let Some(v) = req.style_guardrails {
        style.style_guardrails = v;
    }

    style.validate()?;
    let yaml = style.to_yaml_string()?;
    write_atomic(&card_path, yaml.as_bytes(), "Style card.yaml")?;

    let resolved = ctx.registry().resolve_style(&slug)?;
    state.emit_event(
        "updated",
        "style",
        &slug,
        Some(resolved.style.lifecycle),
        format!("Updated Style '{slug}'"),
    );
    Ok(Json(style_to_dto(&resolved)))
}

async fn upload_style_exemplars_handler(
    State(state): State<AppState>,
    AxumPath(slug): AxumPath<String>,
    Json(req): Json<UploadExemplarsRequest>,
) -> Result<Json<StyleDto>, ApiError> {
    let ctx = resolve_workspace_ctx(&state)?;
    let (style_dir, card_path, mut style) = load_project_style_for_mutation(&ctx, &slug)?;
    style.lifecycle.ensure_mutable(&format!("style:{slug}"))?;

    if req.exemplars_base64.is_empty() {
        return Err(ApiError::bad_request(
            "exemplarsBase64 must contain at least 1 PNG image",
        ));
    }
    let mut new_pngs = Vec::with_capacity(req.exemplars_base64.len());
    for (idx, raw_b64) in req.exemplars_base64.iter().enumerate() {
        new_pngs.push(decode_base64_png_bounded(
            raw_b64,
            &format!("exemplarsBase64[{idx}]"),
        )?);
    }

    let mut existing_pngs: Vec<Vec<u8>> = if req.replace {
        Vec::new()
    } else {
        load_and_verify_style_exemplars(&style_dir, &style)?
            .into_iter()
            .map(|(_, bytes)| bytes)
            .collect()
    };
    existing_pngs.extend(new_pngs);
    if existing_pngs.len() < MIN_STYLE_EXEMPLARS || existing_pngs.len() > MAX_STYLE_EXEMPLARS {
        return Err(ApiError::bad_request(format!(
            "Style '{slug}' exemplar count {} must stay within {MIN_STYLE_EXEMPLARS}..={MAX_STYLE_EXEMPLARS}",
            existing_pngs.len()
        )));
    }

    verify_existing_dir_prefix_no_symlinks(&style_dir, Path::new("exemplars"), "exemplars dir")?;
    fs::create_dir_all(style_dir.join("exemplars")).map_err(|err| CoreError::Io {
        context: format!("creating exemplars dir in {}", style_dir.display()),
        source: err,
    })?;
    verify_contained_path_no_symlinks(&style_dir, Path::new("exemplars"), true, "exemplars dir")?;

    let mut exemplar_rels = Vec::with_capacity(existing_pngs.len());
    for (idx, png_bytes) in existing_pngs.iter().enumerate() {
        let rel = PathBuf::from(format!("exemplars/exemplar_{}.png", idx + 1));
        write_atomic(&style_dir.join(&rel), png_bytes, "Style exemplar PNG")?;
        exemplar_rels.push(rel);
    }
    style.exemplars = exemplar_rels;
    style.validate()?;
    let yaml = style.to_yaml_string()?;
    write_atomic(&card_path, yaml.as_bytes(), "Style card.yaml")?;

    let resolved = ctx.registry().resolve_style(&slug)?;
    state.emit_event(
        "updated",
        "style",
        &slug,
        Some(resolved.style.lifecycle),
        format!("Updated exemplars for Style '{slug}'"),
    );
    Ok(Json(style_to_dto(&resolved)))
}

async fn promote_style_handler(
    State(state): State<AppState>,
    AxumPath(slug): AxumPath<String>,
    body: Bytes,
) -> Result<Json<serde_json::Value>, ApiError> {
    let req: PromoteStyleRequest = parse_optional_json_body(&body)?;
    let ctx = resolve_workspace_ctx(&state)?;
    let (style_dir, card_path, mut style) = load_project_style_for_mutation(&ctx, &slug)?;
    style.lifecycle.ensure_mutable(&format!("style:{slug}"))?;

    // Verify schema and 1..=3 valid non-symlink PNG exemplars before advancing lifecycle.
    style.validate()?;
    let exemplar_pngs = load_and_verify_style_exemplars(&style_dir, &style)?;
    if exemplar_pngs.len() < MIN_STYLE_EXEMPLARS || exemplar_pngs.len() > MAX_STYLE_EXEMPLARS {
        return Err(ApiError::bad_request(format!(
            "Style '{slug}' must have {MIN_STYLE_EXEMPLARS}..={MAX_STYLE_EXEMPLARS} valid exemplars"
        )));
    }

    let next_lifecycle = match req.target_lifecycle {
        Some(target) => target,
        None => match style.lifecycle {
            Lifecycle::Draft => Lifecycle::Candidate,
            Lifecycle::Candidate => Lifecycle::Validated,
            Lifecycle::Validated => Lifecycle::Canonical,
            other => other,
        },
    };
    style.lifecycle = style.lifecycle.transition_to(next_lifecycle)?;
    let yaml = style.to_yaml_string()?;

    let lock_result = if next_lifecycle == Lifecycle::Canonical {
        let mut artifact_hashes = BTreeMap::new();
        let rel_card = card_path
            .strip_prefix(&ctx.root)
            .map_err(|_| ApiError::bad_request("Style card outside workspace"))?
            .to_string_lossy()
            .replace('\\', "/");
        artifact_hashes.insert(rel_card, sha256_hex(yaml.as_bytes()));
        for (rel_ex, bytes) in &exemplar_pngs {
            let full_ex = style_dir.join(rel_ex);
            let rel_ws = full_ex
                .strip_prefix(&ctx.root)
                .map_err(|_| ApiError::bad_request("Style exemplar outside workspace"))?
                .to_string_lossy()
                .replace('\\', "/");
            artifact_hashes.insert(rel_ws, sha256_hex(bytes));
        }

        let lock_store = ctx.lock_store();
        let lock_key = format!("style:{slug}");
        let existing_lock = lock_store.load_or_init()?;
        let next_ver = existing_lock
            .canonical
            .get(&lock_key)
            .map_or(1, |entry| entry.version.saturating_add(1));
        write_atomic(&card_path, yaml.as_bytes(), "Style card.yaml")?;
        let lock = lock_store.promote_canonical(ManifestEntry {
            entry_kind: ManifestEntryKind::Style,
            key: lock_key,
            version: next_ver,
            version_label: RunStore::format_version_label(next_ver),
            lifecycle: Lifecycle::Canonical,
            visibility: Visibility::Public,
            artifact_hashes,
            promoted_at_iso: now_utc_iso(),
        })?;
        Some(lock)
    } else {
        write_atomic(&card_path, yaml.as_bytes(), "Style card.yaml")?;
        None
    };

    let resolved = ctx.registry().resolve_style(&slug)?;
    state.emit_event(
        "promoted",
        "style",
        &slug,
        Some(resolved.style.lifecycle),
        format!(
            "Style '{slug}' transitioned to {}",
            resolved.style.lifecycle.as_str()
        ),
    );

    Ok(Json(serde_json::json!({
        "style": style_to_dto(&resolved),
        "manifestLock": lock_result.map(manifest_lock_to_dto),
    })))
}

async fn create_atmosphere_handler(
    State(state): State<AppState>,
    Json(req): Json<CreateAtmosphereRequest>,
) -> Result<(StatusCode, Json<AtmosphereDto>), ApiError> {
    let ctx = resolve_workspace_ctx(&state)?;
    validate_slug(&req.slug, "Atmosphere").map_err(ApiError::bad_request)?;

    let library_dir = ctx.library_dir();
    let rel_dir = Path::new("atmospheres").join(&req.slug);
    verify_existing_dir_prefix_no_symlinks(&library_dir, &rel_dir, "Atmosphere directory")?;
    let atmo_dir = library_dir.join(&rel_dir);
    if atmo_dir.join(CARD_FILENAME).exists() {
        return Err(ApiError::conflict(
            "atmosphere_exists",
            format!(
                "Atmosphere '{}' already exists in project library",
                req.slug
            ),
        ));
    }

    let atmosphere = Atmosphere {
        schema_version: 1,
        slug: req.slug.clone(),
        display_name: req.display_name,
        key_light_direction: req.key_light_direction,
        secondary_light_direction: req.secondary_light_direction,
        color_temperature_k: req.color_temperature_k,
        max_ambient_tint_blend: req.max_ambient_tint_blend,
        exposure_floor: ExposureFloor {
            min_mean_l_star: req.min_mean_l_star,
            min_p10_l_star: req.min_p10_l_star,
        },
        weather_and_particles: req.weather_and_particles,
    };
    atmosphere.validate()?;
    let yaml = atmosphere.to_yaml_string()?;

    fs::create_dir_all(&atmo_dir).map_err(|err| CoreError::Io {
        context: format!("creating Atmosphere directory {}", atmo_dir.display()),
        source: err,
    })?;
    verify_contained_path_no_symlinks(&library_dir, &rel_dir, true, "Atmosphere directory")?;
    write_atomic(
        &atmo_dir.join(CARD_FILENAME),
        yaml.as_bytes(),
        "Atmosphere card.yaml",
    )?;

    let resolved = ctx.registry().resolve_atmosphere(&req.slug)?;
    state.emit_event(
        "created",
        "atmosphere",
        &req.slug,
        None,
        format!("Created Atmosphere '{}'", req.slug),
    );
    Ok((StatusCode::CREATED, Json(atmosphere_to_dto(&resolved))))
}

async fn update_atmosphere_handler(
    State(state): State<AppState>,
    AxumPath(slug): AxumPath<String>,
    Json(req): Json<UpdateAtmosphereRequest>,
) -> Result<Json<AtmosphereDto>, ApiError> {
    let ctx = resolve_workspace_ctx(&state)?;
    validate_slug(&slug, "Atmosphere").map_err(ApiError::bad_request)?;

    let library_dir = ctx.library_dir();
    let rel_dir = Path::new("atmospheres").join(&slug);
    let Some(atmo_dir) =
        verify_optional_contained_dir_no_symlinks(&library_dir, &rel_dir, "Atmosphere directory")?
    else {
        return Err(ApiError::not_found(format!(
            "Atmosphere '{slug}' does not exist in project library"
        )));
    };
    let card_path = verify_contained_path_no_symlinks(
        &atmo_dir,
        Path::new(CARD_FILENAME),
        false,
        "Atmosphere card.yaml",
    )?;
    let bytes = read_regular_file_nofollow(&card_path, "Atmosphere card.yaml")?;
    let raw = String::from_utf8(bytes).map_err(|err| {
        ApiError::bad_request(format!("Atmosphere card.yaml is not valid UTF-8: {err}"))
    })?;
    let mut atmosphere = Atmosphere::from_yaml_str(&raw)?;

    if let Some(v) = req.display_name {
        atmosphere.display_name = v;
    }
    if let Some(v) = req.key_light_direction {
        atmosphere.key_light_direction = v;
    }
    if let Some(v) = req.secondary_light_direction {
        atmosphere.secondary_light_direction = v;
    }
    if let Some(v) = req.color_temperature_k {
        atmosphere.color_temperature_k = v;
    }
    if let Some(v) = req.max_ambient_tint_blend {
        atmosphere.max_ambient_tint_blend = v;
    }
    if let Some(v) = req.min_mean_l_star {
        atmosphere.exposure_floor.min_mean_l_star = v;
    }
    if let Some(v) = req.min_p10_l_star {
        atmosphere.exposure_floor.min_p10_l_star = v;
    }
    if let Some(v) = req.weather_and_particles {
        atmosphere.weather_and_particles = v;
    }

    atmosphere.validate()?;
    let yaml = atmosphere.to_yaml_string()?;
    write_atomic(&card_path, yaml.as_bytes(), "Atmosphere card.yaml")?;

    let resolved = ctx.registry().resolve_atmosphere(&slug)?;
    state.emit_event(
        "updated",
        "atmosphere",
        &slug,
        None,
        format!("Updated Atmosphere '{slug}'"),
    );
    Ok(Json(atmosphere_to_dto(&resolved)))
}

async fn update_subject_palette_lock_handler(
    State(state): State<AppState>,
    AxumPath(slug): AxumPath<String>,
    Json(req): Json<UpdatePaletteLockRequest>,
) -> Result<Json<SubjectDto>, ApiError> {
    let ctx = resolve_workspace_ctx(&state)?;
    validate_slug(&slug, "Subject").map_err(ApiError::bad_request)?;

    let library_dir = ctx.library_dir();
    let rel_dir = Path::new("subjects").join(&slug);
    let Some(subj_dir) =
        verify_optional_contained_dir_no_symlinks(&library_dir, &rel_dir, "Subject directory")?
    else {
        return Err(ApiError::not_found(format!(
            "Subject '{slug}' does not exist in project library"
        )));
    };
    let card_path = verify_contained_path_no_symlinks(
        &subj_dir,
        Path::new(CARD_FILENAME),
        false,
        "Subject card.yaml",
    )?;
    let bytes = read_regular_file_nofollow(&card_path, "Subject card.yaml")?;
    let raw = String::from_utf8(bytes).map_err(|err| {
        ApiError::bad_request(format!("Subject card.yaml is not valid UTF-8: {err}"))
    })?;
    let mut subject = Subject::from_yaml_str(&raw)?;
    subject
        .lifecycle
        .ensure_mutable(&format!("subject:{slug}"))?;

    let target_name = req.name.trim();
    let slot_opt = req
        .surface_slot
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    if let Some(existing) = subject
        .palette_lock
        .swatches
        .iter_mut()
        .find(|sw| sw.name.eq_ignore_ascii_case(target_name))
    {
        existing.color_hex = req.color_hex;
        if let Some(slot) = slot_opt {
            existing.surface_slot = Some(slot);
        }
    } else {
        subject.palette_lock.swatches.push(PaletteSwatch {
            name: target_name.to_string(),
            color_hex: req.color_hex,
            surface_slot: slot_opt,
        });
    }

    subject.validate_and_normalize()?;
    let yaml = subject.to_yaml_string()?;
    write_atomic(&card_path, yaml.as_bytes(), "Subject card.yaml")?;

    let resolved = ctx.registry().resolve_subject(&slug)?;
    state.emit_event(
        "updated",
        "subject",
        &slug,
        Some(resolved.subject.lifecycle),
        format!("Updated PaletteLock on Subject '{slug}'"),
    );
    Ok(Json(subject_to_dto(&resolved)))
}

async fn validate_guide_handler(
    Json(payload): Json<GuideSpecRequestPayload>,
) -> Json<serde_json::Value> {
    let val = payload.into_value();
    let raw = match serde_json::to_string(&val) {
        Ok(s) => s,
        Err(err) => {
            return Json(serde_json::json!({
                "valid": false,
                "spec": serde_json::Value::Null,
                "errors": [GuideValidationErrorDto {
                    path: "$".to_string(),
                    message: err.to_string(),
                }],
            }));
        }
    };

    match GuideSpec::from_json_str(&raw) {
        Ok(spec) => Json(serde_json::json!({
            "valid": true,
            "spec": spec,
            "errors": Vec::<GuideValidationErrorDto>::new(),
        })),
        Err(err) => Json(serde_json::json!({
            "valid": false,
            "spec": serde_json::Value::Null,
            "errors": [GuideValidationErrorDto {
                path: "$".to_string(),
                message: err.to_string(),
            }],
        })),
    }
}

async fn render_guide_handler(
    State(state): State<AppState>,
    Json(payload): Json<GuideSpecRequestPayload>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let val = payload.into_value();
    let raw = serde_json::to_string(&val)
        .map_err(|err| ApiError::bad_request(format!("invalid GuideSpec JSON: {err}")))?;
    let spec = match GuideSpec::from_json_str(&raw) {
        Ok(s) => s,
        Err(err) => {
            return Err(ApiError::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "guidespec_invalid",
                err.to_string(),
            ));
        }
    };

    let _temp_blockout_dir;
    let render_root = match state.workspace_root {
        Some(_) => resolve_workspace_ctx(&state)?.root,
        None => {
            if spec.variant == imgforge_core::SpatialGuideVariant::OverlayGuide {
                return Err(ApiError::new(
                    StatusCode::PRECONDITION_REQUIRED,
                    "workspace_unconfigured",
                    "OverlayGuide requires an active workspace for targetImagePath containment",
                ));
            }
            _temp_blockout_dir = TempDir::new().map_err(|err| CoreError::Io {
                context: "creating temp render dir for Blockout".to_string(),
                source: err,
            })?;
            _temp_blockout_dir.path().to_path_buf()
        }
    };

    let rendered = GuideRenderer::new().render(&spec, &render_root)?;
    let png_base64 = BASE64_STANDARD.encode(&rendered.png_bytes);
    Ok(Json(serde_json::json!({
        "id": spec.id,
        "variant": rendered.variant,
        "width": rendered.width,
        "height": rendered.height,
        "pngBase64": png_base64,
        "errors": Vec::<GuideValidationErrorDto>::new(),
    })))
}

async fn reslice_pack_handler(
    State(state): State<AppState>,
    AxumPath((subject, style)): AxumPath<(String, String)>,
    Json(req): Json<ReslicePackRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let ctx = resolve_workspace_ctx(&state)?;
    let slices_manifest: SlicesManifest = reslice_reference_pack_sheet(
        &ctx.library_dir(),
        &subject,
        &style,
        req.sheet_role,
        req.mode,
        req.overrides,
    )?;

    let pack_dir = ctx
        .library_dir()
        .join("subjects")
        .join(&subject)
        .join("packs")
        .join(&style);
    let slices_dto: Vec<SliceDto> = slices_manifest
        .slices
        .iter()
        .map(|sl| SliceDto {
            id: sl.id.clone(),
            file: sl.file.clone(),
            image_url: ctx
                .asset_url_for_path(&pack_dir.join(&sl.file))
                .unwrap_or_default(),
            rect: sl.rect,
            source: sl.source,
            label: sl.label.clone(),
            sha256: sl.sha256.clone(),
        })
        .collect();

    state.emit_event(
        "updated",
        "referencePack",
        format!("{subject}/{style}"),
        Some(Lifecycle::Draft),
        format!(
            "Resliced sheet '{}' for '{subject}/{style}'",
            req.sheet_role.filename()
        ),
    );

    Ok(Json(serde_json::json!({
        "schemaVersion": slices_manifest.schema_version,
        "sheetFilename": slices_manifest.sheet_filename,
        "sheetWidth": slices_manifest.sheet_width,
        "sheetHeight": slices_manifest.sheet_height,
        "calibrationStripHeight": slices_manifest.calibration_strip_height,
        "slices": slices_dto,
    })))
}

async fn validate_pack_handler(
    State(state): State<AppState>,
    AxumPath((subject, style)): AxumPath<(String, String)>,
    body: Bytes,
) -> Result<Json<serde_json::Value>, ApiError> {
    let req_body: ValidatePackApiRequest = parse_optional_json_body(&body)?;
    let ctx = resolve_workspace_ctx(&state)?;
    let registry = ctx.registry();

    let mut traits = Vec::with_capacity(req_body.traits.len());
    for t in req_body.traits {
        let category = t.category.parse::<TraitCategory>()?;
        traits.push(TraitCheckInput {
            trait_id: t.trait_id,
            category,
            criterion: t.criterion,
        });
    }

    let critic = VisionCritic::with_codex_bin(state.auth_dir.clone(), state.codex_bin.clone());
    let mut val_req = ValidateReferencePackRequest::new(subject.clone(), style.clone());
    val_req.traits = traits;
    val_req.run_critic = req_body.run_critic;

    let res =
        validate_reference_pack(&ctx.library_dir(), &registry, Some(&critic), &val_req).await?;
    let pack_dir = ctx
        .library_dir()
        .join("subjects")
        .join(&subject)
        .join("packs")
        .join(&style);

    state.emit_event(
        "validated",
        "referencePack",
        format!("{subject}/{style}"),
        Some(res.manifest.lifecycle),
        format!("Validated ReferencePack '{subject}/{style}'"),
    );

    Ok(Json(serde_json::json!({
        "referencePack": pack_to_dto(&ctx, &pack_dir, &res.manifest, None),
        "checks": res.checks,
        "criticReport": res.critic_report,
    })))
}

async fn promote_pack_handler(
    State(state): State<AppState>,
    AxumPath((subject, style)): AxumPath<(String, String)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let ctx = resolve_workspace_ctx(&state)?;
    let lock_store = ctx.lock_store();
    let (manifest, lock) =
        promote_reference_pack(&lock_store, &ctx.library_dir(), &subject, &style, &ctx.root)?;
    let pack_dir = ctx
        .library_dir()
        .join("subjects")
        .join(&subject)
        .join("packs")
        .join(&style);

    state.emit_event(
        "promoted",
        "referencePack",
        format!("{subject}/{style}"),
        Some(manifest.lifecycle),
        format!("Promoted ReferencePack '{subject}/{style}' to canonical"),
    );

    Ok(Json(serde_json::json!({
        "referencePack": pack_to_dto(&ctx, &pack_dir, &manifest, None),
        "canonicalCount": lock.canonical.len(),
        "supersededCount": lock.superseded.len(),
    })))
}

async fn patch_edit_handler(
    State(state): State<AppState>,
    Json(req): Json<PatchEditApiRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let ctx = resolve_workspace_ctx(&state)?;
    let Some(ref backend) = state.backend else {
        return Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "backend_unavailable",
            "no ImageBackend configured in AppState",
        ));
    };

    validate_slug(&req.slug, "Run").map_err(ApiError::bad_request)?;
    let run_store = ctx.run_store();
    let registry = ctx.registry();

    let target_image_path = match (req.target_image_path, req.run_slug, req.run_version) {
        (Some(raw_path), None, None) => {
            resolve_contained_workspace_file(&ctx.root, &raw_path, "targetImagePath")?
        }
        (None, Some(r_slug), Some(r_ver)) => {
            validate_slug(&r_slug, "Run").map_err(ApiError::bad_request)?;
            let rel_run_png =
                Path::new(&r_slug).join(format!("{}.png", RunStore::format_version_label(r_ver)));
            verify_contained_path_no_symlinks(run_store.runs_dir(), &rel_run_png, false, "Run PNG")?
        }
        _ => {
            return Err(ApiError::bad_request(
                "provide either targetImagePath or (runSlug, runVersion)",
            ));
        }
    };

    let _temp_mask_dir;
    let target_mask = match (req.roi, req.mask_png_base64) {
        (Some(roi), None) => PatchTargetMask::Roi(roi),
        (None, Some(mask_b64)) => {
            let png_bytes = decode_base64_png_bounded(&mask_b64, "maskPngBase64")?;
            let tmp = TempDir::new().map_err(|err| CoreError::Io {
                context: "creating temp dir for PatchEdit mask".to_string(),
                source: err,
            })?;
            let mask_path = tmp.path().join("mask.png");
            write_atomic(&mask_path, &png_bytes, "PatchEdit mask PNG")?;
            _temp_mask_dir = tmp;
            PatchTargetMask::MaskImagePath(mask_path)
        }
        _ => {
            return Err(ApiError::bad_request(
                "provide exactly one of roi or maskPngBase64",
            ));
        }
    };

    let mut reference_images = Vec::new();
    for raw_ref in req.reference_images.unwrap_or_default() {
        reference_images.push(resolve_contained_workspace_file(
            &ctx.root,
            &raw_ref,
            "referenceImages",
        )?);
    }

    let config = PatchEditConfig {
        collar_width_px: req.collar_width_px.unwrap_or(DEFAULT_COLLAR_WIDTH_PX),
        feather_px: req.feather_px.unwrap_or(12),
        prep_mode: req.prep_mode.unwrap_or_default(),
        ..PatchEditConfig::default()
    };

    let res = execute_patch_edit(
        backend.as_ref(),
        &run_store,
        Some(&registry),
        &PatchEditRequest {
            slug: req.slug,
            visibility: req.visibility.unwrap_or(Visibility::Public),
            target_image_path: target_image_path.clone(),
            target_mask,
            compiled_prompt: req.prompt,
            reference_images,
            config,
        },
    )
    .await?;

    let color_qa = ColorQaReport {
        passed: res.out_of_roi_report.passed,
        exposure: None,
        palette_adherence: Vec::new(),
        untouched_drift: Some(res.out_of_roi_report.clone()),
        sparkle: None,
        max_delta_e2000: res.out_of_roi_report.max_delta_e2000,
        notes: res.run.metadata.qa_reports.structural_checks.clone(),
    };

    let before_image_url = ctx
        .asset_url_for_path(&target_image_path)
        .unwrap_or_default();
    let after_image_url = ctx
        .asset_url_for_path(&res.run.image_path)
        .unwrap_or_default();
    let run_dto = run_to_dto(&ctx, &res.run.metadata);

    state.emit_event(
        "created",
        "run",
        &run_dto.run_id,
        Some(run_dto.lifecycle),
        format!("PatchEdit recorded '{}'", run_dto.run_id),
    );

    Ok(Json(serde_json::json!({
        "run": run_dto,
        "beforeImageUrl": before_image_url,
        "afterImageUrl": after_image_url,
        "cropRect": res.crop_rect,
        "roiRect": res.roi_rect,
        "reinhardGains": res.reinhard_gains,
        "colorQa": color_qa,
    })))
}

async fn inspect_run_handler(
    State(state): State<AppState>,
    AxumPath((slug, version)): AxumPath<(String, u32)>,
    body: Bytes,
) -> Result<Json<serde_json::Value>, ApiError> {
    let req: InspectRunRequest = parse_optional_json_body(&body)?;
    let ctx = resolve_workspace_ctx(&state)?;
    let run_store = ctx.run_store();
    let registry = ctx.registry();

    let meta = run_store.load_run(&slug, version)?;
    let rel_img = Path::new(&slug).join(format!("{}.png", RunStore::format_version_label(version)));
    let img_path =
        verify_contained_path_no_symlinks(run_store.runs_dir(), &rel_img, false, "Run PNG")?;
    let png_bytes = read_regular_file_nofollow(&img_path, "Run PNG")?;
    let rgba = decode_png_checked(&img_path, &png_bytes)?.to_rgba8();

    let max_samples = req
        .max_samples
        .unwrap_or(DEFAULT_VECTORSCOPE_SAMPLES)
        .min(MAX_VECTORSCOPE_SAMPLES);
    let telemetry = compute_l_star_histogram_and_vectorscope(&rgba, max_samples)?;

    let exposure_cfg = ExposureQaConfig::default();
    let exposure_arg = match req.atmosphere_slug {
        Some(ref atmo_slug) => {
            let resolved = registry.resolve_atmosphere(atmo_slug)?;
            Some((resolved.atmosphere.exposure_floor, &exposure_cfg))
        }
        None => None,
    };

    let mut owned_rois = Vec::with_capacity(req.subject_slugs.len());
    for s_slug in &req.subject_slugs {
        let resolved = registry.resolve_subject(s_slug)?;
        check_visibility_compatibility(
            meta.visibility,
            "InspectRun Subject",
            [(resolved.subject.slug.as_str(), resolved.subject.visibility)],
        )?;
        owned_rois.push(OwnedSubjectRoi {
            subject_slug: resolved.subject.slug,
            visibility: resolved.subject.visibility,
            palette_lock: resolved.subject.palette_lock,
            rois: Vec::new(),
        });
    }

    let qa_specs: Vec<imgforge_core::SubjectRoiSpec<'_>> = owned_rois
        .iter()
        .map(|s| imgforge_core::SubjectRoiSpec {
            subject_slug: &s.subject_slug,
            palette_lock: &s.palette_lock,
            rois: s.rois.clone(),
        })
        .collect();

    let qa_engine = ColorQaEngine::new();
    let palette_cfg = PaletteQaConfig::default();
    let sparkle_cfg = SparkleConfig::default();
    let drift_cfg = UntouchedDriftConfig::default();

    let before_rgba_opt = match (req.before_image_path, req.before_run_version) {
        (Some(raw_before), None) => {
            let before_path =
                resolve_contained_workspace_file(&ctx.root, &raw_before, "beforeImagePath")?;
            let before_bytes = read_regular_file_nofollow(&before_path, "before PNG")?;
            Some(decode_png_checked(&before_path, &before_bytes)?.to_rgba8())
        }
        (None, Some(before_ver)) => {
            let rel_before = Path::new(&slug).join(format!(
                "{}.png",
                RunStore::format_version_label(before_ver)
            ));
            let before_path = verify_contained_path_no_symlinks(
                run_store.runs_dir(),
                &rel_before,
                false,
                "before Run PNG",
            )?;
            let before_bytes = read_regular_file_nofollow(&before_path, "before Run PNG")?;
            Some(decode_png_checked(&before_path, &before_bytes)?.to_rgba8())
        }
        (Some(_), Some(_)) => {
            return Err(ApiError::bad_request(
                "beforeImagePath and beforeRunVersion are mutually exclusive",
            ));
        }
        (None, None) => None,
    };

    let zero_mask = vec![0.0f32; (rgba.width() as usize) * (rgba.height() as usize)];
    let untouched_arg = before_rgba_opt
        .as_ref()
        .map(|before| (before, zero_mask.as_slice(), &drift_cfg));

    let color_qa = qa_engine.evaluate_whole_image(
        &rgba,
        exposure_arg,
        &qa_specs,
        &palette_cfg,
        untouched_arg,
        Some((None, &sparkle_cfg)),
    )?;

    // Render base64 PNG diagnostic heatmaps.
    let (_, sparkle_map) = qa_engine.detect_sparkles(&rgba, None, &sparkle_cfg)?;
    let sparkle_png = qa_engine.render_heatmap_png(
        rgba.width(),
        rgba.height(),
        &sparkle_map,
        0.0,
        0.5,
        HeatmapKind::Sparkle,
    )?;
    let sparkle_b64 = Some(BASE64_STANDARD.encode(sparkle_png));

    let bleed_b64 = if !qa_specs.is_empty() {
        let total = (rgba.width() as usize) * (rgba.height() as usize);
        let mut combined_bleed = vec![0.0f64; total];
        for spec in &qa_specs {
            let (_, map) = qa_engine.evaluate_palette_adherence(&rgba, spec, &palette_cfg)?;
            for (dst, src) in combined_bleed.iter_mut().zip(map.iter()) {
                *dst = dst.max(*src);
            }
        }
        let bleed_png = qa_engine.render_heatmap_png(
            rgba.width(),
            rgba.height(),
            &combined_bleed,
            palette_cfg.max_swatch_delta_e2000 * 0.5,
            palette_cfg.max_swatch_delta_e2000 * 1.5,
            HeatmapKind::Bleed,
        )?;
        Some(BASE64_STANDARD.encode(bleed_png))
    } else {
        None
    };

    let (drift_b64, out_of_roi_b64) = if let Some(ref before_rgba) = before_rgba_opt {
        let drift_png = qa_engine.export_drift_heatmap_png(
            before_rgba,
            &rgba,
            &zero_mask,
            &drift_cfg,
            HeatmapKind::Drift,
        )?;
        let oor_png = qa_engine.export_drift_heatmap_png(
            before_rgba,
            &rgba,
            &zero_mask,
            &drift_cfg,
            HeatmapKind::OutOfRoi,
        )?;
        (
            Some(BASE64_STANDARD.encode(drift_png)),
            Some(BASE64_STANDARD.encode(oor_png)),
        )
    } else {
        (None, None)
    };

    Ok(Json(serde_json::json!({
        "run": run_to_dto(&ctx, &meta),
        "telemetry": telemetry,
        "colorQa": color_qa,
        "heatmaps": {
            "drift": drift_b64,
            "bleed": bleed_b64,
            "sparkle": sparkle_b64,
            "outOfRoi": out_of_roi_b64,
        },
    })))
}

async fn validate_run_handler(
    State(state): State<AppState>,
    AxumPath((slug, version)): AxumPath<(String, u32)>,
    body: Bytes,
) -> Result<Json<serde_json::Value>, ApiError> {
    let req: ValidateRunApiRequest = parse_optional_json_body(&body)?;
    let ctx = resolve_workspace_ctx(&state)?;
    let run_store = ctx.run_store();
    let registry = ctx.registry();

    let mut reference_images = Vec::with_capacity(req.reference_images.len());
    for raw_ref in &req.reference_images {
        reference_images.push(resolve_contained_workspace_file(
            &ctx.root,
            raw_ref,
            "referenceImages",
        )?);
    }

    let mut traits = Vec::with_capacity(req.traits.len());
    for t in req.traits {
        let category = t.category.parse::<TraitCategory>()?;
        traits.push(TraitCheckInput {
            trait_id: t.trait_id,
            category,
            criterion: t.criterion,
        });
    }

    let subject_rois = req
        .subject_slugs
        .into_iter()
        .map(|s| SubjectRoiBinding {
            subject_slug: s,
            rois: Vec::new(),
        })
        .collect();

    let critic = VisionCritic::with_codex_bin(state.auth_dir.clone(), state.codex_bin.clone());
    let res = validate_run(
        &run_store,
        &registry,
        Some(&critic),
        &ValidateRunRequest {
            slug: slug.clone(),
            version,
            atmosphere_slug: req.atmosphere_slug,
            subject_rois,
            explicit_subject_rois: Vec::new(),
            reference_images,
            traits,
            run_critic: req.run_critic,
            exposure_config: ExposureQaConfig::default(),
            palette_config: PaletteQaConfig::default(),
            sparkle_config: SparkleConfig::default(),
        },
    )
    .await?;

    let run_dto = run_to_dto(&ctx, &res.metadata);
    state.emit_event(
        "validated",
        "run",
        &run_dto.run_id,
        Some(run_dto.lifecycle),
        format!("Validated Run '{}'", run_dto.run_id),
    );

    Ok(Json(serde_json::json!({
        "run": run_dto,
        "colorQa": res.color_qa,
        "criticReport": res.critic_report,
    })))
}

async fn promote_run_handler(
    State(state): State<AppState>,
    AxumPath((slug, version)): AxumPath<(String, u32)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let ctx = resolve_workspace_ctx(&state)?;
    let lock_store = ctx.lock_store();
    let run_store = ctx.run_store();
    let (meta, lock) = promote_run(&lock_store, &run_store, &slug, version, &ctx.root)?;
    let run_dto = run_to_dto(&ctx, &meta);

    state.emit_event(
        "promoted",
        "run",
        &run_dto.run_id,
        Some(run_dto.lifecycle),
        format!("Promoted Run '{}' to canonical", run_dto.run_id),
    );

    Ok(Json(serde_json::json!({
        "run": run_dto,
        "canonicalCount": lock.canonical.len(),
        "supersededCount": lock.superseded.len(),
    })))
}

async fn export_handler(
    State(state): State<AppState>,
    Json(req): Json<ExportApiRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let ctx = resolve_workspace_ctx(&state)?;
    let run_store = ctx.run_store();
    let registry = ctx.registry();

    let run_meta = run_store.load_run(&req.run_slug, req.run_version)?;
    if !matches!(
        run_meta.lifecycle,
        Lifecycle::Validated | Lifecycle::Canonical
    ) {
        return Err(ApiError::conflict(
            "export_requires_validated_or_canonical",
            format!(
                "Run '{}' has lifecycle '{}'; advance to 'validated' or 'canonical' before exporting",
                run_meta.run_id,
                run_meta.lifecycle.as_str()
            ),
        ));
    }

    let out_dir = match req.out_dir {
        Some(ref raw_out) => {
            resolve_contained_workspace_dir_for_write(&ctx.root, raw_out, "outDir")?
        }
        None => {
            let rel_exports = ctx.paths.exports_dir.to_string_lossy().into_owned();
            resolve_contained_workspace_dir_for_write(&ctx.root, &rel_exports, "exports_dir")?
        }
    };

    let profile = match req.profile.as_str() {
        "vtt_token" => ExportProfile::VttToken(VttTokenOptions {
            size_px: req.token_size.unwrap_or(DEFAULT_VTT_TOKEN_SIZE_PX),
            ring_color_hex: req
                .ring_color_hex
                .unwrap_or_else(|| DEFAULT_TOKEN_RING_HEX.to_string()),
            jpeg_backdrop_hex: req.jpeg_backdrop_hex,
            jpeg_quality: req.jpeg_quality.unwrap_or(90),
            ..VttTokenOptions::default()
        }),
        "vtt_map" => ExportProfile::VttMap(VttMapOptions {
            cols: req.cols.unwrap_or(24),
            rows: req.rows.unwrap_or(18),
            grid_px: req.grid_px.unwrap_or(DEFAULT_VTT_MAP_GRID_PX),
            scale_factor: req.scale_factor,
            jpeg_quality: req.jpeg_quality.unwrap_or(90),
        }),
        "web_webp" => ExportProfile::WebWebp,
        "grid_2x2" => ExportProfile::Grid2x2,
        other => {
            return Err(ApiError::bad_request(format!(
                "unknown export profile '{other}' (expected vtt_token, vtt_map, web_webp, or grid_2x2)"
            )));
        }
    };

    let match_grade = match req.match_grade {
        Some(mg) if mg.enabled => {
            let reference_image_path = match mg.grade_ref_path {
                Some(ref raw) => Some(resolve_contained_workspace_file(
                    &ctx.root,
                    raw,
                    "gradeRefPath",
                )?),
                None => None,
            };
            let cube_lut_path = match mg.cube_lut_path {
                Some(ref raw) => Some(resolve_contained_workspace_file(
                    &ctx.root,
                    raw,
                    "cubeLutPath",
                )?),
                None => None,
            };
            Some(MatchGradeOptions {
                reference_image_path,
                cdf_damp_strength: mg.grade_strength.unwrap_or(DEFAULT_CDF_DAMP_STRENGTH),
                cube_lut_path,
            })
        }
        _ => None,
    };

    let output_slug = req.output_slug.unwrap_or_else(|| req.run_slug.clone());
    let result = execute_export(
        Some(&run_store),
        Some(&registry),
        &ExportRequest {
            source: ExportSource::Run {
                slug: req.run_slug,
                version: req.run_version,
            },
            output_slug,
            profile_visibility: req.visibility.unwrap_or(Visibility::Public),
            out_dir,
            profile,
            match_grade,
        },
    )?;

    let artifacts: Vec<serde_json::Value> = result
        .artifacts
        .iter()
        .map(|art| {
            let asset_url = ctx.asset_url_for_path(&art.path);
            serde_json::json!({
                "role": art.role,
                "path": art.path,
                "assetUrl": asset_url,
                "width": art.width,
                "height": art.height,
                "sha256": art.sha256,
            })
        })
        .collect();

    state.emit_event(
        "exported",
        "export",
        &result.source_id,
        Some(run_meta.lifecycle),
        format!(
            "Exported {} artifact(s) from '{}'",
            artifacts.len(),
            result.source_id
        ),
    );

    Ok(Json(serde_json::json!({
        "sourceId": result.source_id,
        "sourceVisibility": result.source_visibility,
        "profileVisibility": result.profile_visibility,
        "matchGradeApplied": result.match_grade_applied,
        "artifacts": artifacts,
    })))
}

async fn ws_handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    let rx = state.events_tx.subscribe();
    ws.on_upgrade(move |socket| handle_ws_connection(socket, rx))
}

async fn handle_ws_connection(mut socket: WebSocket, mut rx: broadcast::Receiver<WorkspaceEvent>) {
    loop {
        tokio::select! {
            recv_res = rx.recv() => {
                match recv_res {
                    Ok(ev) => {
                        let Ok(payload) = serde_json::to_string(&ev) else {
                            continue;
                        };
                        if socket.send(Message::Text(payload.into())).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
            ws_msg = socket.recv() => {
                match ws_msg {
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                    Some(Ok(Message::Ping(data))) => {
                        if socket.send(Message::Pong(data)).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(_)) => {}
                }
            }
        }
    }
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

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::io::Cursor;
    #[cfg(unix)]
    use std::os::unix::fs::{PermissionsExt, symlink};
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use axum::body::Body;
    use axum::http::{Method, Request};
    use futures_util::StreamExt;
    use http_body_util::BodyExt;
    use image::{DynamicImage, ImageBuffer, ImageFormat, Rgba, RgbaImage};
    use imgforge_core::calibration::STUDIO_BG_RGBA;
    use imgforge_core::{
        CalibrationStripLayout, GeneratedSheetRecord, ImageGenerationOutput,
        ImageGenerationRequest, RecordRunParams, ReferencePackManifest, ReferenceSheetRole,
        RunBackendMetadata, RunQaReports, SheetStructuralReport, SubjectKind, now_utc_iso,
        sha256_hex, write_atomic,
    };
    use rstest::rstest;
    use tempfile::TempDir;
    use tokio_tungstenite::tungstenite::Message as WsMessage;
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    use tower::ServiceExt;

    use super::*;

    #[derive(Clone)]
    struct StubServerBackend {
        fill: [u8; 3],
        calls: Arc<Mutex<Vec<ImageGenerationRequest>>>,
    }

    impl StubServerBackend {
        fn new(fill: [u8; 3]) -> Self {
            Self {
                fill,
                calls: Arc::new(Mutex::new(Vec::new())),
            }
        }
    }

    #[async_trait]
    impl ImageBackend for StubServerBackend {
        fn name(&self) -> &'static str {
            "stub-server-backend"
        }

        async fn generate(
            &self,
            request: &ImageGenerationRequest,
        ) -> Result<ImageGenerationOutput, CoreError> {
            self.calls.lock().unwrap().push(request.clone());
            let img: RgbaImage = ImageBuffer::from_pixel(
                request.width,
                request.height,
                Rgba([self.fill[0], self.fill[1], self.fill[2], 255]),
            );
            let mut png_bytes = Vec::new();
            DynamicImage::ImageRgba8(img)
                .write_to(&mut Cursor::new(&mut png_bytes), ImageFormat::Png)
                .unwrap();
            Ok(ImageGenerationOutput {
                png_bytes,
                width: request.width,
                height: request.height,
                raw_width: request.width,
                raw_height: request.height,
                thread_id: Some("thr_server_stub".to_string()),
            })
        }
    }

    fn make_png(w: u32, h: u32, fill: [u8; 4]) -> Vec<u8> {
        let img: RgbaImage = ImageBuffer::from_pixel(w, h, Rgba(fill));
        let mut buf = Vec::new();
        DynamicImage::ImageRgba8(img)
            .write_to(&mut Cursor::new(&mut buf), ImageFormat::Png)
            .unwrap();
        buf
    }

    fn setup_test_workspace() -> (TempDir, PathBuf, PathBuf, PathBuf) {
        let root = TempDir::new().unwrap();
        let ws = root.path().join("ws");
        fs::create_dir_all(ws.join("library/subjects/courier-ren")).unwrap();
        fs::create_dir_all(ws.join("runs")).unwrap();
        fs::write(
            ws.join("imgforge.toml"),
            "schema_version = 1\nname = \"phase5-test\"\n",
        )
        .unwrap();

        fs::write(
            ws.join("library/subjects/courier-ren/card.yaml"),
            r##"schema_version: 1
kind: character
slug: courier-ren
display_name: Courier Ren
height_m: 1.74
exclusive_surface_map:
  bindings:
    - slot: coat
      material: waxed canvas
      color_hex: "#2C3E50"
palette_lock:
  swatches:
    - name: coat_slate
      color_hex: "#2C3E50"
      surface_slot: coat
visibility: public
"##,
        )
        .unwrap();

        let auth_dir = root.path().join("auth");
        fs::create_dir_all(&auth_dir).unwrap();
        #[cfg(unix)]
        fs::set_permissions(&auth_dir, fs::Permissions::from_mode(0o700)).unwrap();
        let auth_file = auth_dir.join("auth.json");
        fs::write(&auth_file, r#"{"token":"ok"}"#).unwrap();
        #[cfg(unix)]
        fs::set_permissions(&auth_file, fs::Permissions::from_mode(0o600)).unwrap();

        let stub_codex = root.path().join("codex-critic-stub");
        fs::write(
            &stub_codex,
            r##"#!/usr/bin/env bash
set -euo pipefail
OUT=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    -o) OUT="$2"; shift 2 ;;
    *) shift ;;
  esac
done
cat <<'JSON' > "$OUT"
{"schema_version":1,"passed":true,"checks":[{"trait_id":"zero-leaked-text","category":"zero_leaked_text","passed":true,"observation":"clean"}],"summary":"ok"}
JSON
"##,
        )
        .unwrap();
        #[cfg(unix)]
        fs::set_permissions(&stub_codex, fs::Permissions::from_mode(0o755)).unwrap();

        (root, ws, auth_dir, stub_codex)
    }

    fn seed_run_v1(ws: &Path, slug: &str, visibility: Visibility, fill: [u8; 4]) -> Vec<u8> {
        let store = RunStore::new(ws.join("runs"));
        let png = make_png(128, 128, fill);
        store
            .record_run(RecordRunParams {
                slug: slug.to_string(),
                visibility,
                png_bytes: png.clone(),
                input_hashes: BTreeMap::new(),
                compiled_prompt: "harbor pier scene".to_string(),
                backend: RunBackendMetadata {
                    name: "stub-server-backend".to_string(),
                    width: 128,
                    height: 128,
                    raw_width: 128,
                    raw_height: 128,
                    thread_id: Some("thr_1".to_string()),
                },
                seed: None,
                qa_reports: RunQaReports::default(),
            })
            .unwrap();
        png
    }

    fn seed_pack_for_reslice(ws: &Path) {
        let pack_dir = ws.join("library/subjects/courier-ren/packs/franco-belgian-line");
        fs::create_dir_all(pack_dir.join("slices")).unwrap();
        let mut sheet: RgbaImage = ImageBuffer::from_pixel(
            imgforge_core::REFERENCE_SHEET_WIDTH,
            imgforge_core::REFERENCE_SHEET_HEIGHT,
            Rgba([STUDIO_BG_RGBA[0], STUDIO_BG_RGBA[1], STUDIO_BG_RGBA[2], 255]),
        );
        for y in 120..420 {
            for x in 120..320 {
                sheet.put_pixel(x, y, Rgba([44, 62, 80, 255]));
            }
        }
        // Paint matching calibration swatch in bottom strip at x=24, y=952..1000
        let strip_y = imgforge_core::REFERENCE_SHEET_HEIGHT - 72;
        for y in (strip_y + 12)..(strip_y + 60) {
            for x in 320..368 {
                sheet.put_pixel(x, y, Rgba([0x2C, 0x3E, 0x50, 255]));
            }
        }
        let mut sheet_bytes = Vec::new();
        DynamicImage::ImageRgba8(sheet)
            .write_to(&mut Cursor::new(&mut sheet_bytes), ImageFormat::Png)
            .unwrap();
        fs::write(pack_dir.join("turnaround.png"), &sheet_bytes).unwrap();
        let sheet_sha = sha256_hex(&sheet_bytes);

        let slice_bytes = make_png(200, 300, [44, 62, 80, 255]);
        fs::write(pack_dir.join("slices/turnaround_001.png"), &slice_bytes).unwrap();
        let slice_rec = SliceRecord {
            id: "turnaround_001".to_string(),
            file: "slices/turnaround_001.png".to_string(),
            rect: PixelRect {
                x: 120,
                y: 120,
                width: 200,
                height: 300,
            },
            source: imgforge_core::SliceSource::Auto,
            label: None,
            sha256: sha256_hex(&slice_bytes),
        };
        let slices_manifest = SlicesManifest {
            schema_version: 1,
            sheet_filename: "turnaround.png".to_string(),
            sheet_width: imgforge_core::REFERENCE_SHEET_WIDTH,
            sheet_height: imgforge_core::REFERENCE_SHEET_HEIGHT,
            calibration_strip_height: 72,
            slices: vec![slice_rec.clone()],
        };
        let slices_bytes = serde_json::to_vec_pretty(&slices_manifest).unwrap();
        fs::write(pack_dir.join("slices.json"), &slices_bytes).unwrap();

        let manifest = ReferencePackManifest {
            schema_version: 1,
            version: 1,
            subject_slug: "courier-ren".to_string(),
            style_slug: "franco-belgian-line".to_string(),
            subject_kind: SubjectKind::Character,
            visibility: Visibility::Public,
            lifecycle: Lifecycle::Draft,
            seed: None,
            sheets: vec![GeneratedSheetRecord {
                role: ReferenceSheetRole::Turnaround,
                filename: "turnaround.png".to_string(),
                sha256: sheet_sha.clone(),
                structural_report: SheetStructuralReport {
                    width: imgforge_core::REFERENCE_SHEET_WIDTH,
                    height: imgforge_core::REFERENCE_SHEET_HEIGHT,
                    bg_coverage: 0.9,
                    border_bg_coverage: 1.0,
                    foreground_pixels: 60_000,
                },
                calibration_strip: CalibrationStripLayout {
                    strip_y,
                    strip_height: 72,
                    cell_size_px: 48,
                    gray_wedge_cells: Vec::new(),
                    palette_swatch_cells: vec![imgforge_core::CalibrationSwatchCell {
                        name: "coat_slate".to_string(),
                        color_hex: "#2C3E50".to_string(),
                        rect: PixelRect {
                            x: 320,
                            y: strip_y + 12,
                            width: 48,
                            height: 48,
                        },
                    }],
                },
                thread_id: None,
            }],
            slices: vec![slice_rec.clone()],
            artifact_hashes: BTreeMap::from([
                ("turnaround.png".to_string(), sheet_sha),
                (slice_rec.file, slice_rec.sha256),
                ("slices.json".to_string(), sha256_hex(&slices_bytes)),
            ]),
            created_at_iso: now_utc_iso(),
        };
        write_atomic(
            &pack_dir.join("pack.json"),
            &serde_json::to_vec_pretty(&manifest).unwrap(),
            "pack.json",
        )
        .unwrap();
    }

    async fn send_json(
        app: &Router,
        method: Method,
        uri: &str,
        body: Option<serde_json::Value>,
    ) -> (StatusCode, serde_json::Value) {
        let mut builder = Request::builder()
            .method(method)
            .uri(uri)
            .header(header::HOST, "127.0.0.1:8080");
        let req = if let Some(val) = body {
            builder = builder.header(header::CONTENT_TYPE, "application/json");
            builder
                .body(Body::from(serde_json::to_vec(&val).unwrap()))
                .unwrap()
        } else {
            builder.body(Body::empty()).unwrap()
        };
        let resp = app.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let parsed = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
        (status, parsed)
    }

    #[rstest]
    #[case::auth_provisioned_returns_ok(true, HealthStatusKind::Ok)]
    #[case::auth_missing_returns_degraded(false, HealthStatusKind::Degraded)]
    #[tokio::test]
    async fn api_health_matches_frontend_contract(
        #[case] provision_auth: bool,
        #[case] expected_status: HealthStatusKind,
    ) {
        let dir = TempDir::new().expect("tempdir");
        #[cfg(unix)]
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
        if provision_auth {
            let auth_file = dir.path().join("auth.json");
            fs::write(&auth_file, r#"{"token":"ok"}"#).unwrap();
            #[cfg(unix)]
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
        assert_eq!(
            parsed["version"],
            serde_json::json!(env!("IMGFORGE_VERSION"))
        );

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

    #[rstest]
    #[case::post_absent_origin_allowed(Method::POST, "/api/guide/validate", None, false)]
    #[case::post_loopback_ip_origin_allowed(
        Method::POST,
        "/api/guide/validate",
        Some("http://127.0.0.1:8080"),
        false
    )]
    #[case::post_localhost_origin_allowed(
        Method::POST,
        "/api/guide/validate",
        Some("http://localhost:5173"),
        false
    )]
    #[case::post_ipv6_loopback_origin_allowed(
        Method::POST,
        "/api/guide/validate",
        Some("http://[::1]:8080"),
        false
    )]
    #[case::post_cross_origin_rejected(
        Method::POST,
        "/api/guide/validate",
        Some("https://evil.example"),
        true
    )]
    #[case::post_null_origin_rejected(Method::POST, "/api/guide/validate", Some("null"), true)]
    #[case::post_https_localhost_rejected(
        Method::POST,
        "/api/guide/validate",
        Some("https://localhost:8080"),
        true
    )]
    #[case::post_origin_with_path_rejected(
        Method::POST,
        "/api/guide/validate",
        Some("http://localhost:8080/evil"),
        true
    )]
    #[case::put_cross_origin_rejected(
        Method::PUT,
        "/api/styles/custom",
        Some("http://evil.example:8080"),
        true
    )]
    #[case::delete_cross_origin_rejected(
        Method::DELETE,
        "/api/unknown",
        Some("http://localhost.evil.example"),
        true
    )]
    #[case::ws_cross_origin_rejected(Method::GET, "/api/ws", Some("https://evil.example"), true)]
    #[case::ws_null_origin_rejected(Method::GET, "/api/ws", Some("null"), true)]
    #[tokio::test]
    async fn enforces_strict_origin_allowlist_on_mutations_and_ws(
        #[case] method: Method,
        #[case] uri: &str,
        #[case] origin: Option<&str>,
        #[case] expect_forbidden: bool,
    ) {
        let dir = TempDir::new().expect("tempdir");
        let app = build_router(AppState::new(dir.path().to_path_buf()));

        let mut builder = Request::builder()
            .method(method)
            .uri(uri)
            .header(header::HOST, "127.0.0.1:8080");
        if let Some(orig) = origin {
            builder = builder.header(header::ORIGIN, orig);
        }
        let resp = app
            .oneshot(builder.body(Body::empty()).unwrap())
            .await
            .unwrap();
        if expect_forbidden {
            assert_eq!(resp.status(), StatusCode::FORBIDDEN);
            let bytes = resp.into_body().collect().await.unwrap().to_bytes();
            let parsed: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(parsed["error"], serde_json::json!("forbidden_origin"));
        } else {
            assert_ne!(resp.status(), StatusCode::FORBIDDEN);
        }
    }

    #[rstest]
    #[case::unconfigured_workspace(false)]
    #[case::configured_workspace(true)]
    #[tokio::test]
    async fn api_workspace_returns_structured_summary_with_or_without_workspace(
        #[case] has_workspace: bool,
    ) {
        let (_root, ws, auth_dir, codex_bin) = setup_test_workspace();
        let ws_opt = if has_workspace {
            seed_run_v1(&ws, "harbor-dock", Visibility::Public, [140, 150, 160, 255]);
            seed_pack_for_reslice(&ws);
            Some(ws)
        } else {
            None
        };

        let state = AppState::with_workspace_and_backend(
            ws_opt,
            auth_dir,
            codex_bin.to_string_lossy().into_owned(),
            Some(Arc::new(StubServerBackend::new([140, 150, 160]))),
        );
        let app = build_router(state);

        let (status, parsed) = send_json(&app, Method::GET, "/api/workspace", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(parsed["hasWorkspace"], serde_json::json!(has_workspace));
        assert_eq!(parsed["authProvisioned"], serde_json::json!(true));
        assert!(parsed["styles"].as_array().unwrap().len() >= 5);
        assert!(parsed["atmospheres"].as_array().unwrap().len() >= 4);

        if has_workspace {
            assert_eq!(parsed["subjects"].as_array().unwrap().len(), 1);
            assert_eq!(parsed["referencePacks"].as_array().unwrap().len(), 1);
            assert_eq!(parsed["runs"].as_array().unwrap().len(), 1);
        } else {
            assert!(parsed["subjects"].as_array().unwrap().is_empty());
            assert!(parsed["referencePacks"].as_array().unwrap().is_empty());
            assert!(parsed["runs"].as_array().unwrap().is_empty());
        }
    }

    #[rstest]
    #[case::valid_run_png("runs/harbor-dock/v001.png", StatusCode::OK)]
    #[case::traversal_rejected("runs/../../etc/passwd.png", StatusCode::FORBIDDEN)]
    #[case::non_image_toml_rejected("imgforge.toml", StatusCode::FORBIDDEN)]
    #[case::auth_json_rejected("auth.json", StatusCode::FORBIDDEN)]
    #[case::dotfile_rejected(".git/config.png", StatusCode::FORBIDDEN)]
    #[tokio::test]
    async fn api_assets_and_presets_enforce_containment_and_serve_images(
        #[case] asset_path: &str,
        #[case] expected_status: StatusCode,
    ) {
        let (_root, ws, auth_dir, codex_bin) = setup_test_workspace();
        seed_run_v1(&ws, "harbor-dock", Visibility::Public, [120, 140, 160, 255]);
        fs::write(ws.join("auth.json"), r#"{"token":"secret"}"#).unwrap();

        let app = build_router(AppState::with_workspace_and_backend(
            Some(ws),
            auth_dir,
            codex_bin.to_string_lossy().into_owned(),
            None,
        ));

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/assets/{asset_path}"))
                    .header(header::HOST, "127.0.0.1:8080")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), expected_status);
        if expected_status == StatusCode::OK {
            assert_eq!(
                resp.headers()
                    .get(header::CONTENT_TYPE)
                    .and_then(|v| v.to_str().ok()),
                Some("image/png")
            );
        }

        // Built-in Style preset exemplar endpoint
        let preset_resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/presets/styles/franco-belgian-line/exemplars/0")
                    .header(header::HOST, "127.0.0.1:8080")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(preset_resp.status(), StatusCode::OK);
        let bytes = preset_resp.into_body().collect().await.unwrap().to_bytes();
        assert!(bytes.starts_with(&PNG_SIGNATURE));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn api_assets_rejects_symlinks_fail_closed() {
        let (_root, ws, auth_dir, codex_bin) = setup_test_workspace();
        seed_run_v1(&ws, "harbor-dock", Visibility::Public, [120, 140, 160, 255]);
        let outside = ws.parent().unwrap().join("outside.png");
        fs::write(&outside, make_png(16, 16, [255, 0, 0, 255])).unwrap();
        symlink(&outside, ws.join("runs/link.png")).unwrap();

        let app = build_router(AppState::with_workspace_and_backend(
            Some(ws),
            auth_dir,
            codex_bin.to_string_lossy().into_owned(),
            None,
        ));
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/assets/runs/link.png")
                    .header(header::HOST, "127.0.0.1:8080")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn style_atmosphere_and_subject_palette_lock_crud_enforce_canonical_immutability() {
        let (_root, ws, auth_dir, codex_bin) = setup_test_workspace();
        let app = build_router(AppState::with_workspace_and_backend(
            Some(ws.clone()),
            auth_dir,
            codex_bin.to_string_lossy().into_owned(),
            None,
        ));

        // 1. Clone built-in style to project library
        let (st_create, created_style) = send_json(
            &app,
            Method::POST,
            "/api/styles",
            Some(serde_json::json!({
                "slug": "harbor-clear-line",
                "displayName": "Harbor Clear Line",
                "cloneFromSlug": "franco-belgian-line"
            })),
        )
        .await;
        assert_eq!(st_create, StatusCode::CREATED);
        assert_eq!(created_style["lifecycle"], "draft");
        assert_eq!(created_style["source"], "project");

        // 2. Edit medium & upload 2 PNG exemplars
        let (st_put, updated_style) = send_json(
            &app,
            Method::PUT,
            "/api/styles/harbor-clear-line",
            Some(serde_json::json!({
                "medium": "pen-and-ink with sunlit coastal gouache"
            })),
        )
        .await;
        assert_eq!(st_put, StatusCode::OK);
        assert_eq!(
            updated_style["medium"],
            "pen-and-ink with sunlit coastal gouache"
        );

        let ex_b64 = BASE64_STANDARD.encode(make_png(32, 32, [60, 100, 140, 255]));
        let (st_ex, ex_style) = send_json(
            &app,
            Method::POST,
            "/api/styles/harbor-clear-line/exemplars",
            Some(serde_json::json!({
                "exemplarsBase64": [ex_b64, ex_b64],
                "replace": true
            })),
        )
        .await;
        assert_eq!(st_ex, StatusCode::OK);
        assert_eq!(ex_style["exemplarCount"], 2);

        // 3. Promote draft -> candidate -> validated -> canonical, then verify immutability
        for expected in ["candidate", "validated", "canonical"] {
            let (st_p, promo) = send_json(
                &app,
                Method::POST,
                "/api/styles/harbor-clear-line/promote",
                None,
            )
            .await;
            assert_eq!(st_p, StatusCode::OK);
            assert_eq!(promo["style"]["lifecycle"], expected);
        }

        let (st_mut_canon, mut_err) = send_json(
            &app,
            Method::PUT,
            "/api/styles/harbor-clear-line",
            Some(serde_json::json!({ "medium": "illegal edit" })),
        )
        .await;
        assert_eq!(st_mut_canon, StatusCode::CONFLICT);
        assert_eq!(mut_err["error"], "canonical_immutable");

        // 4. Create & edit Atmosphere
        let (st_atmo, atmo) = send_json(
            &app,
            Method::POST,
            "/api/atmospheres",
            Some(serde_json::json!({
                "slug": "harbor-noon",
                "displayName": "Harbor Noon",
                "keyLightDirection": "high overhead sun",
                "secondaryLightDirection": "sea reflection bounce",
                "colorTemperatureK": 5600,
                "maxAmbientTintBlend": 0.15,
                "minMeanLStar": 38.0,
                "minP10LStar": 14.0,
                "weatherAndParticles": "crisp sea breeze"
            })),
        )
        .await;
        assert_eq!(st_atmo, StatusCode::CREATED);
        assert_eq!(atmo["colorTemperatureKelvin"], 5600);

        let (st_atmo_put, atmo_upd) = send_json(
            &app,
            Method::PUT,
            "/api/atmospheres/harbor-noon",
            Some(serde_json::json!({ "colorTemperatureK": 5900 })),
        )
        .await;
        assert_eq!(st_atmo_put, StatusCode::OK);
        assert_eq!(atmo_upd["colorTemperatureKelvin"], 5900);

        // 5. Update Subject PaletteLock via eyedropper endpoint
        let (st_pal, subj) = send_json(
            &app,
            Method::POST,
            "/api/subjects/courier-ren/palette-lock",
            Some(serde_json::json!({
                "name": "lantern_brass",
                "colorHex": "#d4a843",
                "surfaceSlot": "lantern"
            })),
        )
        .await;
        assert_eq!(st_pal, StatusCode::OK);
        let swatches = subj["paletteLock"].as_array().unwrap();
        assert_eq!(swatches.len(), 2);
        assert_eq!(swatches[1]["hex"], "#D4A843");
    }

    #[rstest]
    #[case::valid_blockout(true, true)]
    #[case::invalid_blockout_bad_schema(false, false)]
    #[tokio::test]
    async fn api_guide_validate_and_render_return_png_base64_and_errors(
        #[case] valid_spec: bool,
        #[case] expect_ok: bool,
    ) {
        let dir = TempDir::new().unwrap();
        let app = build_router(AppState::new(dir.path().to_path_buf()));

        let spec = serde_json::json!({
            "schemaVersion": if valid_spec { "1.0" } else { "9.9" },
            "id": "guide-test-01",
            "title": "Test Blockout",
            "variant": "Blockout",
            "targetImagePath": null,
            "canvasWidth": 128,
            "canvasHeight": 128,
            "depthBands": [{
                "id": "band-1",
                "label": "BG",
                "plane": "background",
                "yStartRatio": 0.0,
                "yEndRatio": 1.0,
                "fillHex": "#2C3344"
            }],
            "mannequins": [],
            "actionVectors": [],
            "overlayRegions": []
        });

        let (st_val, val_body) = send_json(
            &app,
            Method::POST,
            "/api/guide/validate",
            Some(serde_json::json!({ "guideSpec": spec })),
        )
        .await;
        assert_eq!(st_val, StatusCode::OK);
        assert_eq!(val_body["valid"], serde_json::json!(expect_ok));

        let (st_ren, ren_body) = send_json(
            &app,
            Method::POST,
            "/api/guide/render",
            Some(serde_json::json!({ "guideSpec": spec })),
        )
        .await;
        if expect_ok {
            assert_eq!(st_ren, StatusCode::OK);
            assert_eq!(ren_body["width"], 128);
            let png_bytes = BASE64_STANDARD
                .decode(ren_body["pngBase64"].as_str().unwrap())
                .unwrap();
            assert!(png_bytes.starts_with(&PNG_SIGNATURE));
        } else {
            assert_eq!(st_ren, StatusCode::UNPROCESSABLE_ENTITY);
        }
    }

    #[tokio::test]
    async fn api_pack_reslice_validate_promote_and_run_patch_inspect_export_ws_flow() {
        let (_root, ws, auth_dir, codex_bin) = setup_test_workspace();
        seed_run_v1(&ws, "harbor-dock", Visibility::Public, [145, 150, 155, 255]);
        seed_pack_for_reslice(&ws);

        let state = AppState::with_workspace_and_backend(
            Some(ws.clone()),
            auth_dir,
            codex_bin.to_string_lossy().into_owned(),
            Some(Arc::new(StubServerBackend::new([145, 150, 155]))),
        );
        let app = build_router(state.clone());

        // Start a real TCP server on 127.0.0.1:0 to verify live WS WorkspaceEvent streaming
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let bound_addr = listener.local_addr().unwrap();
        let server_task = tokio::spawn(async move {
            let _ = axum::serve(listener, build_router(state)).await;
        });

        let mut ws_req = format!("ws://{bound_addr}/api/ws")
            .into_client_request()
            .unwrap();
        ws_req.headers_mut().insert(
            header::ORIGIN,
            HeaderValue::from_str(&format!("http://{bound_addr}")).unwrap(),
        );
        let (mut ws_stream, _) = tokio_tungstenite::connect_async(ws_req).await.unwrap();

        // 1. Reslice ReferencePack with manual RectOverride
        let (st_reslice, resliced) = send_json(
            &app,
            Method::POST,
            "/api/packs/courier-ren/franco-belgian-line/reslice",
            Some(serde_json::json!({
                "sheetRole": "turnaround",
                "mode": "replace_all",
                "overrides": [{
                    "id": "front_override",
                    "rect": { "x": 110, "y": 110, "width": 220, "height": 320 },
                    "label": "Front"
                }]
            })),
        )
        .await;
        assert_eq!(st_reslice, StatusCode::OK);
        assert_eq!(resliced["slices"][0]["id"], "front_override");

        // 2. Validate & Promote ReferencePack
        let (st_vp, val_pack) = send_json(
            &app,
            Method::POST,
            "/api/packs/courier-ren/franco-belgian-line/validate",
            Some(serde_json::json!({ "runCritic": false })),
        )
        .await;
        assert_eq!(st_vp, StatusCode::OK);
        assert_eq!(val_pack["referencePack"]["lifecycle"], "validated");

        let (st_pp, promo_pack) = send_json(
            &app,
            Method::POST,
            "/api/packs/courier-ren/franco-belgian-line/promote",
            None,
        )
        .await;
        assert_eq!(st_pp, StatusCode::OK);
        assert_eq!(promo_pack["referencePack"]["lifecycle"], "canonical");

        // 3. Execute PatchEdit via ROI (and verify before/after URLs + ColorQaReport)
        let (st_patch, patch_res) = send_json(
            &app,
            Method::POST,
            "/api/edit/patch",
            Some(serde_json::json!({
                "slug": "harbor-dock",
                "runSlug": "harbor-dock",
                "runVersion": 1,
                "roi": { "x": 40, "y": 40, "width": 32, "height": 32 },
                "prepMode": "cue_outline",
                "collarWidthPx": 24,
                "featherPx": 12,
                "prompt": "refine brass lantern gleam",
                "visibility": "public"
            })),
        )
        .await;
        assert_eq!(st_patch, StatusCode::OK);
        assert_eq!(patch_res["run"]["version"], 2);
        assert_eq!(
            patch_res["beforeImageUrl"],
            "/api/assets/runs/harbor-dock/v001.png"
        );
        assert_eq!(
            patch_res["afterImageUrl"],
            "/api/assets/runs/harbor-dock/v002.png"
        );
        assert_eq!(patch_res["colorQa"]["passed"], true);

        // 4. Inspect Run v002 against v001 before-image + Atmosphere + Subject
        let (st_insp, insp) = send_json(
            &app,
            Method::POST,
            "/api/runs/harbor-dock/2/inspect",
            Some(serde_json::json!({
                "atmosphereSlug": "golden-dawn",
                "subjectSlugs": ["courier-ren"],
                "beforeRunVersion": 1,
                "maxSamples": 128
            })),
        )
        .await;
        assert_eq!(st_insp, StatusCode::OK);
        assert_eq!(insp["telemetry"]["lStarBins"].as_array().unwrap().len(), 20);
        assert_eq!(
            insp["telemetry"]["vectorscopePoints"]
                .as_array()
                .unwrap()
                .len(),
            128
        );
        assert!(insp["heatmaps"]["drift"].as_str().is_some());
        assert!(insp["heatmaps"]["bleed"].as_str().is_some());
        assert!(insp["heatmaps"]["sparkle"].as_str().is_some());
        assert!(insp["heatmaps"]["outOfRoi"].as_str().is_some());

        // 5. Export gated until validated/canonical -> validate -> promote -> export
        let (st_exp_early, exp_early_err) = send_json(
            &app,
            Method::POST,
            "/api/export",
            Some(serde_json::json!({
                "runSlug": "harbor-dock",
                "runVersion": 2,
                "profile": "web_webp"
            })),
        )
        .await;
        assert_eq!(st_exp_early, StatusCode::CONFLICT);
        assert_eq!(
            exp_early_err["error"],
            "export_requires_validated_or_canonical"
        );

        let (st_vr, val_run) = send_json(
            &app,
            Method::POST,
            "/api/runs/harbor-dock/2/validate",
            Some(serde_json::json!({
                "atmosphereSlug": "golden-dawn",
                "runCritic": cfg!(unix)
            })),
        )
        .await;
        assert_eq!(st_vr, StatusCode::OK);
        assert_eq!(val_run["run"]["lifecycle"], "validated");

        let (st_pr, promo_run) =
            send_json(&app, Method::POST, "/api/runs/harbor-dock/2/promote", None).await;
        assert_eq!(st_pr, StatusCode::OK);
        assert_eq!(promo_run["run"]["lifecycle"], "canonical");

        let (st_exp, exp_res) = send_json(
            &app,
            Method::POST,
            "/api/export",
            Some(serde_json::json!({
                "runSlug": "harbor-dock",
                "runVersion": 2,
                "profile": "web_webp",
                "visibility": "public"
            })),
        )
        .await;
        assert_eq!(st_exp, StatusCode::OK);
        assert_eq!(exp_res["artifacts"].as_array().unwrap().len(), 1);
        assert_eq!(
            exp_res["artifacts"][0]["assetUrl"],
            "/api/assets/exports/harbor-dock.webp"
        );

        // Verify live WS client received the broadcast WorkspaceEvents
        let mut received_events = Vec::new();
        for _ in 0..7 {
            let msg = tokio::time::timeout(std::time::Duration::from_secs(2), ws_stream.next())
                .await
                .expect("ws event timeout")
                .expect("ws stream open")
                .expect("ws frame");
            if let WsMessage::Text(txt) = msg {
                let ev: WorkspaceEvent = serde_json::from_str(&txt).unwrap();
                received_events.push(ev);
            }
        }
        assert!(
            received_events
                .iter()
                .any(|e| e.resource_kind == "run" && e.event_type == "promoted")
        );
        assert!(
            received_events
                .iter()
                .any(|e| e.resource_kind == "export" && e.event_type == "exported")
        );

        let _ = ws_stream.close(None).await;
        server_task.abort();
    }

    #[tokio::test]
    async fn api_patch_edit_with_mask_png_base64_and_overlay_guide_and_export_visibility_gate() {
        let (_root, ws, auth_dir, codex_bin) = setup_test_workspace();
        seed_run_v1(
            &ws,
            "secret-scene",
            Visibility::Internal,
            [145, 150, 155, 255],
        );

        let app = build_router(AppState::with_workspace_and_backend(
            Some(ws.clone()),
            auth_dir,
            codex_bin.to_string_lossy().into_owned(),
            Some(Arc::new(StubServerBackend::new([145, 150, 155]))),
        ));

        // 1. PatchEdit using maskPngBase64
        let mut mask_img: RgbaImage = ImageBuffer::from_pixel(128, 128, Rgba([0, 0, 0, 0]));
        for y in 48..80 {
            for x in 48..80 {
                mask_img.put_pixel(x, y, Rgba([255, 255, 255, 255]));
            }
        }
        let mut mask_buf = Vec::new();
        DynamicImage::ImageRgba8(mask_img)
            .write_to(&mut Cursor::new(&mut mask_buf), ImageFormat::Png)
            .unwrap();
        let mask_b64 = BASE64_STANDARD.encode(&mask_buf);

        let (st_patch, patch_res) = send_json(
            &app,
            Method::POST,
            "/api/edit/patch",
            Some(serde_json::json!({
                "slug": "secret-scene",
                "targetImagePath": "runs/secret-scene/v001.png",
                "maskPngBase64": mask_b64,
                "prepMode": "blank_inside_mask",
                "prompt": "refine emblem",
                "visibility": "internal"
            })),
        )
        .await;
        assert_eq!(st_patch, StatusCode::OK);
        assert_eq!(patch_res["run"]["version"], 2);
        assert_eq!(patch_res["run"]["visibility"], "internal");

        // 2. OverlayGuide render onto workspace-contained PNG
        let (st_ov, ov_res) = send_json(
            &app,
            Method::POST,
            "/api/guide/render",
            Some(serde_json::json!({
                "schemaVersion": "1.0",
                "id": "ov-01",
                "title": "Overlay",
                "variant": "OverlayGuide",
                "targetImagePath": "runs/secret-scene/v001.png",
                "canvasWidth": 128,
                "canvasHeight": 128,
                "depthBands": [],
                "mannequins": [],
                "actionVectors": [],
                "overlayRegions": [{
                    "id": "r1",
                    "label": "ROI",
                    "kind": "rect",
                    "points": [{ "xRatio": 0.2, "yRatio": 0.2 }, { "xRatio": 0.8, "yRatio": 0.8 }],
                    "strokeHex": "#68D391",
                    "haloHex": "#1A1D24",
                    "fillHex": "#68D391",
                    "opacity": 0.2
                }]
            })),
        )
        .await;
        assert_eq!(st_ov, StatusCode::OK);
        assert_eq!(ov_res["variant"], "OverlayGuide");

        // 3. Validate internal Run -> export to public must fail closed with 403 VisibilityViolation
        let (st_val, _) = send_json(
            &app,
            Method::POST,
            "/api/runs/secret-scene/2/validate",
            Some(serde_json::json!({ "runCritic": false })),
        )
        .await;
        assert_eq!(st_val, StatusCode::OK);

        let (st_leak, leak_err) = send_json(
            &app,
            Method::POST,
            "/api/export",
            Some(serde_json::json!({
                "runSlug": "secret-scene",
                "runVersion": 2,
                "profile": "web_webp",
                "visibility": "public"
            })),
        )
        .await;
        assert_eq!(st_leak, StatusCode::FORBIDDEN);
        assert_eq!(leak_err["error"], "visibility_violation");
    }

    #[test]
    fn api_error_maps_5xx_failures_and_preserves_generated_image_diagnostic_detail() {
        let missing_png = ApiError::from(CoreError::GeneratedImageNotFound {
            search_root: PathBuf::from("/tmp/imgforge-codex-test/generated_images"),
            thread_id: Some("thr_diag".to_string()),
            detail: Some("agent_message: Need style details before calling image_gen".to_string()),
        });
        assert_eq!(missing_png.status, StatusCode::BAD_GATEWAY);
        assert_eq!(missing_png.code, "backend_failed");
        assert!(
            missing_png
                .message
                .contains("agent_message: Need style details before calling image_gen")
        );

        let resp = missing_png.into_response();
        assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);

        let io_err = ApiError::from(CoreError::Io {
            context: "writing temp patch".to_string(),
            source: std::io::Error::other("disk full"),
        });
        assert_eq!(io_err.status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(io_err.code, "io_error");
        assert_eq!(
            io_err.into_response().status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );

        let auth_err = ApiError::from(CoreError::AuthMissing {
            auth_file: PathBuf::from("/tmp/missing/auth.json"),
        });
        assert_eq!(auth_err.status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(auth_err.code, "auth_unavailable");
        assert_eq!(
            auth_err.into_response().status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
    }
}
