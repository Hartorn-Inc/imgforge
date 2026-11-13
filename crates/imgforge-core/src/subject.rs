//! === SUBJECT DOMAIN SCHEMA ===
//!
//! PURPOSE:
//! Defines the style-neutral `Subject` identity card (`library/subjects/<slug>/card.yaml`),
//! `SubjectKind`, `SubjectHandBindings`, and `PaletteLock` (`schema_version = 1`,
//! `#[serde(deny_unknown_fields)]`).
//!
//! THREAT MODEL / DESIGN RATIONALE ("WHY"):
//! 1. Keeping `Subject` identity completely neutral of any rendering `Style` prevents
//!    style lock-in and outline bleed across `ReferencePack` sheets (ADR 0003).
//! 2. Normalizing all `#RRGGBB` swatches in `PaletteLock` and `ExclusiveSurfaceMap` to
//!    uppercase hex and validating unique slots prevents silent color mismatches
//!    during calibration strip generation and OKLab/`DeltaE_2000` QA.
//! 3. Requiring finite positive `height_m` for `character` and `creature` subjects
//!    enforces quantitative metric scale invariants in downstream `SceneSpec` layouts.
//!
//! EXECUTION LIFECYCLE / DATA FLOW:
//! 1. Deserialize YAML/JSON with `#[serde(deny_unknown_fields)]`.
//! 2. Validate `schema_version == 1`, safe slug, `kind`-specific `height_m`,
//!    `ExclusiveSurfaceMap`, `PaletteLock`, and `Visibility`.
//! 3. Normalize all hex swatches to uppercase `#RRGGBB`.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::error::CoreError;
use crate::lifecycle::{Lifecycle, normalize_hex_color, validate_slug};
use crate::scene_spec::{ExclusiveSurfaceMap, HandBinding, Visibility};

/// Supported `Subject` card schema version.
pub const SUPPORTED_SUBJECT_SCHEMA_VERSION: u32 = 1;

/// Canonical categories for a style-neutral `Subject` (`GLOSSARY.md`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubjectKind {
    /// Humanoid or sentient recurring actor (requires positive `height_m`).
    Character,
    /// Non-humanoid fauna or monster (requires positive `height_m`).
    Creature,
    /// Portable or stationary object/artifact (`height_m` optional).
    Prop,
    /// Architectural interior, exterior, or landmark setting (`height_m` optional).
    Location,
}

impl SubjectKind {
    /// Canonical lowercase identifier matching `GLOSSARY.md` and `frontend/src/types/domain.ts`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Character => "character",
            Self::Creature => "creature",
            Self::Prop => "prop",
            Self::Location => "location",
        }
    }

    /// Returns `true` when `height_m` is mandatory (`character` and `creature`).
    #[must_use]
    pub const fn requires_height(self) -> bool {
        matches!(self, Self::Character | Self::Creature)
    }
}

/// Default left- and right-hand bindings stored on a `Subject`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubjectHandBindings {
    /// Default left-hand posture or held item.
    #[serde(default)]
    pub left: Option<HandBinding>,
    /// Default right-hand posture or held item.
    #[serde(default)]
    pub right: Option<HandBinding>,
}

/// Named unshaded base albedo color swatch inside a `PaletteLock`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PaletteSwatch {
    /// Human-readable swatch name (for example `"coat_indigo"`).
    pub name: String,
    /// Exact 6-digit hex albedo value (`#RRGGBB`), normalized uppercase on load.
    pub color_hex: String,
    /// Optional associated surface slot inside `ExclusiveSurfaceMap`.
    #[serde(default)]
    pub surface_slot: Option<String>,
}

/// Set of exact unshaded base albedo hex colors bound to a `Subject` (`GLOSSARY.md`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PaletteLock {
    /// Non-empty list of uniquely named `#RRGGBB` swatches.
    pub swatches: Vec<PaletteSwatch>,
}

