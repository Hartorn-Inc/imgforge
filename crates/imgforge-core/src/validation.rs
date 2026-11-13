//! === LIFECYCLE VALIDATION & CANONICAL PROMOTION SERVICE ===
//!
//! PURPOSE:
//! Implements fail-closed structural + `ColorQaEngine` + `VisionCritic`
//! validation (`draft -> candidate -> validated`) and explicit canonical
//! promotion (`validated -> canonical`, superseding prior canonical versions in
//! `library/manifest.lock.json`) for both `Run`s and style-scoped
//! `ReferencePack`s.
//!
//! THREAT MODEL / DESIGN RATIONALE ("WHY"):
//! 1. `Run` and `ReferencePack` artifacts start in `Lifecycle::Draft` (or
//!    `Candidate`) and must never reach `Canonical` without verified structural
//!    integrity (non-symlink files, valid PNG decode, matching SHA-256 digests),
//!    perceptual `ColorQaEngine` checks, and optional/required `VisionCritic`
//!    per-trait sign-off.
//! 2. `Canonical` artifacts are immutable; promoting a newer version atomically
//!    supersedes the previous `Canonical` version in `manifest.lock.json` and
//!    on disk without split-brain states.
//!
//! EXECUTION LIFECYCLE / DATA FLOW:
//! 1. `validate_run`: loads `RunMetadata`, verifies PNG structure & SHA-256,
//!    runs `ColorQaEngine` and optional `VisionCritic`, advances
//!    `draft -> candidate -> validated`, and persists updated metadata atomically.
//! 2. `validate_reference_pack`: loads `pack.json` & `slices.json`, verifies
//!    sheets, calibration swatches, slices, and SHA-256 hashes, runs sparkle
//!    Color QA and optional `VisionCritic`, advances `draft -> candidate -> validated`,
//!    and persists `pack.json` atomically.
//! 3. `promote_run` / `promote_reference_pack`: enforces `Lifecycle::Validated`,
//!    transitions to `Canonical`, supersedes prior version, and updates
//!    `library/manifest.lock.json` atomically.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::calibration::{
    StructuralValidationConfig, parse_hex_rgb, validate_reference_sheet_structure,
};
use crate::color_qa::{
    ColorQaEngine, ColorQaReport, ExposureQaConfig, PaletteQaConfig, SparkleConfig, SubjectRoiSpec,
    srgb_delta_e_2000,
};
use crate::critic::{
    CriticReport, TraitCategory, TraitCheckInput, VisionCritic, VisionCriticRequest,
};
use crate::edit::OwnedSubjectRoi;
use crate::error::CoreError;
use crate::generate::SubjectRoiBinding;
use crate::lifecycle::{Lifecycle, sha256_hex, validate_slug};
use crate::reference_pack::{REFERENCE_SHEET_HEIGHT, REFERENCE_SHEET_WIDTH, ReferencePackManifest};
use crate::registry::{LibraryRegistry, inspect_asset_image_visibility};
use crate::scene_spec::check_visibility_compatibility;
use crate::slicer::SlicesManifest;
use crate::store::{
    ManifestLock, ManifestLockStore, RunMetadata, RunStore, decode_png_checked,
    read_regular_file_nofollow, validate_strict_relative_path, verify_contained_path_no_symlinks,
    write_atomic,
};

/// Request payload for validating a recorded `Run` (`draft -> candidate -> validated`).
#[derive(Debug, Clone)]
pub struct ValidateRunRequest {
    /// Target `Run` stream slug.
    pub slug: String,
    /// Target `Run` version number (`1`, `2`, ...).
    pub version: u32,
    /// Optional `Atmosphere` slug enforcing `exposure_floor`.
    pub atmosphere_slug: Option<String>,
    /// Optional `Subject` ROI bindings for `PaletteLock` QA.
    pub subject_rois: Vec<SubjectRoiBinding>,
    /// Optional inline `OwnedSubjectRoi` entries for `PaletteLock` QA.
    pub explicit_subject_rois: Vec<OwnedSubjectRoi>,
    /// Reference anchor images passed to `VisionCritic`.
    pub reference_images: Vec<PathBuf>,
    /// Explicit binary trait checklist items for `VisionCritic`.
    pub traits: Vec<TraitCheckInput>,
    /// Run `VisionCritic` even when `traits` is empty (using default `zero_leaked_text` check).
    pub run_critic: bool,
    /// Exposure QA config.
    pub exposure_config: ExposureQaConfig,
    /// Palette QA config.
    pub palette_config: PaletteQaConfig,
    /// Sparkle QA config.
    pub sparkle_config: SparkleConfig,
}

impl ValidateRunRequest {
    /// Construct a default `ValidateRunRequest` for `(slug, version)`.
    #[must_use]
    pub fn new(slug: impl Into<String>, version: u32) -> Self {
        Self {
            slug: slug.into(),
            version,
            atmosphere_slug: None,
            subject_rois: Vec::new(),
            explicit_subject_rois: Vec::new(),
            reference_images: Vec::new(),
            traits: Vec::new(),
            run_critic: false,
            exposure_config: ExposureQaConfig::default(),
            palette_config: PaletteQaConfig::default(),
            sparkle_config: SparkleConfig::default(),
        }
    }
}

/// Structured result emitted after validating a `Run`.
#[derive(Debug, Clone, Serialize)]
pub struct ValidateRunResult {
    /// Updated `RunMetadata` in `Lifecycle::Validated`.
    pub metadata: RunMetadata,
    /// Verified `ColorQaReport`.
    pub color_qa: ColorQaReport,
    /// Optional `CriticReport` when `VisionCritic` executed.
    pub critic_report: Option<CriticReport>,
}

