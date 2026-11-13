//! === RMCP MODEL CONTEXT PROTOCOL SERVER ===
//!
//! PURPOSE:
//! Exposes `imgforge-core` capabilities over the Model Context Protocol (`rmcp`)
//! via stdio transport (default) or loopback-only Streamable HTTP (`127.0.0.1:<port>`),
//! providing exactly 8 canonical tools (`compose`, `generate`, `pack`, `edit`,
//! `guide`, `export`, `validate`, `promote`) and 4 canonical resources
//! (`imgforge://manifest`, `imgforge://subjects`, `imgforge://styles-atmospheres`,
//! `imgforge://runs`).
//!
//! THREAT MODEL / DESIGN RATIONALE ("WHY"):
//! 1. Untrusted MCP tool arguments may supply `..` path traversal, absolute paths
//!    outside the workspace, or symlinks pointing at sensitive host files. Every
//!    filesystem path argument is therefore normalized and confined strictly inside
//!    the resolved workspace root with symlink-free directory checks before any
//!    read or atomic write.
//! 2. Optional HTTP transport (`--http`) refuses non-loopback bind addresses fail-closed
//!    and relies on `rmcp`'s loopback `Host` header validation to prevent DNS
//!    rebinding or LAN exposure.
//! 3. Tool handlers delegate directly to `imgforge-core` using `Arc<dyn ImageBackend>`
//!    so deterministic test backends exercise the real MCP protocol surface
//!    without spawning live `codex exec`.
//!
//! EXECUTION LIFECYCLE / DATA FLOW:
//! 1. `ImgforgeMcpServer` registers the 8 `#[tool]` endpoints via `ToolRouter`.
//! 2. `ServerHandler` advertises tools + resources capabilities and serves
//!    `list_resources` / `read_resource` for the 4 `imgforge://*` URIs.
//! 3. `serve_mcp_stdio` / `serve_mcp_http` runs the `rmcp` transport loop until EOF
//!    or Ctrl+C shutdown.

use std::collections::BTreeMap;
use std::fs;
use std::net::SocketAddr;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, bail};
use axum::Router;
use imgforge_core::guide_renderer::{
    ActionVector, DepthBand, GuideDepthPlane, MannequinFacing, SubjectMannequin, VectorStrokeStyle,
};
use imgforge_core::{
    AtlasPackRequest, AtlasSliceSelection, AuthManager, CodexNativeBackend, CoreError,
    DEFAULT_ATLAS_HEIGHT, DEFAULT_ATLAS_PADDING_PX, DEFAULT_ATLAS_WIDTH, DEFAULT_CDF_DAMP_STRENGTH,
    DEFAULT_COLLAR_WIDTH_PX, DEFAULT_TOKEN_RING_HEX, DEFAULT_VTT_MAP_GRID_PX,
    DEFAULT_VTT_TOKEN_SIZE_PX, DepthPlane, ExportProfile, ExportRequest, ExportSource,
    ExposureQaConfig, GenerateRequest, GenerateSceneInput, GenerationMode, GuideRenderer,
    GuideSpec, ImageBackend, LibraryRegistry, Lifecycle, ManifestLockStore, MatchGradeOptions,
    PaletteQaConfig, PatchEditConfig, PatchEditRequest, PatchPrepMode, PatchTargetMask, PixelRect,
    PromptCompiler, ReferencePackPipeline, ReferencePackRequest, ResolvedWorkspace, RunStore,
    SUPPORTED_GUIDE_SPEC_SCHEMA_VERSION, SceneComposer, SceneSpec, SlicerConfig, SparkleConfig,
    SpatialGuideVariant, SubjectRoiBinding, TraitCategory, TraitCheckInput, UntouchedDriftConfig,
    ValidateReferencePackRequest, ValidateRunRequest, Visibility, VisionCritic, VttMapOptions,
    VttTokenOptions, WholeImageEditRequest, check_visibility_compatibility, execute_export,
    execute_generate, execute_patch_edit, execute_whole_image_edit, inspect_workspace_status,
    pack_dynamic_atlas, promote_reference_pack, promote_run, resolve_workspace_from_env,
    slice_sheet_to_dir, validate_reference_pack, validate_run, write_atomic,
};
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    CallToolResult, ContentBlock, ErrorData as McpError, Implementation, ListResourcesResult,
    PaginatedRequestParams, ReadResourceRequestParams, ReadResourceResponse, ReadResourceResult,
    Resource, ResourceContents, ServerCapabilities, ServerConfig,
};
use rmcp::service::{RequestContext, RoleServer};
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{ServerHandler, ServiceExt, tool, tool_handler, tool_router};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::net::TcpListener;

/// Canonical MCP resource URIs exposed by `ImgforgeMcpServer`.
pub const RESOURCE_URI_MANIFEST: &str = "imgforge://manifest";
/// Canonical MCP resource URI for resolved `Subject` cards.
pub const RESOURCE_URI_SUBJECTS: &str = "imgforge://subjects";
/// Canonical MCP resource URI for resolved `Style` and `Atmosphere` catalogs.
pub const RESOURCE_URI_STYLES_ATMOSPHERES: &str = "imgforge://styles-atmospheres";
/// Canonical MCP resource URI for recorded `Run` index and QA reports.
pub const RESOURCE_URI_RUNS: &str = "imgforge://runs";

// === 1. MCP Tool Parameter Schemas ===

/// Parameters for the `compose` MCP tool.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ComposeToolParams {
    /// Unstructured scene description prose converted via read-only `SceneComposer`.
    pub prose: String,
    /// When true, also emit a deterministic `Blockout` `GuideSpec` recommendation.
    #[serde(default)]
    pub recommend_blockout: Option<bool>,
    /// Optional workspace-relative output path to persist the validated `SceneSpec`.
    #[serde(default)]
    pub out_path: Option<String>,
    /// Output serialization format when writing `out_path` (`"yaml"` or `"json"`).
    #[serde(default)]
    pub format: Option<String>,
}

/// Slice selector for dynamic `Atlas` packing in `generate` / `pack`.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct McpAtlasSelection {
    /// Target `Subject` slug.
    pub subject_slug: String,
    /// Target `Style` slug.
    pub style_slug: String,
    /// Selected `slice_id` entries from `slices.json`.
    pub slice_ids: Vec<String>,
}

/// Pixel rectangle parameter used in `generate` ROIs and `edit` `PatchEdit`.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, JsonSchema)]
pub struct McpPixelRect {
    /// Left X coordinate in pixels.
    pub x: u32,
    /// Top Y coordinate in pixels.
    pub y: u32,
    /// Width in pixels (`> 0`).
    pub width: u32,
    /// Height in pixels (`> 0`).
    pub height: u32,
}

impl From<McpPixelRect> for PixelRect {
    fn from(r: McpPixelRect) -> Self {
        Self {
            x: r.x,
            y: r.y,
            width: r.width,
            height: r.height,
        }
    }
}

/// Subject ROI binding for `PaletteLock` Color QA inside `generate`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct McpSubjectRoi {
    /// Resolved `Subject` slug.
    pub subject_slug: String,
    /// Optional pixel ROIs inside the target image (empty evaluates full canvas).
    #[serde(default)]
    pub rois: Vec<McpPixelRect>,
}

/// Parameters for the `generate` MCP tool.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct GenerateToolParams {
    /// Workspace path to a `SceneSpec` YAML/JSON file.
    #[serde(default)]
    pub scene_path: Option<String>,
    /// Inline `SceneSpec` JSON object.
    #[serde(default)]
    pub scene_spec: Option<serde_json::Value>,
    /// Unstructured scene prose (invokes `SceneComposer` ONLY when no `SceneSpec` is provided).
    #[serde(default)]
    pub compose_prose: Option<String>,
    /// Optional target `Run` stream slug (defaults to `SceneSpec.id`).
    #[serde(default)]
    pub slug: Option<String>,
    /// When true, run `0`-reference `explore` mode without requiring a `ReferencePack`.
    #[serde(default)]
    pub explore: Option<bool>,
    /// Optional width override applied to cloned `SceneSpec` before `PromptCompiler`.
    #[serde(default)]
    pub width: Option<u32>,
    /// Optional height override applied to cloned `SceneSpec` before `PromptCompiler`.
    #[serde(default)]
    pub height: Option<u32>,
    /// Explicit `(subject_slug, style_slug, slice_ids)` selections for dynamic `Atlas` packing.
    #[serde(default)]
    pub atlas_selections: Option<Vec<McpAtlasSelection>>,
    /// Explicit workspace reference PNG paths forwarded to `ImageBackend`.
    #[serde(default)]
    pub reference_images: Option<Vec<String>>,
    /// Optional `SpatialGuide` PNG path attached with explicit role labeling.
    #[serde(default)]
    pub spatial_guide_path: Option<String>,
    /// Subject ROI bindings for `PaletteLock` Color QA.
    #[serde(default)]
    pub subject_rois: Option<Vec<McpSubjectRoi>>,
    /// Optional workspace path to copy the generated PNG in addition to `runs/<slug>/vNNN.png`.
    #[serde(default)]
    pub out_path: Option<String>,
    /// When true, preserve the `Draft` `Run` even if fail-closed Color QA rejects it.
    #[serde(default)]
    pub allow_qa_fail: Option<bool>,
    /// When true, record initial lifecycle as `draft` instead of advancing to `candidate` on QA pass.
    #[serde(default)]
    pub draft: Option<bool>,
}

/// Parameters for the `pack` MCP tool (`generate`, `slice`, or `atlas`).
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct PackToolParams {
    /// Operation: `"generate"`, `"slice"`, or `"atlas"`.
    pub action: String,
    /// `Subject` slug (required for `"generate"`).
    #[serde(default)]
    pub subject_slug: Option<String>,
    /// `Style` slug (required for `"generate"`).
    #[serde(default)]
    pub style_slug: Option<String>,
    /// Optional bootstrap `Seed` PNG path for `"generate"`.
    #[serde(default)]
    pub seed_path: Option<String>,
    /// Optional `Seed` label for `"generate"`.
    #[serde(default)]
    pub seed_label: Option<String>,
    /// Sheet PNG path (required for `"slice"`).
    #[serde(default)]
    pub sheet_path: Option<String>,
    /// Output directory for `"slice"`.
    #[serde(default)]
    pub out_dir: Option<String>,
    /// Bottom calibration strip height in pixels excluded during `"slice"` (default `72`).
    #[serde(default)]
    pub strip_height: Option<u32>,
    /// Slice selections for `"atlas"`.
    #[serde(default)]
    pub selections: Option<Vec<McpAtlasSelection>>,
    /// Output PNG path for `"atlas"`.
    #[serde(default)]
    pub out_path: Option<String>,
    /// Target `Visibility` (`"public"`, `"restricted"`, `"internal"`) for `"atlas"`.
    #[serde(default)]
    pub visibility: Option<String>,
    /// Target canvas width in pixels for `"atlas"`.
    #[serde(default)]
    pub width: Option<u32>,
    /// Target canvas height in pixels for `"atlas"`.
    #[serde(default)]
    pub height: Option<u32>,
}

