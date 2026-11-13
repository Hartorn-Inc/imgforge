//! === IMAGE BACKEND TRAIT & CODEX NATIVE BACKEND ===
//!
//! PURPOSE:
//! Defines the extensible `ImageBackend` async trait (the primary testing seam)
//! and implements `CodexNativeBackend`, which spawns `codex exec --ephemeral --json`
//! inside an isolated `TempCodexHome` using ChatGPT Plus OAuth.
//!
//! THREAT MODEL / DESIGN RATIONALE ("WHY"):
//! 1. `imgforge` excludes `OPENAI_API_KEY` clients and browser CDP automation (ADR 0002);
//!    `CodexNativeBackend` clears `OPENAI_API_KEY` from the child environment and
//!    uses `Command` argument arrays (never shell interpolation) to prevent command
//!    injection via prompts or reference paths.
//! 2. Because `codex` `image_gen` does not guarantee exact output dimensions, the
//!    backend validates the 8-byte PNG signature, rejects aspect-ratio drift > 5%
//!    fail-closed, and resamples with `Lanczos3` to exact requested `WxH`.
//! 3. `thread_id` parsed from JSONL stdout is sanitized against path traversal
//!    before looking up `$CODEX_HOME/generated_images/<thread_id>/`.
//!
//! EXECUTION LIFECYCLE / DATA FLOW:
//! 1. Validate requested `WxH`, non-empty prompt, and existing reference `-i` files.
//! 2. Materialize RAII `TempCodexHome` (`0700` dir, `0600` `auth.json` + `config.toml`).
//! 3. Spawn `codex exec --ephemeral --json [-i <FILE>...] -- <PROMPT>`.
//! 4. Extract `thread_id` from JSONL stdout and locate PNG under
//!    `$CODEX_HOME/generated_images/<thread_id>/` (or newest-PNG fallback).
//! 5. Validate PNG magic header, decode, enforce <= 5% aspect-ratio tolerance,
//!    and Lanczos-resample to exact `WxH`.
//! 6. Drop `TempCodexHome` automatically on return, error, or cancellation.

use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::SystemTime;

use async_trait::async_trait;
use image::imageops::FilterType;
use image::{DynamicImage, ImageFormat};
use tokio::process::Command;

use crate::auth::{DEFAULT_CODEX_BIN, TempCodexHome};
use crate::error::CoreError;
use crate::store::{checked_pixel_count, decode_png_checked, read_regular_file_nofollow};

/// Exact 8-byte PNG file magic header.
pub const PNG_SIGNATURE: [u8; 8] = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];

/// Maximum permitted relative aspect-ratio deviation (`5%`).
pub const MAX_ASPECT_RATIO_ERROR: f64 = 0.05;

/// Maximum character count for sanitized subprocess stderr and JSONL diagnostic summaries.
pub const MAX_FAILURE_DETAIL_CHARS: usize = 300;

/// Input parameters for a single `ImageBackend::generate` call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageGenerationRequest {
    /// Positive-only compiled prompt from `PromptCompiler`.
    pub prompt: String,
    /// Target output width in pixels (`> 0`).
    pub width: u32,
    /// Target output height in pixels (`> 0`).
    pub height: u32,
    /// Optional reference image file paths passed via `-i <FILE>`.
    pub reference_images: Vec<PathBuf>,
}

/// Verified and resampled raster output returned by `ImageBackend::generate`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageGenerationOutput {
    /// Validated PNG bytes matching exact `width x height`.
    pub png_bytes: Vec<u8>,
    /// Final output width in pixels.
    pub width: u32,
    /// Final output height in pixels.
    pub height: u32,
    /// Raw generated width before host Lanczos resampling.
    pub raw_width: u32,
    /// Raw generated height before host Lanczos resampling.
    pub raw_height: u32,
    /// Optional Codex execution `thread_id` parsed from JSONL output.
    pub thread_id: Option<String>,
}

/// Extensible image generation backend trait (primary testing seam).
#[async_trait]
pub trait ImageBackend: Send + Sync {
    /// Canonical backend identifier (for example `"codex-native"`).
    fn name(&self) -> &'static str;

    /// Generate an image matching `request` dimensions and return validated PNG bytes.
    async fn generate(
        &self,
        request: &ImageGenerationRequest,
    ) -> Result<ImageGenerationOutput, CoreError>;
}

/// Construct the reduced integer aspect ratio label (for example `"16:9"` or `"1:1"`).
#[must_use]
pub fn format_aspect_ratio(width: u32, height: u32) -> String {
    let mut a = width;
    let mut b = height;
    while b != 0 {
        let r = a % b;
        a = b;
        b = r;
    }
    let divisor = a.max(1);
    format!("{}:{}", width / divisor, height / divisor)
}

/// Build the full prompt passed to `codex exec` including target `WxH` dimensions,
/// aspect ratio, explicit `image_gen` tool execution directives, and positive-only
/// reference/textless rendering guidance.
#[must_use]
pub fn build_generation_prompt(request: &ImageGenerationRequest) -> String {
    let aspect = format_aspect_ratio(request.width, request.height);
    let (tool_scope, ref_clause) = if request.reference_images.is_empty() {
        ("only this specification", "Render the image faithfully")
    } else {
        (
            "only the attached reference image(s) and this specification",
            "Render the image faithfully matching the attached reference image(s)",
        )
    };
    format!(
        "[RENDER_TARGET]\n\
         dimensions: {}x{}\n\
         aspect_ratio: {}\n\
         tool: image_gen (required; invoke the built-in image_gen tool immediately using {tool_scope} to generate and save the raster PNG output directly)\n\
         instruction: {} at {}x{} ({}) framing via image_gen as purely visual artwork with clean, unlabeled surfaces and zero rendered text or labels.\n\n\
         {}",
        request.width,
        request.height,
        aspect,
        ref_clause,
        request.width,
        request.height,
        aspect,
        request.prompt.trim()
    )
}

/// Sole built-in backend executing `codex exec` with isolated per-job `CODEX_HOME`.
#[derive(Debug, Clone)]
pub struct CodexNativeBackend {
    auth_dir: PathBuf,
    codex_bin: String,
}

