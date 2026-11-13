//! === STYLE DOMAIN SCHEMA ===
//!
//! PURPOSE:
//! Defines the first-class rendering `Style` card (`library/styles/<slug>/card.yaml`
//! plus `1..=3` visual `exemplars/*.png`), supporting `schema_version = 1` and
//! `#[serde(deny_unknown_fields)]`.
//!
//! THREAT MODEL / DESIGN RATIONALE ("WHY"):
//! 1. Separating `Style` from `Subject` and `Atmosphere` enables rendering the
//!    same `Subject` cleanly across multiple target styles without cross-style
//!    bleed (ADR 0003).
//! 2. `exemplars` paths inside `card.yaml` are untrusted workspace inputs; pure
//!    schema validation rejects absolute paths, `..` traversal, backslashes, and
//!    non-PNG extensions before any filesystem access, and store loading verifies
//!    real non-symlink regular files confined inside the style directory.
//!
//! EXECUTION LIFECYCLE / DATA FLOW:
//! 1. Deserialize `Style` with `#[serde(deny_unknown_fields)]`.
//! 2. Validate `schema_version == 1`, safe slug, non-empty rendering descriptors,
//!    and `1..=3` safe relative `.png` exemplar paths.

use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::CoreError;
use crate::lifecycle::{Lifecycle, validate_slug};

/// Supported `Style` card schema version.
pub const SUPPORTED_STYLE_SCHEMA_VERSION: u32 = 1;

/// Minimum number of visual exemplar PNGs required by a `Style`.
pub const MIN_STYLE_EXEMPLARS: usize = 1;

/// Maximum number of visual exemplar PNGs allowed by a `Style`.
pub const MAX_STYLE_EXEMPLARS: usize = 3;

/// First-class rendering specification (`library/styles/<slug>/card.yaml`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Style {
    /// Schema version; must equal `1`.
    pub schema_version: u32,
    /// Stable lowercase kebab-case slug matching the parent directory name.
    pub slug: String,
    /// Human-readable display name.
    pub display_name: String,
    /// Primary rendering medium (for example `"pen-and-ink watercolor wash"`).
    pub medium: String,
    /// Linework weight, contour uniformity, and ink edge behavior.
    pub linework: String,
    /// Shading strategy, hatching/cel/brushwork, and surface texture rules.
    pub shading_and_texture: String,
    /// Color harmony and contrast curve guidance.
    pub palette_profile: String,
    /// Framing, margin, and border presentation rules.
    pub framing_and_border: String,
    /// Positive-only anti-drift guardrails preserving style consistency.
    pub style_guardrails: Vec<String>,
    /// `1..=3` safe relative paths to PNG exemplars inside the style directory.
    pub exemplars: Vec<PathBuf>,
    /// Current lifecycle state (defaults to `draft` when omitted from `card.yaml`).
    #[serde(default)]
    pub lifecycle: Lifecycle,
}

impl Style {
    /// Parse a `Style` from YAML and validate all schema and exemplar-path invariants.
    pub fn from_yaml_str(raw: &str) -> Result<Self, CoreError> {
        let style: Self =
            yaml_serde::from_str(raw).map_err(|err| CoreError::StyleInvalid(err.to_string()))?;
        style.validate()?;
        Ok(style)
    }

    /// Parse a `Style` from JSON and validate all schema and exemplar-path invariants.
    pub fn from_json_str(raw: &str) -> Result<Self, CoreError> {
        let style: Self =
            serde_json::from_str(raw).map_err(|err| CoreError::StyleInvalid(err.to_string()))?;
        style.validate()?;
        Ok(style)
    }

    /// Serialize this `Style` to YAML.
    pub fn to_yaml_string(&self) -> Result<String, CoreError> {
        yaml_serde::to_string(self).map_err(|err| CoreError::StyleInvalid(err.to_string()))
    }

    /// Validate all `Style` domain invariants without touching the filesystem.
    pub fn validate(&self) -> Result<(), CoreError> {
        if self.schema_version != SUPPORTED_STYLE_SCHEMA_VERSION {
            return Err(CoreError::StyleInvalid(format!(
                "unsupported schema_version {} (expected {})",
                self.schema_version, SUPPORTED_STYLE_SCHEMA_VERSION
            )));
        }
        validate_slug(&self.slug, "Style").map_err(CoreError::StyleInvalid)?;

        require_non_empty_field("display_name", &self.display_name)?;
        require_non_empty_field("medium", &self.medium)?;
        require_non_empty_field("linework", &self.linework)?;
        require_non_empty_field("shading_and_texture", &self.shading_and_texture)?;
        require_non_empty_field("palette_profile", &self.palette_profile)?;
        require_non_empty_field("framing_and_border", &self.framing_and_border)?;

        if self.style_guardrails.is_empty() {
            return Err(CoreError::StyleInvalid(
                "style_guardrails must contain at least 1 guardrail".to_string(),
            ));
        }
        for (idx, rule) in self.style_guardrails.iter().enumerate() {
            require_non_empty_field(&format!("style_guardrails[{idx}]"), rule)?;
        }

        if self.exemplars.len() < MIN_STYLE_EXEMPLARS || self.exemplars.len() > MAX_STYLE_EXEMPLARS
        {
            return Err(CoreError::StyleInvalid(format!(
                "exemplars must contain {}..={} entries (got {})",
                MIN_STYLE_EXEMPLARS,
                MAX_STYLE_EXEMPLARS,
                self.exemplars.len()
            )));
        }

        let mut seen = HashSet::with_capacity(self.exemplars.len());
        for (idx, path) in self.exemplars.iter().enumerate() {
            validate_safe_exemplar_rel_path(path, idx)?;
            if !seen.insert(path.clone()) {
                return Err(CoreError::StyleInvalid(format!(
                    "exemplars[{idx}] duplicates path '{}'",
                    path.display()
                )));
            }
        }

        Ok(())
    }
}

