//! === STRUCTURED READ-ONLY VISION CRITIC ===
//!
//! PURPOSE:
//! Evaluates a generated `Run` image against reference anchors using a binary
//! per-trait checklist (`codex exec -s read-only --ephemeral --output-schema ... -i <out> -i <ref>...`)
//! and returns a fail-closed `CriticReport`.
//!
//! THREAT MODEL / DESIGN RATIONALE ("WHY"):
//! Unstructured VLM critiques frequently produce vague praise while missing swapped
//! hand props, malformed weapon geometry, style drift, or leaked UI text. Enforcing
//! a strict JSON schema (`additionalProperties: false`, `#[serde(deny_unknown_fields)]`)
//! with explicit boolean per-trait results and verifying `report.passed == all(checks)`
//! prevents contradictory or rubber-stamped `CriticReport` verdicts.
//!
//! EXECUTION LIFECYCLE / DATA FLOW:
//! 1. Validate target image, reference images, and non-empty binary trait checklist.
//! 2. Materialize isolated `TempCodexHome` and temporary JSON schema / output paths.
//! 3. Execute `codex exec -s read-only --ephemeral --output-schema <schema> -o <out> -i <target> [-i <ref>...] -- <prompt>`.
//! 4. Deserialize `CriticReport`, verify `schema_version == 1` and `passed` consistency.

use std::fmt::Write as _;
use std::fs;
use std::path::PathBuf;
use std::process::Stdio;

use serde::{Deserialize, Serialize};
use tempfile::TempDir;
use tokio::process::Command;

use crate::auth::{DEFAULT_CODEX_BIN, TempCodexHome};
use crate::composer::read_structured_json_output;
use crate::error::CoreError;

/// Supported `CriticReport` schema version.
pub const SUPPORTED_CRITIC_REPORT_SCHEMA_VERSION: u32 = 1;

/// Strict JSON Schema passed to `codex exec --output-schema` for `CriticReport`.
pub const CRITIC_REPORT_OUTPUT_SCHEMA: &str = r#"{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "title": "CriticReport",
  "type": "object",
  "additionalProperties": false,
  "required": ["schema_version", "passed", "checks", "summary"],
  "properties": {
    "schema_version": { "type": "integer", "enum": [1] },
    "passed": { "type": "boolean" },
    "checks": {
      "type": "array",
      "minItems": 1,
      "items": {
        "type": "object",
        "additionalProperties": false,
        "required": ["trait_id", "category", "passed", "observation"],
        "properties": {
          "trait_id": { "type": "string", "minLength": 1 },
          "category": {
            "type": "string",
            "enum": ["identity", "hand_props", "weapon_geometry", "style_adherence", "zero_leaked_text"]
          },
          "passed": { "type": "boolean" },
          "observation": { "type": "string", "minLength": 1 }
        }
      }
    },
    "summary": { "type": "string", "minLength": 1 }
  }
}"#;

/// Canonical inspection category matching `GLOSSARY.md` (`VisionCritic`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TraitCategory {
    /// `Subject` facial, silhouette, and costume identity consistency.
    Identity,
    /// Left/right hand prop placement and grip correctness.
    HandProps,
    /// Structural integrity and alignment of held weapons or tools.
    WeaponGeometry,
    /// Adherence to target `Style` rendering medium and linework.
    StyleAdherence,
    /// Absence of stray watermarks, captions, or leaked prompt text.
    ZeroLeakedText,
}

/// Input trait item evaluated by `VisionCritic`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraitCheckInput {
    /// Stable identifier for the trait (for example `"left-hand-lantern"`).
    pub trait_id: String,
    /// Inspection category.
    pub category: TraitCategory,
    /// Concrete binary verification criterion.
    pub criterion: String,
}

/// Request payload for `VisionCritic::evaluate`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VisionCriticRequest {
    /// Path to the generated target image (`-i <out>`).
    pub target_image: PathBuf,
    /// Paths to reference anchor images (`-i <ref>...`).
    pub reference_images: Vec<PathBuf>,
    /// Binary per-trait checklist items to evaluate.
    pub traits: Vec<TraitCheckInput>,
}