impl CodexNativeBackend {
    /// Construct a `CodexNativeBackend` using the tool-local `auth_dir` and default `codex` binary.
    #[must_use]
    pub fn new(auth_dir: PathBuf) -> Self {
        Self {
            auth_dir,
            codex_bin: DEFAULT_CODEX_BIN.to_string(),
        }
    }

    /// Construct a `CodexNativeBackend` with an explicit `codex` binary path (useful for test stubs).
    #[must_use]
    pub fn with_codex_bin(auth_dir: PathBuf, codex_bin: impl Into<String>) -> Self {
        Self {
            auth_dir,
            codex_bin: codex_bin.into(),
        }
    }
}

#[async_trait]
impl ImageBackend for CodexNativeBackend {
    fn name(&self) -> &'static str {
        "codex-native"
    }

    async fn generate(
        &self,
        request: &ImageGenerationRequest,
    ) -> Result<ImageGenerationOutput, CoreError> {
        if request.width == 0 || request.height == 0 {
            return Err(CoreError::InvalidGenerationRequest(
                "width and height must both be greater than 0".to_string(),
            ));
        }
        checked_pixel_count(request.width, request.height, "CodexNativeBackend request")
            .map_err(|err| CoreError::InvalidGenerationRequest(err.to_string()))?;
        if request.prompt.trim().is_empty() {
            return Err(CoreError::InvalidGenerationRequest(
                "compiled prompt must not be empty".to_string(),
            ));
        }
        let mut resolved_refs = Vec::with_capacity(request.reference_images.len());
        for ref_path in &request.reference_images {
            let ref_bytes =
                read_regular_file_nofollow(ref_path, "CodexNativeBackend reference PNG")?;
            let _ = decode_png_checked(ref_path, &ref_bytes)?;
            let abs_ref = if ref_path.is_absolute() {
                ref_path.clone()
            } else {
                std::env::current_dir()
                    .map_err(|err| CoreError::Io {
                        context: "resolving working directory for CodexNativeBackend reference PNG"
                            .to_string(),
                        source: err,
                    })?
                    .join(ref_path)
            };
            resolved_refs.push(abs_ref);
        }

        let temp_codex_home = TempCodexHome::create(&self.auth_dir)?;
        let full_prompt = build_generation_prompt(request);

        let mut cmd = Command::new(&self.codex_bin);
        cmd.kill_on_drop(true);
        cmd.current_dir(temp_codex_home.path());
        cmd.args(["exec", "--ephemeral", "--json"]);
        for ref_path in &resolved_refs {
            cmd.arg("-i").arg(ref_path);
        }
        cmd.arg("--skip-git-repo-check");
        cmd.arg("--").arg(&full_prompt);
        cmd.env("CODEX_HOME", temp_codex_home.path());
        cmd.env_remove("OPENAI_API_KEY");
        cmd.stdin(Stdio::null());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        let output = cmd
            .output()
            .await
            .map_err(|err| CoreError::SubprocessFailed {
                stage: "CodexNativeBackend",
                reason: format!("failed to spawn '{}': {err}", self.codex_bin),
            })?;

        let stdout_str = String::from_utf8_lossy(&output.stdout);
        if !output.status.success() {
            let stderr_raw = String::from_utf8_lossy(&output.stderr);
            let sanitized_stderr = sanitize_single_line(&stderr_raw);
            let diagnostic = if sanitized_stderr.is_empty() {
                extract_failure_detail_from_jsonl(&stdout_str)
            } else {
                Some(truncate_detail(&sanitized_stderr, MAX_FAILURE_DETAIL_CHARS))
            };
            let reason = match diagnostic {
                Some(detail) => format!("exit status {}: {detail}", output.status),
                None => format!("exit status {}", output.status),
            };
            return Err(CoreError::SubprocessFailed {
                stage: "CodexNativeBackend",
                reason,
            });
        }

        let thread_id = extract_thread_id_from_jsonl(&stdout_str);

        let generated_root = temp_codex_home.path().join("generated_images");
        let png_path = match locate_generated_png(&generated_root, thread_id.as_deref()) {
            Ok(path) => path,
            Err(CoreError::GeneratedImageNotFound {
                search_root,
                thread_id,
                ..
            }) => {
                return Err(CoreError::GeneratedImageNotFound {
                    search_root,
                    thread_id,
                    detail: extract_failure_detail_from_jsonl(&stdout_str),
                });
            }
            Err(other) => return Err(other),
        };

        let raw_bytes = read_regular_png_nofollow(&png_path)?;

        let (png_bytes, raw_width, raw_height) =
            validate_and_resample_png(&png_path, &raw_bytes, request.width, request.height)?;

        Ok(ImageGenerationOutput {
            png_bytes,
            width: request.width,
            height: request.height,
            raw_width,
            raw_height,
            thread_id,
        })
    }
}

/// Extract a path-safe `thread_id` from `codex exec --json` JSONL stdout lines.
#[must_use]
pub fn extract_thread_id_from_jsonl(jsonl: &str) -> Option<String> {
    for line in jsonl.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) else {
            continue;
        };
        let candidate = value.get("thread_id").and_then(|v| v.as_str()).or_else(|| {
            value
                .get("thread")
                .and_then(|t| t.get("id"))
                .and_then(|v| v.as_str())
        });
        if let Some(id) = candidate
            && is_safe_thread_id(id)
        {
            return Some(id.to_string());
        }
    }
    None
}