impl PaletteLock {
    /// Validate swatch names/hexes and normalize all `color_hex` values to uppercase `#RRGGBB`.
    pub fn validate_and_normalize(&mut self) -> Result<(), CoreError> {
        if self.swatches.is_empty() {
            return Err(CoreError::SubjectInvalid(
                "palette_lock.swatches must contain at least 1 swatch".to_string(),
            ));
        }
        let mut seen_names = HashSet::with_capacity(self.swatches.len());
        for (idx, swatch) in self.swatches.iter_mut().enumerate() {
            let trimmed_name = swatch.name.trim();
            if trimmed_name.is_empty() {
                return Err(CoreError::SubjectInvalid(format!(
                    "palette_lock.swatches[{idx}].name must not be empty"
                )));
            }
            if trimmed_name.chars().any(char::is_control) {
                return Err(CoreError::SubjectInvalid(format!(
                    "palette_lock.swatches[{idx}].name must not contain control characters"
                )));
            }
            let key = trimmed_name.to_ascii_lowercase();
            if !seen_names.insert(key) {
                return Err(CoreError::SubjectInvalid(format!(
                    "palette_lock.swatches[{idx}].name duplicates swatch '{}'",
                    swatch.name
                )));
            }
            swatch.name = trimmed_name.to_string();
            swatch.color_hex = normalize_hex_color(
                &swatch.color_hex,
                &format!("palette_lock.swatches[{idx}].color_hex"),
            )
            .map_err(CoreError::SubjectInvalid)?;

            if let Some(ref mut slot) = swatch.surface_slot {
                let trimmed_slot = slot.trim();
                if trimmed_slot.is_empty() {
                    return Err(CoreError::SubjectInvalid(format!(
                        "palette_lock.swatches[{idx}].surface_slot must not be empty when provided"
                    )));
                }
                *slot = trimmed_slot.to_string();
            }
        }
        Ok(())
    }
}

/// Style-neutral recurring entity specification (`library/subjects/<slug>/card.yaml`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Subject {
    /// Schema version; must equal `1`.
    pub schema_version: u32,
    /// Kind of visual entity (`character`, `creature`, `prop`, or `location`).
    pub kind: SubjectKind,
    /// Stable lowercase kebab-case slug matching the parent directory name.
    pub slug: String,
    /// Human-readable display name.
    pub display_name: String,
    /// Physical metric height (`> 0.0`, required for `character` and `creature`).
    #[serde(default)]
    pub height_m: Option<f64>,
    /// Optional default left/right hand bindings.
    #[serde(default)]
    pub hand_bindings: Option<SubjectHandBindings>,
    /// Distinct left/right asymmetric markers (for example `"left shoulder brass buckle"`).
    #[serde(default)]
    pub asymmetric_markers: Vec<String>,
    /// Exhaustive material and `#RRGGBB` hex surface assignment.
    pub exclusive_surface_map: ExclusiveSurfaceMap,
    /// Positive-only geometric descriptors for weapons, tools, or silhouettes.
    #[serde(default)]
    pub positive_weapon_geometry: Vec<String>,
    /// Exact unshaded base albedo swatches used for calibration strips and color QA.
    pub palette_lock: PaletteLock,
    /// Access classification preventing spoiler leaks into public contexts.
    pub visibility: Visibility,
    /// Optional non-empty classification tags.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Current lifecycle state (defaults to `draft` when omitted from `card.yaml`).
    #[serde(default)]
    pub lifecycle: Lifecycle,
}

impl Subject {
    /// Parse a `Subject` from YAML and run full validation + hex normalization.
    pub fn from_yaml_str(raw: &str) -> Result<Self, CoreError> {
        let mut subject: Self =
            yaml_serde::from_str(raw).map_err(|err| CoreError::SubjectInvalid(err.to_string()))?;
        subject.validate_and_normalize()?;
        Ok(subject)
    }

    /// Parse a `Subject` from JSON and run full validation + hex normalization.
    pub fn from_json_str(raw: &str) -> Result<Self, CoreError> {
        let mut subject: Self =
            serde_json::from_str(raw).map_err(|err| CoreError::SubjectInvalid(err.to_string()))?;
        subject.validate_and_normalize()?;
        Ok(subject)
    }