/// Binary pass/fail evaluation for a single trait in `CriticReport`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TraitCheckResult {
    /// Stable identifier matching the requested checklist item.
    pub trait_id: String,
    /// Inspection category.
    pub category: TraitCategory,
    /// Binary verdict (`true` only when fully satisfied).
    pub passed: bool,
    /// Concise visual observation explaining the verdict.
    pub observation: String,
}

/// Structured output returned by `VisionCritic::evaluate`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CriticReport {
    /// Schema version; must equal `1`.
    pub schema_version: u32,
    /// Overall pass verdict; must equal `checks.iter().all(|c| c.passed)`.
    pub passed: bool,
    /// Per-trait binary verdicts.
    pub checks: Vec<TraitCheckResult>,
    /// Concise overall inspection summary.
    pub summary: String,
}

/// Structured read-only VLM inspection stage (`codex exec -s read-only --output-schema`).
#[derive(Debug, Clone)]
pub struct VisionCritic {
    auth_dir: PathBuf,
    codex_bin: String,
}

impl VisionCritic {
    /// Construct a `VisionCritic` using tool-local `auth_dir` and default `codex` binary.
    #[must_use]
    pub fn new(auth_dir: PathBuf) -> Self {
        Self {
            auth_dir,
            codex_bin: DEFAULT_CODEX_BIN.to_string(),
        }
    }

    /// Construct a `VisionCritic` with an explicit `codex` binary path (useful for test stubs).
    #[must_use]
    pub fn with_codex_bin(auth_dir: PathBuf, codex_bin: impl Into<String>) -> Self {
        Self {
            auth_dir,
            codex_bin: codex_bin.into(),
        }
    }