/// Request payload for validating a style-scoped `ReferencePack` (`draft -> candidate -> validated`).
#[derive(Debug, Clone)]
pub struct ValidateReferencePackRequest {
    /// Target `Subject` slug.
    pub subject_slug: String,
    /// Target `Style` slug.
    pub style_slug: String,
    /// Optional structural validation thresholds.
    pub structural_config: StructuralValidationConfig,
    /// Sparkle QA thresholds.
    pub sparkle_config: SparkleConfig,
    /// Explicit binary trait checklist items for `VisionCritic`.
    pub traits: Vec<TraitCheckInput>,
    /// Run `VisionCritic` even when `traits` is empty (using default sheet checklist).
    pub run_critic: bool,
}

impl ValidateReferencePackRequest {
    /// Construct a default `ValidateReferencePackRequest` for `(subject_slug, style_slug)`.
    #[must_use]
    pub fn new(subject_slug: impl Into<String>, style_slug: impl Into<String>) -> Self {
        Self {
            subject_slug: subject_slug.into(),
            style_slug: style_slug.into(),
            structural_config: StructuralValidationConfig::default(),
            sparkle_config: SparkleConfig::default(),
            traits: Vec::new(),
            run_critic: false,
        }
    }
}

/// Structured result emitted after validating a `ReferencePack`.
#[derive(Debug, Clone, Serialize)]
pub struct ValidateReferencePackResult {
    /// Updated `ReferencePackManifest` in `Lifecycle::Validated`.
    pub manifest: ReferencePackManifest,
    /// Structural and color QA notes verified across sheets and slices.
    pub checks: Vec<String>,
    /// Optional `CriticReport` when `VisionCritic` executed.
    pub critic_report: Option<CriticReport>,
}

/// Workspace and `manifest.lock.json` status report returned by `inspect_workspace_status`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceLifecycleStatus {
    /// Number of active `canonical` entries in `manifest.lock.json`.
    pub canonical_count: usize,
    /// Number of historical `superseded` entries in `manifest.lock.json`.
    pub superseded_count: usize,
    /// Active `canonical` lock entries keyed by canonical ID.
    pub canonical: BTreeMap<String, crate::store::ManifestEntry>,
    /// Recorded `Run` metadata grouped by stream slug.
    pub runs: BTreeMap<String, Vec<RunMetadata>>,
}

/// Validate a recorded `Run` (`draft -> candidate -> validated`) with structural,
/// `ColorQaEngine`, and optional/required `VisionCritic` checks.
pub async fn validate_run(
    run_store: &RunStore,
    registry: &LibraryRegistry,
    critic: Option<&VisionCritic>,
    request: &ValidateRunRequest,
) -> Result<ValidateRunResult, CoreError> {
    let mut meta = run_store.load_run(&request.slug, request.version)?;
    meta.lifecycle.ensure_mutable(&meta.run_id)?;
    if meta.lifecycle == Lifecycle::Validated {
        return Err(CoreError::LifecycleTransitionInvalid {
            from: Lifecycle::Validated.as_str().to_string(),
            to: Lifecycle::Validated.as_str().to_string(),
            reason: "Run is already 'validated'; promote it to 'canonical' instead".to_string(),
        });
    }

    // 1. Structural check: symlink-free PNG inside runs_dir, exact SHA-256, exact dimensions.
    let version_label = RunStore::format_version_label(request.version);
    let rel_img = Path::new(&request.slug).join(format!("{version_label}.png"));
    let img_path =
        verify_contained_path_no_symlinks(run_store.runs_dir(), &rel_img, false, "Run PNG")?;
    let png_bytes = read_regular_file_nofollow(&img_path, "Run PNG")?;
    let actual_sha = sha256_hex(&png_bytes);
    if actual_sha != meta.image_sha256 {
        return Err(CoreError::InvalidPng {
            path: img_path,
            reason: format!(
                "Run PNG SHA-256 mismatch (metadata {}, disk {})",
                meta.image_sha256, actual_sha
            ),
        });
    }
    let decoded = decode_png_checked(&img_path, &png_bytes)?.to_rgba8();
    if decoded.width() != meta.backend.width || decoded.height() != meta.backend.height {
        return Err(CoreError::InvalidPng {
            path: img_path,
            reason: format!(
                "Run PNG dimensions {}x{} disagree with recorded {}x{}",
                decoded.width(),
                decoded.height(),
                meta.backend.width,
                meta.backend.height
            ),
        });
    }

    // 2. ColorQaEngine checks.
    let exposure_arg = match request.atmosphere_slug {
        Some(ref atmo_slug) => {
            let resolved = registry.resolve_atmosphere(atmo_slug)?;
            Some((resolved.atmosphere.exposure_floor, &request.exposure_config))
        }
        None => None,
    };

    let mut owned_rois = request.explicit_subject_rois.clone();
    for binding in &request.subject_rois {
        let resolved = registry.resolve_subject(binding.subject_slug.trim())?;
        owned_rois.push(OwnedSubjectRoi {
            subject_slug: resolved.subject.slug,
            visibility: resolved.subject.visibility,
            palette_lock: resolved.subject.palette_lock,
            rois: binding.rois.clone(),
        });
    }
    check_visibility_compatibility(
        meta.visibility,
        "ValidateRun Subject",
        owned_rois
            .iter()
            .map(|s| (s.subject_slug.as_str(), s.visibility)),
    )?;

    for ref_path in &request.reference_images {
        let ref_bytes = read_regular_file_nofollow(ref_path, "ValidateRun reference PNG")?;
        let _ = decode_png_checked(ref_path, &ref_bytes)?;
        if let Some(asset_vis) =
            inspect_asset_image_visibility(Some(run_store), Some(registry), ref_path)?
        {
            check_visibility_compatibility(
                meta.visibility,
                "ValidateRun Reference",
                [(ref_path.display().to_string().as_str(), asset_vis)],
            )?;
        }
    }
    let subject_specs: Vec<SubjectRoiSpec<'_>> = owned_rois
        .iter()
        .map(|s| SubjectRoiSpec {
            subject_slug: &s.subject_slug,
            palette_lock: &s.palette_lock,
            rois: s.rois.clone(),
        })
        .collect();

    let qa_engine = ColorQaEngine::new();
    let color_qa = qa_engine.evaluate_whole_image(
        &decoded,
        exposure_arg,
        &subject_specs,
        &request.palette_config,
        None,
        Some((None, &request.sparkle_config)),
    )?;
    if !color_qa.passed {
        return Err(CoreError::ColorQaFailed(format!(
            "Run '{}' failed Color QA during validation: {}",
            meta.run_id,
            color_qa.notes.join("; ")
        )));
    }

    // 3. Optional/required VisionCritic check.
    let should_run_critic = request.run_critic || !request.traits.is_empty();
    let critic_report = if should_run_critic {
        let vc = critic.ok_or_else(|| {
            CoreError::InvalidGenerationRequest(
                "VisionCritic is required when run_critic=true or traits are supplied".to_string(),
            )
        })?;
        let traits = if request.traits.is_empty() {
            vec![TraitCheckInput {
                trait_id: "zero-leaked-text".to_string(),
                category: TraitCategory::ZeroLeakedText,
                criterion: "No stray text, labels, watermarks, or captions appear on the image"
                    .to_string(),
            }]
        } else {
            request.traits.clone()
        };
        let report = vc
            .evaluate(&VisionCriticRequest {
                target_image: img_path,
                reference_images: request.reference_images.clone(),
                traits,
            })
            .await?;
        if !report.passed {
            return Err(CoreError::StructuredOutputInvalid {
                stage: "VisionCritic",
                reason: format!(
                    "Run '{}' failed VisionCritic: {}",
                    meta.run_id, report.summary
                ),
            });
        }
        Some(report)
    } else {
        meta.qa_reports.critic_report.clone()
    };

    // 4. Advance Lifecycle strictly: draft -> candidate -> validated.
    let mut structural_checks = vec![format!(
        "structural OK ({}x{}, sha256={}...)",
        decoded.width(),
        decoded.height(),
        &actual_sha[..12]
    )];
    structural_checks.extend(color_qa.notes.iter().cloned());

    let updated_qa = crate::store::RunQaReports {
        structural_checks,
        critic_report: critic_report.clone(),
        color_qa_max_delta_e2000: Some(color_qa.max_delta_e2000),
    };
    meta = run_store.update_run_qa(&request.slug, request.version, updated_qa)?;
    if meta.lifecycle == Lifecycle::Draft {
        let _ = run_store.transition_run_lifecycle(
            &request.slug,
            request.version,
            Lifecycle::Candidate,
        )?;
    }
    meta =
        run_store.transition_run_lifecycle(&request.slug, request.version, Lifecycle::Validated)?;

    Ok(ValidateRunResult {
        metadata: meta,
        color_qa,
        critic_report,
    })
}

