//! === WORKSPACE CONFIGURATION & DISCOVERY ===
//!
//! PURPOSE:
//! Parses and validates `imgforge.toml` (`schema_version = 1`, strict unknown-field
//! rejection) and resolves the active workspace directory deterministically.
//!
//! THREAT MODEL / DESIGN RATIONALE ("WHY"):
//! Ambiguous workspace resolution or silently ignored `imgforge.toml` keys can cause
//! assets, `Run` histories, or `Visibility` rules to write into the wrong directory or
//! mask typos in configuration. Discovery therefore follows a strict non-fallback
//! precedence chain when an explicit workspace is requested, and deserialization uses
//! `#[serde(deny_unknown_fields)]` across all configuration structs.
//!
//! EXECUTION LIFECYCLE / DATA FLOW:
//! 1. Inspect `--workspace` argument if present; fail-closed if missing `imgforge.toml`.
//! 2. Else inspect non-empty `IMGFORGE_WORKSPACE`; fail-closed if missing `imgforge.toml`.
//! 3. Else walk upward from `cwd` looking for the nearest `imgforge.toml`.
//! 4. Parse TOML, verify `schema_version == 1`, and reject parent-traversal paths (`..`).

use std::fs;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::CoreError;

/// Canonical filename for workspace configuration manifests.
pub const WORKSPACE_CONFIG_FILENAME: &str = "imgforge.toml";

/// Supported `imgforge.toml` schema version.
pub const SUPPORTED_CONFIG_SCHEMA_VERSION: u32 = 1;

fn default_library_dir() -> PathBuf {
    PathBuf::from("library")
}

fn default_runs_dir() -> PathBuf {
    PathBuf::from("runs")
}

fn default_exports_dir() -> PathBuf {
    PathBuf::from("exports")
}

fn default_server_port() -> u16 {
    8080
}

/// Supported image generation backend identifiers in `imgforge.toml`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BackendKind {
    /// Isolated `codex exec` backend backed by tool-local ChatGPT Plus OAuth.
    #[default]
    CodexNative,
}

impl BackendKind {
    /// Canonical lowercase backend identifier matching `imgforge.toml` and `/api/health`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CodexNative => "codex-native",
        }
    }
}

/// Backend settings declared in `imgforge.toml`.
///
/// Executable paths (`--codex-bin` / `IMGFORGE_CODEX_BIN`) are intentionally excluded
/// from `imgforge.toml` so untrusted workspace manifests can never hijack binary execution.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackendConfig {
    /// Active backend kind (defaults to `codex-native`).
    #[serde(default)]
    pub kind: BackendKind,
}

/// Workspace-relative directory layout declared in `imgforge.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspacePathsConfig {
    /// Relative directory containing `Subject`, `Style`, `Atmosphere`, and `ReferencePack` cards.
    #[serde(default = "default_library_dir")]
    pub library_dir: PathBuf,
    /// Relative directory containing versioned `Run` artifacts (`vNNN.png` + `vNNN.meta.json`).
    #[serde(default = "default_runs_dir")]
    pub runs_dir: PathBuf,
    /// Relative directory containing derived `Export` deliverables.
    #[serde(default = "default_exports_dir")]
    pub exports_dir: PathBuf,
}

impl Default for WorkspacePathsConfig {
    fn default() -> Self {
        Self {
            library_dir: default_library_dir(),
            runs_dir: default_runs_dir(),
            exports_dir: default_exports_dir(),
        }
    }
}

/// Embedded `127.0.0.1` Web Canvas UI server settings in `imgforge.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerConfig {
    /// TCP port bound strictly on `127.0.0.1` by `imgforge ui` (defaults to `8080`).
    #[serde(default = "default_server_port")]
    pub port: u16,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            port: default_server_port(),
        }
    }
}

/// Top-level `imgforge.toml` configuration schema (`schema_version = 1`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceConfig {
    /// Configuration schema version; must equal `1`.
    pub schema_version: u32,
    /// Optional human-readable workspace identifier.
    #[serde(default)]
    pub name: Option<String>,
    /// Image backend settings.
    #[serde(default)]
    pub backend: BackendConfig,
    /// Workspace directory layout relative to the workspace root.
    #[serde(default)]
    pub paths: WorkspacePathsConfig,
    /// Localhost Web Canvas UI server settings.
    #[serde(default)]
    pub server: ServerConfig,
}

