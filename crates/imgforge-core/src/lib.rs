//! === IMGFORGE CORE LIBRARY ===
//!
//! PURPOSE:
//! Exposes the domain-agnostic core engine for `imgforge`: workspace config
//! discovery, tool-local Codex OAuth credential management, `SceneSpec` &
//! `PromptCompiler`, `ImageBackend` & `CodexNativeBackend`, `SceneComposer`,
//! and `VisionCritic`.

pub mod atmosphere;
pub mod auth;
pub mod backend;
pub mod composer;
pub mod config;
pub mod critic;
pub mod error;
pub mod lifecycle;
pub mod prompt_compiler;
pub mod scene_spec;
pub mod style;
pub mod subject;

pub use atmosphere::{Atmosphere, ExposureFloor};
pub use auth::{
    AuthManager, AuthStatusReport, DEFAULT_CODEX_BIN, TempCodexHome, resolve_codex_auth_dir,
    resolve_codex_auth_dir_from_env,
};
pub use backend::{
    CodexNativeBackend, ImageBackend, ImageGenerationOutput, ImageGenerationRequest, PNG_SIGNATURE,
    build_generation_prompt, extract_thread_id_from_jsonl, format_aspect_ratio,
    locate_generated_png, validate_and_resample_png,
};
pub use composer::{
    ComposedScene, MAX_UNTRUSTED_PROSE_BYTES, SCENE_SPEC_OUTPUT_SCHEMA, SceneComposer,
};
pub use config::{
    BackendConfig, BackendKind, ResolvedWorkspace, ServerConfig, WorkspaceConfig,
    WorkspacePathsConfig, WorkspaceResolutionSource, parse_workspace_config, resolve_workspace,
    resolve_workspace_from_env,
};
pub use critic::{
    CRITIC_REPORT_OUTPUT_SCHEMA, CriticReport, TRAIT_CHECKLIST_END, TRAIT_CHECKLIST_START,
    TraitCategory, TraitCheckInput, TraitCheckResult, VisionCritic, VisionCriticRequest,
    build_critic_prompt, validate_critic_report_for_traits, validate_critic_report_json,
};
pub use error::CoreError;
pub use lifecycle::{Lifecycle, SeedInput, now_utc_iso, sha256_hex};
pub use prompt_compiler::{CompiledPrompt, PromptCompiler};
pub use scene_spec::{
    CameraSpec, DepthBandSpec, DepthPlane, ExclusiveSurfaceMap, ForegroundSlot, HandBinding,
    MAX_FOREGROUND_SLOTS, SceneSpec, SurfaceBinding, Visibility, check_visibility_compatibility,
};
pub use style::{MAX_STYLE_EXEMPLARS, MIN_STYLE_EXEMPLARS, Style};
pub use subject::{PaletteLock, PaletteSwatch, Subject, SubjectHandBindings, SubjectKind};