/// Validate a style-scoped `ReferencePack` (`draft -> candidate -> validated`)
/// by verifying sheets, host-composited calibration swatches, slices, SHA-256 digests,
/// sparkle Color QA, and optional `VisionCritic`.
pub async fn validate_reference_pack(
    library_dir: &Path,
    registry: &LibraryRegistry,
    critic: Option<&VisionCritic>,
    request: &ValidateReferencePackRequest,
) -> Result<ValidateReferencePackResult, CoreError> {
    validate_slug(&request.subject_slug, "Subject").map_err(CoreError::SubjectInvalid)?;
    validate_slug(&request.style_slug, "Style").map_err(CoreError::StyleInvalid)?;

    let resolved_subject = registry.resolve_subject(&request.subject_slug)?;
    let _resolved_style = registry.resolve_style(&request.style_slug)?;

    let rel_pack_dir = Path::new("subjects")
        .join(&request.subject_slug)
        .join("packs")
        .join(&request.style_slug);
    let pack_dir = verify_contained_path_no_symlinks(
        library_dir,
        &rel_pack_dir,
        true,
        "ReferencePack directory",
    )?;
    let pack_path =
        verify_contained_path_no_symlinks(&pack_dir, Path::new("pack.json"), false, "pack.json")?;
    let pack_bytes = read_regular_file_nofollow(&pack_path, "pack.json")?;
    let mut manifest: ReferencePackManifest = serde_json::from_slice(&pack_bytes)
        .map_err(|err| CoreError::ReferencePackFailed(format!("parsing pack.json: {err}")))?;

    let pack_id = format!("pack:{}/{}", manifest.subject_slug, manifest.style_slug);
    manifest.lifecycle.ensure_mutable(&pack_id)?;
    if manifest.lifecycle == Lifecycle::Validated {
        return Err(CoreError::LifecycleTransitionInvalid {
            from: Lifecycle::Validated.as_str().to_string(),
            to: Lifecycle::Validated.as_str().to_string(),
            reason: "ReferencePack is already 'validated'; promote it to 'canonical' instead"
                .to_string(),
        });
    }
    if manifest.sheets.is_empty() {
        return Err(CoreError::ReferencePackFailed(
            "ReferencePack contains zero sheets".to_string(),
        ));
    }

    let mut checks = Vec::new();
    let qa_engine = ColorQaEngine::new();
    let mut primary_sheet_path: Option<PathBuf> = None;

    // 1. Verify every sheet PNG, structural #252830 coverage, calibration swatches, and sparkle QA.
    for sheet in &manifest.sheets {
        let rel_sheet = validate_strict_relative_path(&sheet.filename, "sheet filename")
            .map_err(CoreError::ReferencePackFailed)?;
        let sheet_path =
            verify_contained_path_no_symlinks(&pack_dir, &rel_sheet, false, &sheet.filename)?;
        if primary_sheet_path.is_none() {
            primary_sheet_path = Some(sheet_path.clone());
        }
        let sheet_bytes = read_regular_file_nofollow(&sheet_path, &sheet.filename)?;
        let actual_sha = sha256_hex(&sheet_bytes);
        if actual_sha != sheet.sha256 {
            return Err(CoreError::ReferencePackFailed(format!(
                "sheet '{}' SHA-256 mismatch (manifest {}, disk {})",
                sheet.filename, sheet.sha256, actual_sha
            )));
        }

        let structural = validate_reference_sheet_structure(
            &sheet_bytes,
            REFERENCE_SHEET_WIDTH,
            REFERENCE_SHEET_HEIGHT,
            &request.structural_config,
        )?;
        let rgba = decode_png_checked(&sheet_path, &sheet_bytes)?.to_rgba8();

        // Verify host-composited PaletteLock calibration swatches remain pixel-exact.
        for swatch in &sheet.calibration_strip.palette_swatch_cells {
            if swatch.rect.right() > rgba.width() || swatch.rect.bottom() > rgba.height() {
                return Err(CoreError::ReferencePackFailed(format!(
                    "calibration swatch '{}' on '{}' exceeds sheet bounds",
                    swatch.name, sheet.filename
                )));
            }
            let expected_rgb = parse_hex_rgb(&swatch.color_hex)?;
            let cx = swatch.rect.x + swatch.rect.width / 2;
            let cy = swatch.rect.y + swatch.rect.height / 2;
            let px = rgba.get_pixel(cx, cy).0;
            let de = srgb_delta_e_2000([px[0], px[1], px[2]], expected_rgb);
            if de > 0.5 {
                return Err(CoreError::ColorQaFailed(format!(
                    "calibration swatch '{}' on '{}' drifted DeltaE_2000={de:.2}",
                    swatch.name, sheet.filename
                )));
            }
        }

        // Verify PaletteLock swatches in Subject match calibration strip count.
        if sheet.calibration_strip.palette_swatch_cells.len()
            != resolved_subject.subject.palette_lock.swatches.len()
        {
            return Err(CoreError::ReferencePackFailed(format!(
                "sheet '{}' calibration strip swatch count ({}) disagrees with Subject '{}' PaletteLock ({})",
                sheet.filename,
                sheet.calibration_strip.palette_swatch_cells.len(),
                resolved_subject.subject.slug,
                resolved_subject.subject.palette_lock.swatches.len()
            )));
        }

        let w = rgba.width() as usize;
        let h = rgba.height() as usize;
        let mut strip_mask = vec![0.0f32; w * h];
        let strip_start_y = (sheet.calibration_strip.strip_y as usize).min(h);
        strip_mask[strip_start_y * w..].fill(1.0);
        let (sparkle_rep, _) =
            qa_engine.detect_sparkles(&rgba, Some(&strip_mask), &request.sparkle_config)?;
        if !sparkle_rep.passed {
            return Err(CoreError::ColorQaFailed(format!(
                "sheet '{}' failed sparkle Color QA: {}",
                sheet.filename,
                sparkle_rep.reasons.join("; ")
            )));
        }

        checks.push(format!(
            "sheet '{}' OK ({}x{}, bg={:.1}%, swatches={}, {} sparkles)",
            sheet.filename,
            structural.width,
            structural.height,
            structural.bg_coverage * 100.0,
            sheet.calibration_strip.palette_swatch_cells.len(),
            sparkle_rep.flagged_blobs.len()
        ));
    }

    // 2. Verify slices.json and every slice PNG.
    let slices_path = verify_contained_path_no_symlinks(
        &pack_dir,
        Path::new("slices.json"),
        false,
        "slices.json",
    )?;
    let slices_bytes = read_regular_file_nofollow(&slices_path, "slices.json")?;
    let slices_manifest: SlicesManifest = serde_json::from_slice(&slices_bytes)
        .map_err(|err| CoreError::ReferencePackFailed(format!("parsing slices.json: {err}")))?;
    if slices_manifest.slices.is_empty() {
        return Err(CoreError::ReferencePackFailed(
            "slices.json contains zero slices".to_string(),
        ));
    }
    for slice in &slices_manifest.slices {
        let rel_slice = validate_strict_relative_path(&slice.file, "slice file")
            .map_err(CoreError::ReferencePackFailed)?;
        let slice_path =
            verify_contained_path_no_symlinks(&pack_dir, &rel_slice, false, &slice.file)?;
        let slice_bytes = read_regular_file_nofollow(&slice_path, &slice.file)?;
        let _ = decode_png_checked(&slice_path, &slice_bytes)?;
        let actual_sha = sha256_hex(&slice_bytes);
        if actual_sha != slice.sha256 {
            return Err(CoreError::ReferencePackFailed(format!(
                "slice '{}' SHA-256 mismatch (manifest {}, disk {})",
                slice.id, slice.sha256, actual_sha
            )));
        }
    }
    checks.push(format!(
        "slices OK ({} verified)",
        slices_manifest.slices.len()
    ));

    // 3. Optional/required VisionCritic check on the primary sheet.
    let should_run_critic = request.run_critic || !request.traits.is_empty();
    let critic_report = if should_run_critic {
        let vc = critic.ok_or_else(|| {
            CoreError::InvalidGenerationRequest(
                "VisionCritic is required when run_critic=true or traits are supplied".to_string(),
            )
        })?;
        let target_image = primary_sheet_path.ok_or_else(|| {
            CoreError::ReferencePackFailed("ReferencePack missing primary sheet".to_string())
        })?;
        let traits = if request.traits.is_empty() {
            vec![
                TraitCheckInput {
                    trait_id: "zero-leaked-text".to_string(),
                    category: TraitCategory::ZeroLeakedText,
                    criterion: "No stray text, labels, watermarks, or captions appear on the sheet"
                        .to_string(),
                },
                TraitCheckInput {
                    trait_id: "style-adherence".to_string(),
                    category: TraitCategory::StyleAdherence,
                    criterion: format!(
                        "Sheet adheres consistently to style '{}'",
                        request.style_slug
                    ),
                },
            ]
        } else {
            request.traits.clone()
        };
        let report = vc
            .evaluate(&VisionCriticRequest {
                target_image,
                reference_images: Vec::new(),
                traits,
            })
            .await?;
        if !report.passed {
            return Err(CoreError::StructuredOutputInvalid {
                stage: "VisionCritic",
                reason: format!(
                    "ReferencePack '{pack_id}' failed VisionCritic: {}",
                    report.summary
                ),
            });
        }
        checks.push("VisionCritic OK".to_string());
        Some(report)
    } else {
        None
    };

    // 4. Advance Lifecycle strictly: draft -> candidate -> validated.
    if manifest.lifecycle == Lifecycle::Draft {
        manifest.lifecycle = manifest.lifecycle.transition_to(Lifecycle::Candidate)?;
    }
    manifest.lifecycle = manifest.lifecycle.transition_to(Lifecycle::Validated)?;

    let updated_bytes = serde_json::to_vec_pretty(&manifest)
        .map_err(|err| CoreError::ReferencePackFailed(err.to_string()))?;
    write_atomic(&pack_path, &updated_bytes, "pack.json")?;

    Ok(ValidateReferencePackResult {
        manifest,
        checks,
        critic_report,
    })
}

