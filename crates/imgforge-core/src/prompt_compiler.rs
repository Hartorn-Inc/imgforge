//! === DETERMINISTIC PROMPT COMPILER ===
//!
//! PURPOSE:
//! Validates a `SceneSpec` in sub-millisecond Rust and compiles it into a
//! deterministic, positive-only prompt payload for `ImageBackend` execution.
//!
//! THREAT MODEL / DESIGN RATIONALE ("WHY"):
//! 1. Running an LLM on structured inputs adds latency and double-LLM drift on
//!    weapon geometry and left-to-right slot ordering (ADR 0004).
//! 2. Generative image models frequently misinterpret negative phrasing (for
//!    example `"no helmet"` or `"without cloak"`), accidentally rendering the
//!    negated object. `PromptCompiler` therefore rejects active negations across
//!    all subject and scene prose fields fail-closed.
//! 3. `Visibility` mismatches (`restricted`/`internal` `Subject` inside a `public`
//!    `SceneSpec`) are rejected before any backend call can leak spoiler assets.
//!
//! EXECUTION LIFECYCLE / DATA FLOW:
//! 1. Validate `schema_version == 1`, required identifiers, and `Visibility`.
//! 2. Validate positive geometry invariants (depth bands, `<= 3` slots, strictly
//!    increasing `position_x_ratio`, positive `height_meters`, optional `WxH`).
//! 3. Validate `ExclusiveSurfaceMap` non-emptiness, unique slot names, `#RRGGBB` hex.
//! 4. Reject active negation tokens/phrases in all prose fields.
//! 5. Emit deterministic sectioned positive prompt string.

use std::collections::{BTreeMap, HashSet};
use std::fmt::Write as _;

use crate::backend::format_aspect_ratio;
use crate::error::CoreError;
use crate::scene_spec::{
    MAX_FOREGROUND_SLOTS, SUPPORTED_SCENE_SPEC_SCHEMA_VERSION, SceneSpec, Visibility,
};

/// Single-word negation tokens rejected in `SceneSpec` prose fields.
const NEGATION_WORDS: &[&str] = &[
    "no",
    "not",
    "never",
    "without",
    "avoid",
    "avoiding",
    "exclude",
    "excluding",
    "except",
    "neither",
    "nor",
    "don't",
    "doesn't",
    "didn't",
    "won't",
    "cannot",
    "can't",
    "isn't",
    "aren't",
    "wasn't",
    "weren't",
    "wouldn't",
    "couldn't",
    "shouldn't",
    "mustn't",
    "hasn't",
    "haven't",
    "hadn't",
    "ain't",
    "none",
    "nothing",
    "absent",
    "lacking",
    "lacks",
    "missing",
    "minus",
];

/// Multi-word negation phrases matched over contiguous token windows in `SceneSpec` prose fields.
const NEGATION_PHRASES: &[&[&str]] = &[
    &["free", "of"],
    &["devoid", "of"],
    &["absence", "of"],
    &["do", "not"],
    &["does", "not"],
    &["did", "not"],
    &["will", "not"],
    &["must", "not"],
    &["should", "not"],
    &["would", "not"],
    &["could", "not"],
    &["instead", "of"],
];

/// Deterministic output produced by `PromptCompiler`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompiledPrompt {
    /// Positive-only formatted prompt string ready for `ImageBackend`.
    pub prompt: String,
    /// Number of foreground focal slots compiled (`0..=3`).
    pub foreground_slot_count: usize,
    /// Resolved `Visibility` of the compiled scene.
    pub visibility: Visibility,
    /// Effective output width in pixels (defaults to `1024` if unspecified).
    pub target_width: u32,
    /// Effective output height in pixels (defaults to `1024` if unspecified).
    pub target_height: u32,
}

/// Deterministic Rust validator and positive-only prompt formatter for `SceneSpec`.
#[derive(Debug, Default, Clone, Copy)]
pub struct PromptCompiler;

impl PromptCompiler {
    /// Validate `spec` and compile a deterministic positive-only prompt.
    pub fn compile(spec: &SceneSpec) -> Result<CompiledPrompt, CoreError> {
        Self::compile_with_subject_visibility(spec, &BTreeMap::new())
    }