/// Parameters for the `edit` MCP tool (`WholeImageEdit` or `PatchEdit`).
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct EditToolParams {
    /// Edit mode: `"whole"` (`WholeImageEdit`) or `"patch"` (`PatchEdit`).
    pub mode: String,
    /// Target PNG path inside the workspace.
    pub target_image_path: String,
    /// Target `Run` stream slug.
    pub slug: String,
    /// Positive-only compiled edit prompt.
    pub prompt: String,
    /// Target `Visibility` (`"public"`, `"restricted"`, `"internal"`).
    #[serde(default)]
    pub visibility: Option<String>,
    /// Optional additional reference PNG paths inside the workspace.
    #[serde(default)]
    pub reference_images: Option<Vec<String>>,
    /// Optional `Atmosphere` slug enforcing `exposure_floor` (`"whole"` mode).
    #[serde(default)]
    pub atmosphere_slug: Option<String>,
    /// Optional `Subject` slugs enforcing `PaletteLock` (`"whole"` mode).
    #[serde(default)]
    pub subject_slugs: Option<Vec<String>>,
    /// When true, preserve failed `Draft` `Run` instead of failing closed (`"whole"` mode).
    #[serde(default)]
    pub allow_qa_fail: Option<bool>,
    /// Rectangular ROI (`"patch"` mode, mutually exclusive with `mask_path`).
    #[serde(default)]
    pub roi: Option<McpPixelRect>,
    /// Mask PNG path (`"patch"` mode, mutually exclusive with `roi`).
    #[serde(default)]
    pub mask_path: Option<String>,
    /// Patch target prep mode: `"crop_only"`, `"blank_inside_mask"`, or `"cue_outline"`.
    #[serde(default)]
    pub prep_mode: Option<String>,
    /// Collar ring width in pixels (`16..=32`, default `24`).
    #[serde(default)]
    pub collar_width_px: Option<u32>,
    /// Cosine distance-transform feather radius in pixels (default `12`).
    #[serde(default)]
    pub feather_px: Option<u32>,
}

/// Parameters for the `guide` MCP tool (`validate` or `render` `GuideSpec` -> `SpatialGuide`).
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct GuideToolParams {
    /// Action: `"validate"` or `"render"`.
    pub action: String,
    /// Workspace path to a `GuideSpec` JSON file.
    #[serde(default)]
    pub spec_path: Option<String>,
    /// Inline `GuideSpec` JSON object.
    #[serde(default)]
    pub guide_spec: Option<serde_json::Value>,
    /// Output PNG path inside workspace (required for `"render"`).
    #[serde(default)]
    pub out_path: Option<String>,
}

/// Parameters for the `export` MCP tool.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ExportToolParams {
    /// Source `Run` slug (paired with `run_version`).
    #[serde(default)]
    pub run_slug: Option<String>,
    /// Source `Run` version (`1`, `2`, ...).
    #[serde(default)]
    pub run_version: Option<u32>,
    /// Direct source PNG path inside workspace (alternative to `run_slug`/`run_version`).
    #[serde(default)]
    pub input_path: Option<String>,
    /// Explicit source `Visibility` when exporting via `input_path` (auto-raised from `RunStore`).
    #[serde(default)]
    pub source_visibility: Option<String>,
    /// Output `Export` profile: `"vtt_token"`, `"vtt_map"`, `"web_webp"`, or `"grid_2x2"`.
    pub profile: String,
    /// Allowed destination `Visibility` (`"public"`, `"restricted"`, `"internal"`).
    #[serde(default)]
    pub visibility: Option<String>,
    /// Output directory inside workspace.
    pub out_dir: String,
    /// Optional output basename slug.
    #[serde(default)]
    pub output_slug: Option<String>,
    /// Square size in pixels for `vtt_token`.
    #[serde(default)]
    pub token_size: Option<u32>,
    /// Outer ring `#RRGGBB` hex for `vtt_token`.
    #[serde(default)]
    pub ring_color_hex: Option<String>,
    /// Optional flat-backdrop `#RRGGBB` hex for `vtt_token` JPEG copy.
    #[serde(default)]
    pub jpeg_backdrop_hex: Option<String>,
    /// Grid column count for `vtt_map`.
    #[serde(default)]
    pub cols: Option<u32>,
    /// Grid row count for `vtt_map`.
    #[serde(default)]
    pub rows: Option<u32>,
    /// Pixels per grid cell for `vtt_map`.
    #[serde(default)]
    pub grid_px: Option<u32>,
    /// Optional Lanczos3 upscale multiplier for `vtt_map`.
    #[serde(default)]
    pub scale_factor: Option<f64>,
    /// JPEG encoding quality (`1..=100`).
    #[serde(default)]
    pub jpeg_quality: Option<u8>,
    /// Explicit `--match-grade` opt-in (ONLY supported on `export`).
    #[serde(default)]
    pub match_grade: Option<bool>,
    /// Reference PNG path for damped `L*` CDF matching.
    #[serde(default)]
    pub grade_ref_path: Option<String>,
    /// Damped `L*` CDF strength (`0.25..=0.45`).
    #[serde(default)]
    pub grade_strength: Option<f64>,
    /// Adobe `.cube` 3D LUT path inside workspace.
    #[serde(default)]
    pub cube_lut_path: Option<String>,
}

/// Binary trait check input for `VisionCritic` inside the `validate` MCP tool.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct McpTraitCheck {
    /// Stable trait identifier.
    pub trait_id: String,
    /// Category: `"identity"`, `"hand_props"`, `"weapon_geometry"`, `"style_adherence"`, or `"zero_leaked_text"`.
    pub category: String,
    /// Binary verification criterion.
    pub criterion: String,
}

/// Parameters for the `validate` MCP tool (`scene_spec`, `run`, or `reference_pack`).
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ValidateToolParams {
    /// Target kind: `"scene_spec"`, `"run"`, or `"reference_pack"`.
    pub target: String,
    /// Workspace path to `SceneSpec` YAML/JSON (`"scene_spec"` target).
    #[serde(default)]
    pub scene_path: Option<String>,
    /// Inline `SceneSpec` JSON object (`"scene_spec"` target).
    #[serde(default)]
    pub scene_spec: Option<serde_json::Value>,
    /// `Run` stream slug (`"run"` target).
    #[serde(default)]
    pub slug: Option<String>,
    /// `Run` version number (`"run"` target).
    #[serde(default)]
    pub version: Option<u32>,
    /// Optional `Atmosphere` slug for `exposure_floor` check (`"run"` target).
    #[serde(default)]
    pub atmosphere_slug: Option<String>,
    /// Optional `Subject` slugs for `PaletteLock` check (`"run"` target).
    #[serde(default)]
    pub subject_slugs: Option<Vec<String>>,
    /// Reference anchor image paths forwarded to `VisionCritic` (`"run"` target).
    #[serde(default)]
    pub reference_images: Option<Vec<String>>,
    /// `Subject` slug (`"reference_pack"` target).
    #[serde(default)]
    pub subject_slug: Option<String>,
    /// `Style` slug (`"reference_pack"` target).
    #[serde(default)]
    pub style_slug: Option<String>,
    /// Binary `VisionCritic` trait checklist items.
    #[serde(default)]
    pub traits: Option<Vec<McpTraitCheck>>,
    /// Run `VisionCritic` with default checklist even when `traits` is empty.
    #[serde(default)]
    pub run_critic: Option<bool>,
}

/// Parameters for the `promote` MCP tool (`run`, `reference_pack`, or `status`).
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct PromoteToolParams {
    /// Action: `"run"`, `"reference_pack"`, or `"status"`.
    pub action: String,
    /// `Run` stream slug (required for `"run"`, optional filter for `"status"`).
    #[serde(default)]
    pub slug: Option<String>,
    /// `Run` version number (required for `"run"`).
    #[serde(default)]
    pub version: Option<u32>,
    /// `Subject` slug (required for `"reference_pack"`).
    #[serde(default)]
    pub subject_slug: Option<String>,
    /// `Style` slug (required for `"reference_pack"`).
    #[serde(default)]
    pub style_slug: Option<String>,
}

// === 2. MCP Server Implementation ===

/// `rmcp` MCP server exposing the 8 `imgforge` tools and 4 `imgforge://*` resources.
#[derive(Clone)]
pub struct ImgforgeMcpServer {
    workspace_override: Option<PathBuf>,
    auth_dir: PathBuf,
    codex_bin: String,
    backend: Arc<dyn ImageBackend>,
    tool_router: ToolRouter<Self>,
}

impl ImgforgeMcpServer {
    /// Construct an `ImgforgeMcpServer` backed by `CodexNativeBackend`.
    #[must_use]
    pub fn new(workspace_override: Option<PathBuf>, auth_dir: PathBuf, codex_bin: String) -> Self {
        let backend: Arc<dyn ImageBackend> = Arc::new(CodexNativeBackend::with_codex_bin(
            auth_dir.clone(),
            codex_bin.clone(),
        ));
        Self::with_backend(workspace_override, auth_dir, codex_bin, backend)
    }

    /// Construct an `ImgforgeMcpServer` with an explicit `ImageBackend` testing seam.
    #[must_use]
    pub fn with_backend(
        workspace_override: Option<PathBuf>,
        auth_dir: PathBuf,
        codex_bin: String,
        backend: Arc<dyn ImageBackend>,
    ) -> Self {
        Self {
            workspace_override,
            auth_dir,
            codex_bin,
            backend,
            tool_router: Self::tool_router(),
        }
    }

    fn resolve_workspace(&self) -> Result<ResolvedWorkspace, McpError> {
        resolve_workspace_from_env(self.workspace_override.as_deref())
            .map_err(core_err_to_mcp_invalid_params)
    }
}

