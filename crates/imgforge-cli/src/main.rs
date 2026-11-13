//! === IMGFORGE CLI ENTRYPOINT ===
//!
//! PURPOSE:
//! Exposes the `imgforge` binary CLI (`auth`, `compose`, `validate`, `generate`,
//! `edit`, `guide`, `export`, `critic`, `library`, `pack`, `ui`, and `status`)
//! on top of `imgforge-core` and the embedded Web Canvas UI.
//!
//! THREAT MODEL / DESIGN RATIONALE ("WHY"):
//! Explicit `--workspace` flags fail closed when invalid rather than falling back to
//! `IMGFORGE_WORKSPACE` or CWD, while credential commands (`imgforge auth ...`) and
//! standalone UI launch (`imgforge ui`) operate cleanly even outside a workspace.
//!
//! EXECUTION LIFECYCLE / DATA FLOW:
//! 1. Parse CLI arguments via `clap`.
//! 2. Resolve optional workspace (`--workspace` > `IMGFORGE_WORKSPACE` > nearest `imgforge.toml`).
//! 3. Dispatch the requested subcommand and emit structured output or fail closed.

mod server;

use std::fs;
use std::path::PathBuf;

use anyhow::{Context, bail};
use clap::{Args, Parser, Subcommand, ValueEnum};
use imgforge_core::auth::DEFAULT_CODEX_BIN;
use imgforge_core::{
    AtlasPackRequest, AtlasSliceSelection, AuthManager, CodexNativeBackend, CoreError,
    DEFAULT_ATLAS_HEIGHT, DEFAULT_ATLAS_PADDING_PX, DEFAULT_ATLAS_WIDTH, DEFAULT_CDF_DAMP_STRENGTH,
    DEFAULT_COLLAR_WIDTH_PX, DEFAULT_TOKEN_RING_HEX, DEFAULT_VTT_MAP_GRID_PX,
    DEFAULT_VTT_TOKEN_SIZE_PX, ExportProfile, ExportRequest, ExportSource, ExposureQaConfig,
    GuideRenderer, GuideSpec, ImageBackend, ImageGenerationRequest, LibraryRegistry,
    ManifestLockStore, MatchGradeOptions, OwnedSubjectRoi, PaletteQaConfig, PatchEditConfig,
    PatchEditRequest, PatchPrepMode, PatchTargetMask, PixelRect, PromptCompiler,
    ReferencePackPipeline, ReferencePackRequest, ResolvedWorkspace, RunStore, SceneComposer,
    SceneSpec, SlicerConfig, SparkleConfig, TraitCategory, TraitCheckInput, UntouchedDriftConfig,
    Visibility, VisionCritic, VisionCriticRequest, VttMapOptions, VttTokenOptions,
    WholeImageEditRequest, check_visibility_compatibility, execute_export, execute_patch_edit,
    execute_whole_image_edit, pack_dynamic_atlas, resolve_codex_auth_dir_from_env,
    resolve_user_config_dir_from_env, resolve_workspace_from_env, slice_sheet_to_dir, write_atomic,
};
use serde::Serialize;

use crate::server::{AppState, serve_ui};

/// Standalone domain-agnostic visual consistency engine CLI.
#[derive(Debug, Parser)]
#[command(
    name = "imgforge",
    version,
    about = "Standalone visual consistency engine"
)]
struct Cli {
    /// Override workspace root directory containing imgforge.toml.
    #[arg(long, global = true, value_name = "PATH")]
    workspace: Option<PathBuf>,

    /// Override path or executable name for `codex` (also via IMGFORGE_CODEX_BIN).
    #[arg(long, global = true, value_name = "PATH")]
    codex_bin: Option<String>,

    /// Subcommand to execute.
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Manage tool-local ChatGPT Plus OAuth credentials (isolated from ~/.codex).
    Auth(AuthArgs),
    /// Translate unstructured prose into a validated SceneSpec via read-only SceneComposer.
    Compose(ComposeArgs),
    /// Deterministically validate a SceneSpec file via PromptCompiler.
    Validate(ValidateArgs),
    /// Compile a SceneSpec and generate a verified PNG via CodexNativeBackend.
    Generate(GenerateArgs),
    /// Run WholeImageEdit or PatchEdit with OKLab/DeltaE_2000 Color QA and Run recording.
    Edit(EditArgs),
    /// Validate or deterministically render a SpatialGuide from GuideSpec JSON.
    Guide(GuideArgs),
    /// Derive deterministic VTT/Web/Grid Export deliverables from a Run or PNG asset.
    Export(ExportArgs),
    /// Run structured read-only VisionCritic checklist evaluation on a generated image.
    Critic(CriticArgs),
    /// Inspect and verify Subject, Style, Atmosphere, and manifest.lock.json entries.
    Library(LibraryArgs),
    /// Generate, slice, and dynamically pack style-scoped ReferencePack sheets and Atlases.
    Pack(PackArgs),
    /// Start the embedded Web Canvas SPA server strictly on 127.0.0.1:<port>.
    Ui(UiArgs),
    /// Inspect resolved workspace configuration and tool-local auth readiness.
    Status(StatusArgs),
}

#[derive(Debug, Args)]
struct AuthArgs {
    #[command(subcommand)]
    action: AuthSubcommand,
}