    /// Validate `spec` against an optional external `Subject` visibility registry and compile.
    pub fn compile_with_subject_visibility(
        spec: &SceneSpec,
        subject_visibility_overrides: &BTreeMap<String, Visibility>,
    ) -> Result<CompiledPrompt, CoreError> {
        validate_scene_spec(spec, subject_visibility_overrides)?;

        let target_width = spec.width.unwrap_or(1024);
        let target_height = spec.height.unwrap_or(1024);
        let aspect_ratio = format_aspect_ratio(target_width, target_height);

        let mut out = String::with_capacity(512);
        let _ = writeln!(out, "[SCENE]");
        let _ = writeln!(out, "id: {}", spec.id.trim());
        let _ = writeln!(out, "title: {}", collapse_prose(&spec.title));
        let _ = writeln!(out, "visibility: {}", spec.visibility.as_str());
        let _ = writeln!(out, "action: {}", collapse_prose(&spec.action_summary));

        let _ = writeln!(out, "\n[CAMERA]");
        let _ = writeln!(out, "shot: {}", collapse_prose(&spec.camera.shot));
        if let Some(ref angle) = spec.camera.angle {
            let _ = writeln!(out, "angle: {}", collapse_prose(angle));
        }
        if let Some(ref framing) = spec.camera.framing {
            let _ = writeln!(out, "framing: {}", collapse_prose(framing));
        }

        let _ = writeln!(out, "\n[DEPTH_BANDS]");
        for band in &spec.depth_bands {
            let _ = writeln!(
                out,
                "- {} ({:.2}m..{:.2}m): {}",
                band.plane.as_str(),
                band.near_meters,
                band.far_meters,
                collapse_prose(&band.description)
            );
        }

        let _ = writeln!(out, "\n[FOREGROUND_SLOTS_LEFT_TO_RIGHT]");
        for (idx, slot) in spec.foreground_slots.iter().enumerate() {
            let _ = writeln!(
                out,
                "- slot_{} x={:.3} subject={} height={:.2}m pose={}",
                idx + 1,
                slot.position_x_ratio,
                slot.subject_ref.trim(),
                slot.height_meters,
                collapse_prose(&slot.pose)
            );
            let left_desc = match slot.left_hand.item {
                Some(ref item) => format!(
                    "{} ({})",
                    collapse_prose(&slot.left_hand.action),
                    collapse_prose(item)
                ),
                None => collapse_prose(&slot.left_hand.action),
            };
            let right_desc = match slot.right_hand.item {
                Some(ref item) => format!(
                    "{} ({})",
                    collapse_prose(&slot.right_hand.action),
                    collapse_prose(item)
                ),
                None => collapse_prose(&slot.right_hand.action),
            };
            let _ = writeln!(out, "  left_hand: {left_desc}");
            let _ = writeln!(out, "  right_hand: {right_desc}");

            let mut sorted_bindings = slot.exclusive_surface_map.bindings.clone();
            sorted_bindings.sort_by(|a, b| a.slot.cmp(&b.slot));
            let surfaces: Vec<String> = sorted_bindings
                .into_iter()
                .map(|b| {
                    let hex = b.color_hex.trim().to_ascii_uppercase();
                    match b.finish {
                        Some(ref finish) => format!(
                            "{}={} {} ({})",
                            b.slot.trim(),
                            collapse_prose(&b.material),
                            hex,
                            collapse_prose(finish)
                        ),
                        None => {
                            format!("{}={} {}", b.slot.trim(), collapse_prose(&b.material), hex)
                        }
                    }
                })
                .collect();
            let _ = writeln!(out, "  exclusive_surface_map: {}", surfaces.join("; "));
        }

        let _ = writeln!(out, "\n[STYLE]");
        let _ = writeln!(out, "style_ref: {}", spec.style_ref.trim());

        let _ = writeln!(out, "\n[ATMOSPHERE]");
        let _ = writeln!(out, "atmosphere_ref: {}", spec.atmosphere_ref.trim());

        let _ = writeln!(out, "\n[OUTPUT]");
        let _ = writeln!(out, "dimensions: {target_width}x{target_height}");
        let _ = write!(out, "aspect_ratio: {aspect_ratio}");

        Ok(CompiledPrompt {
            prompt: out,
            foreground_slot_count: spec.foreground_slots.len(),
            visibility: spec.visibility,
            target_width,
            target_height,
        })
    }
}