/// Source that determined the resolved workspace root.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceResolutionSource {
    /// Resolved from explicit `--workspace <PATH>` CLI flag.
    CliFlag,
    /// Resolved from `IMGFORGE_WORKSPACE` environment variable.
    EnvVar,
    /// Resolved by ascending parent directories from CWD to nearest `imgforge.toml`.
    AncestorsFromCwd,
}

/// Validated workspace root paired with its parsed `imgforge.toml`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedWorkspace {
    /// Canonicalized directory containing `imgforge.toml`.
    pub root: PathBuf,
    /// Full path to `imgforge.toml`.
    pub config_path: PathBuf,
    /// Which precedence tier resolved the workspace.
    pub source: WorkspaceResolutionSource,
    /// Parsed and validated workspace configuration.
    pub config: WorkspaceConfig,
}

fn validate_relative_workspace_path(
    field_name: &str,
    rel: &Path,
    config_path: &Path,
) -> Result<(), CoreError> {
    if rel.as_os_str().is_empty() {
        return Err(CoreError::ConfigInvalid {
            path: config_path.to_path_buf(),
            reason: format!("{field_name} must not be empty"),
        });
    }
    if rel.is_absolute() {
        return Err(CoreError::ConfigInvalid {
            path: config_path.to_path_buf(),
            reason: format!("{field_name} must be a relative path inside the workspace"),
        });
    }
    if rel.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        return Err(CoreError::ConfigInvalid {
            path: config_path.to_path_buf(),
            reason: format!(
                "{field_name} must be a relative path without '..' or root/prefix components"
            ),
        });
    }
    Ok(())
}

/// Parse and validate `WorkspaceConfig` from raw TOML text.
pub fn parse_workspace_config(
    raw_toml: &str,
    config_path: &Path,
) -> Result<WorkspaceConfig, CoreError> {
    let config: WorkspaceConfig =
        toml::from_str(raw_toml).map_err(|err| CoreError::ConfigInvalid {
            path: config_path.to_path_buf(),
            reason: err.to_string(),
        })?;

    if config.schema_version != SUPPORTED_CONFIG_SCHEMA_VERSION {
        return Err(CoreError::ConfigInvalid {
            path: config_path.to_path_buf(),
            reason: format!(
                "unsupported schema_version {} (expected {})",
                config.schema_version, SUPPORTED_CONFIG_SCHEMA_VERSION
            ),
        });
    }

    validate_relative_workspace_path("paths.library_dir", &config.paths.library_dir, config_path)?;
    validate_relative_workspace_path("paths.runs_dir", &config.paths.runs_dir, config_path)?;
    validate_relative_workspace_path("paths.exports_dir", &config.paths.exports_dir, config_path)?;

    if config.server.port == 0 {
        return Err(CoreError::ConfigInvalid {
            path: config_path.to_path_buf(),
            reason: "server.port must be in 1..=65535".to_string(),
        });
    }

    Ok(config)
}

/// Read and validate `imgforge.toml` inside a specific workspace directory.
pub fn load_workspace_from_dir(
    dir: &Path,
    source: WorkspaceResolutionSource,
) -> Result<ResolvedWorkspace, CoreError> {
    if !dir.is_dir() {
        return Err(CoreError::WorkspaceInvalid {
            path: dir.to_path_buf(),
            reason: "workspace directory does not exist or is not a directory".to_string(),
        });
    }

    let config_path = dir.join(WORKSPACE_CONFIG_FILENAME);
    if !config_path.is_file() {
        return Err(CoreError::WorkspaceInvalid {
            path: dir.to_path_buf(),
            reason: format!("missing {WORKSPACE_CONFIG_FILENAME}"),
        });
    }

    let raw = fs::read_to_string(&config_path).map_err(|err| CoreError::Io {
        context: format!("reading {}", config_path.display()),
        source: err,
    })?;
    let config = parse_workspace_config(&raw, &config_path)?;
    let root = fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    let canonical_config_path = root.join(WORKSPACE_CONFIG_FILENAME);

    Ok(ResolvedWorkspace {
        root,
        config_path: canonical_config_path,
        source,
        config,
    })
}

