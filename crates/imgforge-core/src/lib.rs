//! === IMGFORGE CORE LIBRARY ===
//!
//! PURPOSE:
//! Exposes the domain-agnostic core engine for `imgforge`: workspace config
//! discovery, tool-local Codex OAuth credential management, `SceneSpec` &
//! `PromptCompiler`, `ImageBackend` & `CodexNativeBackend`, `SceneComposer`,
//! and `VisionCritic`.

pub mod atlas;
pub mod atmosphere;
pub mod auth;
pub mod backend;
pub mod calibration;
pub mod color_qa;
pub mod composer;
pub mod config;
pub mod critic;
pub mod edit;
pub mod error;
pub mod export;
pub mod generate;
pub mod guide_renderer;
pub mod lifecycle;
pub mod presets;
pub mod prompt_compiler;
pub mod reference_pack;
pub mod registry;
pub mod scene_spec;
pub mod slicer;
pub mod store;
pub mod style;
pub mod subject;
pub mod validation;

pub use atlas::{
    AtlasPackRequest, AtlasPage, AtlasPlacement, AtlasResult, AtlasSliceSelection,
    DEFAULT_ATLAS_HEIGHT, DEFAULT_ATLAS_PADDING_PX, DEFAULT_ATLAS_WIDTH, MAX_ATLAS_SUBJECTS,
    pack_dynamic_atlas,
};
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
pub use calibration::{
    CALIBRATION_CELL_SIZE_PX, CALIBRATION_STRIP_HEIGHT_PX, CalibrationStripLayout,
    CalibrationSwatchCell, GRAY_WEDGE_L_STAR_STEPS, STUDIO_BG_HEX, STUDIO_BG_RGB, STUDIO_BG_RGBA,
    SheetStructuralReport, StructuralValidationConfig, composite_calibration_strip,
    composite_calibration_strip_png, compute_l_star_gray_wedge, parse_hex_rgb,
    validate_reference_sheet_structure,
};
pub use color_qa::{
    ColorInspectionTelemetry, ColorQaEngine, ColorQaReport, DEFAULT_CLIPPED_BLACK_L_STAR,
    DEFAULT_KMEANS_K, DEFAULT_L_STAR_HISTOGRAM_BINS, DEFAULT_SPARKLE_MAX_AREA_PX,
    DEFAULT_SPARKLE_MIN_AREA_PX, ExposureMetrics, ExposureQaConfig, ExposureReport,
    ForbiddenHueIntrusion, ForbiddenHueSector, HeatmapKind, LStarHistogramBin, OklabClusterSummary,
    OklabVectorscopePoint, PaletteAdherenceReport, PaletteQaConfig, SparkleBlob, SparkleConfig,
    SparkleReport, SubjectRoiSpec, UntouchedDriftConfig, UntouchedDriftReport,
    compute_l_star_histogram_and_vectorscope, delta_e_2000, oklab_to_lab, oklab_to_srgb_u8,
    srgb_delta_e_2000, srgb_u8_to_lab, srgb_u8_to_oklab,
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
pub use edit::{
    DEFAULT_COLLAR_WIDTH_PX, DEFAULT_PYRAMID_LEVELS, LinearBuffer, MAX_COLLAR_WIDTH_PX,
    MIN_COLLAR_WIDTH_PX, OwnedSubjectRoi, PatchEditConfig, PatchEditRequest, PatchEditResult,
    PatchPrepMode, PatchTargetMask, REINHARD_GAIN_MAX, REINHARD_GAIN_MIN, ReinhardGainSummary,
    WholeImageEditRequest, WholeImageEditResult, apply_collar_reinhard_oklab,
    build_laplacian_pyramid, composite_calibrated_patch, euclidean_distance_transform,
    execute_patch_edit, execute_whole_image_edit, laplacian_pyramid_blend,
    reconstruct_laplacian_pyramid,
};
pub use error::CoreError;
pub use export::{
    CubeLut, DEFAULT_CDF_DAMP_STRENGTH, DEFAULT_TOKEN_RING_HEX, DEFAULT_VTT_MAP_GRID_PX,
    DEFAULT_VTT_TOKEN_SIZE_PX, ExportProfile, ExportRequest, ExportResult, ExportSource,
    ExportedArtifact, MAX_CDF_DAMP_STRENGTH, MAX_VTT_TOKEN_SIZE_PX, MIN_CDF_DAMP_STRENGTH,
    MIN_VTT_TOKEN_SIZE_PX, MatchGradeOptions, VttMapOptions, VttTokenOptions,
    apply_damped_l_star_cdf_match, execute_export, render_vtt_token, split_grid_2x2,
};
pub use generate::{
    GenerateRequest, GenerateResult, GenerateSceneInput, GenerationMode, SubjectRoiBinding,
    execute_generate,
};
pub use guide_renderer::{
    ActionVector, DepthBand, GuideDepthPlane, GuideRenderer, GuideSpec, MAX_GUIDE_CANVAS_DIM,
    MannequinFacing, NormalizedPoint, OverlayRegion, OverlayRegionKind, RenderedSpatialGuide,
    SUPPORTED_GUIDE_SPEC_SCHEMA_VERSION, SpatialGuideVariant, SubjectMannequin, VectorStrokeStyle,
    build_recommended_blockout,
};
pub use lifecycle::{Lifecycle, SeedInput, now_utc_iso, sha256_hex, validate_slug};
pub use presets::{
    BUILTIN_ATMOSPHERE_SLUGS, BUILTIN_STYLE_SLUGS, builtin_atmosphere, builtin_atmospheres,
    builtin_style, builtin_style_exemplar_png, builtin_styles,
};
pub use prompt_compiler::{CompiledPrompt, PromptCompiler, compile_scene_spec_with_registry};
pub use reference_pack::{
    DiscoveredReferencePack, GeneratedReferencePack, GeneratedSheetRecord, REFERENCE_SHEET_HEIGHT,
    REFERENCE_SHEET_WIDTH, ReferencePackManifest, ReferencePackPipeline, ReferencePackRequest,
    ReferenceSheetRole, compile_pack_sheet_prompt, list_workspace_reference_packs,
    reslice_reference_pack_sheet,
};
pub use registry::{
    CARD_FILENAME, LibraryRegistry, LibrarySource, ResolvedAtmosphere, ResolvedStyle,
    ResolvedSubject, inspect_asset_image_visibility, load_and_verify_style_exemplars,
    resolve_user_config_dir, resolve_user_config_dir_from_env,
};
pub use scene_spec::{
    CameraSpec, DepthBandSpec, DepthPlane, ExclusiveSurfaceMap, ForegroundSlot, HandBinding,
    MAX_FOREGROUND_SLOTS, SceneSpec, SurfaceBinding, Visibility, check_visibility_compatibility,
};
pub use slicer::{
    ExtractedSlice, OverrideMode, PixelRect, RectOverride, SliceRecord, SliceSource, SlicerConfig,
    SlicesManifest, slice_sheet_image, slice_sheet_to_dir,
};
pub use store::{
    MANIFEST_LOCK_FILENAME, MAX_CUBE_LUT_BYTES, MAX_IMAGE_DECODE_ALLOC_BYTES, MAX_IMAGE_DIM,
    MAX_IMAGE_TOTAL_PIXELS, MAX_REGULAR_FILE_BYTES, MAX_TEXT_FILE_BYTES, ManifestEntry,
    ManifestEntryKind, ManifestLock, ManifestLockStore, RecordRunParams, RecordedRun,
    RunBackendMetadata, RunMetadata, RunQaReports, RunStore, decode_png_checked,
    read_regular_file_nofollow, read_text_file_nofollow_bounded, validate_strict_relative_path,
    verify_contained_path_no_symlinks, verify_existing_dir_prefix_no_symlinks,
    verify_optional_contained_dir_no_symlinks, write_atomic,
};
pub use style::{MAX_STYLE_EXEMPLARS, MIN_STYLE_EXEMPLARS, Style};
pub use subject::{PaletteLock, PaletteSwatch, Subject, SubjectHandBindings, SubjectKind};
pub use validation::{
    ValidateReferencePackRequest, ValidateReferencePackResult, ValidateRunRequest,
    ValidateRunResult, WorkspaceLifecycleStatus, inspect_workspace_status, promote_reference_pack,
    promote_run, validate_reference_pack, validate_run,
};