fn is_safe_thread_id(raw: &str) -> bool {
    !raw.is_empty()
        && raw
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Extract a concise, single-line sanitized diagnostic summary (`<= 300` chars)
/// from the last `error`, `turn.failed`, or `agent_message` event in `codex exec --json`
/// JSONL output.
#[must_use]
pub fn extract_failure_detail_from_jsonl(jsonl: &str) -> Option<String> {
    let mut last_detail: Option<String> = None;

    for line in jsonl.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) else {
            continue;
        };

        let event_type = value.get("type").and_then(|v| v.as_str());
        let candidate: Option<(&str, String)> = match event_type {
            Some("error") => extract_error_text(&value).map(|msg| ("error", msg)),
            Some("turn.failed") => extract_error_text(&value)
                .or_else(|| {
                    value
                        .get("reason")
                        .and_then(|v| v.as_str())
                        .map(str::to_string)
                })
                .map(|msg| ("turn.failed", msg)),
            Some("agent_message") => {
                extract_agent_message_text(&value).map(|msg| ("agent_message", msg))
            }
            _ => value
                .get("item")
                .filter(|item| item.get("type").and_then(|v| v.as_str()) == Some("agent_message"))
                .and_then(extract_agent_message_text)
                .map(|msg| ("agent_message", msg)),
        };

        if let Some((kind, raw_msg)) = candidate {
            let sanitized = sanitize_single_line(&raw_msg);
            if !sanitized.is_empty() {
                last_detail = Some(truncate_detail(
                    &format!("{kind}: {sanitized}"),
                    MAX_FAILURE_DETAIL_CHARS,
                ));
            }
        }
    }

    last_detail
}

fn extract_error_text(value: &serde_json::Value) -> Option<String> {
    value
        .get("error")
        .and_then(|err| {
            err.get("message")
                .and_then(|v| v.as_str())
                .or_else(|| err.as_str())
        })
        .or_else(|| value.get("message").and_then(|v| v.as_str()))
        .or_else(|| value.get("detail").and_then(|v| v.as_str()))
        .map(str::to_string)
}

fn extract_agent_message_text(value: &serde_json::Value) -> Option<String> {
    if let Some(text) = value
        .get("text")
        .and_then(|v| v.as_str())
        .or_else(|| value.get("message").and_then(|v| v.as_str()))
    {
        return Some(text.to_string());
    }

    if let Some(content) = value.get("content") {
        if let Some(text) = content.as_str() {
            return Some(text.to_string());
        }
        if let Some(parts) = content.as_array() {
            let joined: Vec<&str> = parts
                .iter()
                .filter_map(|part| part.get("text").and_then(|v| v.as_str()))
                .filter(|part| !part.trim().is_empty())
                .collect();
            if !joined.is_empty() {
                return Some(joined.join(" "));
            }
        }
    }

    None
}

fn sanitize_single_line(raw: &str) -> String {
    let replaced: String = raw
        .chars()
        .map(|ch| if ch.is_control() { ' ' } else { ch })
        .collect();
    replaced.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn truncate_detail(text: &str, max_chars: usize) -> String {
    let char_count = text.chars().count();
    if char_count <= max_chars {
        return text.to_string();
    }
    let keep = max_chars.saturating_sub(3);
    let mut truncated: String = text.chars().take(keep).collect();
    truncated.push_str("...");
    truncated
}

fn ensure_real_directory(dir: &Path, label: &str) -> Result<bool, CoreError> {
    let meta = match fs::symlink_metadata(dir) {
        Ok(m) => m,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(err) => {
            return Err(CoreError::Io {
                context: format!("inspecting {label} at {}", dir.display()),
                source: err,
            });
        }
    };

    if meta.file_type().is_symlink() || !meta.is_dir() {
        return Err(CoreError::InvalidPng {
            path: dir.to_path_buf(),
            reason: format!("refusing symlink or non-directory {label}"),
        });
    }

    Ok(true)
}

fn collect_png_candidates(
    dir: &Path,
    out: &mut Vec<(SystemTime, PathBuf)>,
) -> Result<(), CoreError> {
    let entries = fs::read_dir(dir).map_err(|err| CoreError::Io {
        context: format!("listing generated images directory {}", dir.display()),
        source: err,
    })?;

    for entry_res in entries {
        let entry = entry_res.map_err(|err| CoreError::Io {
            context: format!("reading entry in {}", dir.display()),
            source: err,
        })?;
        let path = entry.path();
        let meta = fs::symlink_metadata(&path).map_err(|err| CoreError::Io {
            context: format!("inspecting generated entry {}", path.display()),
            source: err,
        })?;

        if meta.file_type().is_symlink() {
            return Err(CoreError::InvalidPng {
                path,
                reason: "refusing symlink inside generated_images".to_string(),
            });
        }

        if meta.is_dir() {
            collect_png_candidates(&path, out)?;
        } else if !meta.is_file() {
            return Err(CoreError::InvalidPng {
                path,
                reason: "refusing non-regular file inside generated_images".to_string(),
            });
        } else if path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("png"))
        {
            let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
            out.push((mtime, path));
        }
    }

    Ok(())
}

fn pick_newest_png(dir: &Path, label: &str) -> Result<Option<PathBuf>, CoreError> {
    if !ensure_real_directory(dir, label)? {
        return Ok(None);
    }
    let mut candidates = Vec::new();
    collect_png_candidates(dir, &mut candidates)?;
    candidates.sort_by(|(mtime_a, path_a), (mtime_b, path_b)| {
        mtime_b.cmp(mtime_a).then_with(|| path_b.cmp(path_a))
    });
    Ok(candidates.into_iter().next().map(|(_, path)| path))
}

fn read_regular_png_nofollow(png_path: &Path) -> Result<Vec<u8>, CoreError> {
    read_regular_file_nofollow(png_path, "generated PNG").map_err(|err| match err {
        CoreError::Io { source, .. } if source.kind() == std::io::ErrorKind::InvalidInput => {
            CoreError::InvalidPng {
                path: png_path.to_path_buf(),
                reason: format!("refusing symlink or non-regular generated PNG: {source}"),
            }
        }
        #[cfg(unix)]
        CoreError::Io { source, .. } if source.raw_os_error() == Some(libc::ELOOP) => {
            CoreError::InvalidPng {
                path: png_path.to_path_buf(),
                reason: "refusing symlink generated PNG (O_NOFOLLOW)".to_string(),
            }
        }
        other => other,
    })
}