#[tool_router(router = tool_router)]
impl ImgforgeMcpServer {
    /// Translate unstructured prose into a validated `SceneSpec` via read-only `SceneComposer`
    /// and optionally emit a `Blockout` `GuideSpec` recommendation.
    #[tool(
        name = "compose",
        description = "Translate unstructured prose into a validated SceneSpec via read-only SceneComposer with optional Blockout SpatialGuide recommendation."
    )]
    pub async fn compose(
        &self,
        Parameters(params): Parameters<ComposeToolParams>,
    ) -> Result<CallToolResult, McpError> {
        let ws = self.resolve_workspace()?;
        let composer = SceneComposer::with_codex_bin(self.auth_dir.clone(), self.codex_bin.clone());
        let composed = composer
            .compose(&params.prose)
            .await
            .map_err(core_err_to_mcp_error)?;

        let mut written_path: Option<PathBuf> = None;
        if let Some(ref raw_out) = params.out_path {
            let dest = resolve_workspace_write_path(&ws.root, raw_out, "SceneSpec out_path")?;
            let fmt = params.format.as_deref().unwrap_or("yaml");
            let serialized = match fmt {
                "yaml" | "yml" => composed
                    .spec
                    .to_yaml_string()
                    .map_err(core_err_to_mcp_error)?,
                "json" => composed
                    .spec
                    .to_json_string()
                    .map_err(core_err_to_mcp_error)?,
                other => {
                    return Err(McpError::invalid_params(
                        format!("unsupported SceneSpec format '{other}' (expected yaml or json)"),
                        None,
                    ));
                }
            };
            write_atomic(&dest, serialized.as_bytes(), "SceneSpec")
                .map_err(core_err_to_mcp_error)?;
            written_path = Some(dest);
        }

        let blockout_recommendation = if params.recommend_blockout.unwrap_or(false) {
            Some(build_recommended_blockout(&composed.spec))
        } else {
            None
        };

        json_tool_result(&serde_json::json!({
            "sceneSpec": composed.spec,
            "compiledPrompt": composed.compiled.prompt,
            "writtenPath": written_path,
            "blockoutRecommendation": blockout_recommendation,
        }))
    }

    /// Generate a verified `Run` (`0`-ref `explore`, `1`-ref, or multi-ref auto-`Atlas`)
    /// with fail-closed `Visibility` and `ColorQaEngine` gates.
    #[tool(
        name = "generate",
        description = "Execute unified scene generation (0-ref explore, 1-ref, or multi-ref dynamic Atlas) with Visibility and ColorQaEngine gates, recording a versioned Run."
    )]
    pub async fn generate(
        &self,
        Parameters(params): Parameters<GenerateToolParams>,
    ) -> Result<CallToolResult, McpError> {
        let ws = self.resolve_workspace()?;
        let run_store = RunStore::new(ws.root.join(&ws.config.paths.runs_dir));
        let registry = LibraryRegistry::from_workspace(&ws);

        let scene_input = match (params.scene_path, params.scene_spec, params.compose_prose) {
            (Some(path_raw), None, None) => {
                let path = resolve_workspace_read_path(&ws.root, &path_raw, "scene_path")?;
                let spec = SceneSpec::load_from_path(&path).map_err(core_err_to_mcp_error)?;
                GenerateSceneInput::Spec(Box::new(spec))
            }
            (None, Some(spec_val), None) => {
                let raw_json = serde_json::to_string(&spec_val)
                    .map_err(|err| McpError::invalid_params(err.to_string(), None))?;
                let spec = SceneSpec::from_json_str(&raw_json).map_err(core_err_to_mcp_error)?;
                GenerateSceneInput::Spec(Box::new(spec))
            }
            (None, None, Some(prose)) => GenerateSceneInput::Prose(prose),
            (None, None, None) => {
                return Err(McpError::invalid_params(
                    "provide exactly one of scene_path, scene_spec, or compose_prose",
                    None,
                ));
            }
            _ => {
                return Err(McpError::invalid_params(
                    "scene_path, scene_spec, and compose_prose are mutually exclusive",
                    None,
                ));
            }
        };

        let atlas_selections = params
            .atlas_selections
            .unwrap_or_default()
            .into_iter()
            .map(|s| AtlasSliceSelection {
                subject_slug: s.subject_slug,
                style_slug: s.style_slug,
                slice_ids: s.slice_ids,
            })
            .collect();

        let mut reference_images = Vec::new();
        for raw_ref in params.reference_images.unwrap_or_default() {
            reference_images.push(resolve_workspace_read_path(
                &ws.root,
                &raw_ref,
                "reference_image",
            )?);
        }

        let spatial_guide_path = match params.spatial_guide_path {
            Some(ref raw) => Some(resolve_workspace_read_path(
                &ws.root,
                raw,
                "spatial_guide_path",
            )?),
            None => None,
        };

        let subject_rois = params
            .subject_rois
            .unwrap_or_default()
            .into_iter()
            .map(|r| SubjectRoiBinding {
                subject_slug: r.subject_slug,
                rois: r.rois.into_iter().map(PixelRect::from).collect(),
            })
            .collect();

        let extra_out_path = match params.out_path {
            Some(ref raw) => Some(resolve_workspace_write_path(&ws.root, raw, "out_path")?),
            None => None,
        };

        let composer = SceneComposer::with_codex_bin(self.auth_dir.clone(), self.codex_bin.clone());
        let res = execute_generate(
            self.backend.as_ref(),
            Some(&composer),
            &registry,
            &run_store,
            &GenerateRequest {
                scene_input,
                slug: params.slug,
                mode: if params.explore.unwrap_or(false) {
                    GenerationMode::Explore
                } else {
                    GenerationMode::Standard
                },
                width_override: params.width,
                height_override: params.height,
                atlas_selections,
                reference_images,
                spatial_guide_path,
                subject_rois,
                explicit_subject_rois: Vec::new(),
                exposure_config: ExposureQaConfig::default(),
                palette_config: PaletteQaConfig::default(),
                allowed_sparkle_mask: None,
                sparkle_config: SparkleConfig::default(),
                fail_closed_on_qa: !params.allow_qa_fail.unwrap_or(false),
                initial_lifecycle: if params.draft.unwrap_or(false) {
                    Lifecycle::Draft
                } else {
                    Lifecycle::Candidate
                },
                extra_out_path,
            },
        )
        .await
        .map_err(core_err_to_mcp_error)?;

        json_tool_result(&res)
    }

    /// Generate a style-scoped `ReferencePack`, slice foreground components against `#252830`,
    /// or pack selected slices into a dynamic `Atlas`.
    #[tool(
        name = "pack",
        description = "Generate a style-scoped ReferencePack with #252830 calibration strip, slice components by foreground bounding boxes, or pack a dynamic Atlas."
    )]
    pub async fn pack(
        &self,
        Parameters(params): Parameters<PackToolParams>,
    ) -> Result<CallToolResult, McpError> {
        let ws = self.resolve_workspace()?;
        let library_dir = ws.root.join(&ws.config.paths.library_dir);
        let registry = LibraryRegistry::from_workspace(&ws);

        match params.action.as_str() {
            "generate" => {
                let subject_slug = require_param(params.subject_slug, "subject_slug")?;
                let style_slug = require_param(params.style_slug, "style_slug")?;
                let mut req = ReferencePackRequest::new(subject_slug, style_slug);
                if let Some(ref raw_seed) = params.seed_path {
                    req.seed_path = Some(resolve_workspace_read_path(
                        &ws.root,
                        raw_seed,
                        "seed_path",
                    )?);
                }
                req.seed_label = params.seed_label;
                let pipeline =
                    ReferencePackPipeline::new(self.backend.as_ref(), &registry, library_dir);
                let generated = pipeline
                    .generate(&req)
                    .await
                    .map_err(core_err_to_mcp_error)?;
                json_tool_result(&serde_json::json!({
                    "packDir": generated.pack_dir,
                    "packManifestPath": generated.pack_manifest_path,
                    "slicesManifestPath": generated.slices_manifest_path,
                    "manifest": generated.manifest,
                }))
            }
            "slice" => {
                let sheet_raw = require_param(params.sheet_path, "sheet_path")?;
                let out_dir_raw = require_param(params.out_dir, "out_dir")?;
                let sheet_path = resolve_workspace_read_path(&ws.root, &sheet_raw, "sheet_path")?;
                let out_dir = resolve_workspace_write_dir(&ws.root, &out_dir_raw, "out_dir")?;
                let manifest = slice_sheet_to_dir(
                    &sheet_path,
                    &out_dir,
                    &SlicerConfig {
                        exclude_bottom_rows: params.strip_height.unwrap_or(72),
                        ..SlicerConfig::default()
                    },
                )
                .map_err(core_err_to_mcp_error)?;
                json_tool_result(&manifest)
            }
            "atlas" => {
                let raw_selections = require_param(params.selections, "selections")?;
                let out_raw = require_param(params.out_path, "out_path")?;
                let out_path = resolve_workspace_write_path(&ws.root, &out_raw, "out_path")?;
                let target_visibility = parse_visibility_opt(params.visibility.as_deref())?;

                let selections = raw_selections
                    .into_iter()
                    .map(|s| AtlasSliceSelection {
                        subject_slug: s.subject_slug,
                        style_slug: s.style_slug,
                        slice_ids: s.slice_ids,
                    })
                    .collect();
                let result = pack_dynamic_atlas(
                    &registry,
                    &library_dir,
                    &AtlasPackRequest {
                        selections,
                        target_visibility,
                        canvas_width: params.width.unwrap_or(DEFAULT_ATLAS_WIDTH),
                        canvas_height: params.height.unwrap_or(DEFAULT_ATLAS_HEIGHT),
                        padding_px: DEFAULT_ATLAS_PADDING_PX,
                    },
                )
                .map_err(core_err_to_mcp_error)?;

                let mut page_paths = Vec::with_capacity(result.pages.len());
                for page in &result.pages {
                    let page_path = if result.pages.len() == 1 {
                        out_path.clone()
                    } else {
                        let stem = out_path
                            .file_stem()
                            .and_then(|s| s.to_str())
                            .unwrap_or("atlas");
                        out_path.with_file_name(format!("{stem}_p{}.png", page.page_index + 1))
                    };
                    write_atomic(&page_path, &page.png_bytes, "Atlas PNG")
                        .map_err(core_err_to_mcp_error)?;
                    page_paths.push(page_path);
                }

                json_tool_result(&serde_json::json!({
                    "pagePaths": page_paths,
                    "placements": result.placements,
                }))
            }
            other => Err(McpError::invalid_params(
                format!("unknown pack action '{other}' (expected generate, slice, or atlas)"),
                None,
            )),
        }
    }

    /// Execute `WholeImageEdit` or `PatchEdit` (OKLab collar Reinhard + 5-level Laplacian)
    /// with fail-closed `Visibility` and `ColorQaEngine` verification.
    #[tool(
        name = "edit",
        description = "Run WholeImageEdit (fail-closed ColorQaEngine) or PatchEdit (1:1 crop + OKLab collar Reinhard + 5-level Laplacian) and record a Run."
    )]
    pub async fn edit(
        &self,
        Parameters(params): Parameters<EditToolParams>,
    ) -> Result<CallToolResult, McpError> {
        let ws = self.resolve_workspace()?;
        let run_store = RunStore::new(ws.root.join(&ws.config.paths.runs_dir));
        let registry = LibraryRegistry::from_workspace(&ws);

        let target_image_path =
            resolve_workspace_read_path(&ws.root, &params.target_image_path, "target_image_path")?;
        let target_visibility = parse_visibility_opt(params.visibility.as_deref())?;

        let mut reference_images = Vec::new();
        for raw_ref in params.reference_images.unwrap_or_default() {
            reference_images.push(resolve_workspace_read_path(
                &ws.root,
                &raw_ref,
                "reference_image",
            )?);
        }

        // Cross-check target & reference Run visibility explicitly.
        let mut run_vis_checks: Vec<(String, Visibility)> = Vec::new();
        if let Some(vis) = run_store
            .inspect_run_image_visibility(&target_image_path)
            .map_err(core_err_to_mcp_error)?
        {
            run_vis_checks.push((target_image_path.display().to_string(), vis));
        }
        for ref_path in &reference_images {
            if let Some(vis) = run_store
                .inspect_run_image_visibility(ref_path)
                .map_err(core_err_to_mcp_error)?
            {
                run_vis_checks.push((ref_path.display().to_string(), vis));
            }
        }
        check_visibility_compatibility(
            target_visibility,
            "McpEdit",
            run_vis_checks.iter().map(|(id, vis)| (id.as_str(), *vis)),
        )
        .map_err(core_err_to_mcp_error)?;

        match params.mode.as_str() {
            "whole" => {
                let exposure_floor = match params.atmosphere_slug {
                    Some(ref atmo_slug) => {
                        let resolved = registry
                            .resolve_atmosphere(atmo_slug)
                            .map_err(core_err_to_mcp_error)?;
                        Some((
                            resolved.atmosphere.exposure_floor,
                            ExposureQaConfig::default(),
                        ))
                    }
                    None => None,
                };
                let mut subject_rois = Vec::new();
                for subj_slug in params.subject_slugs.unwrap_or_default() {
                    let resolved = registry
                        .resolve_subject(&subj_slug)
                        .map_err(core_err_to_mcp_error)?;
                    check_visibility_compatibility(
                        target_visibility,
                        "WholeImageEdit",
                        [(resolved.subject.slug.as_str(), resolved.subject.visibility)],
                    )
                    .map_err(core_err_to_mcp_error)?;
                    subject_rois.push(imgforge_core::OwnedSubjectRoi {
                        subject_slug: resolved.subject.slug,
                        visibility: resolved.subject.visibility,
                        palette_lock: resolved.subject.palette_lock,
                        rois: Vec::new(),
                    });
                }

                let res = execute_whole_image_edit(
                    self.backend.as_ref(),
                    &run_store,
                    &WholeImageEditRequest {
                        slug: params.slug,
                        visibility: target_visibility,
                        target_image_path,
                        compiled_prompt: params.prompt,
                        reference_images,
                        exposure_floor,
                        subject_rois,
                        palette_config: PaletteQaConfig::default(),
                        edit_mask: None,
                        untouched_drift_config: UntouchedDriftConfig::default(),
                        allowed_sparkle_mask: None,
                        sparkle_config: SparkleConfig::default(),
                        fail_closed_on_qa: !params.allow_qa_fail.unwrap_or(false),
                    },
                )
                .await
                .map_err(core_err_to_mcp_error)?;

                json_tool_result(&serde_json::json!({
                    "mode": "whole",
                    "runId": res.run.metadata.run_id,
                    "imagePath": res.run.image_path,
                    "qaReport": res.qa_report,
                }))
            }
            "patch" => {
                let target_mask = match (params.roi, params.mask_path) {
                    (Some(roi), None) => PatchTargetMask::Roi(roi.into()),
                    (None, Some(mask_raw)) => {
                        let mask_path =
                            resolve_workspace_read_path(&ws.root, &mask_raw, "mask_path")?;
                        PatchTargetMask::MaskImagePath(mask_path)
                    }
                    (None, None) => {
                        return Err(McpError::invalid_params(
                            "PatchEdit requires either roi or mask_path",
                            None,
                        ));
                    }
                    (Some(_), Some(_)) => {
                        return Err(McpError::invalid_params(
                            "roi and mask_path are mutually exclusive",
                            None,
                        ));
                    }
                };

                let prep_mode = match params.prep_mode.as_deref().unwrap_or("crop_only") {
                    "crop_only" => PatchPrepMode::CropOnly,
                    "blank_inside_mask" => PatchPrepMode::BlankInsideMask,
                    "cue_outline" => PatchPrepMode::CueOutline,
                    other => {
                        return Err(McpError::invalid_params(
                            format!("unknown prep_mode '{other}'"),
                            None,
                        ));
                    }
                };

                let res = execute_patch_edit(
                    self.backend.as_ref(),
                    &run_store,
                    &PatchEditRequest {
                        slug: params.slug,
                        visibility: target_visibility,
                        target_image_path,
                        target_mask,
                        compiled_prompt: params.prompt,
                        reference_images,
                        config: PatchEditConfig {
                            collar_width_px: params
                                .collar_width_px
                                .unwrap_or(DEFAULT_COLLAR_WIDTH_PX),
                            feather_px: params.feather_px.unwrap_or(12),
                            prep_mode,
                            ..PatchEditConfig::default()
                        },
                    },
                )
                .await
                .map_err(core_err_to_mcp_error)?;

                json_tool_result(&serde_json::json!({
                    "mode": "patch",
                    "runId": res.run.metadata.run_id,
                    "imagePath": res.run.image_path,
                    "cropRect": res.crop_rect,
                    "roiRect": res.roi_rect,
                    "reinhardGains": res.reinhard_gains,
                    "outOfRoiMaxDeltaE2000": res.out_of_roi_report.max_delta_e2000,
                }))
            }
            other => Err(McpError::invalid_params(
                format!("unknown edit mode '{other}' (expected whole or patch)"),
                None,
            )),
        }
    }

    /// Validate or deterministically render a `GuideSpec` into an `OverlayGuide` or `Blockout`
    /// `SpatialGuide` PNG via `GuideRenderer`.
    #[tool(
        name = "guide",
        description = "Validate a GuideSpec JSON document or deterministically render an OverlayGuide or Blockout SpatialGuide PNG via GuideRenderer."
    )]
    pub async fn guide(
        &self,
        Parameters(params): Parameters<GuideToolParams>,
    ) -> Result<CallToolResult, McpError> {
        let ws = self.resolve_workspace()?;
        let spec = match (params.spec_path, params.guide_spec) {
            (Some(path_raw), None) => {
                let path = resolve_workspace_read_path(&ws.root, &path_raw, "spec_path")?;
                let bytes = imgforge_core::store::MAX_REGULAR_FILE_BYTES;
                let _ = bytes;
                let raw = fs::read_to_string(&path).map_err(|err| {
                    McpError::invalid_params(
                        format!("reading GuideSpec {}: {err}", path.display()),
                        None,
                    )
                })?;
                GuideSpec::from_json_str(&raw).map_err(core_err_to_mcp_error)?
            }
            (None, Some(val)) => {
                let raw = serde_json::to_string(&val)
                    .map_err(|err| McpError::invalid_params(err.to_string(), None))?;
                GuideSpec::from_json_str(&raw).map_err(core_err_to_mcp_error)?
            }
            (None, None) => {
                return Err(McpError::invalid_params(
                    "provide either spec_path or guide_spec",
                    None,
                ));
            }
            (Some(_), Some(_)) => {
                return Err(McpError::invalid_params(
                    "spec_path and guide_spec are mutually exclusive",
                    None,
                ));
            }
        };

        match params.action.as_str() {
            "validate" => json_tool_result(&serde_json::json!({
                "valid": true,
                "id": spec.id,
                "variant": spec.variant,
                "canvasWidth": spec.canvas_width,
                "canvasHeight": spec.canvas_height,
                "depthBands": spec.depth_bands.len(),
                "mannequins": spec.mannequins.len(),
                "actionVectors": spec.action_vectors.len(),
                "overlayRegions": spec.overlay_regions.len(),
            })),
            "render" => {
                let out_raw = require_param(params.out_path, "out_path")?;
                let out_path = resolve_workspace_write_path(&ws.root, &out_raw, "out_path")?;
                let rendered = GuideRenderer::new()
                    .render(&spec, &ws.root)
                    .map_err(core_err_to_mcp_error)?;
                write_atomic(&out_path, &rendered.png_bytes, "SpatialGuide PNG")
                    .map_err(core_err_to_mcp_error)?;
                json_tool_result(&serde_json::json!({
                    "id": spec.id,
                    "variant": rendered.variant,
                    "width": rendered.width,
                    "height": rendered.height,
                    "outPath": out_path,
                }))
            }
            other => Err(McpError::invalid_params(
                format!("unknown guide action '{other}' (expected validate or render)"),
                None,
            )),
        }
    }

    /// Derive deterministic `Export` deliverables (`vtt_token`, `vtt_map`, `web_webp`, `grid_2x2`)
    /// with fail-closed `Visibility` checks and optional `--match-grade` LUT/CDF grading.
    #[tool(
        name = "export",
        description = "Derive deterministic VTT/Web/Grid Export deliverables from a Run or PNG with fail-closed Visibility enforcement and optional match_grade."
    )]
    pub async fn export(
        &self,
        Parameters(params): Parameters<ExportToolParams>,
    ) -> Result<CallToolResult, McpError> {
        let ws = self.resolve_workspace()?;
        let run_store = RunStore::new(ws.root.join(&ws.config.paths.runs_dir));
        let out_dir = resolve_workspace_write_dir(&ws.root, &params.out_dir, "out_dir")?;

        let (source, default_slug) = match (params.run_slug, params.run_version, params.input_path)
        {
            (Some(slug), Some(version), None) => (
                ExportSource::Run {
                    slug: slug.clone(),
                    version,
                },
                slug,
            ),
            (None, None, Some(raw_input)) => {
                let path = resolve_workspace_read_path(&ws.root, &raw_input, "input_path")?;
                let stem = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("export")
                    .to_string();
                let source_vis = parse_visibility_opt(params.source_visibility.as_deref())?;
                (
                    ExportSource::PngPath {
                        path,
                        asset_id: stem.clone(),
                        visibility: source_vis,
                    },
                    stem,
                )
            }
            (Some(_), None, None) | (None, Some(_), None) => {
                return Err(McpError::invalid_params(
                    "both run_slug and run_version are required when exporting a Run",
                    None,
                ));
            }
            (None, None, None) => {
                return Err(McpError::invalid_params(
                    "provide either (run_slug, run_version) or input_path",
                    None,
                ));
            }
            _ => {
                return Err(McpError::invalid_params(
                    "run_slug/run_version and input_path are mutually exclusive",
                    None,
                ));
            }
        };

        let profile = match params.profile.as_str() {
            "vtt_token" => ExportProfile::VttToken(VttTokenOptions {
                size_px: params.token_size.unwrap_or(DEFAULT_VTT_TOKEN_SIZE_PX),
                ring_color_hex: params
                    .ring_color_hex
                    .unwrap_or_else(|| DEFAULT_TOKEN_RING_HEX.to_string()),
                jpeg_backdrop_hex: params.jpeg_backdrop_hex,
                jpeg_quality: params.jpeg_quality.unwrap_or(90),
                ..VttTokenOptions::default()
            }),
            "vtt_map" => ExportProfile::VttMap(VttMapOptions {
                cols: params.cols.unwrap_or(24),
                rows: params.rows.unwrap_or(18),
                grid_px: params.grid_px.unwrap_or(DEFAULT_VTT_MAP_GRID_PX),
                scale_factor: params.scale_factor,
                jpeg_quality: params.jpeg_quality.unwrap_or(90),
            }),
            "web_webp" => ExportProfile::WebWebp,
            "grid_2x2" => ExportProfile::Grid2x2,
            other => {
                return Err(McpError::invalid_params(
                    format!("unknown export profile '{other}'"),
                    None,
                ));
            }
        };

        let match_grade = if params.match_grade.unwrap_or(false) {
            let reference_image_path = match params.grade_ref_path {
                Some(ref raw) => Some(resolve_workspace_read_path(
                    &ws.root,
                    raw,
                    "grade_ref_path",
                )?),
                None => None,
            };
            let cube_lut_path = match params.cube_lut_path {
                Some(ref raw) => Some(resolve_workspace_read_path(&ws.root, raw, "cube_lut_path")?),
                None => None,
            };
            Some(MatchGradeOptions {
                reference_image_path,
                cdf_damp_strength: params.grade_strength.unwrap_or(DEFAULT_CDF_DAMP_STRENGTH),
                cube_lut_path,
            })
        } else {
            None
        };

        let result = execute_export(
            Some(&run_store),
            &ExportRequest {
                source,
                output_slug: params.output_slug.unwrap_or(default_slug),
                profile_visibility: parse_visibility_opt(params.visibility.as_deref())?,
                out_dir,
                profile,
                match_grade,
            },
        )
        .map_err(core_err_to_mcp_error)?;

        json_tool_result(&result)
    }

    /// Deterministically validate a `SceneSpec` (`PromptCompiler`), or validate a `Run` /
    /// `ReferencePack` (`structural + ColorQaEngine + VisionCritic`) transitioning `draft -> candidate -> validated`.
    #[tool(
        name = "validate",
        description = "Validate a SceneSpec via PromptCompiler, or run structural + ColorQaEngine + VisionCritic checks to transition a Run or ReferencePack to validated."
    )]
    pub async fn validate(
        &self,
        Parameters(params): Parameters<ValidateToolParams>,
    ) -> Result<CallToolResult, McpError> {
        let ws = self.resolve_workspace()?;
        let registry = LibraryRegistry::from_workspace(&ws);

        let mut traits = Vec::new();
        for t in params.traits.unwrap_or_default() {
            let category = t
                .category
                .parse::<TraitCategory>()
                .map_err(core_err_to_mcp_error)?;
            traits.push(TraitCheckInput {
                trait_id: t.trait_id,
                category,
                criterion: t.criterion,
            });
        }
        let critic = VisionCritic::with_codex_bin(self.auth_dir.clone(), self.codex_bin.clone());

        match params.target.as_str() {
            "scene_spec" => {
                let spec = match (params.scene_path, params.scene_spec) {
                    (Some(raw_path), None) => {
                        let path = resolve_workspace_read_path(&ws.root, &raw_path, "scene_path")?;
                        SceneSpec::load_from_path(&path).map_err(core_err_to_mcp_error)?
                    }
                    (None, Some(val)) => {
                        let raw_json = serde_json::to_string(&val)
                            .map_err(|err| McpError::invalid_params(err.to_string(), None))?;
                        SceneSpec::from_json_str(&raw_json).map_err(core_err_to_mcp_error)?
                    }
                    (None, None) => {
                        return Err(McpError::invalid_params(
                            "scene_spec validation requires scene_path or scene_spec",
                            None,
                        ));
                    }
                    (Some(_), Some(_)) => {
                        return Err(McpError::invalid_params(
                            "scene_path and scene_spec are mutually exclusive",
                            None,
                        ));
                    }
                };
                let mut overrides = BTreeMap::new();
                for slot in &spec.foreground_slots {
                    if let Ok(resolved) = registry.resolve_subject(slot.subject_ref.trim()) {
                        overrides.insert(resolved.subject.slug, resolved.subject.visibility);
                    }
                }
                let compiled = PromptCompiler::compile_with_subject_visibility(&spec, &overrides)
                    .map_err(core_err_to_mcp_error)?;
                json_tool_result(&serde_json::json!({
                    "valid": true,
                    "id": spec.id,
                    "visibility": compiled.visibility,
                    "foregroundSlots": compiled.foreground_slot_count,
                    "targetWidth": compiled.target_width,
                    "targetHeight": compiled.target_height,
                    "compiledPrompt": compiled.prompt,
                }))
            }
            "run" => {
                let slug = require_param(params.slug, "slug")?;
                let version = require_param(params.version, "version")?;
                let run_store = RunStore::new(ws.root.join(&ws.config.paths.runs_dir));
                let mut reference_images = Vec::new();
                for raw_ref in params.reference_images.unwrap_or_default() {
                    reference_images.push(resolve_workspace_read_path(
                        &ws.root,
                        &raw_ref,
                        "reference_image",
                    )?);
                }
                let subject_rois = params
                    .subject_slugs
                    .unwrap_or_default()
                    .into_iter()
                    .map(|s| SubjectRoiBinding {
                        subject_slug: s,
                        rois: Vec::new(),
                    })
                    .collect();
                let res = validate_run(
                    &run_store,
                    &registry,
                    Some(&critic),
                    &ValidateRunRequest {
                        slug,
                        version,
                        atmosphere_slug: params.atmosphere_slug,
                        subject_rois,
                        explicit_subject_rois: Vec::new(),
                        reference_images,
                        traits,
                        run_critic: params.run_critic.unwrap_or(false),
                        exposure_config: ExposureQaConfig::default(),
                        palette_config: PaletteQaConfig::default(),
                        sparkle_config: SparkleConfig::default(),
                    },
                )
                .await
                .map_err(core_err_to_mcp_error)?;
                json_tool_result(&res)
            }
            "reference_pack" => {
                let subject_slug = require_param(params.subject_slug, "subject_slug")?;
                let style_slug = require_param(params.style_slug, "style_slug")?;
                let library_dir = ws.root.join(&ws.config.paths.library_dir);
                let mut req = ValidateReferencePackRequest::new(subject_slug, style_slug);
                req.traits = traits;
                req.run_critic = params.run_critic.unwrap_or(false);
                let res = validate_reference_pack(&library_dir, &registry, Some(&critic), &req)
                    .await
                    .map_err(core_err_to_mcp_error)?;
                json_tool_result(&res)
            }
            other => Err(McpError::invalid_params(
                format!(
                    "unknown validate target '{other}' (expected scene_spec, run, or reference_pack)"
                ),
                None,
            )),
        }
    }

    /// Promote a `Validated` `Run` or `ReferencePack` to `Canonical` in `manifest.lock.json`
    /// (superseding prior canonical versions) or inspect workspace/run lifecycle status.
    #[tool(
        name = "promote",
        description = "Promote a validated Run or ReferencePack to immutable canonical in manifest.lock.json (superseding prior canonicals), or inspect workspace/run status."
    )]
    pub async fn promote(
        &self,
        Parameters(params): Parameters<PromoteToolParams>,
    ) -> Result<CallToolResult, McpError> {
        let ws = self.resolve_workspace()?;
        let library_dir = ws.root.join(&ws.config.paths.library_dir);
        let lock_store = ManifestLockStore::new(&library_dir);
        let run_store = RunStore::new(ws.root.join(&ws.config.paths.runs_dir));

        match params.action.as_str() {
            "run" => {
                let slug = require_param(params.slug, "slug")?;
                let version = require_param(params.version, "version")?;
                let (metadata, lock) =
                    promote_run(&lock_store, &run_store, &slug, version, &ws.root)
                        .map_err(core_err_to_mcp_error)?;
                json_tool_result(&serde_json::json!({
                    "metadata": metadata,
                    "canonicalCount": lock.canonical.len(),
                    "supersededCount": lock.superseded.len(),
                }))
            }
            "reference_pack" => {
                let subject_slug = require_param(params.subject_slug, "subject_slug")?;
                let style_slug = require_param(params.style_slug, "style_slug")?;
                let (manifest, lock) = promote_reference_pack(
                    &lock_store,
                    &library_dir,
                    &subject_slug,
                    &style_slug,
                    &ws.root,
                )
                .map_err(core_err_to_mcp_error)?;
                json_tool_result(&serde_json::json!({
                    "manifest": manifest,
                    "canonicalCount": lock.canonical.len(),
                    "supersededCount": lock.superseded.len(),
                }))
            }
            "status" => {
                let auth_manager = AuthManager::new(self.auth_dir.clone(), self.codex_bin.clone());
                let status =
                    inspect_workspace_status(&lock_store, &run_store, params.slug.as_deref())
                        .map_err(core_err_to_mcp_error)?;
                json_tool_result(&serde_json::json!({
                    "workspaceRoot": ws.root,
                    "backend": ws.config.backend.kind,
                    "authProvisioned": auth_manager.is_provisioned_locally(),
                    "lifecycleStatus": status,
                }))
            }
            other => Err(McpError::invalid_params(
                format!(
                    "unknown promote action '{other}' (expected run, reference_pack, or status)"
                ),
                None,
            )),
        }
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for ImgforgeMcpServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .build(),
        )
        .with_server_info(Implementation::new("imgforge", env!("CARGO_PKG_VERSION")))
        .with_instructions(
            "imgforge visual consistency engine MCP server: compose, generate, pack, edit, guide, export, validate, promote tools and imgforge://* workspace resources.",
        )
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, McpError> {
        let resources = vec![
            Resource::new(RESOURCE_URI_MANIFEST, "manifest")
                .with_title("Canonical Manifest Lock")
                .with_description(
                    "Contents of library/manifest.lock.json (canonical and superseded entries)",
                )
                .with_mime_type("application/json"),
            Resource::new(RESOURCE_URI_SUBJECTS, "subjects")
                .with_title("Resolved Subject Catalog")
                .with_description(
                    "Resolved style-neutral Subject cards across project > user tiers",
                )
                .with_mime_type("application/json"),
            Resource::new(RESOURCE_URI_STYLES_ATMOSPHERES, "styles-atmospheres")
                .with_title("Resolved Style & Atmosphere Catalog")
                .with_description(
                    "Resolved Style and Atmosphere cards with project > user > builtin precedence",
                )
                .with_mime_type("application/json"),
            Resource::new(RESOURCE_URI_RUNS, "runs")
                .with_title("Recorded Run Index & Latest QA Reports")
                .with_description("Recorded Run metadata index and latest QA reports under runs/")
                .with_mime_type("application/json"),
        ];
        Ok(ListResourcesResult::with_all_items(resources))
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, McpError> {
        let ws = self.resolve_workspace()?;
        let library_dir = ws.root.join(&ws.config.paths.library_dir);
        let registry = LibraryRegistry::from_workspace(&ws);

        let payload_json = match request.uri.as_str() {
            RESOURCE_URI_MANIFEST => {
                let lock = ManifestLockStore::new(&library_dir)
                    .load_or_init()
                    .map_err(core_err_to_mcp_error)?;
                serde_json::to_string_pretty(&lock)
                    .map_err(|err| McpError::internal_error(err.to_string(), None))?
            }
            RESOURCE_URI_SUBJECTS => {
                let subjects = registry.list_subjects().map_err(core_err_to_mcp_error)?;
                let rows: Vec<serde_json::Value> = subjects
                    .into_iter()
                    .map(|s| {
                        serde_json::json!({
                            "source": s.source,
                            "subject": s.subject,
                        })
                    })
                    .collect();
                serde_json::to_string_pretty(&rows)
                    .map_err(|err| McpError::internal_error(err.to_string(), None))?
            }
            RESOURCE_URI_STYLES_ATMOSPHERES => {
                let styles = registry.list_styles().map_err(core_err_to_mcp_error)?;
                let atmospheres = registry.list_atmospheres().map_err(core_err_to_mcp_error)?;
                let style_rows: Vec<serde_json::Value> = styles
                    .into_iter()
                    .map(|s| {
                        serde_json::json!({
                            "source": s.source,
                            "style": s.style,
                        })
                    })
                    .collect();
                let atmo_rows: Vec<serde_json::Value> = atmospheres
                    .into_iter()
                    .map(|a| {
                        serde_json::json!({
                            "source": a.source,
                            "atmosphere": a.atmosphere,
                        })
                    })
                    .collect();
                serde_json::to_string_pretty(&serde_json::json!({
                    "styles": style_rows,
                    "atmospheres": atmo_rows,
                }))
                .map_err(|err| McpError::internal_error(err.to_string(), None))?
            }
            RESOURCE_URI_RUNS => {
                let lock_store = ManifestLockStore::new(&library_dir);
                let run_store = RunStore::new(ws.root.join(&ws.config.paths.runs_dir));
                let status = inspect_workspace_status(&lock_store, &run_store, None)
                    .map_err(core_err_to_mcp_error)?;
                let streams: Vec<serde_json::Value> = status
                    .runs
                    .into_iter()
                    .map(|(slug, versions)| {
                        let latest_qa = versions.last().map(|v| {
                            serde_json::json!({
                                "runId": v.run_id,
                                "lifecycle": v.lifecycle,
                                "qaReports": v.qa_reports,
                            })
                        });
                        serde_json::json!({
                            "slug": slug,
                            "latest": latest_qa,
                            "versions": versions,
                        })
                    })
                    .collect();
                serde_json::to_string_pretty(&serde_json::json!({
                    "streams": streams,
                }))
                .map_err(|err| McpError::internal_error(err.to_string(), None))?
            }
            other => {
                return Err(McpError::resource_not_found(
                    format!("unknown imgforge MCP resource URI '{other}'"),
                    None,
                ));
            }
        };

        Ok(ReadResourceResult::new(vec![
            ResourceContents::text(payload_json, request.uri).with_mime_type("application/json"),
        ])
        .into())
    }
}