/// Promote a `Validated` `Run` to `Canonical` in `library/manifest.lock.json`,
/// superseding any prior `Canonical` version.
pub fn promote_run(
    lock_store: &ManifestLockStore,
    run_store: &RunStore,
    slug: &str,
    version: u32,
    workspace_root: &Path,
) -> Result<(RunMetadata, ManifestLock), CoreError> {
    lock_store.promote_run_canonical(run_store, slug, version, workspace_root)
}

/// Promote a `Validated` style-scoped `ReferencePack` to `Canonical` in `library/manifest.lock.json`,
/// superseding any prior `Canonical` version.
pub fn promote_reference_pack(
    lock_store: &ManifestLockStore,
    library_dir: &Path,
    subject_slug: &str,
    style_slug: &str,
    workspace_root: &Path,
) -> Result<(ReferencePackManifest, ManifestLock), CoreError> {
    validate_slug(subject_slug, "Subject").map_err(CoreError::SubjectInvalid)?;
    validate_slug(style_slug, "Style").map_err(CoreError::StyleInvalid)?;
    let rel_pack = Path::new("subjects")
        .join(subject_slug)
        .join("packs")
        .join(style_slug);
    let pack_dir =
        verify_contained_path_no_symlinks(library_dir, &rel_pack, true, "ReferencePack directory")?;
    lock_store.promote_reference_pack_canonical(&pack_dir, workspace_root)
}