/// Resolve the active workspace in strict order:
/// 1. `--workspace` CLI flag (`explicit_workspace`)
/// 2. `IMGFORGE_WORKSPACE` environment variable (`env_workspace`, when non-empty)
/// 3. Nearest `imgforge.toml` ascending from `cwd`
///
/// If an explicit override (`--workspace` or non-empty `IMGFORGE_WORKSPACE`) is provided
/// and invalid, resolution fails closed without falling back to lower tiers.
pub fn resolve_workspace(
    explicit_workspace: Option<&Path>,
    env_workspace: Option<&str>,
    cwd: &Path,
) -> Result<ResolvedWorkspace, CoreError> {
    if let Some(cli_path) = explicit_workspace {
        return load_workspace_from_dir(cli_path, WorkspaceResolutionSource::CliFlag);
    }

    if let Some(env_raw) = env_workspace
        && !env_raw.trim().is_empty()
    {
        return load_workspace_from_dir(Path::new(env_raw), WorkspaceResolutionSource::EnvVar);
    }

    for ancestor in cwd.ancestors() {
        let candidate = ancestor.join(WORKSPACE_CONFIG_FILENAME);
        if candidate.is_file() {
            return load_workspace_from_dir(ancestor, WorkspaceResolutionSource::AncestorsFromCwd);
        }
    }

    Err(CoreError::WorkspaceNotFound {
        cwd: cwd.to_path_buf(),
    })
}

/// Convenience wrapper reading `IMGFORGE_WORKSPACE` and `std::env::current_dir()` from the host process.
pub fn resolve_workspace_from_env(
    explicit_workspace: Option<&Path>,
) -> Result<ResolvedWorkspace, CoreError> {
    let env_workspace = std::env::var("IMGFORGE_WORKSPACE").ok();
    let cwd = std::env::current_dir().map_err(|err| CoreError::Io {
        context: "determining current working directory".to_string(),
        source: err,
    })?;
    resolve_workspace(explicit_workspace, env_workspace.as_deref(), &cwd)
}

#[cfg(test)]
mod tests {
    use rstest::rstest;
    use tempfile::TempDir;

    use super::*;

    #[test]
    fn parses_minimal_valid_imgforge_toml() {
        let cfg = parse_workspace_config("schema_version = 1\n", Path::new("imgforge.toml"))
            .expect("valid minimal config should parse");
        assert_eq!(cfg.schema_version, 1);
        assert_eq!(cfg.backend.kind, BackendKind::CodexNative);
        assert_eq!(cfg.paths.library_dir, PathBuf::from("library"));
        assert_eq!(cfg.paths.runs_dir, PathBuf::from("runs"));
        assert_eq!(cfg.paths.exports_dir, PathBuf::from("exports"));
        assert_eq!(cfg.server.port, 8080);
    }