    /// Serialize this `Subject` to YAML.
    pub fn to_yaml_string(&self) -> Result<String, CoreError> {
        yaml_serde::to_string(self).map_err(|err| CoreError::SubjectInvalid(err.to_string()))
    }

    /// Validate all `Subject` domain invariants and normalize hex colors to uppercase `#RRGGBB`.
    pub fn validate_and_normalize(&mut self) -> Result<(), CoreError> {
        if self.schema_version != SUPPORTED_SUBJECT_SCHEMA_VERSION {
            return Err(CoreError::SubjectInvalid(format!(
                "unsupported schema_version {} (expected {})",
                self.schema_version, SUPPORTED_SUBJECT_SCHEMA_VERSION
            )));
        }
        validate_slug(&self.slug, "Subject").map_err(CoreError::SubjectInvalid)?;

        if self.display_name.trim().is_empty() {
            return Err(CoreError::SubjectInvalid(
                "display_name must not be empty".to_string(),
            ));
        }

        match (self.kind.requires_height(), self.height_m) {
            (true, None) => {
                return Err(CoreError::SubjectInvalid(format!(
                    "height_m is required for Subject kind '{}'",
                    self.kind.as_str()
                )));
            }
            (_, Some(h)) if !h.is_finite() || h <= 0.0 => {
                return Err(CoreError::SubjectInvalid(
                    "height_m must be finite and strictly greater than 0.0".to_string(),
                ));
            }
            _ => {}
        }

        if let Some(ref bindings) = self.hand_bindings {
            if let Some(ref left) = bindings.left {
                validate_hand_binding("hand_bindings.left", left)?;
            }
            if let Some(ref right) = bindings.right {
                validate_hand_binding("hand_bindings.right", right)?;
            }
        }

        validate_string_list("asymmetric_markers", &self.asymmetric_markers)?;
        validate_string_list("positive_weapon_geometry", &self.positive_weapon_geometry)?;
        validate_string_list("tags", &self.tags)?;

        if self.exclusive_surface_map.bindings.is_empty() {
            return Err(CoreError::SubjectInvalid(
                "exclusive_surface_map.bindings must contain at least 1 binding".to_string(),
            ));
        }
        let mut seen_slots = HashSet::with_capacity(self.exclusive_surface_map.bindings.len());
        for (idx, binding) in self.exclusive_surface_map.bindings.iter_mut().enumerate() {
            let trimmed_slot = binding.slot.trim();
            if trimmed_slot.is_empty() || trimmed_slot.chars().any(char::is_control) {
                return Err(CoreError::SubjectInvalid(format!(
                    "exclusive_surface_map.bindings[{idx}].slot must be non-empty without control chars"
                )));
            }
            if binding.material.trim().is_empty() {
                return Err(CoreError::SubjectInvalid(format!(
                    "exclusive_surface_map.bindings[{idx}].material must not be empty"
                )));
            }
            let key = trimmed_slot.to_ascii_lowercase();
            if !seen_slots.insert(key) {
                return Err(CoreError::SubjectInvalid(format!(
                    "exclusive_surface_map.bindings[{idx}].slot duplicates '{}'",
                    binding.slot
                )));
            }
            binding.slot = trimmed_slot.to_string();
            binding.color_hex = normalize_hex_color(
                &binding.color_hex,
                &format!("exclusive_surface_map.bindings[{idx}].color_hex"),
            )
            .map_err(CoreError::SubjectInvalid)?;
        }

        self.palette_lock.validate_and_normalize()?;
        Ok(())
    }
}

fn validate_hand_binding(prefix: &str, hand: &HandBinding) -> Result<(), CoreError> {
    if hand.action.trim().is_empty() {
        return Err(CoreError::SubjectInvalid(format!(
            "{prefix}.action must not be empty"
        )));
    }
    if let Some(ref item) = hand.item
        && item.trim().is_empty()
    {
        return Err(CoreError::SubjectInvalid(format!(
            "{prefix}.item must not be empty when provided"
        )));
    }
    Ok(())
}

