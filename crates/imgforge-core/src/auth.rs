//! === TOOL-LOCAL CODEX AUTH & ISOLATED TEMP_CODEX_HOME ===
//!
//! PURPOSE:
//! Manages tool-local ChatGPT Plus OAuth credentials (`imgforge auth login | status | logout`)
//! and materializes isolated per-job `CODEX_HOME` directories via `TempCodexHome`.
//!
//! THREAT MODEL / DESIGN RATIONALE ("WHY"):
//! 1. Symlinking or mutating `~/.codex/auth.json` leaks user-global CLI state and
//!    session caches into `imgforge` runs. `imgforge` therefore stores credentials
//!    exclusively under `$XDG_DATA_HOME/imgforge/codex/` (only when `$XDG_DATA_HOME` is
//!    non-empty and absolute; else `$HOME/.local/share/imgforge/codex/`) with `0700`
//!    directory permissions and `0600` `auth.json`.
//! 2. Relative `$XDG_DATA_HOME` values could otherwise resolve inside an untrusted CWD.
//! 3. Each `codex exec` invocation receives a private `TempCodexHome` (`0700` dir)
//!    containing a fresh `0600` copy of `auth.json` and a minimal `config.toml`, and
//!    always clears `OPENAI_API_KEY` so OAuth-only execution is enforced.
//! 4. `TempCodexHome` implements RAII `Drop` cleanup via `tempfile::TempDir` so
//!    copied credentials never persist after normal completion, errors, or cancellation.
//!
//! EXECUTION LIFECYCLE / DATA FLOW:
//! 1. `resolve_codex_auth_dir` validates `$XDG_DATA_HOME` / `$HOME`.
//! 2. `AuthManager::login` creates `0700` dir, runs `codex login [--device-auth]`, and
//!    hardens `auth.json` to `0600`.
//! 3. `AuthManager::status` validates local `auth.json` + runs `codex login status`.
//! 4. `TempCodexHome::create` validates local `auth.json` (rejecting symlinks/empty files)
//!    and clones it into a `0700` temp dir for a single `codex exec` invocation.

use std::ffi::OsStr;
use std::fs::{self, DirBuilder, OpenOptions, Permissions};
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Stdio;

use serde::{Deserialize, Serialize};
use tempfile::TempDir;
use tokio::process::Command;

use crate::error::CoreError;

/// Canonical filename for Codex OAuth credential storage.
pub const AUTH_JSON_FILENAME: &str = "auth.json";

/// Canonical filename for per-job isolated Codex configuration.
pub const CODEX_CONFIG_FILENAME: &str = "config.toml";

/// Default `codex` executable name resolved on `PATH`.
pub const DEFAULT_CODEX_BIN: &str = "codex";

/// Structured status returned by `AuthManager::status`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthStatusReport {
    /// Path to the tool-local `auth.json` file.
    pub auth_file: PathBuf,
    /// True when both local `auth.json` and `codex login status` succeeded.
    pub authenticated: bool,
    /// Non-sensitive summary output from `codex login status`.
    pub detail: String,
}

/// Resolve the tool-local Codex credential directory:
/// - `$XDG_DATA_HOME/imgforge/codex` ONLY when `$XDG_DATA_HOME` is non-empty and absolute;
/// - otherwise `$HOME/.local/share/imgforge/codex` when `$HOME` is non-empty and absolute.
pub fn resolve_codex_auth_dir(
    xdg_data_home: Option<&OsStr>,
    home: Option<&OsStr>,
) -> Result<PathBuf, CoreError> {
    if let Some(raw_xdg) = xdg_data_home
        && !raw_xdg.is_empty()
    {
        let xdg_path = Path::new(raw_xdg);
        if xdg_path.is_absolute() {
            return Ok(xdg_path.join("imgforge").join("codex"));
        }
    }

    if let Some(raw_home) = home
        && !raw_home.is_empty()
    {
        let home_path = Path::new(raw_home);
        if home_path.is_absolute() {
            return Ok(home_path
                .join(".local")
                .join("share")
                .join("imgforge")
                .join("codex"));
        }
    }

    Err(CoreError::AuthHomeUnavailable)
}

