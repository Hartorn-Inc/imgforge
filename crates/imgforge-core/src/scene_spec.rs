//! === SCENESPEC DOMAIN SCHEMA ===
//!
//! PURPOSE:
//! Defines `SceneSpec` (`schema_version = 1`, `#[serde(deny_unknown_fields)]`),
//! `Visibility`, depth bands, left-to-right foreground focal slots, metric heights,
//! L/R hand bindings, and `ExclusiveSurfaceMap` bindings.
//!
//! THREAT MODEL / DESIGN RATIONALE ("WHY"):
//! `SceneSpec` is the canonical structured boundary between prose intent (`SceneComposer`
//! or Web Canvas UI) and deterministic compilation (`PromptCompiler`). Rejecting unknown
//! fields with `#[serde(deny_unknown_fields)]` prevents silent schema drift and ensures
//! 1:1 alignment with `codex exec --output-schema` strict JSON schemas.
//!
//! EXECUTION LIFECYCLE / DATA FLOW:
//! 1. Callers load YAML or JSON via `SceneSpec::from_yaml_str`, `SceneSpec::from_json_str`,
//!    or `SceneSpec::load_from_path`.
//! 2. `PromptCompiler::compile` validates all domain, geometry, `Visibility`, and
//!    positive-only phrasing invariants before any `ImageBackend` invocation.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::CoreError;
use crate::store::{MAX_TEXT_FILE_BYTES, read_text_file_nofollow_bounded};

/// Supported `SceneSpec` schema version.
pub const SUPPORTED_SCENE_SPEC_SCHEMA_VERSION: u32 = 1;

/// Maximum number of foreground focal slots allowed in a single `SceneSpec`.
pub const MAX_FOREGROUND_SLOTS: usize = 3;

/// Access classification preventing spoiler-sensitive assets from leaking into public outputs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Visibility {
    /// Safe for unrestricted external publication.
    #[default]
    Public,
    /// Limited to gated or campaign-authorized audiences.
    Restricted,
    /// Strictly internal reference or spoiler-sensitive work-in-progress.
    Internal,
}

impl Visibility {
    /// Canonical lowercase label matching `GLOSSARY.md` and `frontend/src/types/domain.ts`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Public => "public",
            Self::Restricted => "restricted",
            Self::Internal => "internal",
        }
    }

    /// Returns `true` when a scene of `self` visibility may include an asset of `asset_visibility`.
    #[must_use]
    pub fn allows_asset(self, asset_visibility: Self) -> bool {
        asset_visibility <= self
    }

    /// Fail-closed guard verifying that `asset_visibility` does not exceed `self` (`SceneSpec` or `Export` context).
    pub fn ensure_allows(
        self,
        asset_visibility: Self,
        asset_id: &str,
        context: &str,
    ) -> Result<(), CoreError> {
        if self.allows_asset(asset_visibility) {
            Ok(())
        } else {
            Err(CoreError::VisibilityViolation {
                context: context.to_string(),
                asset_id: asset_id.to_string(),
                asset_visibility: asset_visibility.as_str().to_string(),
                context_visibility: self.as_str().to_string(),
            })
        }
    }
}

/// Validate that every `(asset_id, Visibility)` referenced by a `SceneSpec`, `Atlas`, or `Export`
/// is permitted under `target_visibility`, failing closed on the first leak.
pub fn check_visibility_compatibility<'a, I>(
    target_visibility: Visibility,
    context: &str,
    assets: I,
) -> Result<(), CoreError>
where
    I: IntoIterator<Item = (&'a str, Visibility)>,
{
    for (asset_id, asset_vis) in assets {
        target_visibility.ensure_allows(asset_vis, asset_id, context)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case::public_allows_public(Visibility::Public, Visibility::Public, true)]
    #[case::public_blocks_restricted(Visibility::Public, Visibility::Restricted, false)]
    #[case::public_blocks_internal(Visibility::Public, Visibility::Internal, false)]
    #[case::restricted_allows_public(Visibility::Restricted, Visibility::Public, true)]
    #[case::restricted_allows_restricted(Visibility::Restricted, Visibility::Restricted, true)]
    #[case::restricted_blocks_internal(Visibility::Restricted, Visibility::Internal, false)]
    #[case::internal_allows_all(Visibility::Internal, Visibility::Internal, true)]
    fn visibility_compatibility_matrix(
        #[case] context_vis: Visibility,
        #[case] asset_vis: Visibility,
        #[case] expect_ok: bool,
    ) {
        let res = check_visibility_compatibility(
            context_vis,
            "ExportProfile",
            [("subject-alpha", asset_vis)],
        );
        assert_eq!(res.is_ok(), expect_ok);
        if !expect_ok {
            assert!(matches!(
                res.unwrap_err(),
                CoreError::VisibilityViolation { .. }
            ));
        }
    }
}

/// Depth plane assignment for quantitative composition bands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DepthPlane {
    /// Foreground focal plane closest to the camera.
    Foreground,
    /// Midground staging plane.
    Midground,
    /// Distant horizon or backdrop plane.
    Background,
}

impl DepthPlane {
    /// Canonical lowercase label for deterministic prompt formatting.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Foreground => "foreground",
            Self::Midground => "midground",
            Self::Background => "background",
        }
    }
}

/// Camera shot and framing specification inside a `SceneSpec`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CameraSpec {
    /// Shot scale and lens framing (for example `"medium-wide 35mm"`).
    pub shot: String,
    /// Optional camera elevation/angle (for example `"eye-level three-quarter"`).
    #[serde(default)]
    pub angle: Option<String>,
    /// Optional aspect or compositional framing note.
    #[serde(default)]
    pub framing: Option<String>,
}