/// Locate the generated PNG under `$CODEX_HOME/generated_images/<thread_id>/`,
/// falling back to the newest `.png` anywhere under `$CODEX_HOME/generated_images/`.
pub fn locate_generated_png(
    generated_root: &Path,
    thread_id: Option<&str>,
) -> Result<PathBuf, CoreError> {
    if !ensure_real_directory(generated_root, "generated_images root")? {
        return Err(CoreError::GeneratedImageNotFound {
            search_root: generated_root.to_path_buf(),
            thread_id: thread_id.map(str::to_string),
            detail: None,
        });
    }

    if let Some(tid) = thread_id
        && is_safe_thread_id(tid)
    {
        let thread_dir = generated_root.join(tid);
        if let Some(found) = pick_newest_png(&thread_dir, "thread generated_images directory")? {
            return Ok(found);
        }
    }

    if let Some(fallback) = pick_newest_png(generated_root, "generated_images root")? {
        return Ok(fallback);
    }

    Err(CoreError::GeneratedImageNotFound {
        search_root: generated_root.to_path_buf(),
        thread_id: thread_id.map(str::to_string),
        detail: None,
    })
}

/// Validate the 8-byte PNG signature, decode, enforce `<= 5%` aspect-ratio error,
/// and Lanczos3-resample to exact `target_width x target_height`.
pub fn validate_and_resample_png(
    png_path: &Path,
    raw_bytes: &[u8],
    target_width: u32,
    target_height: u32,
) -> Result<(Vec<u8>, u32, u32), CoreError> {
    checked_pixel_count(target_width, target_height, "CodexNativeBackend target").map_err(
        |err| CoreError::InvalidPng {
            path: png_path.to_path_buf(),
            reason: err.to_string(),
        },
    )?;

    if raw_bytes.len() < PNG_SIGNATURE.len() || raw_bytes[..PNG_SIGNATURE.len()] != PNG_SIGNATURE {
        return Err(CoreError::InvalidPng {
            path: png_path.to_path_buf(),
            reason: "missing valid 8-byte PNG file signature".to_string(),
        });
    }

    let decoded = decode_png_checked(png_path, raw_bytes)?;

    let actual_width = decoded.width();
    let actual_height = decoded.height();
    if actual_width == 0 || actual_height == 0 {
        return Err(CoreError::InvalidPng {
            path: png_path.to_path_buf(),
            reason: "PNG image has zero width or height".to_string(),
        });
    }

    let requested_ar = f64::from(target_width) / f64::from(target_height);
    let actual_ar = f64::from(actual_width) / f64::from(actual_height);
    let relative_error = (actual_ar - requested_ar).abs() / requested_ar;
    if relative_error > MAX_ASPECT_RATIO_ERROR {
        return Err(CoreError::AspectRatioExceeded {
            requested_width: target_width,
            requested_height: target_height,
            actual_width,
            actual_height,
            error_percent: relative_error * 100.0,
        });
    }

    if actual_width == target_width && actual_height == target_height {
        return Ok((raw_bytes.to_vec(), actual_width, actual_height));
    }

    let resampled: DynamicImage =
        decoded.resize_exact(target_width, target_height, FilterType::Lanczos3);
    let mut encoded = Vec::new();
    resampled
        .write_to(&mut Cursor::new(&mut encoded), ImageFormat::Png)
        .map_err(|err| CoreError::InvalidPng {
            path: png_path.to_path_buf(),
            reason: format!("failed to re-encode Lanczos-resampled PNG: {err}"),
        })?;

    Ok((encoded, actual_width, actual_height))
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use std::os::unix::fs::{PermissionsExt, symlink};

    use image::{ImageBuffer, Rgba};
    use rstest::rstest;
    use tempfile::TempDir;

    use super::*;

    fn create_secure_auth_dir(dir: &Path) {
        fs::create_dir_all(dir).unwrap();
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).unwrap();
        let auth_file = dir.join("auth.json");
        fs::write(&auth_file, r#"{"token":"oauth"}"#).unwrap();
        fs::set_permissions(&auth_file, fs::Permissions::from_mode(0o600)).unwrap();
    }

    fn make_solid_png(width: u32, height: u32) -> Vec<u8> {
        let img: ImageBuffer<Rgba<u8>, Vec<u8>> =
            ImageBuffer::from_pixel(width, height, Rgba([64, 128, 192, 255]));
        let mut buf = Vec::new();
        DynamicImage::ImageRgba8(img)
            .write_to(&mut Cursor::new(&mut buf), ImageFormat::Png)
            .expect("encode PNG");
        buf
    }

    #[rstest]
    #[case::top_level_thread_id(
        r#"{"type":"thread.started","thread_id":"thr_abc123"}"#,
        Some("thr_abc123")
    )]
    #[case::nested_thread_object(
        "not json\n{\"type\":\"session\",\"thread\":{\"id\":\"thr-nested_9\"}}\n",
        Some("thr-nested_9")
    )]
    #[case::traversal_thread_id_rejected(
        r#"{"type":"thread.started","thread_id":"../../etc"}"#,
        None
    )]
    #[case::missing_thread_id(r#"{"type":"turn.completed"}"#, None)]
    fn parses_jsonl_thread_id_safely(#[case] jsonl: &str, #[case] expected: Option<&str>) {
        assert_eq!(extract_thread_id_from_jsonl(jsonl).as_deref(), expected);
    }

    #[rstest]
    #[case::item_completed_agent_message(
        "{\"type\":\"thread.started\",\"thread_id\":\"thr_1\"}\n{\"type\":\"item.completed\",\"item\":{\"id\":\"item_0\",\"type\":\"agent_message\",\"text\":\"Please clarify the target region.\"}}\n{\"type\":\"turn.completed\"}\n",
        Some("agent_message: Please clarify the target region.")
    )]
    #[case::direct_agent_message_multiline_sanitized(
        "{\"type\":\"agent_message\",\"text\":\"Line one\\n\\tLine\\u0007 two\"}\n",
        Some("agent_message: Line one Line two")
    )]
    #[case::turn_failed_after_agent_message_picks_last(
        "{\"type\":\"item.completed\",\"item\":{\"type\":\"agent_message\",\"text\":\"Working on it\"}}\n{\"type\":\"turn.failed\",\"error\":{\"message\":\"image_gen tool timed out\"}}\n",
        Some("turn.failed: image_gen tool timed out")
    )]
    #[case::error_event_overrides_prior_message(
        "{\"type\":\"error\",\"message\":\"stream interrupted\"}\n",
        Some("error: stream interrupted")
    )]
    #[case::unrelated_or_malformed_lines_return_none(
        "not json\n{\"type\":\"thread.started\",\"thread_id\":\"thr_1\"}\n{\"type\":\"turn.completed\"}\n",
        None
    )]
    fn extracts_failure_detail_from_jsonl_events(
        #[case] jsonl: &str,
        #[case] expected: Option<&str>,
    ) {
        assert_eq!(
            extract_failure_detail_from_jsonl(jsonl).as_deref(),
            expected
        );
    }

    #[test]
    fn extract_failure_detail_from_jsonl_bounds_long_messages_to_300_chars() {
        let long_text = "x".repeat(500);
        let jsonl = format!(r#"{{"type":"error","message":"{long_text}"}}"#);
        let detail = extract_failure_detail_from_jsonl(&jsonl).expect("detail should be present");
        assert_eq!(detail.chars().count(), MAX_FAILURE_DETAIL_CHARS);
        assert!(detail.starts_with("error: "));
        assert!(detail.ends_with("..."));
    }

    #[rstest]
    #[case::exact_dimensions(1024, 1024, 1024, 1024, true)]
    #[case::scaled_square_codex_quirk_1254(1254, 1254, 1024, 1024, true)]
    #[case::within_5_percent_ar_tolerance(1040, 1000, 1000, 1000, true)]
    #[case::exceeds_5_percent_ar_tolerance(1100, 1000, 1000, 1000, false)]
    #[case::landscape_vs_square_rejected(1536, 1024, 1024, 1024, false)]
    fn enforces_aspect_ratio_and_lanczos_resampling(
        #[case] actual_w: u32,
        #[case] actual_h: u32,
        #[case] target_w: u32,
        #[case] target_h: u32,
        #[case] expect_ok: bool,
    ) {
        let raw = make_solid_png(actual_w, actual_h);
        let result = validate_and_resample_png(Path::new("out.png"), &raw, target_w, target_h);
        if expect_ok {
            let (out_bytes, raw_w, raw_h) = result.expect("within tolerance should succeed");
            assert_eq!((raw_w, raw_h), (actual_w, actual_h));
            let decoded = image::load_from_memory_with_format(&out_bytes, ImageFormat::Png)
                .expect("output must decode as PNG");
            assert_eq!((decoded.width(), decoded.height()), (target_w, target_h));
        } else {
            let err = result.expect_err("aspect ratio > 5% must fail closed");
            assert!(matches!(err, CoreError::AspectRatioExceeded { .. }));
        }
    }

    #[test]
    fn rejects_non_png_signature_fail_closed() {
        let err = validate_and_resample_png(Path::new("fake.png"), b"NOT_A_PNG_FILE", 512, 512)
            .expect_err("invalid signature must fail closed");
        assert!(matches!(err, CoreError::InvalidPng { .. }));
    }

    #[tokio::test]
    async fn codex_native_backend_uses_isolated_codex_home_thread_id_and_cleans_up() {
        let root = TempDir::new().expect("tempdir");
        let auth_dir = root.path().join("auth");
        create_secure_auth_dir(&auth_dir);

        let ref_img = root.path().join("ref.png");
        fs::write(&ref_img, make_solid_png(64, 64)).unwrap();

        let generated_png_fixture = root.path().join("fixture_1254.png");
        fs::write(&generated_png_fixture, make_solid_png(1254, 1254)).unwrap();

        let recorded_codex_home = root.path().join("codex_home.txt");
        let recorded_cwd = root.path().join("codex_cwd.txt");
        let recorded_args = root.path().join("codex_args.txt");
        let stub_bin = root.path().join("codex-stub");

        let script = format!(
            r#"#!/usr/bin/env bash
set -euo pipefail
printf '%s' "$CODEX_HOME" > "{home_log}"
pwd > "{cwd_log}"
printf '%s\n' "$@" > "{args_log}"
if [ -n "${{OPENAI_API_KEY:-}}" ]; then
  echo "OPENAI_API_KEY must be unset" >&2
  exit 9
fi
mkdir -p "$CODEX_HOME/generated_images/thr_001"
cp "{fixture}" "$CODEX_HOME/generated_images/thr_001/out.png"
printf '{{"type":"thread.started","thread_id":"thr_001"}}\n'
"#,
            home_log = recorded_codex_home.display(),
            cwd_log = recorded_cwd.display(),
            args_log = recorded_args.display(),
            fixture = generated_png_fixture.display()
        );
        fs::write(&stub_bin, script).unwrap();
        fs::set_permissions(&stub_bin, fs::Permissions::from_mode(0o755)).unwrap();

        let backend = CodexNativeBackend::with_codex_bin(
            auth_dir.clone(),
            stub_bin.to_string_lossy().into_owned(),
        );
        let output = backend
            .generate(&ImageGenerationRequest {
                prompt: "clean ink wash harbor pier".to_string(),
                width: 1024,
                height: 1024,
                reference_images: vec![ref_img.clone()],
            })
            .await
            .expect("stub generation should succeed");

        assert_eq!(output.width, 1024);
        assert_eq!(output.height, 1024);
        assert_eq!((output.raw_width, output.raw_height), (1254, 1254));
        assert_eq!(output.thread_id.as_deref(), Some("thr_001"));

        let decoded = image::load_from_memory_with_format(&output.png_bytes, ImageFormat::Png)
            .expect("valid PNG");
        assert_eq!((decoded.width(), decoded.height()), (1024, 1024));

        // Verify RAII cleanup of isolated CODEX_HOME and CWD isolation
        let used_home = PathBuf::from(fs::read_to_string(&recorded_codex_home).unwrap());
        let used_cwd = PathBuf::from(fs::read_to_string(&recorded_cwd).unwrap().trim());
        assert_eq!(used_cwd, used_home);
        assert_ne!(used_home, auth_dir);
        assert!(
            !used_home.exists(),
            "TempCodexHome must be removed after generation finishes"
        );

        let args = fs::read_to_string(&recorded_args).unwrap();
        assert!(args.contains("exec\n--ephemeral\n--json\n-i\n"));
        assert!(args.contains("--skip-git-repo-check"));
        assert!(args.contains(&ref_img.to_string_lossy().into_owned()));
        assert!(args.contains("1024x1024"));
        assert!(args.contains("1:1"));
        assert!(args.contains("image_gen"));
        assert!(args.contains("attached reference"));
    }

    #[rstest]
    #[case::agent_message_text_only(
        r#"{"type":"item.completed","item":{"id":"item_0","type":"agent_message","text":"Which brush style would you prefer?"}}"#,
        "agent_message: Which brush style would you prefer?"
    )]
    #[case::turn_failed_reason(
        r#"{"type":"turn.failed","error":{"message":"image_gen call was skipped"}}"#,
        "turn.failed: image_gen call was skipped"
    )]
    #[tokio::test]
    async fn codex_native_backend_surfaces_jsonl_diagnostic_when_no_png_found_and_cleans_up(
        #[case] event_line: &str,
        #[case] expected_detail: &str,
    ) {
        let root = TempDir::new().expect("tempdir");
        let auth_dir = root.path().join("auth");
        create_secure_auth_dir(&auth_dir);

        let recorded_codex_home = root.path().join("codex_home_diag.txt");
        let stub_bin = root.path().join("codex-no-png");
        let script = format!(
            r#"#!/usr/bin/env bash
set -euo pipefail
printf '%s' "$CODEX_HOME" > "{home_log}"
printf '{{"type":"thread.started","thread_id":"thr_diag"}}\n'
printf '%s\n' '{event_line}'
"#,
            home_log = recorded_codex_home.display(),
            event_line = event_line
        );
        fs::write(&stub_bin, script).unwrap();
        fs::set_permissions(&stub_bin, fs::Permissions::from_mode(0o755)).unwrap();

        let backend =
            CodexNativeBackend::with_codex_bin(auth_dir, stub_bin.to_string_lossy().into_owned());
        let err = backend
            .generate(&ImageGenerationRequest {
                prompt: "harbor lantern".to_string(),
                width: 512,
                height: 512,
                reference_images: Vec::new(),
            })
            .await
            .expect_err("missing generated PNG must fail closed with diagnostic detail");

        match &err {
            CoreError::GeneratedImageNotFound {
                thread_id, detail, ..
            } => {
                assert_eq!(thread_id.as_deref(), Some("thr_diag"));
                assert_eq!(detail.as_deref(), Some(expected_detail));
            }
            other => panic!("expected GeneratedImageNotFound, got {other:?}"),
        }
        assert!(err.to_string().contains(expected_detail));

        let used_home = PathBuf::from(fs::read_to_string(&recorded_codex_home).unwrap());
        assert!(
            !used_home.exists(),
            "TempCodexHome must be removed even when GeneratedImageNotFound occurs"
        );
    }

    #[tokio::test]
    async fn codex_native_backend_uses_newest_png_fallback_when_thread_id_absent() {
        let root = TempDir::new().expect("tempdir");
        let auth_dir = root.path().join("auth");
        create_secure_auth_dir(&auth_dir);

        let older_fixture = root.path().join("older.png");
        let newer_fixture = root.path().join("newer.png");
        fs::write(&older_fixture, make_solid_png(512, 512)).unwrap();
        fs::write(&newer_fixture, make_solid_png(256, 256)).unwrap();

        let stub_bin = root.path().join("codex-fallback");
        let script = format!(
            r#"#!/usr/bin/env bash
set -euo pipefail
mkdir -p "$CODEX_HOME/generated_images/misc"
cp "{older}" "$CODEX_HOME/generated_images/misc/a.png"
touch -d "2025-01-01 00:00:00 UTC" "$CODEX_HOME/generated_images/misc/a.png"
cp "{newer}" "$CODEX_HOME/generated_images/misc/b.png"
printf '{{"type":"turn.completed"}}\n'
"#,
            older = older_fixture.display(),
            newer = newer_fixture.display()
        );
        fs::write(&stub_bin, script).unwrap();
        fs::set_permissions(&stub_bin, fs::Permissions::from_mode(0o755)).unwrap();

        let backend =
            CodexNativeBackend::with_codex_bin(auth_dir, stub_bin.to_string_lossy().into_owned());
        let output = backend
            .generate(&ImageGenerationRequest {
                prompt: "fallback test".to_string(),
                width: 256,
                height: 256,
                reference_images: Vec::new(),
            })
            .await
            .expect("newest PNG fallback should succeed");

        assert_eq!(output.thread_id, None);
        assert_eq!((output.raw_width, output.raw_height), (256, 256));
    }

    #[tokio::test]
    async fn codex_native_backend_fails_closed_when_auth_missing_and_cleans_up_on_error() {
        let root = TempDir::new().expect("tempdir");
        let missing_auth = root.path().join("missing-auth");
        let backend = CodexNativeBackend::new(missing_auth);

        let err = backend
            .generate(&ImageGenerationRequest {
                prompt: "harbor dawn".to_string(),
                width: 512,
                height: 512,
                reference_images: Vec::new(),
            })
            .await
            .expect_err("missing auth must fail closed");
        assert!(matches!(err, CoreError::AuthMissing { .. }));
    }

    #[tokio::test]
    async fn codex_native_backend_cleans_up_temp_codex_home_when_aspect_ratio_fails() {
        let root = TempDir::new().expect("tempdir");
        let auth_dir = root.path().join("auth");
        create_secure_auth_dir(&auth_dir);

        let bad_ar_fixture = root.path().join("bad_ar.png");
        fs::write(&bad_ar_fixture, make_solid_png(1600, 800)).unwrap();

        let recorded_codex_home = root.path().join("codex_home_err.txt");
        let stub_bin = root.path().join("codex-bad-ar");
        let script = format!(
            r#"#!/usr/bin/env bash
set -euo pipefail
printf '%s' "$CODEX_HOME" > "{home_log}"
mkdir -p "$CODEX_HOME/generated_images/thr_err"
cp "{fixture}" "$CODEX_HOME/generated_images/thr_err/out.png"
printf '{{"type":"thread.started","thread_id":"thr_err"}}\n'
"#,
            home_log = recorded_codex_home.display(),
            fixture = bad_ar_fixture.display()
        );
        fs::write(&stub_bin, script).unwrap();
        fs::set_permissions(&stub_bin, fs::Permissions::from_mode(0o755)).unwrap();

        let backend =
            CodexNativeBackend::with_codex_bin(auth_dir, stub_bin.to_string_lossy().into_owned());
        let err = backend
            .generate(&ImageGenerationRequest {
                prompt: "bad aspect ratio test".to_string(),
                width: 1024,
                height: 1024,
                reference_images: Vec::new(),
            })
            .await
            .expect_err("aspect ratio > 5% must fail closed");
        assert!(matches!(err, CoreError::AspectRatioExceeded { .. }));

        let used_home = PathBuf::from(fs::read_to_string(&recorded_codex_home).unwrap());
        assert!(
            !used_home.exists(),
            "TempCodexHome must be removed even when aspect-ratio validation fails"
        );
    }

    #[rstest]
    #[case::widescreen_with_refs(1280, 720, true, "1280x720", "16:9", true)]
    #[case::square_without_refs(1024, 1024, false, "1024x1024", "1:1", false)]
    fn generation_prompt_includes_target_dimensions_and_aspect_ratio(
        #[case] width: u32,
        #[case] height: u32,
        #[case] with_refs: bool,
        #[case] expected_dim: &str,
        #[case] expected_ar: &str,
        #[case] expect_ref_phrase: bool,
    ) {
        let refs = if with_refs {
            vec![PathBuf::from("ref.png")]
        } else {
            Vec::new()
        };
        let prompt = build_generation_prompt(&ImageGenerationRequest {
            prompt: "harbor pier at dawn".to_string(),
            width,
            height,
            reference_images: refs,
        });
        assert!(prompt.contains(expected_dim));
        assert!(prompt.contains(expected_ar));
        assert!(prompt.contains("image_gen"));
        assert!(prompt.contains("unlabeled surfaces"));
        assert_eq!(prompt.contains("attached reference"), expect_ref_phrase);

        let header = prompt
            .split("\n\n")
            .next()
            .expect("generated prompt must include [RENDER_TARGET] header");
        let lower = header.to_ascii_lowercase();
        let tokens: Vec<&str> = lower
            .split(|c: char| !c.is_ascii_alphanumeric() && c != '\'')
            .map(|tok| tok.trim_matches('\''))
            .filter(|tok| !tok.is_empty())
            .collect();
        for forbidden in [
            "no", "not", "never", "without", "don't", "avoid", "avoiding",
        ] {
            assert!(
                !tokens.contains(&forbidden),
                "generated header must remain positive-only (found '{forbidden}'): {header}"
            );
        }
        assert!(
            !tokens
                .windows(2)
                .any(|pair| pair == ["do", "not"] || pair == ["free", "of"]),
            "generated header must remain positive-only (found negation phrase): {header}"
        );
    }

    #[rstest]
    #[case::stderr_multiline_control_chars_and_bounded(
        format!("line1\t\x07line2\n{}", "a".repeat(350)),
        r#"{"type":"error","message":"ignored when stderr present"}"#,
        Some("line1 line2 "),
        true,
    )]
    #[case::empty_stderr_falls_back_to_jsonl(
        String::new(),
        r#"{"type":"turn.failed","error":{"message":"image_gen quota exhausted"}}"#,
        Some("turn.failed: image_gen quota exhausted"),
        false
    )]
    #[case::whitespace_only_stderr_falls_back_to_jsonl(
        "  \n\t\x07 ".to_string(),
        r#"{"type":"error","message":"auth session expired"}"#,
        Some("error: auth session expired"),
        false,
    )]
    #[case::empty_stderr_and_no_jsonl_diagnostic(
        String::new(),
        r#"{"type":"turn.completed"}"#,
        None,
        false
    )]
    #[tokio::test]
    async fn codex_native_backend_sanitizes_stderr_and_falls_back_to_jsonl_on_nonzero_exit(
        #[case] stderr_payload: String,
        #[case] stdout_payload: &str,
        #[case] expected_substring: Option<&str>,
        #[case] expect_bounded_stderr: bool,
    ) {
        let root = TempDir::new().expect("tempdir");
        let auth_dir = root.path().join("auth");
        create_secure_auth_dir(&auth_dir);

        let stderr_file = root.path().join("stderr.bin");
        let stdout_file = root.path().join("stdout.bin");
        fs::write(&stderr_file, stderr_payload.as_bytes()).unwrap();
        fs::write(&stdout_file, stdout_payload.as_bytes()).unwrap();

        let stub_bin = root.path().join("codex-fail");
        let script = format!(
            r#"#!/usr/bin/env bash
set -euo pipefail
cat "{stdout_file}"
cat "{stderr_file}" >&2
exit 7
"#,
            stdout_file = stdout_file.display(),
            stderr_file = stderr_file.display(),
        );
        fs::write(&stub_bin, script).unwrap();
        fs::set_permissions(&stub_bin, fs::Permissions::from_mode(0o755)).unwrap();

        let backend =
            CodexNativeBackend::with_codex_bin(auth_dir, stub_bin.to_string_lossy().into_owned());
        let err = backend
            .generate(&ImageGenerationRequest {
                prompt: "harbor lantern".to_string(),
                width: 512,
                height: 512,
                reference_images: Vec::new(),
            })
            .await
            .expect_err("non-zero exit must fail closed");

        match err {
            CoreError::SubprocessFailed { stage, reason } => {
                assert_eq!(stage, "CodexNativeBackend");
                assert!(
                    !reason.chars().any(char::is_control),
                    "reason must not contain control characters: {reason:?}"
                );
                if let Some(substr) = expected_substring {
                    assert!(
                        reason.contains(substr),
                        "expected {substr:?} in reason {reason:?}"
                    );
                } else {
                    assert_eq!(reason, "exit status exit status: 7");
                }
                if expect_bounded_stderr {
                    let detail = reason
                        .strip_prefix("exit status exit status: 7: ")
                        .expect("reason should contain sanitized stderr detail");
                    assert_eq!(detail.chars().count(), MAX_FAILURE_DETAIL_CHARS);
                    assert!(detail.ends_with("..."));
                }
            }
            other => panic!("expected SubprocessFailed, got {other:?}"),
        }
    }

    #[rstest]
    #[case::symlink_generated_images_root("root_symlink")]
    #[case::non_directory_generated_images_root("root_file")]
    #[case::symlink_thread_dir("thread_symlink")]
    #[case::symlink_nested_dir("nested_dir_symlink")]
    #[case::symlink_candidate_png("png_symlink")]
    fn locate_generated_png_rejects_symlinks_and_non_directories_fail_closed(
        #[case] scenario: &str,
    ) {
        let root = TempDir::new().expect("tempdir");
        let outside_dir = root.path().join("outside");
        fs::create_dir_all(&outside_dir).unwrap();
        let outside_png = outside_dir.join("secret.png");
        fs::write(&outside_png, make_solid_png(64, 64)).unwrap();

        let generated_root = root.path().join("generated_images");
        match scenario {
            "root_symlink" => {
                symlink(&outside_dir, &generated_root).unwrap();
            }
            "root_file" => {
                fs::write(&generated_root, b"not a dir").unwrap();
            }
            "thread_symlink" => {
                fs::create_dir_all(&generated_root).unwrap();
                symlink(&outside_dir, generated_root.join("thr_1")).unwrap();
            }
            "nested_dir_symlink" => {
                fs::create_dir_all(generated_root.join("thr_1")).unwrap();
                symlink(&outside_dir, generated_root.join("thr_1/sub")).unwrap();
            }
            "png_symlink" => {
                fs::create_dir_all(generated_root.join("thr_1")).unwrap();
                symlink(&outside_png, generated_root.join("thr_1/out.png")).unwrap();
            }
            other => panic!("unexpected scenario: {other}"),
        }

        let err = locate_generated_png(&generated_root, Some("thr_1"))
            .expect_err("symlink or non-directory component must fail closed");
        assert!(matches!(err, CoreError::InvalidPng { .. }));
    }

    #[test]
    fn read_regular_png_nofollow_rejects_symlink_png() {
        let root = TempDir::new().expect("tempdir");
        let real_png = root.path().join("real.png");
        fs::write(&real_png, make_solid_png(32, 32)).unwrap();
        let link_png = root.path().join("link.png");
        symlink(&real_png, &link_png).unwrap();

        let err = read_regular_png_nofollow(&link_png)
            .expect_err("symlinked PNG must be rejected fail-closed");
        assert!(matches!(err, CoreError::InvalidPng { .. }));
    }

    #[rstest]
    #[case::zero_width(0, 512)]
    #[case::exceeds_max_dim(16_385, 512)]
    #[case::exceeds_total_pixels(16_384, 16_384)]
    fn validate_and_resample_png_rejects_oversized_target_dimensions_before_resizing(
        #[case] target_w: u32,
        #[case] target_h: u32,
    ) {
        let raw = make_solid_png(64, 64);
        let err = validate_and_resample_png(Path::new("out.png"), &raw, target_w, target_h)
            .expect_err("oversized or zero target dimensions must fail closed");
        assert!(matches!(err, CoreError::InvalidPng { .. }));
    }

    #[cfg(unix)]
    #[rstest]
    #[case::symlink_reference("symlink_ref")]
    #[case::non_png_reference("non_png_ref")]
    #[case::oversized_target("oversized_target")]
    #[tokio::test]
    async fn codex_native_backend_rejects_invalid_refs_and_oversized_dims_before_spawning(
        #[case] case_kind: &str,
    ) {
        let root = TempDir::new().expect("tempdir");
        let auth_dir = root.path().join("auth");
        create_secure_auth_dir(&auth_dir);

        let valid_png = root.path().join("valid.png");
        fs::write(&valid_png, make_solid_png(32, 32)).unwrap();
        let bad_png = root.path().join("bad.png");
        fs::write(&bad_png, b"NOT_A_PNG").unwrap();
        let link_png = root.path().join("link.png");
        symlink(&valid_png, &link_png).unwrap();

        let spawned_marker = root.path().join("codex_spawned.marker");
        let stub_bin = root.path().join("codex-sentinel");
        fs::write(
            &stub_bin,
            format!(
                "#!/usr/bin/env bash\ntouch \"{}\"\nexit 1\n",
                spawned_marker.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&stub_bin, fs::Permissions::from_mode(0o755)).unwrap();

        let (width, height, refs) = match case_kind {
            "symlink_ref" => (512, 512, vec![link_png]),
            "non_png_ref" => (512, 512, vec![bad_png]),
            "oversized_target" => (16_384, 16_384, Vec::new()),
            _ => unreachable!(),
        };

        let backend =
            CodexNativeBackend::with_codex_bin(auth_dir, stub_bin.to_string_lossy().into_owned());
        let err = backend
            .generate(&ImageGenerationRequest {
                prompt: "clean pier".to_string(),
                width,
                height,
                reference_images: refs,
            })
            .await
            .expect_err("unsafe reference or oversized dimensions must fail closed");
        match case_kind {
            "symlink_ref" => assert!(matches!(err, CoreError::Io { .. })),
            "non_png_ref" => assert!(matches!(err, CoreError::InvalidPng { .. })),
            "oversized_target" => assert!(matches!(err, CoreError::InvalidGenerationRequest(_))),
            _ => unreachable!(),
        }
        assert!(
            !spawned_marker.exists(),
            "codex subprocess must not be spawned on invalid refs or dimensions"
        );
    }
}