/// Resolve the tool-local Codex credential directory from process environment variables.
pub fn resolve_codex_auth_dir_from_env() -> Result<PathBuf, CoreError> {
    let xdg = std::env::var_os("XDG_DATA_HOME");
    let home = std::env::var_os("HOME");
    resolve_codex_auth_dir(xdg.as_deref(), home.as_deref())
}

fn ensure_secure_dir(dir: &Path) -> Result<(), CoreError> {
    DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .map_err(|err| CoreError::Io {
            context: format!("creating auth directory {}", dir.display()),
            source: err,
        })?;

    let meta = fs::symlink_metadata(dir).map_err(|err| CoreError::Io {
        context: format!("inspecting auth directory {}", dir.display()),
        source: err,
    })?;
    if meta.file_type().is_symlink() || !meta.is_dir() {
        return Err(CoreError::AuthInvalid {
            path: dir.to_path_buf(),
            reason: "auth directory must be a real directory, not a symlink".to_string(),
        });
    }

    fs::set_permissions(dir, Permissions::from_mode(0o700)).map_err(|err| CoreError::Io {
        context: format!("setting 0700 permissions on {}", dir.display()),
        source: err,
    })?;
    Ok(())
}

fn validate_auth_dir(auth_dir: &Path) -> Result<(), CoreError> {
    let meta = match fs::symlink_metadata(auth_dir) {
        Ok(m) => m,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Err(CoreError::AuthMissing {
                auth_file: auth_dir.join(AUTH_JSON_FILENAME),
            });
        }
        Err(err) => {
            return Err(CoreError::Io {
                context: format!("inspecting auth directory {}", auth_dir.display()),
                source: err,
            });
        }
    };

    if meta.file_type().is_symlink() || !meta.is_dir() {
        return Err(CoreError::AuthInvalid {
            path: auth_dir.to_path_buf(),
            reason: "auth directory must be a real directory, not a symlink".to_string(),
        });
    }

    let mode = meta.permissions().mode();
    if mode & 0o077 != 0 {
        return Err(CoreError::AuthInvalid {
            path: auth_dir.to_path_buf(),
            reason: format!(
                "auth directory has overly permissive mode {:04o}; expected 0700 (no group/world permissions)",
                mode & 0o777
            ),
        });
    }

    Ok(())
}

fn validate_regular_auth_file(auth_file: &Path) -> Result<Vec<u8>, CoreError> {
    let lstat_meta = match fs::symlink_metadata(auth_file) {
        Ok(m) => m,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Err(CoreError::AuthMissing {
                auth_file: auth_file.to_path_buf(),
            });
        }
        Err(err) => {
            return Err(CoreError::Io {
                context: format!("inspecting {}", auth_file.display()),
                source: err,
            });
        }
    };

    if lstat_meta.file_type().is_symlink() {
        return Err(CoreError::AuthInvalid {
            path: auth_file.to_path_buf(),
            reason: "refusing symlinked auth.json; credentials must be tool-local regular files"
                .to_string(),
        });
    }
    if !lstat_meta.is_file() {
        return Err(CoreError::AuthInvalid {
            path: auth_file.to_path_buf(),
            reason: "auth.json path exists but is not a regular file".to_string(),
        });
    }

    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(auth_file)
        .map_err(|err| {
            if err.raw_os_error() == Some(libc::ELOOP) {
                CoreError::AuthInvalid {
                    path: auth_file.to_path_buf(),
                    reason:
                        "refusing symlinked auth.json; credentials must be tool-local regular files"
                            .to_string(),
                }
            } else {
                CoreError::Io {
                    context: format!("opening {}", auth_file.display()),
                    source: err,
                }
            }
        })?;

    let meta = file.metadata().map_err(|err| CoreError::Io {
        context: format!("inspecting opened {}", auth_file.display()),
        source: err,
    })?;
    if !meta.is_file() {
        return Err(CoreError::AuthInvalid {
            path: auth_file.to_path_buf(),
            reason: "auth.json is not a regular file".to_string(),
        });
    }
    let mode = meta.permissions().mode();
    if mode & 0o077 != 0 {
        return Err(CoreError::AuthInvalid {
            path: auth_file.to_path_buf(),
            reason: format!(
                "auth.json has overly permissive mode {:04o}; expected 0600 (no group/world permissions)",
                mode & 0o777
            ),
        });
    }

    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).map_err(|err| CoreError::Io {
        context: format!("reading {}", auth_file.display()),
        source: err,
    })?;
    if bytes.iter().all(|b| b.is_ascii_whitespace()) {
        return Err(CoreError::AuthInvalid {
            path: auth_file.to_path_buf(),
            reason: "auth.json is empty".to_string(),
        });
    }

    serde_json::from_slice::<serde_json::Value>(&bytes).map_err(|err| CoreError::AuthInvalid {
        path: auth_file.to_path_buf(),
        reason: format!("auth.json is not valid JSON: {err}"),
    })?;

    Ok(bytes)
}

