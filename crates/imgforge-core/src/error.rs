//! === CORE ERROR TAXONOMY ===
//!
//! PURPOSE:
//! Defines `CoreError`, the canonical fail-closed error enumeration shared across
//! workspace discovery, tool-local auth, `PromptCompiler`, `CodexNativeBackend`,
//! `SceneComposer`, and `VisionCritic`.
//!
//! THREAT MODEL / DESIGN RATIONALE ("WHY"):
//! Silent fallbacks or generic string errors obscure trust-boundary failures such as
//! missing tool-local OAuth credentials, unknown YAML/TOML fields, active negations
//! in prompts, `Visibility` leaks, or aspect-ratio drift exceeding 5%. Every variant
//! carries structured, actionable context while never exposing credential contents.
//!
//! EXECUTION LIFECYCLE / DATA FLOW:
//! 1. Subsystems return `Result<T, CoreError>` on any invariant violation.
//! 2. The CLI and `axum` handlers map `CoreError` to non-zero exit codes or HTTP errors.

use std::path::PathBuf;
use thiserror::Error;

/// Canonical fail-closed error type for `imgforge-core`.
#[derive(Debug, Error)]
pub enum CoreError {
    /// No `imgforge.toml` workspace configuration could be resolved.
    #[error(
        "no imgforge workspace found (checked --workspace, IMGFORGE_WORKSPACE, and parent directories of {cwd}); create an imgforge.toml or pass --workspace <PATH>"
    )]
    WorkspaceNotFound {
        /// Working directory from which upward discovery started.
        cwd: PathBuf,
    },

    /// An explicitly configured workspace path (`--workspace` or `IMGFORGE_WORKSPACE`) is invalid.
    #[error("invalid workspace at {path}: {reason}")]
    WorkspaceInvalid {
        /// Workspace path that failed validation.
        path: PathBuf,
        /// Specific reason validation failed.
        reason: String,
    },

    /// Failed to parse or validate `imgforge.toml`.
    #[error("invalid imgforge.toml at {path}: {reason}")]
    ConfigInvalid {
        /// Path to the malformed `imgforge.toml`.
        path: PathBuf,
        /// Deserialization or schema validation error detail.
        reason: String,
    },

    /// Neither a valid absolute `XDG_DATA_HOME` nor an absolute `HOME` was available.
    #[error(
        "cannot determine tool-local auth directory: XDG_DATA_HOME and HOME are unset, empty, or not absolute paths"
    )]
    AuthHomeUnavailable,

    /// Tool-local `auth.json` is missing; fails closed with remediation instructions.
    #[error(
        "tool-local Codex OAuth credentials not found at {auth_file}; run 'imgforge auth login' (or 'imgforge auth login --device-auth') first"
    )]
    AuthMissing {
        /// Expected path of the tool-local `auth.json` file.
        auth_file: PathBuf,
    },

    /// Tool-local `auth.json` or its parent directory failed security/integrity checks.
    #[error("invalid tool-local auth state at {path}: {reason}")]
    AuthInvalid {
        /// Path that failed the auth integrity check.
        path: PathBuf,
        /// Reason the auth file or directory was rejected.
        reason: String,
    },

    /// `codex login` or `codex login status` exited non-zero or failed to provision `auth.json`.
    #[error("codex auth command failed: {reason}")]
    AuthCommandFailed {
        /// Failure summary without any credential payload.
        reason: String,
    },

    /// A `SceneSpec` failed YAML/JSON parsing or schema validation.
    #[error("invalid SceneSpec: {0}")]
    SceneSpecInvalid(String),

    /// `PromptCompiler` rejected a `SceneSpec` due to domain or safety invariant violations.
    #[error("PromptCompiler validation failed: {0}")]
    PromptCompilationFailed(String),

    /// `ImageBackend` received invalid request parameters before execution.
    #[error("invalid image generation request: {0}")]
    InvalidGenerationRequest(String),

    /// Subprocess execution (`codex exec`) failed or exited with a non-zero status.
    #[error("{stage} subprocess failed: {reason}")]
    SubprocessFailed {
        /// Pipeline stage name (`CodexNativeBackend`, `SceneComposer`, `VisionCritic`).
        stage: &'static str,
        /// Exit status and sanitized stderr summary.
        reason: String,
    },

    /// `CodexNativeBackend` could not locate a generated PNG under `$CODEX_HOME/generated_images`.
    #[error("no generated PNG found under {search_root} (thread_id={thread_id:?})")]
    GeneratedImageNotFound {
        /// Isolated `generated_images` directory searched.
        search_root: PathBuf,
        /// Optional `thread_id` parsed from JSONL output.
        thread_id: Option<String>,
    },

    /// Located output file did not match the 8-byte PNG signature or failed PNG decoding.
    #[error("invalid PNG image at {path}: {reason}")]
    InvalidPng {
        /// Path to the invalid image file.
        path: PathBuf,
        /// Validation or decoding error message.
        reason: String,
    },

    /// Generated image aspect ratio deviated by more than the 5% tolerance from requested `WxH`.
    #[error(
        "aspect ratio error {error_percent:.2}% exceeds 5.00% limit (requested {requested_width}x{requested_height}, got {actual_width}x{actual_height})"
    )]
    AspectRatioExceeded {
        /// Requested output width in pixels.
        requested_width: u32,
        /// Requested output height in pixels.
        requested_height: u32,
        /// Actual generated width in pixels.
        actual_width: u32,
        /// Actual generated height in pixels.
        actual_height: u32,
        /// Relative aspect-ratio error expressed as a percentage.
        error_percent: f64,
    },

    /// `SceneComposer` or `VisionCritic` returned output that failed structured validation.
    #[error("{stage} output validation failed: {reason}")]
    StructuredOutputInvalid {
        /// Stage producing the invalid output.
        stage: &'static str,
        /// Validation failure detail.
        reason: String,
    },

    /// Underlying filesystem or process I/O error.
    #[error("I/O error during {context}: {source}")]
    Io {
        /// Operation context describing what failed.
        context: String,
        /// Underlying `std::io::Error`.
        #[source]
        source: std::io::Error,
    },
}