/// Quantitative depth band partitioning the scene from near to far meters.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DepthBandSpec {
    /// Depth plane category (`foreground`, `midground`, or `background`).
    pub plane: DepthPlane,
    /// Near boundary of the band in meters (`>= 0.0`).
    pub near_meters: f64,
    /// Far boundary of the band in meters (`> near_meters`).
    pub far_meters: f64,
    /// Positive visual description of elements inhabiting this depth band.
    pub description: String,
}

/// Explicit left- or right-hand pose and prop binding for a foreground `Subject`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HandBinding {
    /// Optional held prop or weapon description (`None` when hand is empty/open).
    #[serde(default)]
    pub item: Option<String>,
    /// Positive action or posture of the hand (for example `"gripping brass lantern"` or `"open resting at side"`).
    pub action: String,
}

/// Single exhaustive surface/garment slot assignment inside an `ExclusiveSurfaceMap`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SurfaceBinding {
    /// Visible surface or garment slot identifier (for example `"cloak"`, `"tunic"`, `"blade"`).
    pub slot: String,
    /// Physical material description (for example `"boiled leather"`, `"matte wool"`).
    pub material: String,
    /// Exact 6-digit hex base albedo color (`#RRGGBB`).
    pub color_hex: String,
    /// Optional surface finish note (for example `"weathered matte"`).
    #[serde(default)]
    pub finish: Option<String>,
}

/// Exhaustive assignment of material and base color to every visible surface of a `Subject`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExclusiveSurfaceMap {
    /// Non-empty list of unique surface slot bindings.
    pub bindings: Vec<SurfaceBinding>,
}

/// Foreground focal slot positioned strictly left-to-right within the composition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ForegroundSlot {
    /// Horizontal normalized frame position in `[0.0, 1.0]`, strictly increasing left-to-right.
    pub position_x_ratio: f64,
    /// Slug of the referenced style-neutral `Subject`.
    pub subject_ref: String,
    /// `Visibility` classification of the referenced `Subject`.
    #[serde(default)]
    pub visibility: Visibility,
    /// Physical height of the `Subject` in meters (`> 0.0`).
    pub height_meters: f64,
    /// Positive description of posture, orientation, and expression.
    pub pose: String,
    /// Explicit left-hand binding.
    pub left_hand: HandBinding,
    /// Explicit right-hand binding.
    pub right_hand: HandBinding,
    /// Exhaustive material/color map preventing color bleed across neighboring surfaces.
    pub exclusive_surface_map: ExclusiveSurfaceMap,
}

/// Canonical structured composition specification consumed by `PromptCompiler`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SceneSpec {
    /// Schema version; must equal `1`.
    pub schema_version: u32,
    /// Stable slug or identifier for this scene.
    pub id: String,
    /// Human-readable positive scene title.
    pub title: String,
    /// Access classification governing which `Subject` and `Export` profiles are permitted.
    pub visibility: Visibility,
    /// Camera shot and framing parameters.
    pub camera: CameraSpec,
    /// Quantitative depth bands ordered or partitioned in metric space.
    pub depth_bands: Vec<DepthBandSpec>,
    /// Up to 3 foreground focal slots ordered strictly left-to-right.
    pub foreground_slots: Vec<ForegroundSlot>,
    /// Slug of the first-class `Style` specification.
    pub style_ref: String,
    /// Slug of the lighting and mood `Atmosphere` specification.
    pub atmosphere_ref: String,
    /// Positive description of the overarching scene action and setting.
    pub action_summary: String,
    /// Optional target output width in pixels.
    #[serde(default)]
    pub width: Option<u32>,
    /// Optional target output height in pixels.
    #[serde(default)]
    pub height: Option<u32>,
}

impl SceneSpec {
    /// Parse a `SceneSpec` from a YAML string (`#[serde(deny_unknown_fields)]`).
    pub fn from_yaml_str(raw: &str) -> Result<Self, CoreError> {
        yaml_serde::from_str(raw).map_err(|err| CoreError::SceneSpecInvalid(err.to_string()))
    }

    /// Parse a `SceneSpec` from a JSON string (`#[serde(deny_unknown_fields)]`).
    pub fn from_json_str(raw: &str) -> Result<Self, CoreError> {
        serde_json::from_str(raw).map_err(|err| CoreError::SceneSpecInvalid(err.to_string()))
    }

    /// Serialize this `SceneSpec` to canonical YAML.
    pub fn to_yaml_string(&self) -> Result<String, CoreError> {
        yaml_serde::to_string(self).map_err(|err| CoreError::SceneSpecInvalid(err.to_string()))
    }

    /// Serialize this `SceneSpec` to pretty JSON.
    pub fn to_json_string(&self) -> Result<String, CoreError> {
        serde_json::to_string_pretty(self)
            .map_err(|err| CoreError::SceneSpecInvalid(err.to_string()))
    }

    /// Load a `SceneSpec` from a `.json`, `.yaml`, or `.yml` file path via bounded `O_NOFOLLOW` read.
    pub fn load_from_path(path: &Path) -> Result<Self, CoreError> {
        let raw = read_text_file_nofollow_bounded(path, "SceneSpec", MAX_TEXT_FILE_BYTES)?;
        let ext = path
            .extension()
            .and_then(|s| s.to_str())
            .map(str::to_ascii_lowercase);
        match ext.as_deref() {
            Some("json") => Self::from_json_str(&raw),
            Some("yaml" | "yml") => Self::from_yaml_str(&raw),
            _ => {
                if raw.trim_start().starts_with('{') {
                    Self::from_json_str(&raw)
                } else {
                    Self::from_yaml_str(&raw)
                }
            }
        }
    }
}