// === 3. Transports (Stdio Default & Optional Loopback Streamable HTTP) ===

/// Serve the `imgforge` MCP server over stdio until EOF or shutdown.
pub async fn serve_mcp_stdio(server: ImgforgeMcpServer) -> anyhow::Result<()> {
    let service = server
        .serve(rmcp::transport::io::stdio())
        .await
        .context("starting rmcp stdio server")?;
    service
        .waiting()
        .await
        .context("waiting on rmcp stdio server")?;
    Ok(())
}

/// Serve the `imgforge` MCP server over Streamable HTTP strictly on a loopback address (`127.0.0.1:<port>`).
pub async fn serve_mcp_http(server: ImgforgeMcpServer, bind_addr: &str) -> anyhow::Result<()> {
    let addr: SocketAddr = bind_addr
        .parse()
        .with_context(|| format!("parsing --http socket address '{bind_addr}'"))?;
    if !addr.ip().is_loopback() {
        bail!(
            "refusing non-loopback --http bind address '{addr}'; imgforge MCP HTTP must bind strictly to 127.0.0.1 or ::1"
        );
    }

    let listener = TcpListener::bind(addr).await?;
    let bound_addr = listener.local_addr()?;
    tracing::info!("imgforge MCP Streamable HTTP listening on http://{bound_addr}/mcp");

    let mut config = StreamableHttpServerConfig::default();
    config.json_response = true;
    let cancel_token = config.cancellation_token.clone();
    let mcp_service = StreamableHttpService::new(
        move || Ok(server.clone()),
        Arc::new(LocalSessionManager::default()),
        config,
    );

    let router = Router::new().nest_service("/mcp", mcp_service);
    axum::serve(listener, router)
        .with_graceful_shutdown(async move {
            let _ = tokio::signal::ctrl_c().await;
            cancel_token.cancel();
        })
        .await?;
    Ok(())
}

