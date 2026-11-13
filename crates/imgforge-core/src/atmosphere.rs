//! === ATMOSPHERE DOMAIN SCHEMA ===
//!
//! PURPOSE:
//! Defines the lighting and environmental `Atmosphere` card
//! (`library/atmospheres/<slug>/card.yaml`, `schema_version = 1`,
//! `#[serde(deny_unknown_fields)]`).
//!
//! THREAT MODEL / DESIGN RATIONALE ("WHY"):
//! Separating `Atmosphere` lighting and `exposure_floor` from `Style` and
//! `Subject` preserves consistent color identity across varied times of day and
//! weather while preventing crushed shadows (`min_mean_l_star`, `min_p10_l_star`)
//! or excessive color-cast washes (`max_ambient_tint_blend` in `0.0..=1.0`).
//!
//! EXECUTION LIFECYCLE / DATA FLOW:
//! 1. Deserialize `Atmosphere` with `#[serde(deny_unknown_fields)]`.
//! 2. Validate `schema_version == 1`, safe slug, Kelvin bounds, `0.0..=1.0` tint
//!    blend cap, and `0.0..=100.0` `exposure_floor` invariants.

use serde::{Deserialize, Serialize};

use crate::error::CoreError;
use crate::lifecycle::validate_slug;

/// Supported `Atmosphere` card schema version.
pub const SUPPORTED_ATMOSPHERE_SCHEMA_VERSION: u32 = 1;

/// Minimum and maximum permitted correlated color temperature in Kelvin.
pub const MIN_COLOR_TEMPERATURE_K: u32 = 1_000;
/// Maximum permitted correlated color temperature in Kelvin.
pub const MAX_COLOR_TEMPERATURE_K: u32 = 20_000;

/// Minimum CIE `L*` luminance thresholds preventing crushed shadows (`GLOSSARY.md`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExposureFloor {
    /// Minimum permitted mean `L*` luminance across the rendered image (`0.0..=100.0`).
    pub min_mean_l_star: f64,
    /// Minimum permitted 10th-percentile `L*` luminance (`0.0..=min_mean_l_star`).
    pub min_p10_l_star: f64,
}

/// Lighting and environmental mood specification (`library/atmospheres/<slug>/card.yaml`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Atmosphere {
    /// Schema version; must equal `1`.
    pub schema_version: u32,
    /// Stable lowercase kebab-case slug matching the parent directory name.
    pub slug: String,
    /// Human-readable display name.
    pub display_name: String,
    /// Primary key light direction and elevation description.
    pub key_light_direction: String,
    /// Secondary fill or rim light direction description.
    pub secondary_light_direction: String,
    /// Correlated color temperature in Kelvin (`1000..=20000`).
    pub color_temperature_k: u32,
    /// Maximum ambient tint blend factor (`0.0..=1.0`) preserving `PaletteLock` readability.
    pub max_ambient_tint_blend: f64,
    /// Minimum shadow and overall exposure floor thresholds.
    pub exposure_floor: ExposureFloor,
    /// Positive description of atmospheric haze, precipitation, or suspended particles.
    pub weather_and_particles: String,
}

impl Atmosphere {
    /// Parse an `Atmosphere` from YAML and validate all schema invariants.
    pub fn from_yaml_str(raw: &str) -> Result<Self, CoreError> {
        let atmo: Self = yaml_serde::from_str(raw)
            .map_err(|err| CoreError::AtmosphereInvalid(err.to_string()))?;
        atmo.validate()?;
        Ok(atmo)
    }

    /// Parse an `Atmosphere` from JSON and validate all schema invariants.
    pub fn from_json_str(raw: &str) -> Result<Self, CoreError> {
        let atmo: Self = serde_json::from_str(raw)
            .map_err(|err| CoreError::AtmosphereInvalid(err.to_string()))?;
        atmo.validate()?;
        Ok(atmo)
    }

    /// Serialize this `Atmosphere` to YAML.
    pub fn to_yaml_string(&self) -> Result<String, CoreError> {
        yaml_serde::to_string(self).map_err(|err| CoreError::AtmosphereInvalid(err.to_string()))
    }

