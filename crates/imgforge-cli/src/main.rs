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
    AuthManager, CodexNativeBackend, CoreError, ImageBackend, ImageGenerationRequest,
    PromptCompiler, ResolvedWorkspace, SceneComposer, SceneSpec, TraitCategory, TraitCheckInput,
    VisionCritic, VisionCriticRequest, resolve_codex_auth_dir_from_env, resolve_workspace_from_env,
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

fn resolve_codex_bin(ws: Option<&ResolvedWorkspace>) -> String {
    ws.and_then(|w| w.config.backend.codex_bin.clone())
        .unwrap_or_else(|| DEFAULT_CODEX_BIN.to_string())
}

fn parse_trait_arg(index: usize, raw: &str) -> anyhow::Result<TraitCheckInput> {
    let parts: Vec<&str> = raw.splitn(3, ':').collect();
    if parts.len() == 3 {
        let category = match parts[1].trim() {
            "identity" => TraitCategory::Identity,
            "hand_props" => TraitCategory::HandProps,
            "weapon_geometry" => TraitCategory::WeaponGeometry,
            "style_adherence" => TraitCategory::StyleAdherence,
            "zero_leaked_text" => TraitCategory::ZeroLeakedText,
            other => bail!(
                "unknown trait category '{other}' (expected identity, hand_props, weapon_geometry, style_adherence, or zero_leaked_text)"
            ),
        };
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

    match cli.command {
        Commands::Auth(args) => {
            let ws = resolve_optional_workspace(cli.workspace.as_ref())?;
            let codex_bin = resolve_codex_bin(ws.as_ref());
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
            let ws = resolve_optional_workspace(cli.workspace.as_ref())?;
            let codex_bin = resolve_codex_bin(ws.as_ref());

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
            let ws = resolve_optional_workspace(cli.workspace.as_ref())?;
            let codex_bin = resolve_codex_bin(ws.as_ref());

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
            let ws = resolve_optional_workspace(cli.workspace.as_ref())?;
            let codex_bin = resolve_codex_bin(ws.as_ref());

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
            let auth_manager = AuthManager::new(auth_dir.clone(), resolve_codex_bin(Some(&ws)));
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
                println!("auth_dir: {}", summary.auth_dir.display());
                println!("auth_provisioned: {}", summary.auth_provisioned);
                println!("server_port: {}", summary.server_port);
            }
        }
    }

    Ok(())
}