fn validate_string_list(field: &str, values: &[String]) -> Result<(), CoreError> {
    for (idx, entry) in values.iter().enumerate() {
        if entry.trim().is_empty() || entry.chars().any(char::is_control) {
            return Err(CoreError::SubjectInvalid(format!(
                "{field}[{idx}] must be non-empty without control characters"
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    fn sample_character_yaml() -> &'static str {
        r##"
schema_version: 1
kind: character
slug: courier-ren
display_name: Courier Ren
height_m: 1.74
hand_bindings:
  left:
    item: brass lantern
    action: holding aloft
  right:
    action: resting on satchel strap
asymmetric_markers:
  - brass compass clipped to left lapel
exclusive_surface_map:
  bindings:
    - slot: coat
      material: waxed canvas
      color_hex: "#2c3e50"
      finish: matte
    - slot: scarf
      material: coarse wool
      color_hex: "#b84a39"
positive_weapon_geometry:
  - curved brass astrolabe arm with engraved degree ring
palette_lock:
  swatches:
    - name: coat_slate
      color_hex: "#2c3e50"
      surface_slot: coat
    - name: scarf_rust
      color_hex: "#b84a39"
      surface_slot: scarf
visibility: public
tags:
  - harbor
  - messenger
"##
    }

    #[test]
    fn parses_character_subject_and_normalizes_hex_uppercase() {
        let subject = Subject::from_yaml_str(sample_character_yaml()).expect("valid Subject");
        assert_eq!(subject.kind, SubjectKind::Character);
        assert_eq!(subject.height_m, Some(1.74));
        assert_eq!(subject.palette_lock.swatches[0].color_hex, "#2C3E50");
        assert_eq!(subject.palette_lock.swatches[1].color_hex, "#B84A39");
        assert_eq!(
            subject.exclusive_surface_map.bindings[0].color_hex,
            "#2C3E50"
        );
        assert_eq!(subject.lifecycle, Lifecycle::Draft);
    }

    #[test]
    fn prop_allows_omitted_height_m_while_creature_requires_height_m() {
        let prop_yaml = sample_character_yaml()
            .replace("kind: character", "kind: prop")
            .replace("height_m: 1.74\n", "");
        let prop = Subject::from_yaml_str(&prop_yaml).expect("prop without height_m is valid");
        assert_eq!(prop.kind, SubjectKind::Prop);
        assert_eq!(prop.height_m, None);

        let creature_yaml = sample_character_yaml()
            .replace("kind: character", "kind: creature")
            .replace("height_m: 1.74\n", "");
        let err = Subject::from_yaml_str(&creature_yaml)
            .expect_err("creature missing height_m must fail closed");
        assert!(matches!(err, CoreError::SubjectInvalid(_)));
    }

    #[rstest]
    #[case::unknown_field("extra_key: true\n", true)]
    #[case::wrong_schema_version("schema_version: 2", false)]
    #[case::invalid_slug("slug: ../escape", false)]
    #[case::zero_height("height_m: 0.0", false)]
    #[case::negative_height("height_m: -1.5", false)]
    #[case::invalid_palette_hex("color_hex: \"#2c3e50\"", true)]
    fn rejects_invalid_subject_yaml(#[case] replacement: &str, #[case] append_or_bad_hex: bool) {
        let mut yaml = sample_character_yaml().to_string();
        if replacement == "extra_key: true\n" {
            yaml.push_str(replacement);
        } else if append_or_bad_hex {
            yaml = yaml.replacen(replacement, "color_hex: \"not-hex\"", 1);
        } else if replacement.starts_with("schema_version") {
            yaml = yaml.replacen("schema_version: 1", replacement, 1);
        } else if replacement.starts_with("slug:") {
            yaml = yaml.replacen("slug: courier-ren", replacement, 1);
        } else {
            yaml = yaml.replacen("height_m: 1.74", replacement, 1);
        }

        let err = Subject::from_yaml_str(&yaml).expect_err("invalid Subject must fail closed");
        assert!(matches!(err, CoreError::SubjectInvalid(_)));
    }
}