#[derive(Debug, Subcommand)]
enum AuthSubcommand {
    /// Provision tool-local ChatGPT Plus OAuth credentials via `codex login`.
    Login {
        /// Use headless device-code OAuth flow (`codex login --device-auth`).
        #[arg(long)]
        device_auth: bool,
    },
    /// Verify tool-local `auth.json` integrity and `codex login status`.
    Status,
    /// Remove tool-local `auth.json` credentials.
    Logout,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum OutputFormat {
    Yaml,
    Json,
}

#[derive(Debug, Args)]
struct ComposeArgs {
    /// Inline unstructured scene description prose.
    #[arg(long, conflicts_with = "prose_file")]
    prose: Option<String>,
    /// Path to a text file containing unstructured scene prose.
    #[arg(long, value_name = "PATH")]
    prose_file: Option<PathBuf>,
    /// Optional output path for the validated SceneSpec.
    #[arg(long, value_name = "PATH")]
    out: Option<PathBuf>,
    /// Serialization format for the emitted SceneSpec.
    #[arg(long, value_enum, default_value_t = OutputFormat::Yaml)]
    format: OutputFormat,
}

#[derive(Debug, Args)]
struct ValidateArgs {
    /// Path to the SceneSpec YAML or JSON file.
    scene_spec: PathBuf,
}

#[derive(Debug, Args)]
struct GenerateArgs {
    /// Path to the SceneSpec YAML or JSON file.
    #[arg(long, value_name = "PATH")]
    scene: PathBuf,
    /// Output PNG destination path.
    #[arg(long, value_name = "PATH")]
    out: PathBuf,
    /// Override output width in pixels.
    #[arg(long)]
    width: Option<u32>,
    /// Override output height in pixels.
    #[arg(long)]
    height: Option<u32>,
    /// Reference image path(s) forwarded to `codex exec -i <FILE>`.
    #[arg(short = 'i', long = "ref", value_name = "FILE")]
    references: Vec<PathBuf>,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum CliPatchPrepMode {
    CropOnly,
    BlankInsideMask,
    CueOutline,
}

impl From<CliPatchPrepMode> for PatchPrepMode {
    fn from(val: CliPatchPrepMode) -> Self {
        match val {
            CliPatchPrepMode::CropOnly => Self::CropOnly,
            CliPatchPrepMode::BlankInsideMask => Self::BlankInsideMask,
            CliPatchPrepMode::CueOutline => Self::CueOutline,
        }
    }
}

#[derive(Debug, Args)]
struct EditArgs {
    #[command(subcommand)]
    action: EditSubcommand,
}

#[derive(Debug, Subcommand)]
enum EditSubcommand {
    /// WholeImageEdit: global or multi-region edit with fail-closed ColorQaEngine gating.
    Whole {
        /// Target PNG image path to edit.
        #[arg(long, value_name = "PATH")]
        target: PathBuf,
        /// Target Run stream slug.
        #[arg(long)]
        slug: String,
        /// Positive-only compiled edit prompt.
        #[arg(long)]
        prompt: String,
        /// Access classification for the recorded Run.
        #[arg(long, value_enum, default_value_t = CliVisibility::Public)]
        visibility: CliVisibility,
        /// Optional Atmosphere slug enforcing exposure_floor.
        #[arg(long)]
        atmosphere: Option<String>,
        /// Optional Subject slug(s) enforcing PaletteLock adherence.
        #[arg(long = "subject")]
        subjects: Vec<String>,
        /// Reference anchor image path(s).
        #[arg(short = 'i', long = "ref", value_name = "FILE")]
        references: Vec<PathBuf>,
        /// Record the Draft Run even when fail-closed Color QA rejects it (default fails closed).
        #[arg(long)]
        allow_qa_fail: bool,
        /// Emit machine-readable JSON summary.
        #[arg(long)]
        json: bool,
    },
    /// PatchEdit: 1:1 square crop + collar OKLab Reinhard transfer + 5-level Laplacian blend.
    Patch {
        /// Target PNG image path to edit.
        #[arg(long, value_name = "PATH")]
        target: PathBuf,
        /// Target Run stream slug.
        #[arg(long)]
        slug: String,
        /// Positive-only localized edit prompt.
        #[arg(long)]
        prompt: String,
        /// Access classification for the recorded Run.
        #[arg(long, value_enum, default_value_t = CliVisibility::Public)]
        visibility: CliVisibility,
        /// ROI bounding box formatted as `x,y,width,height`.
        #[arg(long, value_name = "X,Y,W,H", conflicts_with = "mask")]
        roi: Option<String>,
        /// Alpha/grayscale PNG mask path.
        #[arg(long, value_name = "PATH", conflicts_with = "roi")]
        mask: Option<PathBuf>,
        /// Optional pre-edit target preparation mode.
        #[arg(long, value_enum, default_value_t = CliPatchPrepMode::CropOnly)]
        prep: CliPatchPrepMode,
        /// Context collar ring width in pixels (16..=32).
        #[arg(long, default_value_t = DEFAULT_COLLAR_WIDTH_PX)]
        collar_width: u32,
        /// Cosine distance-transform feather radius in pixels.
        #[arg(long, default_value_t = 12)]
        feather: u32,
        /// Additional reference anchor image path(s).
        #[arg(short = 'i', long = "ref", value_name = "FILE")]
        references: Vec<PathBuf>,
        /// Emit machine-readable JSON summary.
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Args)]
struct GuideArgs {
    #[command(subcommand)]
    action: GuideSubcommand,
}

#[derive(Debug, Subcommand)]
enum GuideSubcommand {
    /// Validate a GuideSpec JSON document against the canonical schema.
    Validate {
        /// Path to the GuideSpec JSON file.
        spec: PathBuf,
    },
    /// Deterministically render an OverlayGuide or Blockout SpatialGuide PNG.
    Render {
        /// Path to the GuideSpec JSON file.
        #[arg(long, value_name = "PATH")]
        spec: PathBuf,
        /// Output PNG destination path.
        #[arg(long, value_name = "PATH")]
        out: PathBuf,
        /// Emit machine-readable JSON summary.
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum CliExportProfile {
    VttToken,
    VttMap,
    WebWebp,
    Grid2x2,
}

#[derive(Debug, Args)]
struct ExportArgs {
    /// Source recorded Run formatted as `slug:version` (e.g. `harbor-dock:1` or `harbor-dock:v001`).
    #[arg(long, value_name = "SLUG:VER", conflicts_with = "input")]
    run: Option<String>,
    /// Direct source PNG path (requires --slug or uses file stem).
    #[arg(long, value_name = "PATH", conflicts_with = "run")]
    input: Option<PathBuf>,
    /// Source Visibility classification when exporting via --input.
    #[arg(long, value_enum, default_value_t = CliVisibility::Public)]
    source_visibility: CliVisibility,
    /// Export profile to produce.
    #[arg(long, value_enum)]
    profile: CliExportProfile,
    /// Allowed Visibility of the export destination (fails closed on leaks).
    #[arg(long, value_enum, default_value_t = CliVisibility::Public)]
    visibility: CliVisibility,
    /// Output directory receiving deterministic deliverables.
    #[arg(long, value_name = "DIR")]
    out_dir: PathBuf,
    /// Output basename slug (defaults to Run slug or input stem).
    #[arg(long)]
    slug: Option<String>,
    /// Square size in pixels for vtt-token.
    #[arg(long, default_value_t = DEFAULT_VTT_TOKEN_SIZE_PX)]
    token_size: u32,
    /// Role-colored outer ring #RRGGBB hex for vtt-token.
    #[arg(long, default_value = DEFAULT_TOKEN_RING_HEX)]
    ring_color: String,
    /// Optional flat-backdrop #RRGGBB hex to also emit a vtt-token JPEG copy.
    #[arg(long)]
    jpeg_backdrop: Option<String>,
    /// Grid column count for vtt-map.
    #[arg(long, default_value_t = 24)]
    cols: u32,
    /// Grid row count for vtt-map.
    #[arg(long, default_value_t = 18)]
    rows: u32,
    /// Pixels per grid cell for vtt-map when --scale-factor is omitted.
    #[arg(long, default_value_t = DEFAULT_VTT_MAP_GRID_PX)]
    grid_px: u32,
    /// Optional explicit Lanczos3 upscale multiplier for vtt-map.
    #[arg(long)]
    scale_factor: Option<f64>,
    /// JPEG quality (1..=100) for vtt-map and optional vtt-token JPEG.
    #[arg(long, default_value_t = 90)]
    jpeg_quality: u8,
    /// Opt-in explicit color grading (ONLY supported on export).
    #[arg(long)]
    match_grade: bool,
    /// Reference PNG path for damped L* CDF matching (requires --match-grade).
    #[arg(long, value_name = "PATH", requires = "match_grade")]
    grade_ref: Option<PathBuf>,
    /// Damped L* CDF blend strength in 0.25..=0.45 (requires --match-grade).
    #[arg(long, default_value_t = DEFAULT_CDF_DAMP_STRENGTH, requires = "match_grade")]
    grade_strength: f64,
    /// Adobe .cube 3D LUT file path (requires --match-grade).
    #[arg(long, value_name = "PATH", requires = "match_grade")]
    lut: Option<PathBuf>,
    /// Emit machine-readable JSON summary.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct CriticArgs {
    /// Target generated PNG image path.
    #[arg(long, value_name = "PATH")]
    image: PathBuf,
    /// Reference anchor image path(s).
    #[arg(short = 'i', long = "ref", value_name = "PATH")]
    references: Vec<PathBuf>,
    /// Trait check specification formatted as `id:category:criterion` or plain `criterion`.
    #[arg(long = "trait", value_name = "SPEC", required = true)]
    traits: Vec<String>,
}

#[derive(Debug, Args)]
struct UiArgs {
    /// Port on 127.0.0.1 (overrides imgforge.toml server.port; defaults to 8080).
    #[arg(long)]
    port: Option<u16>,
}

#[derive(Debug, Args)]
struct StatusArgs {
    /// Emit machine-readable JSON output.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum LibraryResourceKind {
    Subject,
    Style,
    Atmosphere,
}

#[derive(Debug, Args)]
struct LibraryArgs {
    #[command(subcommand)]
    action: LibrarySubcommand,
}

#[derive(Debug, Subcommand)]
enum LibrarySubcommand {
    /// List resolved Subject, Style, and Atmosphere resources across project > user > builtin.
    List {
        /// Filter to a single library resource kind.
        #[arg(long, value_enum)]
        kind: Option<LibraryResourceKind>,
        /// Emit machine-readable JSON output.
        #[arg(long)]
        json: bool,
    },
    /// Show a single resolved Subject, Style, or Atmosphere card.
    Show {
        /// Library resource kind to inspect.
        #[arg(value_enum)]
        kind: LibraryResourceKind,
        /// Resource slug.
        slug: String,
        /// Emit machine-readable JSON output.
        #[arg(long)]
        json: bool,
    },
    /// Verify SHA-256 checksums of all canonical entries in library/manifest.lock.json.
    VerifyLock,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum CliVisibility {
    Public,
    Restricted,
    Internal,
}

impl From<CliVisibility> for Visibility {
    fn from(val: CliVisibility) -> Self {
        match val {
            CliVisibility::Public => Self::Public,
            CliVisibility::Restricted => Self::Restricted,
            CliVisibility::Internal => Self::Internal,
        }
    }
}

#[derive(Debug, Args)]
struct PackArgs {
    #[command(subcommand)]
    action: PackSubcommand,
}

#[derive(Debug, Subcommand)]
enum PackSubcommand {
    /// Generate a style-scoped ReferencePack for a Subject and Style.
    Generate {
        /// Subject slug.
        #[arg(long)]
        subject: String,
        /// Style slug.
        #[arg(long)]
        style: String,
        /// Optional bootstrap Seed PNG path.
        #[arg(long, value_name = "PATH")]
        seed: Option<PathBuf>,
        /// Optional Seed label.
        #[arg(long)]
        seed_label: Option<String>,
        /// Emit machine-readable JSON summary.
        #[arg(long)]
        json: bool,
    },
    /// Slice foreground components against #252830 from a ReferencePack sheet PNG.
    Slice {
        /// Path to the sheet PNG image.
        #[arg(long, value_name = "PATH")]
        sheet: PathBuf,
        /// Output directory receiving slices/*.png and slices.json.
        #[arg(long, value_name = "PATH")]
        out_dir: PathBuf,
        /// Bottom calibration strip height in pixels to exclude from slicing.
        #[arg(long, default_value_t = 72)]
        strip_height: u32,
        /// Emit machine-readable JSON manifest.
        #[arg(long)]
        json: bool,
    },
    /// Dynamically pack selected ReferencePack slices into a #252830 Atlas PNG.
    Atlas {
        /// Output PNG path (multi-page atlases append _pN before .png).
        #[arg(long, value_name = "PATH")]
        out: PathBuf,
        /// Slice selector formatted as `subject_slug:style_slug:slice_id1[,slice_id2...]`.
        #[arg(long = "slice", value_name = "SPEC", required = true)]
        slices: Vec<String>,
        /// Target Visibility classification for the Atlas.
        #[arg(long, value_enum, default_value_t = CliVisibility::Public)]
        visibility: CliVisibility,
        /// Target Atlas canvas width in pixels.
        #[arg(long, default_value_t = DEFAULT_ATLAS_WIDTH)]
        width: u32,
        /// Target Atlas canvas height in pixels.
        #[arg(long, default_value_t = DEFAULT_ATLAS_HEIGHT)]
        height: u32,
        /// Emit machine-readable JSON placement report.
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Serialize)]
struct LibrarySummaryRow {
    kind: &'static str,
    slug: String,
    display_name: String,
    source: imgforge_core::LibrarySource,
}

fn parse_atlas_selection(raw: &str) -> anyhow::Result<AtlasSliceSelection> {
    let parts: Vec<&str> = raw.splitn(3, ':').collect();
    if parts.len() != 3 {
        bail!("--slice '{raw}' must be formatted as subject_slug:style_slug:slice_id1[,slice_id2]");
    }
    let slice_ids: Vec<String> = parts[2]
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    if slice_ids.is_empty() {
        bail!("--slice '{raw}' must include at least one slice_id");
    }
    Ok(AtlasSliceSelection {
        subject_slug: parts[0].trim().to_string(),
        style_slug: parts[1].trim().to_string(),
        slice_ids,
    })
}

#[derive(Debug, Serialize)]
struct WorkspaceStatusSummary {
    workspace_root: PathBuf,
    config_path: PathBuf,
    resolution_source: imgforge_core::WorkspaceResolutionSource,
    schema_version: u32,
    workspace_name: Option<String>,
    backend: imgforge_core::BackendKind,
    auth_dir: PathBuf,
    auth_provisioned: bool,
    server_port: u16,
}

fn resolve_optional_workspace(
    explicit_workspace: Option<&PathBuf>,
) -> Result<Option<ResolvedWorkspace>, CoreError> {
    match resolve_workspace_from_env(explicit_workspace.map(PathBuf::as_path)) {
        Ok(ws) => Ok(Some(ws)),
        Err(CoreError::WorkspaceNotFound { .. }) if explicit_workspace.is_none() => Ok(None),
        Err(err) => Err(err),
    }
}

fn resolve_codex_bin(explicit_flag: Option<&str>, env_val: Option<&str>) -> anyhow::Result<String> {
    if let Some(flag) = explicit_flag {
        if flag.trim().is_empty() {
            bail!("--codex-bin must not be empty");
        }
        return Ok(flag.to_string());
    }
    if let Some(env_raw) = env_val
        && !env_raw.trim().is_empty()
    {
        return Ok(env_raw.to_string());
    }
    Ok(DEFAULT_CODEX_BIN.to_string())
}

fn parse_roi_arg(raw: &str) -> anyhow::Result<PixelRect> {
    let parts: Vec<&str> = raw.split(',').collect();
    if parts.len() != 4 {
        bail!("--roi '{raw}' must be formatted as x,y,width,height");
    }
    let x: u32 = parts[0].trim().parse().context("parsing ROI x")?;
    let y: u32 = parts[1].trim().parse().context("parsing ROI y")?;
    let width: u32 = parts[2].trim().parse().context("parsing ROI width")?;
    let height: u32 = parts[3].trim().parse().context("parsing ROI height")?;
    if width == 0 || height == 0 {
        bail!("--roi width and height must both be > 0");
    }
    Ok(PixelRect {
        x,
        y,
        width,
        height,
    })
}

fn parse_run_ref_arg(raw: &str) -> anyhow::Result<(String, u32)> {
    let (slug, ver_raw) = raw
        .split_once(':')
        .ok_or_else(|| anyhow::anyhow!("--run '{raw}' must be formatted as slug:version"))?;
    let trimmed_slug = slug.trim();
    if trimmed_slug.is_empty() {
        bail!("--run '{raw}' slug must not be empty");
    }
    let digits = ver_raw.trim().strip_prefix('v').unwrap_or(ver_raw.trim());
    let version: u32 = digits
        .parse()
        .with_context(|| format!("parsing version in --run '{raw}'"))?;
    if version == 0 {
        bail!("--run version must be >= 1");
    }
    Ok((trimmed_slug.to_string(), version))
}

fn parse_trait_arg(index: usize, raw: &str) -> anyhow::Result<TraitCheckInput> {
    let parts: Vec<&str> = raw.splitn(3, ':').collect();
    if parts.len() == 3 {
        let category = parts[1].parse::<TraitCategory>()?;
        Ok(TraitCheckInput {
            trait_id: parts[0].trim().to_string(),
            category,
            criterion: parts[2].trim().to_string(),
        })
    } else {
        Ok(TraitCheckInput {
            trait_id: format!("trait-{}", index + 1),
            category: TraitCategory::Identity,
            criterion: raw.trim().to_string(),
        })
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();
    let auth_dir = resolve_codex_auth_dir_from_env()?;
    let env_codex_bin = std::env::var("IMGFORGE_CODEX_BIN").ok();
    let codex_bin = resolve_codex_bin(cli.codex_bin.as_deref(), env_codex_bin.as_deref())?;

    match cli.command {
        Commands::Auth(args) => {
            // `imgforge auth` intentionally never reads workspace `imgforge.toml`.
            let manager = AuthManager::new(auth_dir, codex_bin);

            match args.action {
                AuthSubcommand::Login { device_auth } => {
                    let auth_file = manager.login(device_auth).await?;
                    println!(
                        "Provisioned tool-local Codex OAuth credentials at {}",
                        auth_file.display()
                    );
                }
                AuthSubcommand::Status => {
                    let report = manager.status().await?;
                    println!("auth_file: {}", report.auth_file.display());
                    println!("authenticated: {}", report.authenticated);
                    println!("detail: {}", report.detail);
                }
                AuthSubcommand::Logout => {
                    let removed = manager.logout()?;
                    if removed {
                        println!(
                            "Removed tool-local Codex credentials from {}",
                            manager.auth_file_path().display()
                        );
                    } else {
                        println!(
                            "No tool-local Codex credentials present at {}",
                            manager.auth_file_path().display()
                        );
                    }
                }
            }
        }
        Commands::Compose(args) => {
            let _ws = resolve_optional_workspace(cli.workspace.as_ref())?;

            let prose = match (args.prose, args.prose_file) {
                (Some(inline), None) => inline,
                (None, Some(path)) => fs::read_to_string(&path)
                    .with_context(|| format!("reading prose file {}", path.display()))?,
                (None, None) => bail!("either --prose or --prose-file must be provided"),
                (Some(_), Some(_)) => unreachable!("clap conflicts_with prevents both"),
            };

            let composer = SceneComposer::with_codex_bin(auth_dir, codex_bin);
            let composed = composer.compose(&prose).await?;
            let rendered = match args.format {
                OutputFormat::Yaml => composed.spec.to_yaml_string()?,
                OutputFormat::Json => composed.spec.to_json_string()?,
            };

            if let Some(out_path) = args.out {
                fs::write(&out_path, &rendered)
                    .with_context(|| format!("writing SceneSpec to {}", out_path.display()))?;
                println!("Wrote validated SceneSpec to {}", out_path.display());
            } else {
                println!("{rendered}");
            }
        }
        Commands::Validate(args) => {
            let _ws = resolve_optional_workspace(cli.workspace.as_ref())?;

            let spec = SceneSpec::load_from_path(&args.scene_spec)?;
            let compiled = PromptCompiler::compile(&spec)?;
            println!(
                "SceneSpec '{}' valid (visibility={}, slots={}, target={}x{})",
                spec.id,
                compiled.visibility.as_str(),
                compiled.foreground_slot_count,
                compiled.target_width,
                compiled.target_height
            );
            println!("\n{}", compiled.prompt);
        }
        Commands::Generate(args) => {
            let _ws = resolve_optional_workspace(cli.workspace.as_ref())?;

            let spec = SceneSpec::load_from_path(&args.scene)?;
            let compiled = PromptCompiler::compile(&spec)?;
            let width = args.width.unwrap_or(compiled.target_width);
            let height = args.height.unwrap_or(compiled.target_height);

            let backend = CodexNativeBackend::with_codex_bin(auth_dir, codex_bin);
            let output = backend
                .generate(&ImageGenerationRequest {
                    prompt: compiled.prompt,
                    width,
                    height,
                    reference_images: args.references,
                })
                .await?;

            if let Some(parent) = args.out.parent()
                && !parent.as_os_str().is_empty()
            {
                fs::create_dir_all(parent).with_context(|| {
                    format!("creating output parent directory {}", parent.display())
                })?;
            }
            fs::write(&args.out, &output.png_bytes)
                .with_context(|| format!("writing PNG to {}", args.out.display()))?;
            println!(
                "Generated {}x{} PNG (raw {}x{}, thread_id={:?}) at {}",
                output.width,
                output.height,
                output.raw_width,
                output.raw_height,
                output.thread_id,
                args.out.display()
            );
        }
        Commands::Edit(args) => {
            let resolved_ws = resolve_workspace_from_env(cli.workspace.as_deref())?;
            let runs_dir = resolved_ws.root.join(&resolved_ws.config.paths.runs_dir);
            let run_store = RunStore::new(runs_dir);
            let registry = LibraryRegistry::from_workspace(&resolved_ws);
            let backend = CodexNativeBackend::with_codex_bin(auth_dir, codex_bin);

            match args.action {
                EditSubcommand::Whole {
                    target,
                    slug,
                    prompt,
                    visibility,
                    atmosphere,
                    subjects,
                    references,
                    allow_qa_fail,
                    json,
                } => {
                    let exposure_floor = match atmosphere {
                        Some(atmo_slug) => {
                            let resolved = registry.resolve_atmosphere(&atmo_slug)?;
                            Some((
                                resolved.atmosphere.exposure_floor,
                                ExposureQaConfig::default(),
                            ))
                        }
                        None => None,
                    };
                    let target_visibility: Visibility = visibility.into();
                    let mut run_vis_checks: Vec<(String, Visibility)> = Vec::new();
                    if let Some(run_vis) = run_store.inspect_run_image_visibility(&target)? {
                        run_vis_checks.push((target.display().to_string(), run_vis));
                    }
                    for ref_path in &references {
                        if let Some(run_vis) = run_store.inspect_run_image_visibility(ref_path)? {
                            run_vis_checks.push((ref_path.display().to_string(), run_vis));
                        }
                    }
                    check_visibility_compatibility(
                        target_visibility,
                        "WholeImageEdit",
                        run_vis_checks.iter().map(|(id, vis)| (id.as_str(), *vis)),
                    )?;
                    let mut subject_rois = Vec::with_capacity(subjects.len());
                    for subj_slug in &subjects {
                        let resolved = registry.resolve_subject(subj_slug)?;
                        check_visibility_compatibility(
                            target_visibility,
                            "WholeImageEdit",
                            [(resolved.subject.slug.as_str(), resolved.subject.visibility)],
                        )?;
                        subject_rois.push(OwnedSubjectRoi {
                            subject_slug: resolved.subject.slug,
                            visibility: resolved.subject.visibility,
                            palette_lock: resolved.subject.palette_lock,
                            rois: Vec::new(),
                        });
                    }
                    let res = execute_whole_image_edit(
                        &backend,
                        &run_store,
                        &WholeImageEditRequest {
                            slug,
                            visibility: target_visibility,
                            target_image_path: target,
                            compiled_prompt: prompt,
                            reference_images: references,
                            exposure_floor,
                            subject_rois,
                            palette_config: PaletteQaConfig::default(),
                            edit_mask: None,
                            untouched_drift_config: UntouchedDriftConfig::default(),
                            allowed_sparkle_mask: None,
                            sparkle_config: SparkleConfig::default(),
                            fail_closed_on_qa: !allow_qa_fail,
                        },
                    )
                    .await?;

                    if json {
                        println!(
                            "{}",
                            serde_json::to_string_pretty(&serde_json::json!({
                                "runId": res.run.metadata.run_id,
                                "imagePath": res.run.image_path,
                                "qaReport": res.qa_report,
                            }))?
                        );
                    } else {
                        println!(
                            "WholeImageEdit recorded Run '{}' (qa_passed={}) at {}",
                            res.run.metadata.run_id,
                            res.qa_report.passed,
                            res.run.image_path.display()
                        );
                    }
                }
                EditSubcommand::Patch {
                    target,
                    slug,
                    prompt,
                    visibility,
                    roi,
                    mask,
                    prep,
                    collar_width,
                    feather,
                    references,
                    json,
                } => {
                    let target_visibility: Visibility = visibility.into();
                    let mut run_vis_checks: Vec<(String, Visibility)> = Vec::new();
                    if let Some(run_vis) = run_store.inspect_run_image_visibility(&target)? {
                        run_vis_checks.push((target.display().to_string(), run_vis));
                    }
                    for ref_path in &references {
                        if let Some(run_vis) = run_store.inspect_run_image_visibility(ref_path)? {
                            run_vis_checks.push((ref_path.display().to_string(), run_vis));
                        }
                    }
                    check_visibility_compatibility(
                        target_visibility,
                        "PatchEdit",
                        run_vis_checks.iter().map(|(id, vis)| (id.as_str(), *vis)),
                    )?;
                    let target_mask = match (roi, mask) {
                        (Some(raw_roi), None) => PatchTargetMask::Roi(parse_roi_arg(&raw_roi)?),
                        (None, Some(mask_path)) => PatchTargetMask::MaskImagePath(mask_path),
                        (None, None) => bail!("either --roi X,Y,W,H or --mask PATH is required"),
                        (Some(_), Some(_)) => unreachable!("clap conflicts_with prevents both"),
                    };
                    let res = execute_patch_edit(
                        &backend,
                        &run_store,
                        &PatchEditRequest {
                            slug,
                            visibility: target_visibility,
                            target_image_path: target,
                            target_mask,
                            compiled_prompt: prompt,
                            reference_images: references,
                            config: PatchEditConfig {
                                collar_width_px: collar_width,
                                feather_px: feather,
                                prep_mode: prep.into(),
                                ..PatchEditConfig::default()
                            },
                        },
                    )
                    .await?;

                    if json {
                        println!(
                            "{}",
                            serde_json::to_string_pretty(&serde_json::json!({
                                "runId": res.run.metadata.run_id,
                                "imagePath": res.run.image_path,
                                "cropRect": res.crop_rect,
                                "roiRect": res.roi_rect,
                                "reinhardGains": res.reinhard_gains,
                                "outOfRoiMaxDeltaE2000": res.out_of_roi_report.max_delta_e2000,
                            }))?
                        );
                    } else {
                        println!(
                            "PatchEdit recorded Run '{}' (crop={}x{}+{}+{}, out_of_roi max DeltaE_2000={:.4}) at {}",
                            res.run.metadata.run_id,
                            res.crop_rect.width,
                            res.crop_rect.height,
                            res.crop_rect.x,
                            res.crop_rect.y,
                            res.out_of_roi_report.max_delta_e2000,
                            res.run.image_path.display()
                        );
                    }
                }
            }
        }
        Commands::Guide(args) => {
            let ws = resolve_optional_workspace(cli.workspace.as_ref())?;
            match args.action {
                GuideSubcommand::Validate { spec } => {
                    let raw = fs::read_to_string(&spec)
                        .with_context(|| format!("reading GuideSpec at {}", spec.display()))?;
                    let parsed = GuideSpec::from_json_str(&raw)?;
                    println!(
                        "GuideSpec '{}' valid (variant={:?}, canvas={}x{}, bands={}, mannequins={}, vectors={}, regions={})",
                        parsed.id,
                        parsed.variant,
                        parsed.canvas_width,
                        parsed.canvas_height,
                        parsed.depth_bands.len(),
                        parsed.mannequins.len(),
                        parsed.action_vectors.len(),
                        parsed.overlay_regions.len()
                    );
                }
                GuideSubcommand::Render { spec, out, json } => {
                    let raw = fs::read_to_string(&spec)
                        .with_context(|| format!("reading GuideSpec at {}", spec.display()))?;
                    let parsed = GuideSpec::from_json_str(&raw)?;
                    let ws_root = match ws {
                        Some(resolved) => resolved.root,
                        None => std::env::current_dir()
                            .context("resolving current working directory")?,
                    };
                    let rendered = GuideRenderer::new().render(&parsed, &ws_root)?;
                    write_atomic(&out, &rendered.png_bytes, "SpatialGuide PNG").with_context(
                        || format!("writing SpatialGuide PNG to {}", out.display()),
                    )?;
                    if json {
                        println!(
                            "{}",
                            serde_json::to_string_pretty(&serde_json::json!({
                                "id": parsed.id,
                                "variant": parsed.variant,
                                "width": rendered.width,
                                "height": rendered.height,
                                "outPath": out,
                            }))?
                        );
                    } else {
                        println!(
                            "Rendered {:?} SpatialGuide ({}x{}) at {}",
                            rendered.variant,
                            rendered.width,
                            rendered.height,
                            out.display()
                        );
                    }
                }
            }
        }
        Commands::Export(args) => {
            let ws = resolve_optional_workspace(cli.workspace.as_ref())?;
            let run_store = ws
                .as_ref()
                .map(|w| RunStore::new(w.root.join(&w.config.paths.runs_dir)));

            let (source, default_slug) = match (args.run, args.input) {
                (Some(run_ref), None) => {
                    if run_store.is_none() {
                        let resolved_ws = resolve_workspace_from_env(cli.workspace.as_deref())?;
                        let _ = resolved_ws;
                    }
                    let (slug, version) = parse_run_ref_arg(&run_ref)?;
                    (
                        ExportSource::Run {
                            slug: slug.clone(),
                            version,
                        },
                        slug,
                    )
                }
                (None, Some(path)) => {
                    let mut effective_source_vis: Visibility = args.source_visibility.into();
                    if let Some(ref store) = run_store
                        && let Some(run_vis) = store.inspect_run_image_visibility(&path)?
                    {
                        effective_source_vis = std::cmp::max(effective_source_vis, run_vis);
                    }
                    let stem = path
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("export")
                        .to_string();
                    (
                        ExportSource::PngPath {
                            path,
                            asset_id: stem.clone(),
                            visibility: effective_source_vis,
                        },
                        stem,
                    )
                }
                (None, None) => bail!("either --run SLUG:VER or --input PATH must be provided"),
                (Some(_), Some(_)) => unreachable!("clap conflicts_with prevents both"),
            };

            let output_slug = args.slug.unwrap_or(default_slug);
            let profile = match args.profile {
                CliExportProfile::VttToken => ExportProfile::VttToken(VttTokenOptions {
                    size_px: args.token_size,
                    ring_color_hex: args.ring_color,
                    jpeg_backdrop_hex: args.jpeg_backdrop,
                    jpeg_quality: args.jpeg_quality,
                    ..VttTokenOptions::default()
                }),
                CliExportProfile::VttMap => ExportProfile::VttMap(VttMapOptions {
                    cols: args.cols,
                    rows: args.rows,
                    grid_px: args.grid_px,
                    scale_factor: args.scale_factor,
                    jpeg_quality: args.jpeg_quality,
                }),
                CliExportProfile::WebWebp => ExportProfile::WebWebp,
                CliExportProfile::Grid2x2 => ExportProfile::Grid2x2,
            };

            let match_grade = if args.match_grade {
                Some(MatchGradeOptions {
                    reference_image_path: args.grade_ref,
                    cdf_damp_strength: args.grade_strength,
                    cube_lut_path: args.lut,
                })
            } else {
                None
            };

            let result = execute_export(
                run_store.as_ref(),
                &ExportRequest {
                    source,
                    output_slug,
                    profile_visibility: args.visibility.into(),
                    out_dir: args.out_dir,
                    profile,
                    match_grade,
                },
            )?;

            if args.json {
                println!("{}", serde_json::to_string_pretty(&result)?);
            } else {
                println!(
                    "Exported {} artifact(s) from '{}' (visibility={}, match_grade={})",
                    result.artifacts.len(),
                    result.source_id,
                    result.source_visibility.as_str(),
                    result.match_grade_applied
                );
                for art in &result.artifacts {
                    println!(
                        "  {}\t{}x{}\t{}",
                        art.role,
                        art.width,
                        art.height,
                        art.path.display()
                    );
                }
            }
        }
        Commands::Critic(args) => {
            let _ws = resolve_optional_workspace(cli.workspace.as_ref())?;

            let mut traits = Vec::with_capacity(args.traits.len());
            for (idx, raw) in args.traits.iter().enumerate() {
                traits.push(parse_trait_arg(idx, raw)?);
            }

            let critic = VisionCritic::with_codex_bin(auth_dir, codex_bin);
            let report = critic
                .evaluate(&VisionCriticRequest {
                    target_image: args.image,
                    reference_images: args.references,
                    traits,
                })
                .await?;

            println!("{}", serde_json::to_string_pretty(&report)?);
            if !report.passed {
                bail!("VisionCritic failed one or more trait checks");
            }
        }
        Commands::Library(args) => {
            let ws = resolve_optional_workspace(cli.workspace.as_ref())?;
            let registry = match ws.as_ref() {
                Some(resolved) => LibraryRegistry::from_workspace(resolved),
                None => LibraryRegistry::new(None, resolve_user_config_dir_from_env()),
            };

            match args.action {
                LibrarySubcommand::List { kind, json } => {
                    let mut rows = Vec::new();
                    if kind.is_none() || matches!(kind, Some(LibraryResourceKind::Subject)) {
                        for s in registry.list_subjects()? {
                            rows.push(LibrarySummaryRow {
                                kind: "subject",
                                slug: s.subject.slug,
                                display_name: s.subject.display_name,
                                source: s.source,
                            });
                        }
                    }
                    if kind.is_none() || matches!(kind, Some(LibraryResourceKind::Style)) {
                        for s in registry.list_styles()? {
                            rows.push(LibrarySummaryRow {
                                kind: "style",
                                slug: s.style.slug,
                                display_name: s.style.display_name,
                                source: s.source,
                            });
                        }
                    }
                    if kind.is_none() || matches!(kind, Some(LibraryResourceKind::Atmosphere)) {
                        for a in registry.list_atmospheres()? {
                            rows.push(LibrarySummaryRow {
                                kind: "atmosphere",
                                slug: a.atmosphere.slug,
                                display_name: a.atmosphere.display_name,
                                source: a.source,
                            });
                        }
                    }
                    if json {
                        println!("{}", serde_json::to_string_pretty(&rows)?);
                    } else {
                        for row in &rows {
                            println!(
                                "{}\t{}\t{}\t({})",
                                row.kind,
                                row.slug,
                                row.source.as_str(),
                                row.display_name
                            );
                        }
                    }
                }
                LibrarySubcommand::Show { kind, slug, json } => match kind {
                    LibraryResourceKind::Subject => {
                        let resolved = registry.resolve_subject(&slug)?;
                        if json {
                            println!("{}", serde_json::to_string_pretty(&resolved.subject)?);
                        } else {
                            println!("{}", resolved.subject.to_yaml_string()?);
                        }
                    }
                    LibraryResourceKind::Style => {
                        let resolved = registry.resolve_style(&slug)?;
                        if json {
                            println!("{}", serde_json::to_string_pretty(&resolved.style)?);
                        } else {
                            println!("{}", resolved.style.to_yaml_string()?);
                        }
                    }
                    LibraryResourceKind::Atmosphere => {
                        let resolved = registry.resolve_atmosphere(&slug)?;
                        if json {
                            println!("{}", serde_json::to_string_pretty(&resolved.atmosphere)?);
                        } else {
                            println!("{}", resolved.atmosphere.to_yaml_string()?);
                        }
                    }
                },
                LibrarySubcommand::VerifyLock => {
                    let resolved_ws = resolve_workspace_from_env(cli.workspace.as_deref())?;
                    let library_dir = resolved_ws.root.join(&resolved_ws.config.paths.library_dir);
                    let lock_store = ManifestLockStore::new(&library_dir);
                    lock_store.verify_canonical_checksums(&resolved_ws.root)?;
                    println!(
                        "Verified canonical checksums at {}",
                        lock_store.lock_path().display()
                    );
                }
            }
        }
        Commands::Pack(args) => match args.action {
            PackSubcommand::Generate {
                subject,
                style,
                seed,
                seed_label,
                json,
            } => {
                let resolved_ws = resolve_workspace_from_env(cli.workspace.as_deref())?;
                let library_dir = resolved_ws.root.join(&resolved_ws.config.paths.library_dir);
                let registry = LibraryRegistry::from_workspace(&resolved_ws);
                let backend = CodexNativeBackend::with_codex_bin(auth_dir, codex_bin);
                let pipeline = ReferencePackPipeline::new(&backend, &registry, library_dir);
                let mut req = ReferencePackRequest::new(subject, style);
                req.seed_path = seed;
                req.seed_label = seed_label;
                let generated = pipeline.generate(&req).await?;
                if json {
                    println!("{}", serde_json::to_string_pretty(&generated.manifest)?);
                } else {
                    println!(
                        "Generated ReferencePack ({} sheets, {} slices) at {}",
                        generated.manifest.sheets.len(),
                        generated.manifest.slices.len(),
                        generated.pack_dir.display()
                    );
                }
            }
            PackSubcommand::Slice {
                sheet,
                out_dir,
                strip_height,
                json,
            } => {
                let _ws = resolve_optional_workspace(cli.workspace.as_ref())?;
                let manifest = slice_sheet_to_dir(
                    &sheet,
                    &out_dir,
                    &SlicerConfig {
                        exclude_bottom_rows: strip_height,
                        ..SlicerConfig::default()
                    },
                )?;
                if json {
                    println!("{}", serde_json::to_string_pretty(&manifest)?);
                } else {
                    println!(
                        "Sliced {} components from {} into {}",
                        manifest.slices.len(),
                        sheet.display(),
                        out_dir.display()
                    );
                }
            }
            PackSubcommand::Atlas {
                out,
                slices,
                visibility,
                width,
                height,
                json,
            } => {
                let resolved_ws = resolve_workspace_from_env(cli.workspace.as_deref())?;
                let library_dir = resolved_ws.root.join(&resolved_ws.config.paths.library_dir);
                let registry = LibraryRegistry::from_workspace(&resolved_ws);

                let mut selections = Vec::with_capacity(slices.len());
                for raw in &slices {
                    selections.push(parse_atlas_selection(raw)?);
                }
                let req = AtlasPackRequest {
                    selections,
                    target_visibility: visibility.into(),
                    canvas_width: width,
                    canvas_height: height,
                    padding_px: DEFAULT_ATLAS_PADDING_PX,
                };
                let result = pack_dynamic_atlas(&registry, &library_dir, &req)?;

                if let Some(parent) = out.parent()
                    && !parent.as_os_str().is_empty()
                {
                    fs::create_dir_all(parent).with_context(|| {
                        format!("creating Atlas output directory {}", parent.display())
                    })?;
                }
                for page in &result.pages {
                    let page_path = if result.pages.len() == 1 {
                        out.clone()
                    } else {
                        let stem = out.file_stem().and_then(|s| s.to_str()).unwrap_or("atlas");
                        out.with_file_name(format!("{stem}_p{}.png", page.page_index + 1))
                    };
                    fs::write(&page_path, &page.png_bytes)
                        .with_context(|| format!("writing Atlas PNG {}", page_path.display()))?;
                }

                if json {
                    println!("{}", serde_json::to_string_pretty(&result.placements)?);
                } else {
                    println!(
                        "Packed {} slices onto {} Atlas page(s) at {}",
                        result.placements.len(),
                        result.pages.len(),
                        out.display()
                    );
                }
            }
        },
        Commands::Ui(args) => {
            let ws = resolve_optional_workspace(cli.workspace.as_ref())?;
            let port = args
                .port
                .or_else(|| ws.as_ref().map(|w| w.config.server.port))
                .unwrap_or(8080);
            serve_ui(AppState::new(auth_dir), port).await?;
        }
        Commands::Status(args) => {
            let ws = resolve_workspace_from_env(cli.workspace.as_deref())?;
            let auth_manager = AuthManager::new(auth_dir.clone(), codex_bin);
            let summary = WorkspaceStatusSummary {
                workspace_root: ws.root,
                config_path: ws.config_path,
                resolution_source: ws.source,
                schema_version: ws.config.schema_version,
                workspace_name: ws.config.name,
                backend: ws.config.backend.kind,
                auth_dir,
                auth_provisioned: auth_manager.is_provisioned_locally(),
                server_port: ws.config.server.port,
            };

            if args.json {
                println!("{}", serde_json::to_string_pretty(&summary)?);
            } else {
                println!("workspace_root: {}", summary.workspace_root.display());
                println!("config_path: {}", summary.config_path.display());
                println!("resolution_source: {:?}", summary.resolution_source);
                println!("schema_version: {}", summary.schema_version);
                println!("backend: {}", summary.backend.as_str());
                println!("auth_dir: {}", summary.auth_dir.display());
                println!("auth_provisioned: {}", summary.auth_provisioned);
                println!("server_port: {}", summary.server_port);
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case::cli_flag_overrides_env(Some("/cli/codex"), Some("/env/codex"), Some("/cli/codex"))]
    #[case::env_overrides_default(None, Some("/env/codex"), Some("/env/codex"))]
    #[case::empty_env_falls_back_to_default(None, Some("   "), Some(DEFAULT_CODEX_BIN))]
    #[case::unset_falls_back_to_default(None, None, Some(DEFAULT_CODEX_BIN))]
    #[case::empty_cli_flag_fails_closed(Some("  "), Some("/env/codex"), None)]
    fn resolves_codex_bin_in_strict_order(
        #[case] flag: Option<&str>,
        #[case] env_val: Option<&str>,
        #[case] expected: Option<&str>,
    ) {
        let res = resolve_codex_bin(flag, env_val);
        match expected {
            Some(exp) => assert_eq!(res.expect("should resolve"), exp),
            None => assert!(res.is_err()),
        }
    }

    #[rstest]
    #[case::valid_roi("10,20,64,64", Some(PixelRect { x: 10, y: 20, width: 64, height: 64 }))]
    #[case::zero_width_rejected("10,20,0,64", None)]
    #[case::missing_component_rejected("10,20,64", None)]
    fn parses_roi_arg_strictly(#[case] raw: &str, #[case] expected: Option<PixelRect>) {
        let res = parse_roi_arg(raw);
        match expected {
            Some(rect) => assert_eq!(res.expect("valid ROI"), rect),
            None => assert!(res.is_err()),
        }
    }

    #[rstest]
    #[case::numeric_version("harbor-dock:2", Some(("harbor-dock", 2)))]
    #[case::prefixed_version("harbor-dock:v003", Some(("harbor-dock", 3)))]
    #[case::zero_version_rejected("harbor-dock:0", None)]
    #[case::missing_colon_rejected("harbor-dock", None)]
    fn parses_run_ref_arg_strictly(#[case] raw: &str, #[case] expected: Option<(&str, u32)>) {
        let res = parse_run_ref_arg(raw);
        match expected {
            Some((slug, ver)) => {
                let parsed = res.expect("valid run ref");
                assert_eq!(parsed.0, slug);
                assert_eq!(parsed.1, ver);
            }
            None => assert!(res.is_err()),
        }
    }
}