    #[rstest]
    #[case::unknown_top_level_key("schema_version = 1\nunexpected_key = true\n")]
    #[case::unknown_nested_backend_key(
        "schema_version = 1\n[backend]\nopenai_api_key = \"sk-test\"\n"
    )]
    #[case::unknown_nested_paths_key("schema_version = 1\n[paths]\ntemp_dir = \"tmp\"\n")]
    #[case::unknown_nested_server_key("schema_version = 1\n[server]\nhost = \"0.0.0.0\"\n")]
    #[case::wrong_schema_version("schema_version = 2\n")]
    #[case::zero_schema_version("schema_version = 0\n")]
    #[case::missing_schema_version("name = \"demo\"\n")]
    #[case::parent_dir_escape_in_paths("schema_version = 1\n[paths]\nruns_dir = \"../outside\"\n")]
    #[case::absolute_path_in_paths("schema_version = 1\n[paths]\nlibrary_dir = \"/tmp/lib\"\n")]
    #[case::zero_port("schema_version = 1\n[server]\nport = 0\n")]
    #[case::untrusted_codex_bin_rejected(
        "schema_version = 1\n[backend]\ncodex_bin = \"/tmp/evil-codex\"\n"
    )]
    #[case::untrusted_empty_codex_bin_rejected(
        "schema_version = 1\n[backend]\ncodex_bin = \"   \"\n"
    )]
    fn rejects_invalid_imgforge_toml(#[case] raw_toml: &str) {
        let err = parse_workspace_config(raw_toml, Path::new("imgforge.toml"))
            .expect_err("invalid config must fail closed");
        assert!(matches!(err, CoreError::ConfigInvalid { .. }));
    }

    #[test]
    fn resolves_workspace_in_strict_precedence_order() {
        let root = TempDir::new().expect("tempdir");
        let cli_ws = root.path().join("cli-ws");
        let env_ws = root.path().join("env-ws");
        let parent_ws = root.path().join("parent-ws");
        let nested_cwd = parent_ws.join("scenes/act1");

        fs::create_dir_all(&cli_ws).expect("mkdir cli");
        fs::create_dir_all(&env_ws).expect("mkdir env");
        fs::create_dir_all(&nested_cwd).expect("mkdir nested");

        fs::write(
            cli_ws.join("imgforge.toml"),
            "schema_version = 1\nname = \"cli\"\n",
        )
        .unwrap();
        fs::write(
            env_ws.join("imgforge.toml"),
            "schema_version = 1\nname = \"env\"\n",
        )
        .unwrap();
        fs::write(
            parent_ws.join("imgforge.toml"),
            "schema_version = 1\nname = \"cwd\"\n",
        )
        .unwrap();

        // 1. --workspace overrides IMGFORGE_WORKSPACE and CWD
        let resolved_cli =
            resolve_workspace(Some(&cli_ws), Some(env_ws.to_str().unwrap()), &nested_cwd)
                .expect("cli should win");
        assert_eq!(resolved_cli.source, WorkspaceResolutionSource::CliFlag);
        assert_eq!(resolved_cli.config.name.as_deref(), Some("cli"));

        // 2. IMGFORGE_WORKSPACE overrides CWD
        let resolved_env = resolve_workspace(None, Some(env_ws.to_str().unwrap()), &nested_cwd)
            .expect("env should win");
        assert_eq!(resolved_env.source, WorkspaceResolutionSource::EnvVar);
        assert_eq!(resolved_env.config.name.as_deref(), Some("env"));

        // 3. Nearest imgforge.toml ascending from CWD (empty env ignored)
        let resolved_cwd = resolve_workspace(None, Some("   "), &nested_cwd)
            .expect("ancestor discovery should succeed");
        assert_eq!(
            resolved_cwd.source,
            WorkspaceResolutionSource::AncestorsFromCwd
        );
        assert_eq!(resolved_cwd.config.name.as_deref(), Some("cwd"));
    }

    #[rstest]
    #[case::explicit_cli_missing_config(true, false)]
    #[case::explicit_env_missing_config(false, true)]
    fn explicit_invalid_workspace_fails_closed_without_cwd_fallback(
        #[case] use_cli: bool,
        #[case] use_env: bool,
    ) {
        let root = TempDir::new().expect("tempdir");
        let valid_cwd = root.path().join("valid-cwd");
        let invalid_target = root.path().join("missing-ws");
        fs::create_dir_all(&valid_cwd).unwrap();
        fs::create_dir_all(&invalid_target).unwrap();
        fs::write(valid_cwd.join("imgforge.toml"), "schema_version = 1\n").unwrap();

        let cli = if use_cli {
            Some(invalid_target.as_path())
        } else {
            None
        };
        let env = if use_env {
            invalid_target.to_str()
        } else {
            None
        };

        let err = resolve_workspace(cli, env, &valid_cwd)
            .expect_err("invalid explicit workspace must not fall back to CWD");
        assert!(matches!(err, CoreError::WorkspaceInvalid { .. }));
    }

    #[test]
    fn fails_closed_when_no_workspace_exists_anywhere() {
        let root = TempDir::new().expect("tempdir");
        let err = resolve_workspace(None, None, root.path())
            .expect_err("missing workspace must fail closed");
        assert!(matches!(err, CoreError::WorkspaceNotFound { .. }));
    }
}