// === 4. Workspace Path Containment & Helpers ===

fn json_tool_result<T: Serialize>(value: &T) -> Result<CallToolResult, McpError> {
    let json = serde_json::to_string_pretty(value)
        .map_err(|err| McpError::internal_error(err.to_string(), None))?;
    Ok(CallToolResult::success(vec![ContentBlock::text(json)]))
}

fn require_param<T>(val: Option<T>, name: &str) -> Result<T, McpError> {
    val.ok_or_else(|| {
        McpError::invalid_params(format!("missing required parameter '{name}'"), None)
    })
}

fn parse_visibility_opt(raw: Option<&str>) -> Result<Visibility, McpError> {
    match raw.map(str::trim) {
        None | Some("public") => Ok(Visibility::Public),
        Some("restricted") => Ok(Visibility::Restricted),
        Some("internal") => Ok(Visibility::Internal),
        Some(other) => Err(McpError::invalid_params(
            format!("invalid visibility '{other}' (expected public, restricted, or internal)"),
            None,
        )),
    }
}

fn core_err_to_mcp_invalid_params(err: CoreError) -> McpError {
    McpError::invalid_params(err.to_string(), None)
}

fn core_err_to_mcp_error(err: CoreError) -> McpError {
    match err {
        CoreError::WorkspaceNotFound { .. }
        | CoreError::WorkspaceInvalid { .. }
        | CoreError::SceneSpecInvalid(_)
        | CoreError::PromptCompilationFailed(_)
        | CoreError::InvalidGenerationRequest(_)
        | CoreError::SubjectInvalid(_)
        | CoreError::StyleInvalid(_)
        | CoreError::AtmosphereInvalid(_)
        | CoreError::VisibilityViolation { .. }
        | CoreError::LifecycleTransitionInvalid { .. }
        | CoreError::CanonicalImmutable { .. }
        | CoreError::RegistryResolutionFailed { .. } => {
            McpError::invalid_params(err.to_string(), None)
        }
        other => McpError::internal_error(other.to_string(), None),
    }
}