/// Inspect `library/manifest.lock.json` and recorded `Run` metadata across all streams (or one `slug`).
pub fn inspect_workspace_status(
    lock_store: &ManifestLockStore,
    run_store: &RunStore,
    filter_slug: Option<&str>,
) -> Result<WorkspaceLifecycleStatus, CoreError> {
    let lock = lock_store.load_or_init()?;
    let mut runs = BTreeMap::new();

    if let Some(slug) = filter_slug {
        let list = run_store.list_runs(slug)?;
        if !list.is_empty() {
            runs.insert(slug.to_string(), list);
        }
    } else if run_store.runs_dir().exists() {
        let entries = fs::read_dir(run_store.runs_dir()).map_err(|err| CoreError::Io {
            context: format!("listing runs directory {}", run_store.runs_dir().display()),
            source: err,
        })?;
        let mut slugs = Vec::new();
        for entry_res in entries {
            let entry = entry_res.map_err(|err| CoreError::Io {
                context: format!("reading runs directory {}", run_store.runs_dir().display()),
                source: err,
            })?;
            let meta = fs::symlink_metadata(entry.path()).map_err(|err| CoreError::Io {
                context: format!("inspecting run entry {}", entry.path().display()),
                source: err,
            })?;
            if meta.file_type().is_symlink() || !meta.is_dir() {
                continue;
            }
            if let Some(name) = entry.file_name().to_str()
                && validate_slug(name, "Run").is_ok()
            {
                slugs.push(name.to_string());
            }
        }
        slugs.sort();
        for slug in slugs {
            let list = run_store.list_runs(&slug)?;
            if !list.is_empty() {
                runs.insert(slug, list);
            }
        }
    }

    Ok(WorkspaceLifecycleStatus {
        canonical_count: lock.canonical.len(),
        superseded_count: lock.superseded.len(),
        canonical: lock.canonical,
        runs,
    })
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    use async_trait::async_trait;
    use image::{DynamicImage, ImageBuffer, ImageFormat, Rgba, RgbaImage};
    use rstest::rstest;
    use tempfile::TempDir;

    use super::*;
    use crate::backend::{ImageBackend, ImageGenerationOutput, ImageGenerationRequest};
    use crate::calibration::STUDIO_BG_RGBA;
    use crate::reference_pack::{ReferencePackPipeline, ReferencePackRequest};
    use crate::scene_spec::Visibility;
    use crate::store::{RecordRunParams, RunBackendMetadata, RunQaReports};

    #[derive(Clone)]
    struct MockPackBackend;

    #[async_trait]
    impl ImageBackend for MockPackBackend {
        fn name(&self) -> &'static str {
            "mock-pack-backend"
        }

        async fn generate(
            &self,
            request: &ImageGenerationRequest,
        ) -> Result<ImageGenerationOutput, CoreError> {
            let mut img: RgbaImage = ImageBuffer::from_pixel(
                request.width,
                request.height,
                Rgba([STUDIO_BG_RGBA[0], STUDIO_BG_RGBA[1], STUDIO_BG_RGBA[2], 255]),
            );
            for (start_x, color) in [(180u32, [210u8, 160, 120]), (820u32, [130u8, 170, 210])] {
                for y in 120..760 {
                    for x in start_x..(start_x + 260) {
                        img.put_pixel(x, y, Rgba([color[0], color[1], color[2], 255]));
                    }
                }
            }
            let mut buf = Vec::new();
            DynamicImage::ImageRgba8(img)
                .write_to(&mut Cursor::new(&mut buf), ImageFormat::Png)
                .unwrap();
            Ok(ImageGenerationOutput {
                png_bytes: buf,
                width: request.width,
                height: request.height,
                raw_width: request.width,
                raw_height: request.height,
                thread_id: Some("thr_mock".to_string()),
            })
        }
    }

    fn make_png(w: u32, h: u32, fill: [u8; 4]) -> Vec<u8> {
        let img: RgbaImage = ImageBuffer::from_pixel(w, h, Rgba(fill));
        let mut buf = Vec::new();
        DynamicImage::ImageRgba8(img)
            .write_to(&mut Cursor::new(&mut buf), ImageFormat::Png)
            .unwrap();
        buf
    }

    fn write_stub_critic_codex(dir: &Path, passed: bool) -> (PathBuf, PathBuf) {
        let auth_dir = dir.join("auth");
        fs::create_dir_all(&auth_dir).unwrap();
        fs::set_permissions(&auth_dir, fs::Permissions::from_mode(0o700)).unwrap();
        let auth_file = auth_dir.join("auth.json");
        fs::write(&auth_file, r#"{"token":"ok"}"#).unwrap();
        fs::set_permissions(&auth_file, fs::Permissions::from_mode(0o600)).unwrap();

        let stub = dir.join("codex-critic");
        let obs = if passed { "clean" } else { "watermark found" };
        let summary = if passed { "ok" } else { "failed" };
        let script = format!(
            r##"#!/usr/bin/env bash
set -euo pipefail
OUT=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    -o) OUT="$2"; shift 2 ;;
    *) shift ;;
  esac
done
cat <<'JSON' > "$OUT"
{{"schema_version":1,"passed":{passed},"checks":[{{"trait_id":"zero-leaked-text","category":"zero_leaked_text","passed":{passed},"observation":"{obs}"}}],"summary":"{summary}"}}
JSON
"##
        );
        fs::write(&stub, script).unwrap();
        fs::set_permissions(&stub, fs::Permissions::from_mode(0o755)).unwrap();
        (auth_dir, stub)
    }

    #[rstest]
    #[case::draft_run_passes_critic_to_validated(false, true, true)]
    #[case::candidate_run_passes_critic_to_validated(true, true, true)]
    #[case::draft_run_fails_critic_stays_draft(false, false, false)]
    #[tokio::test]
    async fn validate_run_transitions_to_validated_and_gates_on_critic(
        #[case] start_as_candidate: bool,
        #[case] critic_passes: bool,
        #[case] expect_ok: bool,
    ) {
        let root = TempDir::new().unwrap();
        let ws = root.path();
        let library_dir = ws.join("library");
        fs::create_dir_all(&library_dir).unwrap();
        let registry = LibraryRegistry::new(Some(library_dir.clone()), None);
        let run_store = RunStore::new(ws.join("runs"));
        let lock_store = ManifestLockStore::new(&library_dir);

        let rec = run_store
            .record_run(RecordRunParams {
                slug: "harbor".to_string(),
                visibility: Visibility::Public,
                png_bytes: make_png(64, 64, [150, 155, 160, 255]),
                input_hashes: BTreeMap::new(),
                compiled_prompt: "harbor pier".to_string(),
                backend: RunBackendMetadata {
                    name: "stub".to_string(),
                    width: 64,
                    height: 64,
                    raw_width: 64,
                    raw_height: 64,
                    thread_id: None,
                },
                seed: None,
                qa_reports: RunQaReports::default(),
            })
            .unwrap();
        if start_as_candidate {
            run_store
                .transition_run_lifecycle("harbor", rec.metadata.version, Lifecycle::Candidate)
                .unwrap();
        }

        let (auth_dir, stub_bin) = write_stub_critic_codex(ws, critic_passes);
        let critic =
            VisionCritic::with_codex_bin(auth_dir, stub_bin.to_string_lossy().into_owned());

        let mut req = ValidateRunRequest::new("harbor", 1);
        req.atmosphere_slug = Some("golden-dawn".to_string());
        req.run_critic = true;

        let res = validate_run(&run_store, &registry, Some(&critic), &req).await;
        assert_eq!(res.is_ok(), expect_ok);

        if expect_ok {
            let ok = res.unwrap();
            assert_eq!(ok.metadata.lifecycle, Lifecycle::Validated);
            assert!(ok.critic_report.is_some());

            let (promoted, lock) = promote_run(&lock_store, &run_store, "harbor", 1, ws).unwrap();
            assert_eq!(promoted.lifecycle, Lifecycle::Canonical);
            assert_eq!(lock.canonical["run:harbor"].version, 1);
            lock_store.verify_canonical_checksums(ws).unwrap();
        } else {
            let loaded = run_store.load_run("harbor", 1).unwrap();
            assert_eq!(loaded.lifecycle, Lifecycle::Draft);
        }
    }

    #[tokio::test]
    async fn validate_and_promote_reference_pack_locks_and_supersedes_prior_canonical() {
        let root = TempDir::new().unwrap();
        let ws = root.path();
        let library_dir = ws.join("library");
        let subj_dir = library_dir.join("subjects/brass-lantern");
        fs::create_dir_all(&subj_dir).unwrap();
        fs::write(
            subj_dir.join("card.yaml"),
            r##"schema_version: 1
kind: prop
slug: brass-lantern
display_name: Brass Lantern
exclusive_surface_map:
  bindings:
    - slot: body
      material: brass
      color_hex: "#D4A843"
palette_lock:
  swatches:
    - name: brass
      color_hex: "#D4A843"
      surface_slot: body
visibility: public
"##,
        )
        .unwrap();

        let registry = LibraryRegistry::new(Some(library_dir.clone()), None);
        let pipeline =
            ReferencePackPipeline::new(&MockPackBackend, &registry, None, library_dir.clone());
        let lock_store = ManifestLockStore::new(&library_dir);

        // v1: generate -> validate -> promote canonical
        pipeline
            .generate(&ReferencePackRequest::new(
                "brass-lantern",
                "franco-belgian-line",
            ))
            .await
            .unwrap();
        let val1 = validate_reference_pack(
            &library_dir,
            &registry,
            None,
            &ValidateReferencePackRequest::new("brass-lantern", "franco-belgian-line"),
        )
        .await
        .expect("v1 ReferencePack validation succeeds");
        assert_eq!(val1.manifest.lifecycle, Lifecycle::Validated);
        assert_eq!(val1.manifest.version, 1);

        let (promo1, lock1) = promote_reference_pack(
            &lock_store,
            &library_dir,
            "brass-lantern",
            "franco-belgian-line",
            ws,
        )
        .expect("v1 ReferencePack canonical promotion succeeds");
        assert_eq!(promo1.lifecycle, Lifecycle::Canonical);
        assert_eq!(
            lock1.canonical["pack:brass-lantern/franco-belgian-line"].version,
            1
        );
        lock_store.verify_canonical_checksums(ws).unwrap();

        // Simulate a newer validated v2 pack and promote -> v1 must become Superseded in lockfile.
        let pack_path =
            library_dir.join("subjects/brass-lantern/packs/franco-belgian-line/pack.json");
        let mut v2_manifest: ReferencePackManifest =
            serde_json::from_slice(&fs::read(&pack_path).unwrap()).unwrap();
        v2_manifest.version = 2;
        v2_manifest.lifecycle = Lifecycle::Draft;
        write_atomic(
            &pack_path,
            &serde_json::to_vec_pretty(&v2_manifest).unwrap(),
            "pack.json",
        )
        .unwrap();

        let val2 = validate_reference_pack(
            &library_dir,
            &registry,
            None,
            &ValidateReferencePackRequest::new("brass-lantern", "franco-belgian-line"),
        )
        .await
        .expect("v2 ReferencePack validation succeeds");
        assert_eq!(val2.manifest.lifecycle, Lifecycle::Validated);

        let (promo2, lock2) = promote_reference_pack(
            &lock_store,
            &library_dir,
            "brass-lantern",
            "franco-belgian-line",
            ws,
        )
        .expect("v2 ReferencePack canonical promotion succeeds");
        assert_eq!(promo2.lifecycle, Lifecycle::Canonical);
        assert_eq!(
            lock2.canonical["pack:brass-lantern/franco-belgian-line"].version,
            2
        );
        assert_eq!(lock2.superseded.len(), 1);
        assert_eq!(lock2.superseded[0].version, 1);
        assert_eq!(lock2.superseded[0].lifecycle, Lifecycle::Superseded);
        lock_store.verify_canonical_checksums(ws).unwrap();
    }

    #[rstest]
    #[case::symlinked_ref_rejected("symlink")]
    #[case::invalid_png_ref_rejected("invalid_png")]
    #[case::internal_run_ref_rejected("internal_run")]
    #[case::internal_pack_ref_rejected("internal_pack")]
    #[tokio::test]
    async fn validate_run_rejects_unsafe_invalid_or_leaking_reference_images(
        #[case] case_kind: &str,
    ) {
        let root = TempDir::new().unwrap();
        let ws = root.path();
        let library_dir = ws.join("library");
        fs::create_dir_all(&library_dir).unwrap();
        let run_store = RunStore::new(ws.join("runs"));

        let pub_run = run_store
            .record_run(RecordRunParams {
                slug: "harbor-pub".to_string(),
                visibility: Visibility::Public,
                png_bytes: make_png(64, 64, [150, 155, 160, 255]),
                input_hashes: BTreeMap::new(),
                compiled_prompt: "harbor pier".to_string(),
                backend: RunBackendMetadata {
                    name: "stub".to_string(),
                    width: 64,
                    height: 64,
                    raw_width: 64,
                    raw_height: 64,
                    thread_id: None,
                },
                seed: None,
                qa_reports: RunQaReports::default(),
            })
            .unwrap();

        let ref_path = match case_kind {
            "symlink" => {
                #[cfg(unix)]
                {
                    let real = ws.join("real.png");
                    fs::write(&real, make_png(32, 32, [100, 110, 120, 255])).unwrap();
                    let link = ws.join("ref_link.png");
                    std::os::unix::fs::symlink(&real, &link).unwrap();
                    link
                }
                #[cfg(not(unix))]
                {
                    ws.join("missing.png")
                }
            }
            "invalid_png" => {
                let bad = ws.join("not_a_png.png");
                fs::write(&bad, b"NOT_PNG_DATA").unwrap();
                bad
            }
            "internal_run" => {
                let secret = run_store
                    .record_run(RecordRunParams {
                        slug: "secret-run".to_string(),
                        visibility: Visibility::Internal,
                        png_bytes: make_png(32, 32, [100, 110, 120, 255]),
                        input_hashes: BTreeMap::new(),
                        compiled_prompt: "secret".to_string(),
                        backend: RunBackendMetadata {
                            name: "stub".to_string(),
                            width: 32,
                            height: 32,
                            raw_width: 32,
                            raw_height: 32,
                            thread_id: None,
                        },
                        seed: None,
                        qa_reports: RunQaReports::default(),
                    })
                    .unwrap();
                secret.image_path
            }
            "internal_pack" => {
                let slices_dir =
                    library_dir.join("subjects/secret-envoy/packs/franco-belgian-line/slices");
                fs::create_dir_all(&slices_dir).unwrap();
                fs::write(
                    library_dir.join("subjects/secret-envoy/card.yaml"),
                    "schema_version: 1\nkind: prop\nslug: secret-envoy\ndisplay_name: Envoy\nexclusive_surface_map:\n  bindings:\n    - slot: body\n      material: wood\n      color_hex: \"#2C3E50\"\npalette_lock:\n  swatches:\n    - name: body\n      color_hex: \"#2C3E50\"\nvisibility: internal\n",
                )
                .unwrap();
                let p = slices_dir.join("sheet_001.png");
                fs::write(&p, make_png(32, 32, [100, 110, 120, 255])).unwrap();
                p
            }
            _ => unreachable!(),
        };

        let registry = LibraryRegistry::new(Some(library_dir), None);
        let (auth_dir, stub_bin) = write_stub_critic_codex(ws, true);
        let critic =
            VisionCritic::with_codex_bin(auth_dir, stub_bin.to_string_lossy().into_owned());

        let mut req = ValidateRunRequest::new("harbor-pub", pub_run.metadata.version);
        req.reference_images = vec![ref_path];
        req.run_critic = true;

        let err = validate_run(&run_store, &registry, Some(&critic), &req)
            .await
            .expect_err("unsafe or leaking reference image must fail closed");
        match case_kind {
            "symlink" => assert!(matches!(err, CoreError::Io { .. })),
            "invalid_png" => assert!(matches!(err, CoreError::InvalidPng { .. })),
            "internal_run" | "internal_pack" => {
                assert!(matches!(err, CoreError::VisibilityViolation { .. }))
            }
            _ => unreachable!(),
        }
    }

    #[rstest]
    #[case::bright_yellow_swatch_and_strip_artifacts_masked_out(false, true)]
    #[case::excessive_upper_sheet_sparkles_fail_closed(true, false)]
    #[tokio::test]
    async fn validate_reference_pack_masks_calibration_strip_from_sparkle_qa(
        #[case] add_upper_sparkles: bool,
        #[case] expect_ok: bool,
    ) {
        let root = TempDir::new().unwrap();
        let ws = root.path();
        let library_dir = ws.join("library");
        let subj_dir = library_dir.join("subjects/sun-herald");
        fs::create_dir_all(&subj_dir).unwrap();
        fs::write(
            subj_dir.join("card.yaml"),
            r##"schema_version: 1
kind: prop
slug: sun-herald
display_name: Sun Herald
exclusive_surface_map:
  bindings:
    - slot: emblem
      material: enamel
      color_hex: "#FFF014"
palette_lock:
  swatches:
    - name: bright-yellow
      color_hex: "#FFF014"
      surface_slot: emblem
visibility: public
"##,
        )
        .unwrap();

        let registry = LibraryRegistry::new(Some(library_dir.clone()), None);
        let pipeline =
            ReferencePackPipeline::new(&MockPackBackend, &registry, None, library_dir.clone());
        pipeline
            .generate(&ReferencePackRequest::new(
                "sun-herald",
                "franco-belgian-line",
            ))
            .await
            .unwrap();

        let pack_dir = library_dir.join("subjects/sun-herald/packs/franco-belgian-line");
        let pack_path = pack_dir.join("pack.json");
        let mut manifest: ReferencePackManifest =
            serde_json::from_slice(&fs::read(&pack_path).unwrap()).unwrap();

        let first_sheet = &mut manifest.sheets[0];
        let sheet_path = pack_dir.join(&first_sheet.filename);
        let mut rgba = image::open(&sheet_path).unwrap().to_rgba8();
        let strip_y = first_sheet.calibration_strip.strip_y;
        for dy in 0..3 {
            for dx in 0..3 {
                rgba.put_pixel(1400 + dx, strip_y + 12 + dy, Rgba([255, 240, 20, 255]));
            }
        }
        if add_upper_sparkles {
            for i in 0..270u32 {
                let bx = 20 + (i % 45) * 32;
                let by = 20 + (i / 45) * 14;
                for dy in 0..2 {
                    for dx in 0..2 {
                        rgba.put_pixel(bx + dx, by + dy, Rgba([255, 240, 20, 255]));
                    }
                }
            }
        }
        let mut updated_png = Vec::new();
        DynamicImage::ImageRgba8(rgba)
            .write_to(&mut Cursor::new(&mut updated_png), ImageFormat::Png)
            .unwrap();
        write_atomic(&sheet_path, &updated_png, &first_sheet.filename).unwrap();
        first_sheet.sha256 = sha256_hex(&updated_png);
        write_atomic(
            &pack_path,
            &serde_json::to_vec_pretty(&manifest).unwrap(),
            "pack.json",
        )
        .unwrap();

        let res = validate_reference_pack(
            &library_dir,
            &registry,
            None,
            &ValidateReferencePackRequest::new("sun-herald", "franco-belgian-line"),
        )
        .await;
        assert_eq!(res.is_ok(), expect_ok, "result: {:?}", res);
        if expect_ok {
            let val = res.unwrap();
            assert_eq!(val.manifest.lifecycle, Lifecycle::Validated);
            assert!(
                val.checks.iter().any(|c| c.contains("0 sparkles")),
                "expected 0 sparkles check in {:?}",
                val.checks
            );
        } else {
            let err = res.unwrap_err();
            assert!(
                matches!(err, CoreError::ColorQaFailed(_)),
                "expected ColorQaFailed, got {err:?}"
            );
        }
    }
}