fn write_private_new_file(path: &Path, contents: &[u8], context: &str) -> Result<(), CoreError> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|err| CoreError::Io {
            context: context.to_string(),
            source: err,
        })?;
    file.write_all(contents).map_err(|err| CoreError::Io {
        context: context.to_string(),
        source: err,
    })?;
    Ok(())
}

/// Manager for tool-local ChatGPT Plus OAuth provisioning via `codex login`.
#[derive(Debug, Clone)]
pub struct AuthManager {
    auth_dir: PathBuf,
    codex_bin: String,
}

impl AuthManager {
    /// Construct an `AuthManager` targeting a specific tool-local auth directory and `codex` binary.
    #[must_use]
    pub fn new(auth_dir: PathBuf, codex_bin: impl Into<String>) -> Self {
        Self {
            auth_dir,
            codex_bin: codex_bin.into(),
        }
    }

    /// Return the tool-local `CODEX_HOME` directory path.
    #[must_use]
    pub fn auth_dir(&self) -> &Path {
        &self.auth_dir
    }

    /// Return the expected path to the tool-local `auth.json` file.
    #[must_use]
    pub fn auth_file_path(&self) -> PathBuf {
        self.auth_dir.join(AUTH_JSON_FILENAME)
    }

    /// Fast local check (used by `GET /api/health`) verifying `auth.json` exists, is secure, and is valid JSON.
    #[must_use]
    pub fn is_provisioned_locally(&self) -> bool {
        validate_auth_dir(&self.auth_dir).is_ok()
            && validate_regular_auth_file(&self.auth_file_path()).is_ok()
    }

    /// Invoke `codex login` (optionally `--device-auth`) with `CODEX_HOME` bound to the tool-local dir.
    pub async fn login(&self, device_auth: bool) -> Result<PathBuf, CoreError> {
        ensure_secure_dir(&self.auth_dir)?;

        let mut cmd = Command::new(&self.codex_bin);
        cmd.kill_on_drop(true);
        cmd.arg("login");
        if device_auth {
            cmd.arg("--device-auth");
        }
        cmd.env("CODEX_HOME", &self.auth_dir);
        cmd.env_remove("OPENAI_API_KEY");

        let status = cmd
            .status()
            .await
            .map_err(|err| CoreError::AuthCommandFailed {
                reason: format!("failed to launch '{}': {err}", self.codex_bin),
            })?;
        if !status.success() {
            return Err(CoreError::AuthCommandFailed {
                reason: format!("'{} login' exited with status {status}", self.codex_bin),
            });
        }

        let auth_file = self.auth_file_path();
        let meta = match fs::symlink_metadata(&auth_file) {
            Ok(m) => m,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Err(CoreError::AuthMissing { auth_file });
            }
            Err(err) => {
                return Err(CoreError::Io {
                    context: format!("inspecting {}", auth_file.display()),
                    source: err,
                });
            }
        };
        if meta.file_type().is_symlink() || !meta.is_file() {
            return Err(CoreError::AuthInvalid {
                path: auth_file,
                reason: "codex login produced a symlink or non-regular auth.json".to_string(),
            });
        }
        fs::set_permissions(&auth_file, Permissions::from_mode(0o600)).map_err(|err| {
            CoreError::Io {
                context: format!("setting 0600 permissions on {}", auth_file.display()),
                source: err,
            }
        })?;
        let _ = validate_regular_auth_file(&auth_file)?;