fn validate_no_traversal_or_nul(raw: &str, label: &str) -> Result<(), McpError> {
    if raw.trim().is_empty() || raw.contains('\0') {
        return Err(McpError::invalid_params(
            format!("{label} must be a non-empty path without NUL bytes"),
            None,
        ));
    }
    for comp in Path::new(raw).components() {
        if matches!(comp, Component::ParentDir) {
            return Err(McpError::invalid_params(
                format!("{label} '{raw}' must not contain '..' parent directory traversal"),
                None,
            ));
        }
    }
    Ok(())
}

fn resolve_workspace_read_path(
    ws_root: &Path,
    raw: &str,
    label: &str,
) -> Result<PathBuf, McpError> {
    validate_no_traversal_or_nul(raw, label)?;
    let canon_root = fs::canonicalize(ws_root).map_err(|err| {
        McpError::internal_error(
            format!("canonicalizing workspace root {}: {err}", ws_root.display()),
            None,
        )
    })?;

    let input = Path::new(raw);
    let candidate = if input.is_absolute() {
        input.to_path_buf()
    } else {
        ws_root.join(input)
    };

    let lstat = fs::symlink_metadata(&candidate).map_err(|err| {
        McpError::invalid_params(
            format!("inspecting {label} at {}: {err}", candidate.display()),
            None,
        )
    })?;
    if lstat.file_type().is_symlink() || !lstat.is_file() {
        return Err(McpError::invalid_params(
            format!("{label} '{}' must be a regular non-symlink file", raw),
            None,
        ));
    }
    let canon_target = fs::canonicalize(&candidate).map_err(|err| {
        McpError::invalid_params(
            format!("canonicalizing {label} '{}': {err}", candidate.display()),
            None,
        )
    })?;
    if !canon_target.starts_with(&canon_root) {
        return Err(McpError::invalid_params(
            format!("{label} '{raw}' escapes workspace root"),
            None,
        ));
    }
    Ok(canon_target)
}