fn collapse_prose(raw: &str) -> String {
    raw.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn require_identifier(field: &str, value: &str) -> Result<(), CoreError> {
    require_non_empty(field, value)?;
    if value.chars().any(char::is_control) {
        return Err(CoreError::PromptCompilationFailed(format!(
            "{field} must not contain control characters or newlines"
        )));
    }
    Ok(())
}

fn validate_prose_field(field: &str, value: &str) -> Result<(), CoreError> {
    require_non_empty(field, value)?;
    if value
        .chars()
        .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
    {
        return Err(CoreError::PromptCompilationFailed(format!(
            "{field} must not contain non-whitespace control characters"
        )));
    }
    check_no_active_negations(field, value)
}

fn require_non_empty(field: &str, value: &str) -> Result<(), CoreError> {
    if value.trim().is_empty() {
        return Err(CoreError::PromptCompilationFailed(format!(
            "{field} must not be empty"
        )));
    }
    Ok(())
}

fn check_no_active_negations(field: &str, text: &str) -> Result<(), CoreError> {
    let normalized_quotes = text.replace(['\u{2019}', '\u{2018}', '\u{02BC}'], "'");
    let lower = normalized_quotes.to_ascii_lowercase();
    let normalized_tokens: Vec<String> = lower
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '\'')
        .map(|tok| tok.trim_matches('\''))
        .filter(|tok| !tok.is_empty())
        .map(str::to_string)
        .collect();

    for token in &normalized_tokens {
        if NEGATION_WORDS.contains(&token.as_str()) {
            return Err(CoreError::PromptCompilationFailed(format!(
                "{field} contains active negation token '{token}'; prompts must use positive-only phrasing"
            )));
        }
    }

    let token_refs: Vec<&str> = normalized_tokens.iter().map(String::as_str).collect();
    for phrase in NEGATION_PHRASES {
        if token_refs.windows(phrase.len()).any(|win| win == *phrase) {
            return Err(CoreError::PromptCompilationFailed(format!(
                "{field} contains active negation phrase '{}'; prompts must use positive-only phrasing",
                phrase.join(" ")
            )));
        }
    }

    Ok(())
}

fn is_valid_hex_color(raw: &str) -> bool {
    let bytes = raw.as_bytes();
    bytes.len() == 7 && bytes[0] == b'#' && bytes[1..].iter().all(u8::is_ascii_hexdigit)
}