    /// Validate all `Atmosphere` domain invariants.
    pub fn validate(&self) -> Result<(), CoreError> {
        if self.schema_version != SUPPORTED_ATMOSPHERE_SCHEMA_VERSION {
            return Err(CoreError::AtmosphereInvalid(format!(
                "unsupported schema_version {} (expected {})",
                self.schema_version, SUPPORTED_ATMOSPHERE_SCHEMA_VERSION
            )));
        }
        validate_slug(&self.slug, "Atmosphere").map_err(CoreError::AtmosphereInvalid)?;

        require_non_empty("display_name", &self.display_name)?;
        require_non_empty("key_light_direction", &self.key_light_direction)?;
        require_non_empty("secondary_light_direction", &self.secondary_light_direction)?;
        require_non_empty("weather_and_particles", &self.weather_and_particles)?;

        if !(MIN_COLOR_TEMPERATURE_K..=MAX_COLOR_TEMPERATURE_K).contains(&self.color_temperature_k)
        {
            return Err(CoreError::AtmosphereInvalid(format!(
                "color_temperature_k {} must be within {}..={}",
                self.color_temperature_k, MIN_COLOR_TEMPERATURE_K, MAX_COLOR_TEMPERATURE_K
            )));
        }

        if !self.max_ambient_tint_blend.is_finite()
            || !(0.0..=1.0).contains(&self.max_ambient_tint_blend)
        {
            return Err(CoreError::AtmosphereInvalid(format!(
                "max_ambient_tint_blend {} must be finite within 0.0..=1.0",
                self.max_ambient_tint_blend
            )));
        }

        let mean = self.exposure_floor.min_mean_l_star;
        let p10 = self.exposure_floor.min_p10_l_star;
        if !mean.is_finite() || !(0.0..=100.0).contains(&mean) {
            return Err(CoreError::AtmosphereInvalid(format!(
                "exposure_floor.min_mean_l_star {} must be finite within 0.0..=100.0",
                mean
            )));
        }
        if !p10.is_finite() || !(0.0..=100.0).contains(&p10) {
            return Err(CoreError::AtmosphereInvalid(format!(
                "exposure_floor.min_p10_l_star {} must be finite within 0.0..=100.0",
                p10
            )));
        }
        if p10 > mean {
            return Err(CoreError::AtmosphereInvalid(format!(
                "exposure_floor.min_p10_l_star ({p10}) must not exceed min_mean_l_star ({mean})"
            )));
        }

        Ok(())
    }
}

fn require_non_empty(field: &str, value: &str) -> Result<(), CoreError> {
    if value.trim().is_empty() || value.chars().any(char::is_control) {
        return Err(CoreError::AtmosphereInvalid(format!(
            "{field} must be non-empty without control characters"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    fn sample_atmosphere_yaml() -> &'static str {
        r#"
schema_version: 1
slug: golden-dawn
display_name: Golden Dawn
key_light_direction: low warm eastern key light at 18 degrees elevation
secondary_light_direction: soft cool sky fill from opposite zenith
color_temperature_k: 3400
max_ambient_tint_blend: 0.25
exposure_floor:
  min_mean_l_star: 32.0
  min_p10_l_star: 10.0
weather_and_particles: light drifting morning mist
"#
    }

    #[test]
    fn parses_valid_atmosphere_yaml() {
        let atmo = Atmosphere::from_yaml_str(sample_atmosphere_yaml()).expect("valid atmosphere");
        assert_eq!(atmo.slug, "golden-dawn");
        assert_eq!(atmo.color_temperature_k, 3400);
        assert_eq!(atmo.max_ambient_tint_blend, 0.25);
    }

    #[rstest]
    #[case::unknown_field("extra_key: true", true)]
    #[case::kelvin_too_low("color_temperature_k: 500", false)]
    #[case::kelvin_too_high("color_temperature_k: 30000", false)]
    #[case::tint_above_one("max_ambient_tint_blend: 1.2", false)]
    #[case::tint_negative("max_ambient_tint_blend: -0.1", false)]
    #[case::p10_exceeds_mean("min_p10_l_star: 40.0", false)]
    #[case::mean_above_100("min_mean_l_star: 105.0", false)]
    fn rejects_invalid_atmosphere_yaml(#[case] replacement: &str, #[case] append: bool) {
        let yaml = if append {
            format!("{}\n{}", sample_atmosphere_yaml(), replacement)
        } else {
            let key = replacement.split(':').next().unwrap();
            let line = sample_atmosphere_yaml()
                .lines()
                .find(|l| l.trim_start().starts_with(key))
                .unwrap();
            let indent = if line.starts_with("  ") { "  " } else { "" };
            sample_atmosphere_yaml().replace(line, &format!("{indent}{replacement}"))
        };
        let err =
            Atmosphere::from_yaml_str(&yaml).expect_err("invalid Atmosphere must fail closed");
        assert!(matches!(err, CoreError::AtmosphereInvalid(_)));
    }
}