fn resolve_workspace_write_path(
    ws_root: &Path,
    raw: &str,
    label: &str,
) -> Result<PathBuf, McpError> {
    validate_no_traversal_or_nul(raw, label)?;
    let canon_root = fs::canonicalize(ws_root).map_err(|err| {
        McpError::internal_error(
            format!("canonicalizing workspace root {}: {err}", ws_root.display()),
            None,
        )
    })?;

    let input = Path::new(raw);
    let rel = if input.is_absolute() {
        input
            .strip_prefix(ws_root)
            .or_else(|_| input.strip_prefix(&canon_root))
            .map_err(|_| {
                McpError::invalid_params(format!("{label} '{raw}' escapes workspace root"), None)
            })?
    } else {
        input
    };

    let mut cursor = canon_root.clone();
    let components: Vec<_> = rel
        .components()
        .filter(|c| !matches!(c, Component::CurDir))
        .collect();
    if components.is_empty() {
        return Err(McpError::invalid_params(
            format!("{label} '{raw}' must point to a file inside the workspace"),
            None,
        ));
    }
    for (idx, comp) in components.iter().enumerate() {
        let Component::Normal(seg) = comp else {
            return Err(McpError::invalid_params(
                format!("{label} '{raw}' contains invalid path segment"),
                None,
            ));
        };
        cursor.push(seg);
        let is_last = idx + 1 == components.len();
        match fs::symlink_metadata(&cursor) {
            Ok(meta) => {
                if meta.file_type().is_symlink() {
                    return Err(McpError::invalid_params(
                        format!("refusing symlink in {label} path: {}", cursor.display()),
                        None,
                    ));
                }
                if !is_last && !meta.is_dir() {
                    return Err(McpError::invalid_params(
                        format!("{label} parent '{}' is not a directory", cursor.display()),
                        None,
                    ));
                }
                if is_last && meta.is_dir() {
                    return Err(McpError::invalid_params(
                        format!("{label} '{}' is a directory", cursor.display()),
                        None,
                    ));
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => {
                return Err(McpError::invalid_params(
                    format!("inspecting {label} at {}: {err}", cursor.display()),
                    None,
                ));
            }
        }
    }
    Ok(cursor)
}

fn resolve_workspace_write_dir(
    ws_root: &Path,
    raw: &str,
    label: &str,
) -> Result<PathBuf, McpError> {
    validate_no_traversal_or_nul(raw, label)?;
    let canon_root = fs::canonicalize(ws_root).map_err(|err| {
        McpError::internal_error(
            format!("canonicalizing workspace root {}: {err}", ws_root.display()),
            None,
        )
    })?;
    let input = Path::new(raw);
    let rel = if input.is_absolute() {
        input
            .strip_prefix(ws_root)
            .or_else(|_| input.strip_prefix(&canon_root))
            .map_err(|_| {
                McpError::invalid_params(format!("{label} '{raw}' escapes workspace root"), None)
            })?
    } else {
        input
    };

    let mut cursor = canon_root.clone();
    for comp in rel.components() {
        match comp {
            Component::CurDir => {}
            Component::Normal(seg) => {
                cursor.push(seg);
                match fs::symlink_metadata(&cursor) {
                    Ok(meta) => {
                        if meta.file_type().is_symlink() || !meta.is_dir() {
                            return Err(McpError::invalid_params(
                                format!("{label} '{}' must not traverse symlinks or files", raw),
                                None,
                            ));
                        }
                    }
                    Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                    Err(err) => {
                        return Err(McpError::invalid_params(
                            format!("inspecting {label} at {}: {err}", cursor.display()),
                            None,
                        ));
                    }
                }
            }
            _ => {
                return Err(McpError::invalid_params(
                    format!("{label} '{raw}' contains invalid path segment"),
                    None,
                ));
            }
        }
    }
    Ok(cursor)
}

fn build_recommended_blockout(spec: &SceneSpec) -> GuideSpec {
    let w = spec.width.unwrap_or(1024);
    let h = spec.height.unwrap_or(1024);

    let mut depth_bands = Vec::with_capacity(spec.depth_bands.len());
    let n_bands = spec.depth_bands.len().max(1);
    for (idx, b) in spec.depth_bands.iter().enumerate() {
        let y0 = (idx as f64) / (n_bands as f64);
        let y1 = ((idx + 1) as f64) / (n_bands as f64);
        let (plane, fill_hex) = match b.plane {
            DepthPlane::Background => (GuideDepthPlane::Background, "#2C3344".to_string()),
            DepthPlane::Midground => (GuideDepthPlane::Midground, "#3B4254".to_string()),
            DepthPlane::Foreground => (GuideDepthPlane::Foreground, "#4A5164".to_string()),
        };
        depth_bands.push(DepthBand {
            id: format!("band-{}", idx + 1),
            label: b.plane.as_str().to_string(),
            plane,
            y_start_ratio: y0,
            y_end_ratio: y1,
            fill_hex,
        });
    }

    let mannequins = spec
        .foreground_slots
        .iter()
        .enumerate()
        .map(|(idx, slot)| {
            let palette_hex = slot
                .exclusive_surface_map
                .bindings
                .first()
                .map(|b| b.color_hex.clone())
                .unwrap_or_else(|| "#63B3ED".to_string());
            SubjectMannequin {
                id: format!("mannequin-{}", idx + 1),
                subject_slug: slot.subject_ref.clone(),
                label: slot.subject_ref.clone(),
                x_ratio: slot.position_x_ratio,
                y_ratio: 0.85,
                height_meters: slot.height_meters,
                scale_factor: 1.0,
                facing: MannequinFacing::Front,
                pose_hint: slot.pose.clone(),
                palette_hex,
            }
        })
        .collect();

    let action_vectors = if spec.foreground_slots.len() >= 2 {
        vec![ActionVector {
            id: "vector-1".to_string(),
            label: "Primary interaction".to_string(),
            from_x_ratio: spec.foreground_slots[0].position_x_ratio,
            from_y_ratio: 0.65,
            to_x_ratio: spec.foreground_slots[1].position_x_ratio,
            to_y_ratio: 0.65,
            stroke_hex: "#F6AD55".to_string(),
            halo_hex: "#1A1D24".to_string(),
            stroke_width_px: 4.0,
            style: VectorStrokeStyle::Solid,
        }]
    } else {
        Vec::new()
    };

    GuideSpec {
        schema_version: SUPPORTED_GUIDE_SPEC_SCHEMA_VERSION.to_string(),
        id: format!("{}-blockout", spec.id),
        title: format!("{} Blockout", spec.title),
        variant: SpatialGuideVariant::Blockout,
        target_image_path: None,
        canvas_width: w,
        canvas_height: h,
        depth_bands,
        mannequins,
        action_vectors,
        overlay_regions: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    use async_trait::async_trait;
    use image::{DynamicImage, ImageBuffer, ImageFormat, Rgba, RgbaImage};
    use rmcp::ServiceExt;
    use rmcp::model::{CallToolRequestParams, ReadResourceRequestParams};
    use tempfile::TempDir;

    use super::*;
    use imgforge_core::ImageGenerationOutput;
    use imgforge_core::calibration::STUDIO_BG_RGBA;

    #[derive(Clone)]
    struct StubMcpBackend;

    #[async_trait]
    impl ImageBackend for StubMcpBackend {
        fn name(&self) -> &'static str {
            "stub-mcp-backend"
        }

        async fn generate(
            &self,
            request: &imgforge_core::ImageGenerationRequest,
        ) -> Result<ImageGenerationOutput, CoreError> {
            let mut img: RgbaImage = if request.width == 1536
                && request.height == 1024
                && request.prompt.contains("[REFERENCE_PACK_SHEET]")
            {
                let mut sheet = ImageBuffer::from_pixel(
                    request.width,
                    request.height,
                    Rgba([STUDIO_BG_RGBA[0], STUDIO_BG_RGBA[1], STUDIO_BG_RGBA[2], 255]),
                );
                for (start_x, color) in [(180u32, [210u8, 160, 120]), (820u32, [130u8, 170, 210])] {
                    for y in 120..760 {
                        for x in start_x..(start_x + 260) {
                            sheet.put_pixel(x, y, Rgba([color[0], color[1], color[2], 255]));
                        }
                    }
                }
                sheet
            } else {
                ImageBuffer::from_pixel(request.width, request.height, Rgba([150, 155, 160, 255]))
            };

            let _ = &mut img;
            let mut buf = Vec::new();
            DynamicImage::ImageRgba8(img)
                .write_to(&mut Cursor::new(&mut buf), ImageFormat::Png)
                .unwrap();
            Ok(ImageGenerationOutput {
                png_bytes: buf,
                width: request.width,
                height: request.height,
                raw_width: request.width,
                raw_height: request.height,
                thread_id: Some("thr_mcp_stub".to_string()),
            })
        }
    }

    fn setup_mcp_test_workspace() -> (TempDir, PathBuf, PathBuf, PathBuf) {
        let root = TempDir::new().unwrap();
        let ws = root.path().join("ws");
        fs::create_dir_all(ws.join("library/subjects/brass-lantern")).unwrap();
        fs::create_dir_all(ws.join("runs")).unwrap();
        fs::write(
            ws.join("imgforge.toml"),
            "schema_version = 1\nname = \"mcp-test\"\n",
        )
        .unwrap();
        fs::write(
            ws.join("library/subjects/brass-lantern/card.yaml"),
            r##"schema_version: 1
kind: prop
slug: brass-lantern
display_name: Brass Lantern
exclusive_surface_map:
  bindings:
    - slot: body
      material: brushed brass
      color_hex: "#D4A843"
palette_lock:
  swatches:
    - name: brass
      color_hex: "#D4A843"
      surface_slot: body
visibility: public
"##,
        )
        .unwrap();

        let auth_dir = root.path().join("auth");
        fs::create_dir_all(&auth_dir).unwrap();
        fs::set_permissions(&auth_dir, fs::Permissions::from_mode(0o700)).unwrap();
        let auth_file = auth_dir.join("auth.json");
        fs::write(&auth_file, r#"{"token":"mcp-stub"}"#).unwrap();
        fs::set_permissions(&auth_file, fs::Permissions::from_mode(0o600)).unwrap();

        let codex_stub = root.path().join("codex-mcp-stub");
        fs::write(
            &codex_stub,
            r##"#!/usr/bin/env bash
set -euo pipefail
OUT=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    -o) OUT="$2"; shift 2 ;;
    *) shift ;;
  esac
done
if grep -q '"CriticReport"' "$(dirname "$OUT")/critic_report.schema.json" 2>/dev/null; then
  cat <<'JSON' > "$OUT"
{"schema_version":1,"passed":true,"checks":[{"trait_id":"zero-leaked-text","category":"zero_leaked_text","passed":true,"observation":"clean"}],"summary":"ok"}
JSON
  exit 0
fi
cat <<'JSON' > "$OUT"
{"schema_version":1,"id":"harbor-pier","title":"Harbor Pier","visibility":"public","camera":{"shot":"medium 35mm","angle":null,"framing":null},"depth_bands":[{"plane":"foreground","near_meters":1.0,"far_meters":4.0,"description":"stone pier"}],"foreground_slots":[],"style_ref":"franco-belgian-line","atmosphere_ref":"golden-dawn","action_summary":"Harbor pier bathed in morning sunlight","width":256,"height":256}
JSON
"##,
        )
        .unwrap();
        fs::set_permissions(&codex_stub, fs::Permissions::from_mode(0o755)).unwrap();

        (root, ws, auth_dir, codex_stub)
    }

    fn call_params(name: &'static str, args: serde_json::Value) -> CallToolRequestParams {
        let map = args.as_object().unwrap().clone();
        CallToolRequestParams::new(name).with_arguments(map)
    }

    fn extract_tool_json(result: &CallToolResult) -> serde_json::Value {
        let text = result
            .content
            .first()
            .and_then(|c| c.as_text())
            .expect("tool result text content");
        serde_json::from_str(&text.text).expect("valid JSON tool output")
    }

    #[tokio::test]
    async fn mcp_server_exposes_exact_8_tools_and_4_resources_and_executes_all_end_to_end() {
        let (_root, ws, auth_dir, codex_stub) = setup_mcp_test_workspace();
        let server = ImgforgeMcpServer::with_backend(
            Some(ws.clone()),
            auth_dir,
            codex_stub.to_string_lossy().into_owned(),
            Arc::new(StubMcpBackend),
        );

        let (server_io, client_io) = tokio::io::duplex(64 * 1024);
        let server_task = tokio::spawn(async move {
            let running = server.serve(server_io).await.unwrap();
            let _ = running.waiting().await;
        });

        let client = ().serve(client_io).await.unwrap();

        // Verify exact 8 tools
        let tools_res = client.list_tools(Default::default()).await.unwrap();
        let mut tool_names: Vec<String> = tools_res
            .tools
            .into_iter()
            .map(|t| t.name.into_owned())
            .collect();
        tool_names.sort();
        assert_eq!(
            tool_names,
            vec![
                "compose", "edit", "export", "generate", "guide", "pack", "promote", "validate"
            ]
        );

        // Verify exact 4 resources
        let resources_res = client.list_resources(Default::default()).await.unwrap();
        let mut uris: Vec<String> = resources_res
            .resources
            .into_iter()
            .map(|r| r.uri.clone())
            .collect();
        uris.sort();
        assert_eq!(
            uris,
            vec![
                "imgforge://manifest",
                "imgforge://runs",
                "imgforge://styles-atmospheres",
                "imgforge://subjects",
            ]
        );

        // 1. Tool `compose` (prose -> SceneSpec + Blockout recommendation)
        let composed = extract_tool_json(
            &client
                .call_tool(call_params(
                    "compose",
                    serde_json::json!({
                        "prose": "Harbor pier bathed in morning sunlight",
                        "recommend_blockout": true,
                        "out_path": "scenes/harbor.yaml",
                        "format": "yaml"
                    }),
                ))
                .await
                .unwrap(),
        );
        assert_eq!(composed["sceneSpec"]["id"], "harbor-pier");
        assert!(composed["blockoutRecommendation"]["variant"] == "Blockout");
        assert!(ws.join("scenes/harbor.yaml").is_file());

        // 2. Tool `guide` (render Blockout recommendation -> SpatialGuide PNG)
        let guided = extract_tool_json(
            &client
                .call_tool(call_params(
                    "guide",
                    serde_json::json!({
                        "action": "render",
                        "guide_spec": composed["blockoutRecommendation"],
                        "out_path": "guides/harbor_blockout.png"
                    }),
                ))
                .await
                .unwrap(),
        );
        assert_eq!(guided["width"], 256);
        assert!(ws.join("guides/harbor_blockout.png").is_file());

        // 3. Tool `pack` (generate ReferencePack + pack Atlas)
        let packed = extract_tool_json(
            &client
                .call_tool(call_params(
                    "pack",
                    serde_json::json!({
                        "action": "generate",
                        "subject_slug": "brass-lantern",
                        "style_slug": "franco-belgian-line"
                    }),
                ))
                .await
                .unwrap(),
        );
        assert_eq!(packed["manifest"]["slices"].as_array().unwrap().len(), 2);

        let atlas = extract_tool_json(
            &client
                .call_tool(call_params(
                    "pack",
                    serde_json::json!({
                        "action": "atlas",
                        "selections": [{
                            "subject_slug": "brass-lantern",
                            "style_slug": "franco-belgian-line",
                            "slice_ids": ["sheet_001"]
                        }],
                        "out_path": "exports/atlas.png"
                    }),
                ))
                .await
                .unwrap(),
        );
        assert_eq!(atlas["placements"].as_array().unwrap().len(), 1);

        // 4. Tool `generate` (0-ref explore + width/height override + SpatialGuide)
        let generated = extract_tool_json(
            &client
                .call_tool(call_params(
                    "generate",
                    serde_json::json!({
                        "scene_path": "scenes/harbor.yaml",
                        "explore": true,
                        "width": 128,
                        "height": 128,
                        "spatial_guide_path": "guides/harbor_blockout.png"
                    }),
                ))
                .await
                .unwrap(),
        );
        assert_eq!(generated["run_id"], "harbor-pier/v001");
        assert_eq!(generated["lifecycle"], "candidate");

        // 5. Tool `edit` (PatchEdit on generated Run)
        let edited = extract_tool_json(
            &client
                .call_tool(call_params(
                    "edit",
                    serde_json::json!({
                        "mode": "patch",
                        "target_image_path": "runs/harbor-pier/v001.png",
                        "slug": "harbor-pier-edit",
                        "prompt": "brighten brass railing",
                        "roi": { "x": 40, "y": 40, "width": 32, "height": 32 },
                        "prep_mode": "cue_outline"
                    }),
                ))
                .await
                .unwrap(),
        );
        assert_eq!(edited["runId"], "harbor-pier-edit/v001");
        assert_eq!(edited["outOfRoiMaxDeltaE2000"], 0.0);

        // 6. Tool `validate` (SceneSpec & Run with VisionCritic)
        let val_scene = extract_tool_json(
            &client
                .call_tool(call_params(
                    "validate",
                    serde_json::json!({
                        "target": "scene_spec",
                        "scene_path": "scenes/harbor.yaml"
                    }),
                ))
                .await
                .unwrap(),
        );
        assert_eq!(val_scene["valid"], true);

        let val_run = extract_tool_json(
            &client
                .call_tool(call_params(
                    "validate",
                    serde_json::json!({
                        "target": "run",
                        "slug": "harbor-pier",
                        "version": 1,
                        "atmosphere_slug": "golden-dawn",
                        "run_critic": true
                    }),
                ))
                .await
                .unwrap(),
        );
        assert_eq!(val_run["metadata"]["lifecycle"], "validated");

        // 7. Tool `promote` (Run -> Canonical + status)
        let promoted = extract_tool_json(
            &client
                .call_tool(call_params(
                    "promote",
                    serde_json::json!({
                        "action": "run",
                        "slug": "harbor-pier",
                        "version": 1
                    }),
                ))
                .await
                .unwrap(),
        );
        assert_eq!(promoted["metadata"]["lifecycle"], "canonical");
        assert_eq!(promoted["canonicalCount"], 1);

        let status = extract_tool_json(
            &client
                .call_tool(call_params(
                    "promote",
                    serde_json::json!({
                        "action": "status"
                    }),
                ))
                .await
                .unwrap(),
        );
        assert_eq!(status["lifecycleStatus"]["canonical_count"], 1);

        // 8. Tool `export` (web_webp from Canonical Run)
        let exported = extract_tool_json(
            &client
                .call_tool(call_params(
                    "export",
                    serde_json::json!({
                        "run_slug": "harbor-pier",
                        "run_version": 1,
                        "profile": "web_webp",
                        "out_dir": "exports/web"
                    }),
                ))
                .await
                .unwrap(),
        );
        assert_eq!(exported["artifacts"].as_array().unwrap().len(), 1);
        assert!(ws.join("exports/web/harbor-pier.webp").is_file());

        // Read all 4 MCP resources and verify live workspace state
        for uri in [
            RESOURCE_URI_MANIFEST,
            RESOURCE_URI_SUBJECTS,
            RESOURCE_URI_STYLES_ATMOSPHERES,
            RESOURCE_URI_RUNS,
        ] {
            let res = client
                .read_resource(ReadResourceRequestParams::new(uri))
                .await
                .unwrap();
            let text = match &res.contents[0] {
                ResourceContents::TextResourceContents { text, .. } => text,
                _ => panic!("expected text JSON resource for {uri}"),
            };
            let parsed: serde_json::Value = serde_json::from_str(text).unwrap();
            match uri {
                RESOURCE_URI_MANIFEST => {
                    assert!(parsed["canonical"]["run:harbor-pier"].is_object())
                }
                RESOURCE_URI_SUBJECTS => assert_eq!(parsed.as_array().unwrap().len(), 1),
                RESOURCE_URI_STYLES_ATMOSPHERES => {
                    assert!(parsed["styles"].as_array().unwrap().len() >= 5);
                    assert!(parsed["atmospheres"].as_array().unwrap().len() >= 4);
                }
                RESOURCE_URI_RUNS => {
                    assert!(!parsed["streams"].as_array().unwrap().is_empty());
                }
                _ => unreachable!(),
            }
        }

        // Verify path traversal is rejected fail-closed in MCP tools
        let traversal_err = client
            .call_tool(call_params(
                "guide",
                serde_json::json!({
                    "action": "validate",
                    "spec_path": "../outside.json"
                }),
            ))
            .await;
        assert!(traversal_err.is_err());

        client.cancel().await.unwrap();
        let _ = server_task.await;
    }
}