fn require_non_empty_field(field: &str, value: &str) -> Result<(), CoreError> {
    if value.trim().is_empty() || value.chars().any(char::is_control) {
        return Err(CoreError::StyleInvalid(format!(
            "{field} must be non-empty without control characters"
        )));
    }
    Ok(())
}

/// Validate that an exemplar path is a safe relative `.png` path inside the style directory.
pub(crate) fn validate_safe_exemplar_rel_path(path: &Path, idx: usize) -> Result<(), CoreError> {
    let raw = path.to_string_lossy();
    if raw.trim().is_empty() || raw.contains('\\') || raw.contains('\0') {
        return Err(CoreError::StyleInvalid(format!(
            "exemplars[{idx}] path '{raw}' is invalid"
        )));
    }
    if path.is_absolute() {
        return Err(CoreError::StyleInvalid(format!(
            "exemplars[{idx}] path '{raw}' must be relative to the style directory"
        )));
    }

    let mut normal_count = 0usize;
    for comp in path.components() {
        match comp {
            Component::Normal(seg) => {
                let s = seg.to_string_lossy();
                if s.is_empty() || s.chars().any(char::is_control) {
                    return Err(CoreError::StyleInvalid(format!(
                        "exemplars[{idx}] path '{raw}' contains invalid segment"
                    )));
                }
                normal_count += 1;
            }
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(CoreError::StyleInvalid(format!(
                    "exemplars[{idx}] path '{raw}' must not traverse outside the style directory"
                )));
            }
        }
    }

    if normal_count == 0 {
        return Err(CoreError::StyleInvalid(format!(
            "exemplars[{idx}] path '{raw}' must point to a PNG file"
        )));
    }

    let is_png = path
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("png"));
    if !is_png {
        return Err(CoreError::StyleInvalid(format!(
            "exemplars[{idx}] path '{raw}' must have a .png extension"
        )));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    fn sample_style_yaml() -> &'static str {
        r#"
schema_version: 1
slug: franco-belgian-line
display_name: Franco-Belgian Clear Line
medium: technical ink outlines with flat gouache color fills
linework: uniform crisp dark ink contour lines with steady pen weight
shading_and_texture: clean two-tone cel shadows and smooth matte fills
palette_profile: balanced mid-saturation hues with clear local color separation
framing_and_border: clean full-frame composition with crisp subject silhouette separation
style_guardrails:
  - maintain uniform contour line thickness across foreground and background
  - preserve flat cel-shaded color planes with minimal soft airbrushing
exemplars:
  - exemplars/ex1.png
"#
    }

    #[test]
    fn parses_valid_style_yaml() {
        let style = Style::from_yaml_str(sample_style_yaml()).expect("valid style");
        assert_eq!(style.slug, "franco-belgian-line");
        assert_eq!(style.exemplars.len(), 1);
    }

    #[rstest]
    #[case::unknown_field("unexpected: 1\n", false)]
    #[case::zero_exemplars("exemplars: []", true)]
    #[case::four_exemplars(
        "exemplars:\n  - exemplars/1.png\n  - exemplars/2.png\n  - exemplars/3.png\n  - exemplars/4.png",
        true
    )]
    #[case::traversal_exemplar("exemplars:\n  - ../outside.png", true)]
    #[case::absolute_exemplar("exemplars:\n  - /etc/passwd.png", true)]
    #[case::non_png_exemplar("exemplars:\n  - exemplars/ex1.jpg", true)]
    fn rejects_invalid_style_yaml(#[case] snippet: &str, #[case] replace_exemplars: bool) {
        let yaml = if replace_exemplars {
            sample_style_yaml().replace("exemplars:\n  - exemplars/ex1.png", snippet)
        } else {
            format!("{}\n{}", sample_style_yaml(), snippet)
        };
        let err = Style::from_yaml_str(&yaml).expect_err("invalid Style must fail closed");
        assert!(matches!(err, CoreError::StyleInvalid(_)));
    }
}