        Ok(auth_file)
    }

    /// Verify local `auth.json` presence/integrity and query `codex login status` in `CODEX_HOME`.
    pub async fn status(&self) -> Result<AuthStatusReport, CoreError> {
        validate_auth_dir(&self.auth_dir)?;
        let auth_file = self.auth_file_path();
        let _ = validate_regular_auth_file(&auth_file)?;

        let mut cmd = Command::new(&self.codex_bin);
        cmd.kill_on_drop(true);
        cmd.args(["login", "status"])
            .env("CODEX_HOME", &self.auth_dir)
            .env_remove("OPENAI_API_KEY")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let output = cmd
            .output()
            .await
            .map_err(|err| CoreError::AuthCommandFailed {
                reason: format!("failed to launch '{} login status': {err}", self.codex_bin),
            })?;

        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if !output.status.success() {
            let detail = if !stderr.is_empty() {
                stderr
            } else if !stdout.is_empty() {
                stdout
            } else {
                format!("exit status {}", output.status)
            };
            return Err(CoreError::AuthCommandFailed {
                reason: format!(
                    "codex login status failed ({detail}); run 'imgforge auth login' to re-authenticate"
                ),
            });
        }

        let detail = if !stdout.is_empty() {
            stdout
        } else if !stderr.is_empty() {
            stderr
        } else {
            "Authenticated via tool-local ChatGPT Plus OAuth".to_string()
        };

        Ok(AuthStatusReport {
            auth_file,
            authenticated: true,
            detail,
        })
    }

    /// Remove tool-local `auth.json`. Returns `true` if a file existed and was deleted.
    pub fn logout(&self) -> Result<bool, CoreError> {
        let auth_file = self.auth_file_path();
        match fs::symlink_metadata(&auth_file) {
            Ok(meta) => {
                if meta.is_dir() {
                    return Err(CoreError::AuthInvalid {
                        path: auth_file,
                        reason: "expected regular file for auth.json, found directory".to_string(),
                    });
                }
                fs::remove_file(&auth_file).map_err(|err| CoreError::Io {
                    context: format!("removing {}", auth_file.display()),
                    source: err,
                })?;
                Ok(true)
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(err) => Err(CoreError::Io {
                context: format!("inspecting {}", auth_file.display()),
                source: err,
            }),
        }
    }
}

/// RAII temporary `CODEX_HOME` directory guard isolating a single `codex exec` invocation.
///
/// Materializes a `0700` directory with a private `0600` copy of `auth.json` and a minimal
/// `0600` `config.toml`, then removes the entire directory tree on drop.
#[derive(Debug)]
pub struct TempCodexHome {
    temp_dir: TempDir,
}

impl TempCodexHome {
    /// Materialize a fresh isolated `CODEX_HOME` directory from `source_auth_dir/auth.json`.
    pub fn create(source_auth_dir: &Path) -> Result<Self, CoreError> {
        validate_auth_dir(source_auth_dir)?;
        let source_auth_file = source_auth_dir.join(AUTH_JSON_FILENAME);
        let auth_bytes = validate_regular_auth_file(&source_auth_file)?;

        let temp_dir = tempfile::Builder::new()
            .prefix("imgforge-codex-")
            .tempdir()
            .map_err(|err| CoreError::Io {
                context: "creating temporary CODEX_HOME directory".to_string(),
                source: err,
            })?;

        fs::set_permissions(temp_dir.path(), Permissions::from_mode(0o700)).map_err(|err| {
            CoreError::Io {
                context: format!(
                    "setting 0700 permissions on temporary CODEX_HOME {}",
                    temp_dir.path().display()
                ),
                source: err,
            }
        })?;

        let dest_auth_file = temp_dir.path().join(AUTH_JSON_FILENAME);
        write_private_new_file(
            &dest_auth_file,
            &auth_bytes,
            &format!("copying auth.json into {}", dest_auth_file.display()),
        )?;

        let dest_config_file = temp_dir.path().join(CODEX_CONFIG_FILENAME);
        let minimal_config = b"# Isolated per-job imgforge CODEX_HOME\n";
        write_private_new_file(
            &dest_config_file,
            minimal_config,
            &format!(
                "writing minimal config.toml at {}",
                dest_config_file.display()
            ),
        )?;

        Ok(Self { temp_dir })
    }