fn validate_scene_spec(
    spec: &SceneSpec,
    subject_visibility_overrides: &BTreeMap<String, Visibility>,
) -> Result<(), CoreError> {
    if spec.schema_version != SUPPORTED_SCENE_SPEC_SCHEMA_VERSION {
        return Err(CoreError::PromptCompilationFailed(format!(
            "unsupported SceneSpec schema_version {} (expected {})",
            spec.schema_version, SUPPORTED_SCENE_SPEC_SCHEMA_VERSION
        )));
    }

    require_identifier("id", &spec.id)?;
    require_identifier("style_ref", &spec.style_ref)?;
    require_identifier("atmosphere_ref", &spec.atmosphere_ref)?;

    validate_prose_field("title", &spec.title)?;
    validate_prose_field("action_summary", &spec.action_summary)?;
    validate_prose_field("camera.shot", &spec.camera.shot)?;
    if let Some(ref angle) = spec.camera.angle {
        validate_prose_field("camera.angle", angle)?;
    }
    if let Some(ref framing) = spec.camera.framing {
        validate_prose_field("camera.framing", framing)?;
    }

    if let Some(w) = spec.width
        && w == 0
    {
        return Err(CoreError::PromptCompilationFailed(
            "width must be greater than 0".to_string(),
        ));
    }
    if let Some(h) = spec.height
        && h == 0
    {
        return Err(CoreError::PromptCompilationFailed(
            "height must be greater than 0".to_string(),
        ));
    }

    if spec.depth_bands.is_empty() {
        return Err(CoreError::PromptCompilationFailed(
            "depth_bands must contain at least 1 band".to_string(),
        ));
    }

    for (idx, band) in spec.depth_bands.iter().enumerate() {
        let prefix = format!("depth_bands[{idx}]");
        if !band.near_meters.is_finite() || band.near_meters < 0.0 {
            return Err(CoreError::PromptCompilationFailed(format!(
                "{prefix}.near_meters must be finite and >= 0.0"
            )));
        }
        if !band.far_meters.is_finite() || band.far_meters <= band.near_meters {
            return Err(CoreError::PromptCompilationFailed(format!(
                "{prefix}.far_meters must be finite and strictly greater than near_meters"
            )));
        }
        validate_prose_field(&format!("{prefix}.description"), &band.description)?;
    }

    if spec.foreground_slots.len() > MAX_FOREGROUND_SLOTS {
        return Err(CoreError::PromptCompilationFailed(format!(
            "foreground_slots has {} entries, exceeding maximum of {}",
            spec.foreground_slots.len(),
            MAX_FOREGROUND_SLOTS
        )));
    }

    let mut previous_x: Option<f64> = None;
    for (idx, slot) in spec.foreground_slots.iter().enumerate() {
        let prefix = format!("foreground_slots[{idx}]");
        require_identifier(&format!("{prefix}.subject_ref"), &slot.subject_ref)?;
        validate_prose_field(&format!("{prefix}.pose"), &slot.pose)?;

        if !slot.position_x_ratio.is_finite()
            || slot.position_x_ratio < 0.0
            || slot.position_x_ratio > 1.0
        {
            return Err(CoreError::PromptCompilationFailed(format!(
                "{prefix}.position_x_ratio must be finite within [0.0, 1.0]"
            )));
        }

        if let Some(prev) = previous_x
            && slot.position_x_ratio <= prev
        {
            return Err(CoreError::PromptCompilationFailed(format!(
                "{prefix}.position_x_ratio ({:.3}) must be strictly greater than previous slot ({:.3}) for left-to-right ordering",
                slot.position_x_ratio, prev
            )));
        }
        previous_x = Some(slot.position_x_ratio);

        if !slot.height_meters.is_finite() || slot.height_meters <= 0.0 {
            return Err(CoreError::PromptCompilationFailed(format!(
                "{prefix}.height_meters must be finite and strictly positive"
            )));
        }

        let effective_subject_vis = subject_visibility_overrides
            .get(slot.subject_ref.trim())
            .copied()
            .unwrap_or(slot.visibility);
        if !spec.visibility.allows_asset(effective_subject_vis) {
            return Err(CoreError::PromptCompilationFailed(format!(
                "{prefix} references Subject '{}' with visibility '{}' which exceeds SceneSpec visibility '{}'",
                slot.subject_ref,
                effective_subject_vis.as_str(),
                spec.visibility.as_str()
            )));
        }

        validate_prose_field(
            &format!("{prefix}.left_hand.action"),
            &slot.left_hand.action,
        )?;
        if let Some(ref item) = slot.left_hand.item {
            validate_prose_field(&format!("{prefix}.left_hand.item"), item)?;
        }

        validate_prose_field(
            &format!("{prefix}.right_hand.action"),
            &slot.right_hand.action,
        )?;
        if let Some(ref item) = slot.right_hand.item {
            validate_prose_field(&format!("{prefix}.right_hand.item"), item)?;
        }

        if slot.exclusive_surface_map.bindings.is_empty() {
            return Err(CoreError::PromptCompilationFailed(format!(
                "{prefix}.exclusive_surface_map.bindings must not be empty"
            )));
        }

        let mut seen_slots = HashSet::new();
        for (b_idx, binding) in slot.exclusive_surface_map.bindings.iter().enumerate() {
            let b_prefix = format!("{prefix}.exclusive_surface_map.bindings[{b_idx}]");
            require_identifier(&format!("{b_prefix}.slot"), &binding.slot)?;
            validate_prose_field(&format!("{b_prefix}.material"), &binding.material)?;

            let slot_key = binding.slot.trim().to_ascii_lowercase();
            if !seen_slots.insert(slot_key) {
                return Err(CoreError::PromptCompilationFailed(format!(
                    "{b_prefix}.slot duplicates surface slot '{}'",
                    binding.slot
                )));
            }

            if !is_valid_hex_color(binding.color_hex.trim()) {
                return Err(CoreError::PromptCompilationFailed(format!(
                    "{b_prefix}.color_hex '{}' must be a 6-digit hex color (#RRGGBB)",
                    binding.color_hex
                )));
            }

            if let Some(ref finish) = binding.finish {
                validate_prose_field(&format!("{b_prefix}.finish"), finish)?;
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;
    use crate::scene_spec::{
        CameraSpec, DepthBandSpec, DepthPlane, ExclusiveSurfaceMap, ForegroundSlot, HandBinding,
        SurfaceBinding,
    };

    fn sample_slot(x: f64, subject_ref: &str, visibility: Visibility) -> ForegroundSlot {
        ForegroundSlot {
            position_x_ratio: x,
            subject_ref: subject_ref.to_string(),
            visibility,
            height_meters: 1.78,
            pose: "facing three-quarters right in balanced stance".to_string(),
            left_hand: HandBinding {
                item: Some("brass astrolabe".to_string()),
                action: "cradling".to_string(),
            },
            right_hand: HandBinding {
                item: None,
                action: "open palm resting on leather belt".to_string(),
            },
            exclusive_surface_map: ExclusiveSurfaceMap {
                bindings: vec![
                    SurfaceBinding {
                        slot: "cloak".to_string(),
                        material: "heavy wool".to_string(),
                        color_hex: "#2B3A4A".to_string(),
                        finish: Some("matte".to_string()),
                    },
                    SurfaceBinding {
                        slot: "tunic".to_string(),
                        material: "woven linen".to_string(),
                        color_hex: "#D6CBB8".to_string(),
                        finish: None,
                    },
                ],
            },
        }
    }

    fn sample_valid_scene() -> SceneSpec {
        SceneSpec {
            schema_version: 1,
            id: "harbor-bridge-exchange".to_string(),
            title: "Northern Pier Lantern Exchange".to_string(),
            visibility: Visibility::Public,
            camera: CameraSpec {
                shot: "medium-wide 35mm".to_string(),
                angle: Some("eye-level three-quarter".to_string()),
                framing: Some("rule-of-thirds horizontal".to_string()),
            },
            depth_bands: vec![
                DepthBandSpec {
                    plane: DepthPlane::Foreground,
                    near_meters: 1.5,
                    far_meters: 4.0,
                    description: "wet stone dock planks and brass mooring bollard".to_string(),
                },
                DepthBandSpec {
                    plane: DepthPlane::Background,
                    near_meters: 12.0,
                    far_meters: 80.0,
                    description: "misty northern lighthouse tower on rocky headland".to_string(),
                },
            ],
            foreground_slots: vec![
                sample_slot(0.28, "navigator-kael", Visibility::Public),
                sample_slot(0.72, "quartermaster-soren", Visibility::Public),
            ],
            style_ref: "clean-ink-wash".to_string(),
            atmosphere_ref: "cold-dawn-mist".to_string(),
            action_summary: "Two travelers inspect a brass astrolabe along a rain-slick stone pier"
                .to_string(),
            width: Some(1280),
            height: Some(720),
        }
    }

    #[test]
    fn compiles_valid_scene_spec_deterministically() {
        let spec = sample_valid_scene();
        let compiled_a = PromptCompiler::compile(&spec).expect("valid SceneSpec compiles");
        let compiled_b = PromptCompiler::compile(&spec).expect("deterministic re-compile");

        assert_eq!(compiled_a, compiled_b);
        assert_eq!(compiled_a.foreground_slot_count, 2);
        assert_eq!(compiled_a.target_width, 1280);
        assert_eq!(compiled_a.target_height, 720);
        assert!(
            compiled_a
                .prompt
                .contains("slot_1 x=0.280 subject=navigator-kael")
        );
        assert!(
            compiled_a
                .prompt
                .contains("slot_2 x=0.720 subject=quartermaster-soren")
        );
        assert!(
            compiled_a
                .prompt
                .contains("cloak=heavy wool #2B3A4A (matte); tunic=woven linen #D6CBB8")
        );
        assert!(
            compiled_a
                .prompt
                .contains("[OUTPUT]\ndimensions: 1280x720\naspect_ratio: 16:9")
        );
    }

    #[rstest]
    #[case::northern_knotwork(
        "Northern Knotwork Observatory",
        "Scholars study knotted astronomical charts facing north"
    )]
    #[case::candid_note(
        "Candid Note Exchange",
        "Courier hands a candid note across the harbor table"
    )]
    #[case::carefree_office(
        "Carefree Office Morning",
        "Artisan works in a sunlit carefree office by the pier"
    )]
    #[case::pseudo_notice(
        "Pseudo Notice Board",
        "Traveler inspects a weathered pseudo notice pinned to oak"
    )]
    fn allows_benign_words_and_token_boundaries_without_false_positives(
        #[case] title: &str,
        #[case] action: &str,
    ) {
        let mut spec = sample_valid_scene();
        spec.title = title.to_string();
        spec.action_summary = action.to_string();
        PromptCompiler::compile(&spec).expect("benign token boundaries must pass");
    }

    #[rstest]
    #[case::do_not("Travelers do not cross the pier")]
    #[case::curly_apostrophe_dont("Travelers don\u{2019}t cross the pier")]
    #[case::curly_apostrophe_isnt("Lantern isn\u{2019}t lit")]
    #[case::none_token("Pier holds none of the cargo")]
    #[case::nothing_token("Bench carries nothing")]
    #[case::free_of_phrase("Harbor deck free of barrels")]
    fn rejects_negation_phrases_and_typographic_apostrophes(#[case] action: &str) {
        let mut spec = sample_valid_scene();
        spec.action_summary = action.to_string();
        let err = PromptCompiler::compile(&spec).expect_err("negation must fail closed");
        assert!(matches!(err, CoreError::PromptCompilationFailed(_)));
    }

    #[rstest]
    #[case::newline_in_id(|s: &mut SceneSpec| s.id = "bad\n[STYLE]\nstyle_ref: evil".to_string())]
    #[case::carriage_return_in_style_ref(|s: &mut SceneSpec| s.style_ref = "ink\revil".to_string())]
    #[case::nul_in_atmosphere_ref(|s: &mut SceneSpec| s.atmosphere_ref = "dawn\0mist".to_string())]
    #[case::newline_in_subject_ref(|s: &mut SceneSpec| s.foreground_slots[0].subject_ref = "kael\nevil".to_string())]
    #[case::newline_in_surface_slot(|s: &mut SceneSpec| s.foreground_slots[0].exclusive_surface_map.bindings[0].slot = "cloak\nevil".to_string())]
    #[case::nul_in_prose(|s: &mut SceneSpec| s.action_summary = "harbor\0pier".to_string())]
    fn rejects_control_characters_in_identifiers_and_prose(#[case] mutate: fn(&mut SceneSpec)) {
        let mut spec = sample_valid_scene();
        mutate(&mut spec);
        let err = PromptCompiler::compile(&spec)
            .expect_err("control characters in identifiers or prose must fail closed");
        assert!(matches!(err, CoreError::PromptCompilationFailed(_)));
    }

    #[test]
    fn collapses_newlines_in_prose_to_prevent_section_spoofing() {
        let mut spec = sample_valid_scene();
        spec.action_summary = "Courier walks along pier\n[STYLE]\nstyle_ref: hijacked".to_string();
        let compiled = PromptCompiler::compile(&spec).expect("valid prose with newline compiles");
        assert!(
            compiled
                .prompt
                .contains("action: Courier walks along pier [STYLE] style_ref: hijacked\n")
        );
        assert_eq!(compiled.prompt.matches("\n[STYLE]\n").count(), 1);
    }

    #[rstest]
    #[case::wrong_schema_version(|s: &mut SceneSpec| s.schema_version = 2)]
    #[case::active_negation_no_in_action(|s: &mut SceneSpec| s.action_summary = "Travelers stand with no lantern".to_string())]
    #[case::active_negation_without_in_pose(|s: &mut SceneSpec| s.foreground_slots[0].pose = "standing without helmet".to_string())]
    #[case::active_negation_phrase_free_of(|s: &mut SceneSpec| s.depth_bands[0].description = "clean dock free of crates".to_string())]
    #[case::active_negation_avoid_in_hand(|s: &mut SceneSpec| s.foreground_slots[0].left_hand.action = "raised to avoid glare".to_string())]
    #[case::too_many_foreground_slots(|s: &mut SceneSpec| {
        s.foreground_slots.push(sample_slot(0.85, "third-envoy", Visibility::Public));
        s.foreground_slots.push(sample_slot(0.95, "fourth-envoy", Visibility::Public));
    })]
    #[case::out_of_order_left_to_right_slots(|s: &mut SceneSpec| {
        s.foreground_slots[0].position_x_ratio = 0.70;
        s.foreground_slots[1].position_x_ratio = 0.30;
    })]
    #[case::equal_x_ratio_slots(|s: &mut SceneSpec| {
        s.foreground_slots[0].position_x_ratio = 0.50;
        s.foreground_slots[1].position_x_ratio = 0.50;
    })]
    #[case::non_positive_height_meters(|s: &mut SceneSpec| s.foreground_slots[0].height_meters = 0.0)]
    #[case::inverted_depth_band(|s: &mut SceneSpec| {
        s.depth_bands[0].near_meters = 5.0;
        s.depth_bands[0].far_meters = 3.0;
    })]
    #[case::visibility_leak_restricted_in_public(|s: &mut SceneSpec| {
        s.visibility = Visibility::Public;
        s.foreground_slots[1].visibility = Visibility::Restricted;
    })]
    #[case::visibility_leak_internal_in_restricted(|s: &mut SceneSpec| {
        s.visibility = Visibility::Restricted;
        s.foreground_slots[1].visibility = Visibility::Internal;
    })]
    #[case::empty_exclusive_surface_map(|s: &mut SceneSpec| {
        s.foreground_slots[0].exclusive_surface_map.bindings.clear();
    })]
    #[case::duplicate_surface_slot(|s: &mut SceneSpec| {
        let dup = s.foreground_slots[0].exclusive_surface_map.bindings[0].clone();
        s.foreground_slots[0].exclusive_surface_map.bindings.push(dup);
    })]
    #[case::invalid_hex_color(|s: &mut SceneSpec| {
        s.foreground_slots[0].exclusive_surface_map.bindings[0].color_hex = "red".to_string();
    })]
    fn rejects_invalid_scene_specs_fail_closed(#[case] mutate: fn(&mut SceneSpec)) {
        let mut spec = sample_valid_scene();
        mutate(&mut spec);
        let err = PromptCompiler::compile(&spec).expect_err("invalid SceneSpec must fail closed");
        assert!(matches!(err, CoreError::PromptCompilationFailed(_)));
    }

    #[test]
    fn enforces_external_subject_visibility_overrides() {
        let spec = sample_valid_scene();
        let mut overrides = BTreeMap::new();
        overrides.insert("quartermaster-soren".to_string(), Visibility::Internal);

        let err = PromptCompiler::compile_with_subject_visibility(&spec, &overrides)
            .expect_err("internal subject override must fail in public scene");
        assert!(matches!(err, CoreError::PromptCompilationFailed(_)));
    }

    #[test]
    fn scene_spec_yaml_rejects_unknown_fields() {
        let raw = r#"
schema_version: 1
id: test
title: Harbor Morning
visibility: public
camera:
  shot: medium
depth_bands:
  - plane: foreground
    near_meters: 1.0
    far_meters: 3.0
    description: wooden pier
foreground_slots: []
style_ref: ink
atmosphere_ref: dawn
action_summary: quiet pier
unexpected_field: 123
"#;
        let err = SceneSpec::from_yaml_str(raw).expect_err("unknown field must fail closed");
        assert!(matches!(err, CoreError::SceneSpecInvalid(_)));
    }
}