    /// Evaluate `request.target_image` against `request.reference_images` using binary trait checks.
    pub async fn evaluate(&self, request: &VisionCriticRequest) -> Result<CriticReport, CoreError> {
        if !request.target_image.is_file() {
            return Err(CoreError::InvalidGenerationRequest(format!(
                "target image does not exist or is not a regular file: {}",
                request.target_image.display()
            )));
        }
        for ref_path in &request.reference_images {
            if !ref_path.is_file() {
                return Err(CoreError::InvalidGenerationRequest(format!(
                    "reference image does not exist or is not a regular file: {}",
                    ref_path.display()
                )));
            }
        }
        if request.traits.is_empty() {
            return Err(CoreError::InvalidGenerationRequest(
                "VisionCritic requires at least 1 trait check item".to_string(),
            ));
        }
        for item in &request.traits {
            if item.trait_id.trim().is_empty() || item.criterion.trim().is_empty() {
                return Err(CoreError::InvalidGenerationRequest(
                    "VisionCritic trait_id and criterion must not be empty".to_string(),
                ));
            }
        }

        let temp_codex_home = TempCodexHome::create(&self.auth_dir)?;
        let io_dir = TempDir::new().map_err(|err| CoreError::Io {
            context: "creating temporary VisionCritic schema directory".to_string(),
            source: err,
        })?;
        let schema_path = io_dir.path().join("critic_report.schema.json");
        let out_path = io_dir.path().join("critic_report.out.json");
        fs::write(&schema_path, CRITIC_REPORT_OUTPUT_SCHEMA).map_err(|err| CoreError::Io {
            context: format!("writing {}", schema_path.display()),
            source: err,
        })?;

        let prompt = build_critic_prompt(&request.traits);

        let mut cmd = Command::new(&self.codex_bin);
        cmd.args(["exec", "-s", "read-only", "--ephemeral", "--output-schema"])
            .arg(&schema_path)
            .arg("-o")
            .arg(&out_path)
            .arg("-i")
            .arg(&request.target_image);
        for ref_path in &request.reference_images {
            cmd.arg("-i").arg(ref_path);
        }
        cmd.arg("--").arg(&prompt);
        cmd.env("CODEX_HOME", temp_codex_home.path());
        cmd.env_remove("OPENAI_API_KEY");
        cmd.stdin(Stdio::null());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        let output = cmd
            .output()
            .await
            .map_err(|err| CoreError::SubprocessFailed {
                stage: "VisionCritic",
                reason: format!("failed to spawn '{}': {err}", self.codex_bin),
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let detail = if stderr.is_empty() {
                format!("exit status {}", output.status)
            } else {
                format!("exit status {}: {stderr}", output.status)
            };
            return Err(CoreError::SubprocessFailed {
                stage: "VisionCritic",
                reason: detail,
            });
        }

        let raw_json = read_structured_json_output(&out_path, &output.stdout, "VisionCritic")?;
        validate_critic_report_json(&raw_json)
    }
}

fn build_critic_prompt(traits: &[TraitCheckInput]) -> String {
    let mut prompt = String::from(
        "Inspect the first attached target image against the subsequent reference anchor image(s).\n\
         Evaluate each binary trait below strictly as true (fully satisfied) or false (any defect).\n\
         Set top-level passed=true ONLY if every trait check passed=true.\n\n\
         Checklist:\n",
    );
    for item in traits {
        let cat = match item.category {
            TraitCategory::Identity => "identity",
            TraitCategory::HandProps => "hand_props",
            TraitCategory::WeaponGeometry => "weapon_geometry",
            TraitCategory::StyleAdherence => "style_adherence",
            TraitCategory::ZeroLeakedText => "zero_leaked_text",
        };
        let _ = writeln!(
            prompt,
            "- [{}] ({}): {}",
            item.trait_id.trim(),
            cat,
            item.criterion.trim()
        );
    }
    prompt
}

/// Deserialize and validate a `CriticReport` JSON payload fail-closed.
pub fn validate_critic_report_json(raw_json: &str) -> Result<CriticReport, CoreError> {
    let report: CriticReport =
        serde_json::from_str(raw_json).map_err(|err| CoreError::StructuredOutputInvalid {
            stage: "VisionCritic",
            reason: err.to_string(),
        })?;

    if report.schema_version != SUPPORTED_CRITIC_REPORT_SCHEMA_VERSION {
        return Err(CoreError::StructuredOutputInvalid {
            stage: "VisionCritic",
            reason: format!(
                "unsupported schema_version {} (expected {})",
                report.schema_version, SUPPORTED_CRITIC_REPORT_SCHEMA_VERSION
            ),
        });
    }
    if report.checks.is_empty() {
        return Err(CoreError::StructuredOutputInvalid {
            stage: "VisionCritic",
            reason: "checks list must not be empty".to_string(),
        });
    }
    if report.summary.trim().is_empty() {
        return Err(CoreError::StructuredOutputInvalid {
            stage: "VisionCritic",
            reason: "summary must not be empty".to_string(),
        });
    }
    for check in &report.checks {
        if check.trait_id.trim().is_empty() || check.observation.trim().is_empty() {
            return Err(CoreError::StructuredOutputInvalid {
                stage: "VisionCritic",
                reason: "every check must have non-empty trait_id and observation".to_string(),
            });
        }
    }

    let expected_passed = report.checks.iter().all(|c| c.passed);
    if report.passed != expected_passed {
        return Err(CoreError::StructuredOutputInvalid {
            stage: "VisionCritic",
            reason: format!(
                "inconsistent CriticReport: passed={} disagrees with checks (expected {})",
                report.passed, expected_passed
            ),
        });
    }

    Ok(report)
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case::all_passed(
        r#"{"schema_version":1,"passed":true,"checks":[{"trait_id":"t1","category":"hand_props","passed":true,"observation":"lantern in left hand"}],"summary":"all traits verified"}"#,
        Some(true)
    )]
    #[case::one_failed_correctly_propagated(
        r#"{"schema_version":1,"passed":false,"checks":[{"trait_id":"t1","category":"zero_leaked_text","passed":false,"observation":"corner watermark visible"}],"summary":"failed text check"}"#,
        Some(false)
    )]
    #[case::contradictory_passed_true_with_failing_check(
        r#"{"schema_version":1,"passed":true,"checks":[{"trait_id":"t1","category":"identity","passed":false,"observation":"coat color wrong"}],"summary":"rubber stamped"}"#,
        None
    )]
    #[case::unknown_field_rejected(
        r#"{"schema_version":1,"passed":true,"extra":1,"checks":[{"trait_id":"t1","category":"identity","passed":true,"observation":"ok"}],"summary":"ok"}"#,
        None
    )]
    #[case::empty_checks_rejected(
        r#"{"schema_version":1,"passed":true,"checks":[],"summary":"empty"}"#,
        None
    )]
    fn validates_critic_report_fail_closed(
        #[case] raw: &str,
        #[case] expected_passed: Option<bool>,
    ) {
        let res = validate_critic_report_json(raw);
        match expected_passed {
            Some(expected) => assert_eq!(res.expect("valid report").passed, expected),
            None => assert!(matches!(
                res.expect_err("invalid report must fail closed"),
                CoreError::StructuredOutputInvalid { .. }
            )),
        }
    }

    #[tokio::test]
    async fn vision_critic_passes_target_and_reference_images_and_parses_report() {
        let root = TempDir::new().expect("tempdir");
        let auth_dir = root.path().join("auth");
        fs::create_dir_all(&auth_dir).unwrap();
        fs::write(auth_dir.join("auth.json"), r#"{"token":"oauth"}"#).unwrap();

        let target_img = root.path().join("out.png");
        let ref_img = root.path().join("ref.png");
        fs::write(&target_img, b"stub-target").unwrap();
        fs::write(&ref_img, b"stub-ref").unwrap();

        let args_log = root.path().join("critic_args.txt");
        let stub_bin = root.path().join("codex-critic");
        let script = format!(
            r#"#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$@" > "{args_log}"
OUT_PATH=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    -o) OUT_PATH="$2"; shift 2 ;;
    *) shift ;;
  esac
done
cat <<'JSON' > "$OUT_PATH"
{{"schema_version":1,"passed":true,"checks":[{{"trait_id":"hand-lantern","category":"hand_props","passed":true,"observation":"brass lantern held in left hand"}},{{"trait_id":"no-text","category":"zero_leaked_text","passed":true,"observation":"no stray glyphs"}}],"summary":"all traits passed"}}
JSON
"#,
            args_log = args_log.display()
        );
        fs::write(&stub_bin, script).unwrap();
        fs::set_permissions(&stub_bin, fs::Permissions::from_mode(0o755)).unwrap();

        let critic =
            VisionCritic::with_codex_bin(auth_dir, stub_bin.to_string_lossy().into_owned());
        let report = critic
            .evaluate(&VisionCriticRequest {
                target_image: target_img.clone(),
                reference_images: vec![ref_img.clone()],
                traits: vec![
                    TraitCheckInput {
                        trait_id: "hand-lantern".to_string(),
                        category: TraitCategory::HandProps,
                        criterion: "Left hand holds brass lantern".to_string(),
                    },
                    TraitCheckInput {
                        trait_id: "no-text".to_string(),
                        category: TraitCategory::ZeroLeakedText,
                        criterion: "No stray text or watermarks".to_string(),
                    },
                ],
            })
            .await
            .expect("VisionCritic should succeed");

        assert!(report.passed);
        assert_eq!(report.checks.len(), 2);

        let args = fs::read_to_string(&args_log).unwrap();
        assert!(args.contains("exec\n-s\nread-only\n--ephemeral\n--output-schema\n"));
        assert!(args.contains(&format!(
            "-i\n{}\n-i\n{}",
            target_img.display(),
            ref_img.display()
        )));
    }
}