    /// Return the path to the isolated temporary `CODEX_HOME` directory.
    #[must_use]
    pub fn path(&self) -> &Path {
        self.temp_dir.path()
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::symlink;

    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case::absolute_xdg(
        Some("/tmp/xdg-data"),
        Some("/home/tester"),
        Some("/tmp/xdg-data/imgforge/codex")
    )]
    #[case::empty_xdg_falls_back_to_home(
        Some(""),
        Some("/home/tester"),
        Some("/home/tester/.local/share/imgforge/codex")
    )]
    #[case::relative_xdg_falls_back_to_home(
        Some("relative/xdg"),
        Some("/home/tester"),
        Some("/home/tester/.local/share/imgforge/codex")
    )]
    #[case::unset_xdg_uses_home(
        None,
        Some("/home/tester"),
        Some("/home/tester/.local/share/imgforge/codex")
    )]
    #[case::both_relative_fails_closed(Some("rel/xdg"), Some("rel/home"), None)]
    #[case::both_missing_fails_closed(None, None, None)]
    fn resolves_tool_local_auth_dir(
        #[case] xdg: Option<&str>,
        #[case] home: Option<&str>,
        #[case] expected: Option<&str>,
    ) {
        let result = resolve_codex_auth_dir(xdg.map(OsStr::new), home.map(OsStr::new));
        match expected {
            Some(exp) => assert_eq!(result.expect("should resolve"), PathBuf::from(exp)),
            None => assert!(matches!(result, Err(CoreError::AuthHomeUnavailable))),
        }
    }

    #[test]
    fn temp_codex_home_copies_auth_with_strict_permissions_and_cleans_up_on_drop() {
        let auth_dir = TempDir::new().expect("auth tempdir");
        fs::set_permissions(auth_dir.path(), Permissions::from_mode(0o700)).unwrap();
        let auth_file = auth_dir.path().join("auth.json");
        fs::write(&auth_file, r#"{"access_token":"test-token"}"#).unwrap();
        fs::set_permissions(&auth_file, Permissions::from_mode(0o600)).unwrap();

        let isolated_path: PathBuf;
        {
            let guard = TempCodexHome::create(auth_dir.path()).expect("TempCodexHome create");
            isolated_path = guard.path().to_path_buf();

            let dir_mode = fs::metadata(&isolated_path).unwrap().permissions().mode() & 0o777;
            let auth_mode = fs::metadata(isolated_path.join("auth.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            let cfg_mode = fs::metadata(isolated_path.join("config.toml"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777;

            assert_eq!(dir_mode, 0o700);
            assert_eq!(auth_mode, 0o600);
            assert_eq!(cfg_mode, 0o600);
            assert!(isolated_path.join("auth.json").is_file());
            assert!(isolated_path.join("config.toml").is_file());
        }

        assert!(
            !isolated_path.exists(),
            "TempCodexHome must remove isolated directory on drop"
        );
    }

    #[test]
    fn temp_codex_home_rejects_missing_and_symlinked_auth_json() {
        let auth_dir = TempDir::new().expect("auth tempdir");
        fs::set_permissions(auth_dir.path(), Permissions::from_mode(0o700)).unwrap();
        let missing_err = TempCodexHome::create(auth_dir.path()).expect_err("missing auth");
        assert!(matches!(missing_err, CoreError::AuthMissing { .. }));

        let target = auth_dir.path().join("real.json");
        fs::write(&target, r#"{"token":"secret"}"#).unwrap();
        symlink(&target, auth_dir.path().join("auth.json")).unwrap();

        let symlink_err = TempCodexHome::create(auth_dir.path()).expect_err("symlink auth");
        assert!(matches!(symlink_err, CoreError::AuthInvalid { .. }));
    }

    #[rstest]
    #[case::group_readable_dir(0o740, 0o600)]
    #[case::world_readable_dir(0o755, 0o600)]
    #[case::world_writable_dir(0o702, 0o600)]
    #[case::group_readable_auth_file(0o700, 0o640)]
    #[case::world_readable_auth_file(0o700, 0o644)]
    #[case::group_writable_auth_file(0o700, 0o660)]
    fn temp_codex_home_rejects_group_or_world_permissions(
        #[case] dir_mode: u32,
        #[case] file_mode: u32,
    ) {
        let root = TempDir::new().expect("root tempdir");
        let auth_dir = root.path().join("codex");
        fs::create_dir(&auth_dir).unwrap();
        let auth_file = auth_dir.join("auth.json");
        fs::write(&auth_file, r#"{"access_token":"test-token"}"#).unwrap();
        fs::set_permissions(&auth_file, Permissions::from_mode(file_mode)).unwrap();
        fs::set_permissions(&auth_dir, Permissions::from_mode(dir_mode)).unwrap();

        let err = TempCodexHome::create(&auth_dir)
            .expect_err("group/world permission bits on auth_dir or auth.json must fail closed");
        assert!(matches!(err, CoreError::AuthInvalid { .. }));
    }

    #[tokio::test]
    async fn auth_login_status_logout_with_stub_codex_binary() {
        let root = TempDir::new().expect("tempdir");
        let auth_dir = root.path().join("xdg/imgforge/codex");
        let stub_bin = root.path().join("codex-stub");
        let args_log = root.path().join("args.log");

        let script = format!(
            r#"#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >> "{log}"
printf 'CODEX_HOME=%s\n' "${{CODEX_HOME:-}}" >> "{log}"
printf 'OPENAI_API_KEY=%s\n' "${{OPENAI_API_KEY:-unset}}" >> "{log}"
if [ "${{1:-}}" = "login" ] && [ "${{2:-}}" = "status" ]; then
  if [ ! -f "$CODEX_HOME/auth.json" ]; then
    echo "missing auth" >&2
    exit 1
  fi
  echo "Logged in via ChatGPT Plus OAuth"
  exit 0
fi
if [ "${{1:-}}" = "login" ]; then
  printf '{{"oauth_token":"stub-token"}}\n' > "$CODEX_HOME/auth.json"
  exit 0
fi
exit 2
"#,
            log = args_log.display()
        );
        fs::write(&stub_bin, script).unwrap();
        fs::set_permissions(&stub_bin, Permissions::from_mode(0o755)).unwrap();

        let manager = AuthManager::new(auth_dir.clone(), stub_bin.to_string_lossy().into_owned());

        // Missing auth fails closed on status
        let missing_status = manager.status().await.expect_err("must fail without login");
        assert!(matches!(missing_status, CoreError::AuthMissing { .. }));
        assert!(!manager.is_provisioned_locally());

        // Login with --device-auth provisions 0700 dir and 0600 auth.json
        let auth_file = manager.login(true).await.expect("login succeeds");
        assert_eq!(
            fs::metadata(&auth_dir).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(&auth_file).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert!(manager.is_provisioned_locally());

        // Status succeeds
        let report = manager.status().await.expect("status succeeds");
        assert!(report.authenticated);
        assert!(report.detail.contains("ChatGPT Plus OAuth"));

        // Verify env isolation & --device-auth forwarding
        let log_content = fs::read_to_string(&args_log).unwrap();
        assert!(log_content.contains("login --device-auth"));
        assert!(log_content.contains(&format!("CODEX_HOME={}", auth_dir.display())));
        assert!(log_content.contains("OPENAI_API_KEY=unset"));

        // Logout deletes auth.json cleanly and is idempotent
        assert!(manager.logout().expect("logout removes file"));
        assert!(!auth_file.exists());
        assert!(!manager.logout().expect("second logout is idempotent"));
        assert!(!manager.is_provisioned_locally());
    }
}
