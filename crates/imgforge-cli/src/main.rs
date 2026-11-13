//! === IMGFORGE CLI ENTRYPOINT ===
//!
//! PURPOSE:
//! Exposes the `imgforge` binary CLI (`auth`, `compose`, `validate`, `generate`,
//! `critic`, `ui`, and `status`) on top of `imgforge-core` and the embedded Web Canvas UI.
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
    DEFAULT_ATLAS_HEIGHT, DEFAULT_ATLAS_PADDING_PX, DEFAULT_ATLAS_WIDTH, ImageBackend,
    ImageGenerationRequest, LibraryRegistry, ManifestLockStore, PromptCompiler,
    ReferencePackPipeline, ReferencePackRequest, ResolvedWorkspace, SceneComposer, SceneSpec,
    SlicerConfig, TraitCategory, TraitCheckInput, Visibility, VisionCritic, VisionCriticRequest,
    pack_dynamic_atlas, resolve_codex_auth_dir_from_env, resolve_user_config_dir_from_env,
    resolve_workspace_from_env, slice_sheet_to_dir,
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
}
