//! === UNIFIED EDITING PIPELINES (WHOLE-IMAGE EDIT & PATCH EDIT) ===
//!
//! PURPOSE:
//! Implements `WholeImageEdit` (global/multi-region edit with fail-closed
//! `ColorQaEngine` verification) and `PatchEdit` (unified ROI/mask 1:1 square
//! crop extraction, optional target prep, collar-ring OKLab Reinhard color/tone
//! harmonization clamped to `[0.75, 1.35]`, 5-level Laplacian multi-band pyramid
//! blending with cosine distance-transform feathering, zero outside-footprint
//! composite, and out-of-ROI QA verification).
//!
//! THREAT MODEL / DESIGN RATIONALE ("WHY"):
//! 1. Generative edits often shift exposure, bleed colors, or alter untouched
//!    background pixels outside the requested ROI.
//! 2. `WholeImageEdit` never mutates or re-grades outputs; it gates strictly via
//!    `ColorQaEngine` (ADR 0005).
//! 3. `PatchEdit` isolates generation to a `1:1` square crop with context collar,
//!    harmonizes tone/color using trimmed statistics exclusively from the
//!    `16..=32 px` outer collar ring (gain clamped strictly to `[0.75, 1.35]` to
//!    prevent wild contrast blow-ups), and blends via a 5-level Laplacian
//!    pyramid so seams vanish while pixels outside the edit footprint remain
//!    bit-for-bit untouched.
//!
//! EXECUTION LIFECYCLE / DATA FLOW:
//! 1. `execute_whole_image_edit`: validates inputs, invokes `ImageBackend`,
//!    evaluates `ColorQaEngine`, records a `Run`, and fails closed if QA fails.
//! 2. `execute_patch_edit`: unifies `PixelRect` ROI or alpha/grayscale mask into
//!    a normalized mask, extracts a `1:1` crop + collar, prepares target cue,
//!    invokes `ImageBackend` at `WxW`, applies collar Reinhard transfer
//!    (`[0.75, 1.35]`), blends via 5-level Laplacian pyramid + cosine DT mask,
//!    composites back with zero out-of-footprint mutation, verifies `1 - M`
//!    drift, and records a `Run`.

use std::collections::BTreeMap;
use std::f32::consts::PI;
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};

use image::imageops::crop_imm;
use image::{DynamicImage, ImageFormat, Rgba, RgbaImage};
use palette::{Clamp, IntoColor, LinSrgb, Oklab, Srgb};
use serde::{Deserialize, Serialize};
use tempfile::TempDir;

use crate::atmosphere::ExposureFloor;
use crate::backend::{ImageBackend, ImageGenerationRequest, PNG_SIGNATURE};
use crate::calibration::STUDIO_BG_RGBA;
use crate::color_qa::{
    ColorQaEngine, ColorQaReport, ExposureQaConfig, PaletteQaConfig, SparkleConfig, SubjectRoiSpec,
    UntouchedDriftConfig, UntouchedDriftReport,
};
use crate::error::CoreError;
use crate::lifecycle::sha256_hex;
use crate::scene_spec::Visibility;
use crate::slicer::PixelRect;
use crate::store::{
    RecordRunParams, RecordedRun, RunBackendMetadata, RunQaReports, RunStore,
    read_regular_file_nofollow,
};
use crate::subject::PaletteLock;

/// Minimum allowed outer collar ring width in pixels for `PatchEdit` Reinhard calibration.
pub const MIN_COLLAR_WIDTH_PX: u32 = 16;

/// Maximum allowed outer collar ring width in pixels for `PatchEdit` Reinhard calibration.
pub const MAX_COLLAR_WIDTH_PX: u32 = 32;

/// Default outer collar ring width (`24 px`, within `16..=32 px`).
pub const DEFAULT_COLLAR_WIDTH_PX: u32 = 24;

/// Strict lower clamp for per-channel OKLab Reinhard gain.
pub const REINHARD_GAIN_MIN: f32 = 0.75;

/// Strict upper clamp for per-channel OKLab Reinhard gain.
pub const REINHARD_GAIN_MAX: f32 = 1.35;

/// Canonical multi-band Laplacian pyramid level count (`5` levels: `L0..L3` + `G4`).
pub const DEFAULT_PYRAMID_LEVELS: usize = 5;

// === 1. WholeImageEdit Contracts & Execution ===

/// Owned `Subject` ROI + `PaletteLock` pairing for `WholeImageEdit` Color QA.
#[derive(Debug, Clone)]
pub struct OwnedSubjectRoi {
    /// `Subject` slug for reporting.
    pub subject_slug: String,
    /// Bound `PaletteLock` swatches.
    pub palette_lock: PaletteLock,
    /// Target ROIs inside the canvas (empty evaluates the full image).
    pub rois: Vec<PixelRect>,
}

/// Request parameters for `execute_whole_image_edit`.
#[derive(Debug, Clone)]
pub struct WholeImageEditRequest {
    /// Target `Run` stream slug.
    pub slug: String,
    /// Access classification for the recorded `Run`.
    pub visibility: Visibility,
    /// Existing target PNG image path to edit.
    pub target_image_path: PathBuf,
    /// Positive-only compiled edit prompt.
    pub compiled_prompt: String,
    /// Additional reference anchor PNG paths forwarded to `ImageBackend`.
    pub reference_images: Vec<PathBuf>,
    /// Optional `Atmosphere.exposure_floor` and QA tolerances.
    pub exposure_floor: Option<(ExposureFloor, ExposureQaConfig)>,
    /// Optional per-subject `PaletteLock` ROI checks.
    pub subject_rois: Vec<OwnedSubjectRoi>,
    /// `PaletteLock` QA configuration.
    pub palette_config: PaletteQaConfig,
    /// Optional edit mask (`0.0..=1.0`, `W * H`) for untouched-region (`1 - M`) drift QA.
    pub edit_mask: Option<Vec<f32>>,
    /// Untouched-region drift thresholds.
    pub untouched_drift_config: UntouchedDriftConfig,
    /// Optional allowed-mask for yellow-sparkle DoG detector (`true` where yellow dots allowed).
    pub allowed_sparkle_mask: Option<Vec<f32>>,
    /// Sparkle detector configuration.
    pub sparkle_config: SparkleConfig,
    /// Fail closed (`Err(CoreError::ColorQaFailed)`) when any Color QA gate fails.
    pub fail_closed_on_qa: bool,
}

/// Result returned by `execute_whole_image_edit`.
#[derive(Debug, Clone)]
pub struct WholeImageEditResult {
    /// Recorded `Run` persisted in `RunStore`.
    pub run: RecordedRun,
    /// Fail-closed `ColorQaReport` (never mutates whole images).
    pub qa_report: ColorQaReport,
    /// Final PNG bytes returned by `ImageBackend`.
    pub png_bytes: Vec<u8>,
}

/// Execute `WholeImageEdit`: pass target image + reference anchors to `ImageBackend`,
/// run fail-closed `ColorQaEngine` checks without re-grading, and record a `Run`.
pub async fn execute_whole_image_edit(
    backend: &dyn ImageBackend,
    run_store: &RunStore,
    request: &WholeImageEditRequest,
) -> Result<WholeImageEditResult, CoreError> {
    if request.compiled_prompt.trim().is_empty() {
        return Err(CoreError::EditFailed(
            "WholeImageEdit compiled_prompt must not be empty".to_string(),
        ));
    }

    let target_bytes = read_regular_file_nofollow(&request.target_image_path, "target PNG")?;
    let target_img = decode_png_checked(&request.target_image_path, &target_bytes)?.to_rgba8();
    let width = target_img.width();
    let height = target_img.height();

    let mut all_refs = Vec::with_capacity(1 + request.reference_images.len());
    all_refs.push(request.target_image_path.clone());
    all_refs.extend(request.reference_images.iter().cloned());

    let mut input_hashes = BTreeMap::new();
    input_hashes.insert("target_image".to_string(), sha256_hex(&target_bytes));
    for (idx, ref_path) in request.reference_images.iter().enumerate() {
        let bytes = read_regular_file_nofollow(ref_path, "reference PNG")?;
        input_hashes.insert(format!("ref_{idx}"), sha256_hex(&bytes));
    }

    let output = backend
        .generate(&ImageGenerationRequest {
            prompt: request.compiled_prompt.clone(),
            width,
            height,
            reference_images: all_refs,
        })
        .await?;

    let edited_img =
        decode_png_checked(Path::new("whole_edit_output.png"), &output.png_bytes)?.to_rgba8();

    let qa_engine = ColorQaEngine::new();
    let subject_specs: Vec<SubjectRoiSpec<'_>> = request
        .subject_rois
        .iter()
        .map(|s| SubjectRoiSpec {
            subject_slug: &s.subject_slug,
            palette_lock: &s.palette_lock,
            rois: s.rois.clone(),
        })
        .collect();

    let exposure_arg = request
        .exposure_floor
        .as_ref()
        .map(|(floor, cfg)| (*floor, cfg));
    let untouched_arg = request
        .edit_mask
        .as_deref()
        .map(|mask| (&target_img, mask, &request.untouched_drift_config));
    let sparkle_arg = Some((
        request.allowed_sparkle_mask.as_deref(),
        &request.sparkle_config,
    ));

    let qa_report = qa_engine.evaluate_whole_image(
        &edited_img,
        exposure_arg,
        &subject_specs,
        &request.palette_config,
        untouched_arg,
        sparkle_arg,
    )?;

    let run = run_store.record_run(RecordRunParams {
        slug: request.slug.clone(),
        visibility: request.visibility,
        png_bytes: output.png_bytes.clone(),
        input_hashes,
        compiled_prompt: request.compiled_prompt.clone(),
        backend: RunBackendMetadata {
            name: backend.name().to_string(),
            width: output.width,
            height: output.height,
            raw_width: output.raw_width,
            raw_height: output.raw_height,
            thread_id: output.thread_id,
        },
        seed: None,
        qa_reports: RunQaReports {
            structural_checks: qa_report.notes.clone(),
            critic_report: None,
            color_qa_max_delta_e2000: Some(qa_report.max_delta_e2000),
        },
    })?;

    if request.fail_closed_on_qa && !qa_report.passed {
        return Err(CoreError::ColorQaFailed(format!(
            "WholeImageEdit '{}' failed Color QA (recorded as Draft '{}'): {}",
            request.slug,
            run.metadata.run_id,
            qa_report.notes.join("; ")
        )));
    }

    Ok(WholeImageEditResult {
        run,
        qa_report,
        png_bytes: output.png_bytes,
    })
}

// === 2. PatchEdit Contracts & Unified Masking ===

/// Unified localization input for `PatchEdit` (ROI bounding box or alpha/grayscale mask).
#[derive(Debug, Clone, PartialEq)]
pub enum PatchTargetMask {
    /// Axis-aligned rectangular ROI `(x, y, width, height)`.
    Roi(PixelRect),
    /// Square ROI `(x, y, size)`.
    SquareRoi {
        /// Left X coordinate in pixels.
        x: u32,
        /// Top Y coordinate in pixels.
        y: u32,
        /// Square side length in pixels.
        size: u32,
    },
    /// Path to a PNG mask (`alpha` channel when varied, otherwise luminance `0.0..=1.0`).
    MaskImagePath(PathBuf),
    /// Direct normalized mask buffer (`values.len() == width * height`, entries in `0.0..=1.0`).
    MaskValues {
        /// Mask width matching target canvas width.
        width: u32,
        /// Mask height matching target canvas height.
        height: u32,
        /// Normalized mask weights in `[0.0, 1.0]`.
        values: Vec<f32>,
    },
}

/// Optional pre-edit target preparation applied to the `1:1` crop before `ImageBackend`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PatchPrepMode {
    /// Send the extracted `1:1` square crop unmodified.
    #[default]
    CropOnly,
    /// Blank pixels inside the active mask (`mask > 0.5`) to neutral `#252830`.
    BlankInsideMask,
    /// Draw a crisp `#F6AD55` / `#1A1D24` haloed outline along the mask boundary.
    CueOutline,
}

/// Tunable configuration for `PatchEdit` collar calibration and 5-level Laplacian blending.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PatchEditConfig {
    /// Context collar width in pixels (`16..=32`, default `24`).
    pub collar_width_px: u32,
    /// Cosine distance-transform feather radius in pixels (`>= 2`, `<= collar_width_px`, default `12`).
    pub feather_px: u32,
    /// Pre-edit square crop preparation mode.
    pub prep_mode: PatchPrepMode,
    /// Multi-band Laplacian pyramid level count (`5`).
    pub pyramid_levels: usize,
    /// Thresholds verifying zero drift outside the crop/collar footprint (`1 - M`).
    pub untouched_drift_config: UntouchedDriftConfig,
}

impl Default for PatchEditConfig {
    fn default() -> Self {
        Self {
            collar_width_px: DEFAULT_COLLAR_WIDTH_PX,
            feather_px: 12,
            prep_mode: PatchPrepMode::CropOnly,
            pyramid_levels: DEFAULT_PYRAMID_LEVELS,
            untouched_drift_config: UntouchedDriftConfig {
                max_mean_delta_e2000: 0.5,
                max_p95_delta_e2000: 1.0,
                max_pixel_delta_e2000: 2.0,
                max_fraction_exceeding: 0.0,
                untouched_mask_epsilon: 1e-4,
            },
        }
    }
}

impl PatchEditConfig {
    /// Validate `PatchEditConfig` invariants fail-closed.
    pub fn validate(&self) -> Result<(), CoreError> {
        if !(MIN_COLLAR_WIDTH_PX..=MAX_COLLAR_WIDTH_PX).contains(&self.collar_width_px) {
            return Err(CoreError::EditFailed(format!(
                "collar_width_px {} must be within {}..={}",
                self.collar_width_px, MIN_COLLAR_WIDTH_PX, MAX_COLLAR_WIDTH_PX
            )));
        }
        if self.feather_px < 2 || self.feather_px > self.collar_width_px {
            return Err(CoreError::EditFailed(format!(
                "feather_px {} must be within 2..={}",
                self.feather_px, self.collar_width_px
            )));
        }
        if self.pyramid_levels == 0 || self.pyramid_levels > 8 {
            return Err(CoreError::EditFailed(format!(
                "pyramid_levels {} must be within 1..=8",
                self.pyramid_levels
            )));
        }
        Ok(())
    }
}

/// Recorded per-channel OKLab Reinhard gain factors applied during `PatchEdit`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ReinhardGainSummary {
    /// Clamped `[0.75, 1.35]` gain applied to OKLab `L`.
    pub gain_l: f32,
    /// Clamped `[0.75, 1.35]` gain applied to OKLab `a`.
    pub gain_a: f32,
    /// Clamped `[0.75, 1.35]` gain applied to OKLab `b`.
    pub gain_b: f32,
    /// Number of outer collar ring pixels (`16..=32 px` outside mask) used for statistics.
    pub collar_pixel_count: usize,
}

/// Request parameters for `execute_patch_edit`.
#[derive(Debug, Clone)]
pub struct PatchEditRequest {
    /// Target `Run` stream slug.
    pub slug: String,
    /// Access classification for the recorded `Run`.
    pub visibility: Visibility,
    /// Full-canvas target PNG path.
    pub target_image_path: PathBuf,
    /// Unified ROI or mask specifying the region to edit.
    pub target_mask: PatchTargetMask,
    /// Positive-only compiled prompt describing the localized edit.
    pub compiled_prompt: String,
    /// Optional extra reference anchor PNGs forwarded to `ImageBackend`.
    pub reference_images: Vec<PathBuf>,
    /// `PatchEdit` collar, prep, and pyramid blending configuration.
    pub config: PatchEditConfig,
}

/// Result returned by `execute_patch_edit`.
#[derive(Debug, Clone)]
pub struct PatchEditResult {
    /// Recorded `Run` persisted in `RunStore`.
    pub run: RecordedRun,
    /// Extracted `1:1` square crop rectangle inside the full target canvas.
    pub crop_rect: PixelRect,
    /// Tight ROI bounding box of the active target mask inside the full canvas.
    pub roi_rect: PixelRect,
    /// Clamped `[0.75, 1.35]` OKLab Reinhard gain summary from collar pixels.
    pub reinhard_gains: ReinhardGainSummary,
    /// Out-of-footprint `DeltaE_2000` drift verification report.
    pub out_of_roi_report: UntouchedDriftReport,
    /// Final composited full-canvas PNG bytes.
    pub png_bytes: Vec<u8>,
}

/// Execute `PatchEdit` on a target PNG using `ImageBackend` and record a `Run`.
pub async fn execute_patch_edit(
    backend: &dyn ImageBackend,
    run_store: &RunStore,
    request: &PatchEditRequest,
) -> Result<PatchEditResult, CoreError> {
    request.config.validate()?;
    if request.compiled_prompt.trim().is_empty() {
        return Err(CoreError::EditFailed(
            "PatchEdit compiled_prompt must not be empty".to_string(),
        ));
    }

    let target_bytes = read_regular_file_nofollow(&request.target_image_path, "target PNG")?;
    let target_img = decode_png_checked(&request.target_image_path, &target_bytes)?.to_rgba8();
    let canvas_w = target_img.width();
    let canvas_h = target_img.height();

    let (full_mask, roi_rect) = resolve_unified_mask(&request.target_mask, canvas_w, canvas_h)?;
    let crop_rect =
        compute_square_crop_rect(roi_rect, canvas_w, canvas_h, request.config.collar_width_px)?;

    let orig_crop = crop_imm(
        &target_img,
        crop_rect.x,
        crop_rect.y,
        crop_rect.width,
        crop_rect.height,
    )
    .to_image();

    let crop_w = crop_rect.width as usize;
    let crop_h = crop_rect.height as usize;
    let mut crop_mask = vec![0.0f32; crop_w * crop_h];
    for ly in 0..crop_h {
        let gy = crop_rect.y as usize + ly;
        for lx in 0..crop_w {
            let gx = crop_rect.x as usize + lx;
            crop_mask[ly * crop_w + lx] = full_mask[gy * (canvas_w as usize) + gx];
        }
    }

    let prepared_crop = prepare_patch_target(&orig_crop, &crop_mask, request.config.prep_mode);
    let temp_dir = TempDir::new().map_err(|err| CoreError::Io {
        context: "creating temp dir for PatchEdit crop".to_string(),
        source: err,
    })?;
    let crop_png_path = temp_dir.path().join("patch_target_crop.png");
    let mut prepared_bytes = Vec::new();
    DynamicImage::ImageRgba8(prepared_crop)
        .write_to(&mut Cursor::new(&mut prepared_bytes), ImageFormat::Png)
        .map_err(|err| CoreError::EditFailed(format!("encoding prepared crop PNG: {err}")))?;
    fs::write(&crop_png_path, &prepared_bytes).map_err(|err| CoreError::Io {
        context: "writing prepared crop PNG".to_string(),
        source: err,
    })?;

    let mut refs = Vec::with_capacity(1 + request.reference_images.len());
    refs.push(crop_png_path);
    refs.extend(request.reference_images.iter().cloned());

    let mut input_hashes = BTreeMap::new();
    input_hashes.insert("target_image".to_string(), sha256_hex(&target_bytes));
    input_hashes.insert("prepared_crop".to_string(), sha256_hex(&prepared_bytes));
    for (idx, ref_path) in request.reference_images.iter().enumerate() {
        let bytes = read_regular_file_nofollow(ref_path, "reference PNG")?;
        input_hashes.insert(format!("ref_{idx}"), sha256_hex(&bytes));
    }

    let backend_output = backend
        .generate(&ImageGenerationRequest {
            prompt: request.compiled_prompt.clone(),
            width: crop_rect.width,
            height: crop_rect.height,
            reference_images: refs,
        })
        .await?;

    let gen_patch =
        decode_png_checked(Path::new("patch_output.png"), &backend_output.png_bytes)?.to_rgba8();

    let (composited, reinhard_gains, out_of_roi_report) = composite_calibrated_patch(
        &target_img,
        &gen_patch,
        crop_rect,
        &crop_mask,
        &request.config,
    )?;

    let mut out_png_bytes = Vec::new();
    DynamicImage::ImageRgba8(composited)
        .write_to(&mut Cursor::new(&mut out_png_bytes), ImageFormat::Png)
        .map_err(|err| CoreError::EditFailed(format!("encoding PatchEdit output PNG: {err}")))?;

    let run = run_store.record_run(RecordRunParams {
        slug: request.slug.clone(),
        visibility: request.visibility,
        png_bytes: out_png_bytes.clone(),
        input_hashes,
        compiled_prompt: request.compiled_prompt.clone(),
        backend: RunBackendMetadata {
            name: backend.name().to_string(),
            width: canvas_w,
            height: canvas_h,
            raw_width: backend_output.raw_width,
            raw_height: backend_output.raw_height,
            thread_id: backend_output.thread_id,
        },
        seed: None,
        qa_reports: RunQaReports {
            structural_checks: vec![
                format!(
                    "PatchEdit crop={}x{}+{}+{} reinhard_gains=({:.2},{:.2},{:.2})",
                    crop_rect.width,
                    crop_rect.height,
                    crop_rect.x,
                    crop_rect.y,
                    reinhard_gains.gain_l,
                    reinhard_gains.gain_a,
                    reinhard_gains.gain_b
                ),
                format!(
                    "out_of_roi max_delta_e2000={:.4} (untouched_px={})",
                    out_of_roi_report.max_delta_e2000, out_of_roi_report.untouched_pixel_count
                ),
            ],
            critic_report: None,
            color_qa_max_delta_e2000: Some(out_of_roi_report.max_delta_e2000),
        },
    })?;

    Ok(PatchEditResult {
        run,
        crop_rect,
        roi_rect,
        reinhard_gains,
        out_of_roi_report,
        png_bytes: out_png_bytes,
    })
}

/// Composite a generated `WxW` patch back into `target_img` via collar-ring OKLab Reinhard
/// transfer (`[0.75, 1.35]`), cosine distance-transform feathering, and 5-level Laplacian
/// multi-band pyramid blending, then verify zero outside-footprint `DeltaE_2000` drift.
pub fn composite_calibrated_patch(
    target_img: &RgbaImage,
    gen_patch: &RgbaImage,
    crop_rect: PixelRect,
    crop_mask: &[f32],
    config: &PatchEditConfig,
) -> Result<(RgbaImage, ReinhardGainSummary, UntouchedDriftReport), CoreError> {
    config.validate()?;
    let canvas_w = target_img.width();
    let canvas_h = target_img.height();
    if crop_rect.width == 0
        || crop_rect.height == 0
        || crop_rect.width != crop_rect.height
        || crop_rect.right() > canvas_w
        || crop_rect.bottom() > canvas_h
    {
        return Err(CoreError::EditFailed(format!(
            "crop_rect {:?} must be a non-empty 1:1 square within {canvas_w}x{canvas_h}",
            crop_rect
        )));
    }
    if gen_patch.width() != crop_rect.width || gen_patch.height() != crop_rect.height {
        return Err(CoreError::EditFailed(format!(
            "generated patch dimensions {}x{} do not match crop {}x{}",
            gen_patch.width(),
            gen_patch.height(),
            crop_rect.width,
            crop_rect.height
        )));
    }
    let crop_len = (crop_rect.width as usize) * (crop_rect.height as usize);
    if crop_mask.len() != crop_len {
        return Err(CoreError::EditFailed(format!(
            "crop_mask length {} does not match crop pixel count {crop_len}",
            crop_mask.len()
        )));
    }

    let orig_crop = crop_imm(
        target_img,
        crop_rect.x,
        crop_rect.y,
        crop_rect.width,
        crop_rect.height,
    )
    .to_image();

    let w = crop_rect.width as usize;
    let h = crop_rect.height as usize;

    // 1. Compute Euclidean distance outside the binary mask (mask > 0.5).
    let binary_mask: Vec<bool> = crop_mask.iter().map(|&m| m > 0.5).collect();
    let dist_from_mask = euclidean_distance_transform(&binary_mask, w, h);

    // 2. Collar-ring OKLab Reinhard transfer strictly on pixels outside mask within 1..=collar_width_px.
    let (calibrated_patch, gains) = apply_collar_reinhard_oklab(
        &orig_crop,
        gen_patch,
        crop_mask,
        &dist_from_mask,
        config.collar_width_px,
    )?;

    // 3. Cosine distance-transform feathered alpha mask strictly reaching 0.0 inside crop bounds.
    let feathered_alpha = build_cosine_feathered_alpha(
        crop_mask,
        &dist_from_mask,
        w,
        h,
        crop_rect,
        canvas_w,
        canvas_h,
        config.feather_px,
    );

    // 4. 5-level Laplacian multi-band pyramid blend in linear-light sRGB.
    let blended_crop = laplacian_pyramid_blend(
        &orig_crop,
        &calibrated_patch,
        &feathered_alpha,
        config.pyramid_levels,
    );

    // 5. Composite strictly where feathered_alpha > 0.0 so outer pixels remain bit-for-bit identical.
    let mut output = target_img.clone();
    let mut full_footprint_mask = vec![0.0f32; (canvas_w as usize) * (canvas_h as usize)];
    for ly in 0..h {
        let gy = crop_rect.y + ly as u32;
        for lx in 0..w {
            let alpha = feathered_alpha[ly * w + lx];
            if alpha > 0.0 {
                let gx = crop_rect.x + lx as u32;
                let px = blended_crop.get_pixel(lx as u32, ly as u32);
                output.put_pixel(gx, gy, *px);
                full_footprint_mask[(gy as usize) * (canvas_w as usize) + (gx as usize)] = 1.0;
            }
        }
    }

    let qa = ColorQaEngine::new();
    let (out_of_roi_report, _) = qa.evaluate_untouched_drift(
        target_img,
        &output,
        &full_footprint_mask,
        &config.untouched_drift_config,
    )?;
    if !out_of_roi_report.passed {
        return Err(CoreError::ColorQaFailed(format!(
            "PatchEdit modified pixels outside crop/collar footprint: {}",
            out_of_roi_report.reasons.join("; ")
        )));
    }

    Ok((output, gains, out_of_roi_report))
}

/// Apply trimmed-mean / trimmed-std OKLab Reinhard color/tone transfer from `orig_crop`
/// to `gen_patch` using exclusively pixels in the outer collar ring (`1..=collar_width_px`
/// outside `crop_mask`), clamping per-channel gain strictly to `[0.75, 1.35]`.
pub fn apply_collar_reinhard_oklab(
    orig_crop: &RgbaImage,
    gen_patch: &RgbaImage,
    crop_mask: &[f32],
    dist_from_mask: &[f32],
    collar_width_px: u32,
) -> Result<(RgbaImage, ReinhardGainSummary), CoreError> {
    let w = orig_crop.width() as usize;
    let h = orig_crop.height() as usize;
    let total = w * h;
    if gen_patch.width() as usize != w
        || gen_patch.height() as usize != h
        || crop_mask.len() != total
        || dist_from_mask.len() != total
    {
        return Err(CoreError::EditFailed(
            "apply_collar_reinhard_oklab input dimensions do not match".to_string(),
        ));
    }

    let max_d = collar_width_px as f32;
    let mut orig_l = Vec::new();
    let mut orig_a = Vec::new();
    let mut orig_b = Vec::new();
    let mut gen_l = Vec::new();
    let mut gen_a = Vec::new();
    let mut gen_b = Vec::new();

    for y in 0..h {
        for x in 0..w {
            let idx = y * w + x;
            let d = dist_from_mask[idx];
            if crop_mask[idx] <= 1e-3 && d >= 1.0 && d <= max_d {
                let po = orig_crop.get_pixel(x as u32, y as u32).0;
                let pg = gen_patch.get_pixel(x as u32, y as u32).0;
                if po[3] == 0 || pg[3] == 0 {
                    continue;
                }
                let lo = srgb_u8_to_oklab_f32([po[0], po[1], po[2]]);
                let lg = srgb_u8_to_oklab_f32([pg[0], pg[1], pg[2]]);
                orig_l.push(lo.l);
                orig_a.push(lo.a);
                orig_b.push(lo.b);
                gen_l.push(lg.l);
                gen_a.push(lg.a);
                gen_b.push(lg.b);
            }
        }
    }

    if orig_l.is_empty() {
        return Err(CoreError::EditFailed(
            "PatchEdit collar ring contains zero pixels outside the target mask".to_string(),
        ));
    }

    let collar_pixel_count = orig_l.len();
    let (mean_ol, std_ol) = trimmed_mean_std(&mut orig_l, 0.10);
    let (mean_oa, std_oa) = trimmed_mean_std(&mut orig_a, 0.10);
    let (mean_ob, std_ob) = trimmed_mean_std(&mut orig_b, 0.10);

    let (mean_gl, std_gl) = trimmed_mean_std(&mut gen_l, 0.10);
    let (mean_ga, std_ga) = trimmed_mean_std(&mut gen_a, 0.10);
    let (mean_gb, std_gb) = trimmed_mean_std(&mut gen_b, 0.10);

    let gain_l = clamped_reinhard_gain(std_ol, std_gl);
    let gain_a = clamped_reinhard_gain(std_oa, std_ga);
    let gain_b = clamped_reinhard_gain(std_ob, std_gb);

    let mut out = RgbaImage::new(orig_crop.width(), orig_crop.height());
    for y in 0..h {
        for x in 0..w {
            let px = gen_patch.get_pixel(x as u32, y as u32).0;
            let ok = srgb_u8_to_oklab_f32([px[0], px[1], px[2]]);
            let transferred = Oklab::new(
                (ok.l - mean_gl) * gain_l + mean_ol,
                (ok.a - mean_ga) * gain_a + mean_oa,
                (ok.b - mean_gb) * gain_b + mean_ob,
            );
            let rgb = oklab_f32_to_srgb_u8(transferred);
            out.put_pixel(x as u32, y as u32, Rgba([rgb[0], rgb[1], rgb[2], px[3]]));
        }
    }

    Ok((
        out,
        ReinhardGainSummary {
            gain_l,
            gain_a,
            gain_b,
            collar_pixel_count,
        },
    ))
}

#[must_use]
fn clamped_reinhard_gain(std_orig: f32, std_gen: f32) -> f32 {
    if std_orig < 1e-4 && std_gen < 1e-4 {
        1.0
    } else {
        (std_orig / std_gen.max(1e-4)).clamp(REINHARD_GAIN_MIN, REINHARD_GAIN_MAX)
    }
}

fn trimmed_mean_std(values: &mut [f32], trim_ratio: f32) -> (f32, f32) {
    values.sort_by(f32::total_cmp);
    let n = values.len();
    let trim = if n >= 10 {
        ((n as f32) * trim_ratio).floor() as usize
    } else {
        0
    };
    let slice = &values[trim..(n - trim).max(trim + 1)];
    let count = slice.len() as f64;
    let mean = slice.iter().map(|&v| f64::from(v)).sum::<f64>() / count;
    let var = slice
        .iter()
        .map(|&v| {
            let d = f64::from(v) - mean;
            d * d
        })
        .sum::<f64>()
        / count;
    (mean as f32, var.sqrt() as f32)
}

// === 3. Distance Transform, Cosine Feathering & 5-Level Laplacian Pyramid ===

/// Exact linear-time 2D Euclidean distance transform (`O(W * H)`) returning distance
/// from the nearest `mask[i] == true` pixel (`0.0` inside mask).
#[must_use]
pub fn euclidean_distance_transform(mask: &[bool], w: usize, h: usize) -> Vec<f32> {
    const INF: f32 = 1e20;
    let mut grid = vec![INF; w * h];
    for i in 0..(w * h) {
        if mask[i] {
            grid[i] = 0.0;
        }
    }

    // 1D pass along columns
    let mut f = vec![0.0f32; h.max(w)];
    let mut v = vec![0usize; h.max(w) + 1];
    let mut z = vec![0.0f32; h.max(w) + 2];

    for x in 0..w {
        for y in 0..h {
            f[y] = grid[y * w + x];
        }
        let d = edt_1d(&f[..h], &mut v, &mut z);
        for (y, val) in d.into_iter().enumerate() {
            grid[y * w + x] = val;
        }
    }

    // 1D pass along rows
    for y in 0..h {
        let row = &grid[y * w..(y + 1) * w];
        f[..w].copy_from_slice(row);
        let d = edt_1d(&f[..w], &mut v, &mut z);
        for (x, val) in d.into_iter().enumerate() {
            grid[y * w + x] = val.sqrt();
        }
    }

    grid
}

fn edt_1d(f: &[f32], v: &mut [usize], z: &mut [f32]) -> Vec<f32> {
    let n = f.len();
    let mut out = vec![0.0f32; n];
    if n == 0 {
        return out;
    }
    let mut k = 0usize;
    v[0] = 0;
    z[0] = f32::NEG_INFINITY;
    z[1] = f32::INFINITY;

    for q in 1..n {
        loop {
            let vk = v[k];
            let s = ((f[q] + (q * q) as f32) - (f[vk] + (vk * vk) as f32))
                / (2.0 * (q as f32 - vk as f32));
            if s <= z[k] {
                if k == 0 {
                    break;
                }
                k -= 1;
            } else {
                k += 1;
                v[k] = q;
                z[k] = s;
                z[k + 1] = f32::INFINITY;
                break;
            }
        }
        if k == 0 {
            let vk = v[0];
            let s = ((f[q] + (q * q) as f32) - (f[vk] + (vk * vk) as f32))
                / (2.0 * (q as f32 - vk as f32));
            if s <= z[0] {
                v[0] = q;
                z[0] = f32::NEG_INFINITY;
                z[1] = f32::INFINITY;
            }
        }
    }

    let mut k_scan = 0usize;
    for (q, dst) in out.iter_mut().enumerate() {
        while z[k_scan + 1] < q as f32 {
            k_scan += 1;
        }
        let dq = q as f32 - v[k_scan] as f32;
        *dst = dq * dq + f[v[k_scan]];
    }
    out
}

#[allow(clippy::too_many_arguments)]
#[must_use]
fn build_cosine_feathered_alpha(
    crop_mask: &[f32],
    dist_from_mask: &[f32],
    w: usize,
    h: usize,
    crop_rect: PixelRect,
    canvas_w: u32,
    canvas_h: u32,
    feather_px: u32,
) -> Vec<f32> {
    let feather = feather_px as f32;
    let mut alpha = vec![0.0f32; w * h];

    let touches_left = crop_rect.x == 0;
    let touches_top = crop_rect.y == 0;
    let touches_right = crop_rect.right() == canvas_w;
    let touches_bottom = crop_rect.bottom() == canvas_h;

    for y in 0..h {
        for x in 0..w {
            let idx = y * w + x;
            let base = if crop_mask[idx] > 0.5 {
                crop_mask[idx].clamp(0.0, 1.0)
            } else {
                let d = dist_from_mask[idx];
                if d >= feather {
                    0.0
                } else {
                    0.5 * (1.0 + (PI * d / feather).cos())
                }
            };

            // Enforce zero seam on any interior crop boundary not touching the canvas edge.
            let mut edge_dist = usize::MAX;
            if !touches_left {
                edge_dist = edge_dist.min(x);
            }
            if !touches_right {
                edge_dist = edge_dist.min(w - 1 - x);
            }
            if !touches_top {
                edge_dist = edge_dist.min(y);
            }
            if !touches_bottom {
                edge_dist = edge_dist.min(h - 1 - y);
            }
            let edge_taper = if edge_dist == 0 {
                0.0
            } else if edge_dist < 3 {
                (edge_dist as f32) / 3.0
            } else {
                1.0
            };

            alpha[idx] = (base * edge_taper).clamp(0.0, 1.0);
        }
    }
    alpha
}

/// Linear-light floating-point RGB buffer `(width, height, [R, G, B])`.
#[derive(Debug, Clone)]
pub struct LinearBuffer {
    /// Width in pixels.
    pub width: usize,
    /// Height in pixels.
    pub height: usize,
    /// Row-major linear RGB pixels.
    pub pixels: Vec<[f32; 3]>,
}

/// Build a `levels`-band Laplacian pyramid (`levels - 1` bandpass detail levels + 1 coarsest Gaussian base).
#[must_use]
pub fn build_laplacian_pyramid(input: &LinearBuffer, levels: usize) -> Vec<LinearBuffer> {
    let max_levels = levels.max(1);
    let mut gauss = Vec::with_capacity(max_levels);
    gauss.push(input.clone());

    for _ in 1..max_levels {
        let prev = gauss.last().expect("at least level 0");
        if prev.width < 2 || prev.height < 2 {
            break;
        }
        let blurred = gaussian_5x5_rgb(prev);
        gauss.push(downsample_half_rgb(&blurred));
    }

    let actual_levels = gauss.len();
    let mut laplacian = Vec::with_capacity(actual_levels);
    for k in 0..(actual_levels - 1) {
        let expanded = upsample_double_rgb(&gauss[k + 1], gauss[k].width, gauss[k].height);
        let mut diff = Vec::with_capacity(gauss[k].pixels.len());
        for (g, e) in gauss[k].pixels.iter().zip(expanded.pixels.iter()) {
            diff.push([g[0] - e[0], g[1] - e[1], g[2] - e[2]]);
        }
        laplacian.push(LinearBuffer {
            width: gauss[k].width,
            height: gauss[k].height,
            pixels: diff,
        });
    }
    laplacian.push(gauss.pop().expect("coarsest gaussian base"));
    laplacian
}

/// Reconstruct a `LinearBuffer` from a Laplacian pyramid returned by `build_laplacian_pyramid`.
#[must_use]
pub fn reconstruct_laplacian_pyramid(pyramid: &[LinearBuffer]) -> LinearBuffer {
    let mut current = pyramid.last().expect("pyramid non-empty").clone();
    for k in (0..(pyramid.len() - 1)).rev() {
        let level = &pyramid[k];
        let up = upsample_double_rgb(&current, level.width, level.height);
        let mut combined = Vec::with_capacity(level.pixels.len());
        for (l, u) in level.pixels.iter().zip(up.pixels.iter()) {
            combined.push([l[0] + u[0], l[1] + u[1], l[2] + u[2]]);
        }
        current = LinearBuffer {
            width: level.width,
            height: level.height,
            pixels: combined,
        };
    }
    current
}

/// Blend `orig_crop` and `patch_crop` using a `levels`-level Laplacian pyramid and cosine-feathered `alpha`.
#[must_use]
pub fn laplacian_pyramid_blend(
    orig_crop: &RgbaImage,
    patch_crop: &RgbaImage,
    alpha_mask: &[f32],
    levels: usize,
) -> RgbaImage {
    let w = orig_crop.width() as usize;
    let h = orig_crop.height() as usize;

    let orig_lin = rgba_to_linear_buffer(orig_crop);
    let patch_lin = rgba_to_linear_buffer(patch_crop);

    let pyr_orig = build_laplacian_pyramid(&orig_lin, levels);
    let pyr_patch = build_laplacian_pyramid(&patch_lin, levels);
    let effective_levels = pyr_orig.len().min(pyr_patch.len());

    // Build matching Gaussian pyramid for scalar alpha mask.
    let mut alpha_pyr = Vec::with_capacity(effective_levels);
    alpha_pyr.push((w, h, alpha_mask.to_vec()));
    for _ in 1..effective_levels {
        let (pw, ph, ref prev) = *alpha_pyr.last().expect("alpha level 0");
        let blurred = gaussian_5x5_scalar(prev, pw, ph);
        let (nw, nh, down) = downsample_half_scalar(&blurred, pw, ph);
        alpha_pyr.push((nw, nh, down));
    }

    let mut blended_pyr = Vec::with_capacity(effective_levels);
    for k in 0..effective_levels {
        let lo = &pyr_orig[k];
        let lp = &pyr_patch[k];
        let (_, _, ref a_vec) = alpha_pyr[k];
        let mut pixels = Vec::with_capacity(lo.pixels.len());
        for ((o, p), &a) in lo.pixels.iter().zip(lp.pixels.iter()).zip(a_vec.iter()) {
            let a = a.clamp(0.0, 1.0);
            let inv = 1.0 - a;
            pixels.push([
                a * p[0] + inv * o[0],
                a * p[1] + inv * o[1],
                a * p[2] + inv * o[2],
            ]);
        }
        blended_pyr.push(LinearBuffer {
            width: lo.width,
            height: lo.height,
            pixels,
        });
    }

    let reconstructed = reconstruct_laplacian_pyramid(&blended_pyr);
    linear_buffer_to_rgba(&reconstructed, orig_crop)
}

// === 4. Helper Utilities ===

fn decode_png_checked(path: &Path, bytes: &[u8]) -> Result<DynamicImage, CoreError> {
    if bytes.len() < PNG_SIGNATURE.len() || bytes[..PNG_SIGNATURE.len()] != PNG_SIGNATURE {
        return Err(CoreError::InvalidPng {
            path: path.to_path_buf(),
            reason: "missing valid 8-byte PNG signature".to_string(),
        });
    }
    image::load_from_memory_with_format(bytes, ImageFormat::Png).map_err(|err| {
        CoreError::InvalidPng {
            path: path.to_path_buf(),
            reason: format!("PNG decode failed: {err}"),
        }
    })
}

fn resolve_unified_mask(
    target_mask: &PatchTargetMask,
    canvas_w: u32,
    canvas_h: u32,
) -> Result<(Vec<f32>, PixelRect), CoreError> {
    let total = (canvas_w as usize) * (canvas_h as usize);
    let mut mask = vec![0.0f32; total];

    match target_mask {
        PatchTargetMask::Roi(rect) => {
            if rect.width == 0
                || rect.height == 0
                || rect.right() > canvas_w
                || rect.bottom() > canvas_h
            {
                return Err(CoreError::EditFailed(format!(
                    "ROI {:?} is empty or exceeds canvas bounds {canvas_w}x{canvas_h}",
                    rect
                )));
            }
            for y in rect.y..rect.bottom() {
                for x in rect.x..rect.right() {
                    mask[(y as usize) * (canvas_w as usize) + (x as usize)] = 1.0;
                }
            }
            Ok((mask, *rect))
        }
        PatchTargetMask::SquareRoi { x, y, size } => {
            let rect = PixelRect {
                x: *x,
                y: *y,
                width: *size,
                height: *size,
            };
            if rect.width == 0
                || rect.height == 0
                || rect.right() > canvas_w
                || rect.bottom() > canvas_h
            {
                return Err(CoreError::EditFailed(format!(
                    "SquareRoi {:?} is empty or exceeds canvas bounds {canvas_w}x{canvas_h}",
                    rect
                )));
            }
            for py in rect.y..rect.bottom() {
                for px in rect.x..rect.right() {
                    mask[(py as usize) * (canvas_w as usize) + (px as usize)] = 1.0;
                }
            }
            Ok((mask, rect))
        }
        PatchTargetMask::MaskImagePath(path) => {
            let bytes = read_regular_file_nofollow(path, "PatchEdit mask PNG")?;
            let img = decode_png_checked(path, &bytes)?.to_rgba8();
            if img.width() != canvas_w || img.height() != canvas_h {
                return Err(CoreError::EditFailed(format!(
                    "mask PNG dimensions {}x{} do not match target image {canvas_w}x{canvas_h}",
                    img.width(),
                    img.height()
                )));
            }
            let has_varied_alpha = img.pixels().any(|p| p.0[3] < 255);
            for y in 0..canvas_h {
                for x in 0..canvas_w {
                    let p = img.get_pixel(x, y).0;
                    let val = if has_varied_alpha {
                        f32::from(p[3]) / 255.0
                    } else {
                        (0.2126 * f32::from(p[0])
                            + 0.7152 * f32::from(p[1])
                            + 0.0722 * f32::from(p[2]))
                            / 255.0
                    };
                    mask[(y as usize) * (canvas_w as usize) + (x as usize)] = val.clamp(0.0, 1.0);
                }
            }
            let bbox = compute_mask_bbox(&mask, canvas_w, canvas_h)?;
            Ok((mask, bbox))
        }
        PatchTargetMask::MaskValues {
            width,
            height,
            values,
        } => {
            if *width != canvas_w || *height != canvas_h || values.len() != total {
                return Err(CoreError::EditFailed(format!(
                    "MaskValues ({width}x{height}, len={}) does not match target image {canvas_w}x{canvas_h}",
                    values.len()
                )));
            }
            for (idx, &v) in values.iter().enumerate() {
                if !v.is_finite() || !(0.0..=1.0).contains(&v) {
                    return Err(CoreError::EditFailed(format!(
                        "MaskValues[{idx}]={v} must be finite within [0.0, 1.0]"
                    )));
                }
            }
            let bbox = compute_mask_bbox(values, canvas_w, canvas_h)?;
            Ok((values.clone(), bbox))
        }
    }
}

fn compute_mask_bbox(mask: &[f32], w: u32, h: u32) -> Result<PixelRect, CoreError> {
    let mut min_x = u32::MAX;
    let mut min_y = u32::MAX;
    let mut max_x = 0u32;
    let mut max_y = 0u32;
    let mut found = false;

    for y in 0..h {
        for x in 0..w {
            if mask[(y as usize) * (w as usize) + (x as usize)] > 0.01 {
                found = true;
                min_x = min_x.min(x);
                min_y = min_y.min(y);
                max_x = max_x.max(x);
                max_y = max_y.max(y);
            }
        }
    }

    if !found {
        return Err(CoreError::EditFailed(
            "PatchEdit target mask contains zero active pixels".to_string(),
        ));
    }

    Ok(PixelRect {
        x: min_x,
        y: min_y,
        width: max_x - min_x + 1,
        height: max_y - min_y + 1,
    })
}

fn compute_square_crop_rect(
    roi: PixelRect,
    canvas_w: u32,
    canvas_h: u32,
    collar_width_px: u32,
) -> Result<PixelRect, CoreError> {
    let min_dim = canvas_w.min(canvas_h);
    if min_dim == 0 {
        return Err(CoreError::EditFailed(
            "cannot extract square crop from 0-pixel canvas".to_string(),
        ));
    }
    let desired = roi
        .width
        .max(roi.height)
        .saturating_add(collar_width_px.saturating_mul(2));
    let crop_size = desired.min(min_dim);

    let cx = roi.x + roi.width / 2;
    let cy = roi.y + roi.height / 2;
    let crop_x = cx.saturating_sub(crop_size / 2).min(canvas_w - crop_size);
    let crop_y = cy.saturating_sub(crop_size / 2).min(canvas_h - crop_size);

    Ok(PixelRect {
        x: crop_x,
        y: crop_y,
        width: crop_size,
        height: crop_size,
    })
}

fn prepare_patch_target(
    orig_crop: &RgbaImage,
    crop_mask: &[f32],
    mode: PatchPrepMode,
) -> RgbaImage {
    let mut out = orig_crop.clone();
    let w = out.width();
    let h = out.height();

    match mode {
        PatchPrepMode::CropOnly => out,
        PatchPrepMode::BlankInsideMask => {
            for y in 0..h {
                for x in 0..w {
                    if crop_mask[(y as usize) * (w as usize) + (x as usize)] > 0.5 {
                        out.put_pixel(x, y, Rgba(STUDIO_BG_RGBA));
                    }
                }
            }
            out
        }
        PatchPrepMode::CueOutline => {
            let is_inside = |px: i32, py: i32| -> bool {
                if px < 0 || py < 0 || px >= w as i32 || py >= h as i32 {
                    return false;
                }
                crop_mask[(py as usize) * (w as usize) + (px as usize)] > 0.5
            };
            for y in 0..h {
                for x in 0..w {
                    let ix = x as i32;
                    let iy = y as i32;
                    let center = is_inside(ix, iy);
                    let border_inner = center
                        && (!is_inside(ix - 1, iy)
                            || !is_inside(ix + 1, iy)
                            || !is_inside(ix, iy - 1)
                            || !is_inside(ix, iy + 1));
                    let border_halo = !center
                        && (is_inside(ix - 1, iy)
                            || is_inside(ix + 1, iy)
                            || is_inside(ix, iy - 1)
                            || is_inside(ix, iy + 1));
                    if border_inner {
                        out.put_pixel(x, y, Rgba([246, 173, 85, 255]));
                    } else if border_halo {
                        out.put_pixel(x, y, Rgba([26, 29, 36, 255]));
                    }
                }
            }
            out
        }
    }
}

fn srgb_u8_to_oklab_f32(rgb: [u8; 3]) -> Oklab<f32> {
    let srgb = Srgb::new(
        f32::from(rgb[0]) / 255.0,
        f32::from(rgb[1]) / 255.0,
        f32::from(rgb[2]) / 255.0,
    );
    srgb.into_linear::<f32>().into_color()
}

fn oklab_f32_to_srgb_u8(oklab: Oklab<f32>) -> [u8; 3] {
    let lin: LinSrgb<f32> = oklab.into_color();
    let srgb: Srgb<u8> = Srgb::<f32>::from_linear(lin).clamp().into_format();
    [srgb.red, srgb.green, srgb.blue]
}

fn rgba_to_linear_buffer(img: &RgbaImage) -> LinearBuffer {
    let w = img.width() as usize;
    let h = img.height() as usize;
    let mut pixels = Vec::with_capacity(w * h);
    for px in img.pixels() {
        let srgb = Srgb::new(
            f32::from(px.0[0]) / 255.0,
            f32::from(px.0[1]) / 255.0,
            f32::from(px.0[2]) / 255.0,
        );
        let lin: LinSrgb<f32> = srgb.into_linear();
        pixels.push([lin.red, lin.green, lin.blue]);
    }
    LinearBuffer {
        width: w,
        height: h,
        pixels,
    }
}

fn linear_buffer_to_rgba(buf: &LinearBuffer, alpha_source: &RgbaImage) -> RgbaImage {
    let mut out = RgbaImage::new(buf.width as u32, buf.height as u32);
    for y in 0..buf.height {
        for x in 0..buf.width {
            let lin = buf.pixels[y * buf.width + x];
            let srgb: Srgb<u8> = Srgb::<f32>::from_linear(LinSrgb::new(lin[0], lin[1], lin[2]))
                .clamp()
                .into_format();
            let alpha = alpha_source.get_pixel(x as u32, y as u32).0[3];
            out.put_pixel(
                x as u32,
                y as u32,
                Rgba([srgb.red, srgb.green, srgb.blue, alpha]),
            );
        }
    }
    out
}

const GAUSS_5_KERNEL: [f32; 5] = [1.0 / 16.0, 4.0 / 16.0, 6.0 / 16.0, 4.0 / 16.0, 1.0 / 16.0];

fn gaussian_5x5_rgb(buf: &LinearBuffer) -> LinearBuffer {
    let w = buf.width;
    let h = buf.height;
    let mut horiz = vec![[0.0f32; 3]; w * h];
    for y in 0..h {
        for x in 0..w {
            let mut acc = [0.0f32; 3];
            for (ki, &kw) in GAUSS_5_KERNEL.iter().enumerate() {
                let sx = (x as isize + ki as isize - 2).clamp(0, w as isize - 1) as usize;
                let p = buf.pixels[y * w + sx];
                acc[0] += p[0] * kw;
                acc[1] += p[1] * kw;
                acc[2] += p[2] * kw;
            }
            horiz[y * w + x] = acc;
        }
    }
    let mut pixels = vec![[0.0f32; 3]; w * h];
    for y in 0..h {
        for x in 0..w {
            let mut acc = [0.0f32; 3];
            for (ki, &kw) in GAUSS_5_KERNEL.iter().enumerate() {
                let sy = (y as isize + ki as isize - 2).clamp(0, h as isize - 1) as usize;
                let p = horiz[sy * w + x];
                acc[0] += p[0] * kw;
                acc[1] += p[1] * kw;
                acc[2] += p[2] * kw;
            }
            pixels[y * w + x] = acc;
        }
    }
    LinearBuffer {
        width: w,
        height: h,
        pixels,
    }
}

fn downsample_half_rgb(buf: &LinearBuffer) -> LinearBuffer {
    let nw = buf.width.div_ceil(2).max(1);
    let nh = buf.height.div_ceil(2).max(1);
    let mut pixels = Vec::with_capacity(nw * nh);
    for y in 0..nh {
        let sy = (y * 2).min(buf.height - 1);
        for x in 0..nw {
            let sx = (x * 2).min(buf.width - 1);
            pixels.push(buf.pixels[sy * buf.width + sx]);
        }
    }
    LinearBuffer {
        width: nw,
        height: nh,
        pixels,
    }
}

fn upsample_double_rgb(buf: &LinearBuffer, target_w: usize, target_h: usize) -> LinearBuffer {
    let mut pixels = Vec::with_capacity(target_w * target_h);
    for y in 0..target_h {
        let fy = (y as f32) * 0.5;
        let y0 = (fy.floor() as usize).min(buf.height - 1);
        let y1 = (y0 + 1).min(buf.height - 1);
        let wy = fy - y0 as f32;
        for x in 0..target_w {
            let fx = (x as f32) * 0.5;
            let x0 = (fx.floor() as usize).min(buf.width - 1);
            let x1 = (x0 + 1).min(buf.width - 1);
            let wx = fx - x0 as f32;

            let p00 = buf.pixels[y0 * buf.width + x0];
            let p10 = buf.pixels[y0 * buf.width + x1];
            let p01 = buf.pixels[y1 * buf.width + x0];
            let p11 = buf.pixels[y1 * buf.width + x1];

            let mut out = [0.0f32; 3];
            for c in 0..3 {
                let top = p00[c] * (1.0 - wx) + p10[c] * wx;
                let bot = p01[c] * (1.0 - wx) + p11[c] * wx;
                out[c] = top * (1.0 - wy) + bot * wy;
            }
            pixels.push(out);
        }
    }
    LinearBuffer {
        width: target_w,
        height: target_h,
        pixels,
    }
}

fn gaussian_5x5_scalar(data: &[f32], w: usize, h: usize) -> Vec<f32> {
    let mut horiz = vec![0.0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            let mut acc = 0.0f32;
            for (ki, &kw) in GAUSS_5_KERNEL.iter().enumerate() {
                let sx = (x as isize + ki as isize - 2).clamp(0, w as isize - 1) as usize;
                acc += data[y * w + sx] * kw;
            }
            horiz[y * w + x] = acc;
        }
    }
    let mut out = vec![0.0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            let mut acc = 0.0f32;
            for (ki, &kw) in GAUSS_5_KERNEL.iter().enumerate() {
                let sy = (y as isize + ki as isize - 2).clamp(0, h as isize - 1) as usize;
                acc += horiz[sy * w + x] * kw;
            }
            out[y * w + x] = acc;
        }
    }
    out
}

fn downsample_half_scalar(data: &[f32], w: usize, h: usize) -> (usize, usize, Vec<f32>) {
    let nw = w.div_ceil(2).max(1);
    let nh = h.div_ceil(2).max(1);
    let mut out = Vec::with_capacity(nw * nh);
    for y in 0..nh {
        let sy = (y * 2).min(h - 1);
        for x in 0..nw {
            let sx = (x * 2).min(w - 1);
            out.push(data[sy * w + sx]);
        }
    }
    (nw, nh, out)
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use image::ImageBuffer;
    use rstest::rstest;
    use tempfile::TempDir;

    use super::*;
    use crate::backend::ImageGenerationOutput;
    use crate::subject::PaletteSwatch;

    struct StubEditBackend {
        response_fill: [u8; 3],
        seen_requests: Arc<Mutex<Vec<ImageGenerationRequest>>>,
    }

    #[async_trait]
    impl ImageBackend for StubEditBackend {
        fn name(&self) -> &'static str {
            "stub-edit-backend"
        }

        async fn generate(
            &self,
            request: &ImageGenerationRequest,
        ) -> Result<ImageGenerationOutput, CoreError> {
            self.seen_requests.lock().unwrap().push(request.clone());
            let img: RgbaImage = ImageBuffer::from_pixel(
                request.width,
                request.height,
                Rgba([
                    self.response_fill[0],
                    self.response_fill[1],
                    self.response_fill[2],
                    255,
                ]),
            );
            let mut png_bytes = Vec::new();
            DynamicImage::ImageRgba8(img)
                .write_to(&mut Cursor::new(&mut png_bytes), ImageFormat::Png)
                .unwrap();
            Ok(ImageGenerationOutput {
                png_bytes,
                width: request.width,
                height: request.height,
                raw_width: request.width,
                raw_height: request.height,
                thread_id: Some("thr_stub_edit".to_string()),
            })
        }
    }

    fn write_test_png(path: &Path, img: &RgbaImage) {
        let mut buf = Vec::new();
        DynamicImage::ImageRgba8(img.clone())
            .write_to(&mut Cursor::new(&mut buf), ImageFormat::Png)
            .unwrap();
        fs::write(path, buf).unwrap();
    }

    #[test]
    fn laplacian_5_level_pyramid_reconstructs_exact_roundtrip() {
        let w = 64usize;
        let h = 64usize;
        let mut pixels = Vec::with_capacity(w * h);
        for y in 0..h {
            for x in 0..w {
                pixels.push([
                    (x as f32 / w as f32),
                    (y as f32 / h as f32),
                    (((x + y) % 17) as f32 / 17.0),
                ]);
            }
        }
        let orig = LinearBuffer {
            width: w,
            height: h,
            pixels,
        };
        let pyr = build_laplacian_pyramid(&orig, 5);
        assert_eq!(pyr.len(), 5);
        let recon = reconstruct_laplacian_pyramid(&pyr);
        let max_err = orig
            .pixels
            .iter()
            .zip(recon.pixels.iter())
            .flat_map(|(a, b)| {
                [
                    (a[0] - b[0]).abs(),
                    (a[1] - b[1]).abs(),
                    (a[2] - b[2]).abs(),
                ]
            })
            .fold(0.0f32, f32::max);
        assert!(
            max_err < 1e-5,
            "5-level Laplacian roundtrip max error {max_err} exceeded 1e-5"
        );
    }

    #[rstest]
    #[case::high_contrast_patch_clamped_to_075(true)]
    #[case::low_contrast_patch_clamped_to_135(false)]
    fn collar_reinhard_transfer_clamps_gain_strictly_to_075_135(#[case] high_gen_contrast: bool) {
        let w = 64u32;
        let h = 64u32;
        let mut orig = RgbaImage::new(w, h);
        let mut generated = RgbaImage::new(w, h);
        let mut mask = vec![0.0f32; (w * h) as usize];

        for y in 0..h {
            for x in 0..w {
                if (24..40).contains(&x) && (24..40).contains(&y) {
                    mask[(y * w + x) as usize] = 1.0;
                }
                let stripe = ((x + y) % 2) as i16;
                if high_gen_contrast {
                    // Orig collar has narrow contrast; gen collar has huge contrast => gain < 0.75 before clamp.
                    let vo = (120 + stripe * 10) as u8;
                    let vg = if stripe == 0 { 20 } else { 235 };
                    orig.put_pixel(x, y, Rgba([vo, vo, vo, 255]));
                    generated.put_pixel(x, y, Rgba([vg, vg, vg, 255]));
                } else {
                    // Orig collar has wide contrast; gen collar has tiny contrast => gain > 1.35 before clamp.
                    let vo = if stripe == 0 { 20 } else { 235 };
                    let vg = (120 + stripe * 4) as u8;
                    orig.put_pixel(x, y, Rgba([vo, vo, vo, 255]));
                    generated.put_pixel(x, y, Rgba([vg, vg, vg, 255]));
                }
            }
        }

        let binary: Vec<bool> = mask.iter().map(|&m| m > 0.5).collect();
        let dist = euclidean_distance_transform(&binary, w as usize, h as usize);
        let (_, gains) = apply_collar_reinhard_oklab(&orig, &generated, &mask, &dist, 24)
            .expect("collar reinhard succeeds");

        assert!(gains.collar_pixel_count > 100);
        assert!(
            (REINHARD_GAIN_MIN..=REINHARD_GAIN_MAX).contains(&gains.gain_l),
            "gain_l {} must stay within [0.75, 1.35]",
            gains.gain_l
        );
        if high_gen_contrast {
            assert!((gains.gain_l - REINHARD_GAIN_MIN).abs() < 1e-5);
        } else {
            assert!((gains.gain_l - REINHARD_GAIN_MAX).abs() < 1e-5);
        }
    }

    #[rstest]
    #[case::roi_rect_crop_only(PatchPrepMode::CropOnly, false)]
    #[case::roi_rect_blank_inside_mask(PatchPrepMode::BlankInsideMask, false)]
    #[case::alpha_mask_png_cue_outline(PatchPrepMode::CueOutline, true)]
    #[tokio::test]
    async fn patch_edit_preserves_zero_outside_crop_drift_and_records_run(
        #[case] prep_mode: PatchPrepMode,
        #[case] use_mask_png: bool,
    ) {
        let root = TempDir::new().expect("tempdir");
        let run_store = RunStore::new(root.path().join("runs"));
        let target_path = root.path().join("target.png");

        let mut target: RgbaImage = ImageBuffer::from_pixel(128, 128, Rgba([70, 95, 120, 255]));
        for y in 48..80 {
            for x in 48..80 {
                target.put_pixel(x, y, Rgba([160, 120, 80, 255]));
            }
        }
        write_test_png(&target_path, &target);

        let target_mask = if use_mask_png {
            let mut mask_img: RgbaImage = ImageBuffer::from_pixel(128, 128, Rgba([0, 0, 0, 0]));
            for y in 52..76 {
                for x in 52..76 {
                    mask_img.put_pixel(x, y, Rgba([255, 255, 255, 255]));
                }
            }
            let mask_path = root.path().join("mask.png");
            write_test_png(&mask_path, &mask_img);
            PatchTargetMask::MaskImagePath(mask_path)
        } else {
            PatchTargetMask::SquareRoi {
                x: 52,
                y: 52,
                size: 24,
            }
        };

        let seen = Arc::new(Mutex::new(Vec::new()));
        let backend = StubEditBackend {
            response_fill: [185, 130, 85],
            seen_requests: Arc::clone(&seen),
        };

        let result = execute_patch_edit(
            &backend,
            &run_store,
            &PatchEditRequest {
                slug: "harbor-patch".to_string(),
                visibility: Visibility::Public,
                target_image_path: target_path,
                target_mask,
                compiled_prompt: "adjust lantern brass highlight cleanly".to_string(),
                reference_images: Vec::new(),
                config: PatchEditConfig {
                    prep_mode,
                    ..PatchEditConfig::default()
                },
            },
        )
        .await
        .expect("PatchEdit succeeds");

        // Backend was invoked at 1:1 square WxW.
        let reqs = seen.lock().unwrap();
        assert_eq!(reqs.len(), 1);
        assert_eq!(reqs[0].width, reqs[0].height);
        assert_eq!(result.crop_rect.width, result.crop_rect.height);

        // Zero drift outside crop/collar footprint.
        assert!(result.out_of_roi_report.passed);
        assert_eq!(result.out_of_roi_report.max_delta_e2000, 0.0);

        let out_img = image::load_from_memory(&result.png_bytes)
            .unwrap()
            .to_rgba8();
        // Far corner outside crop footprint is bit-for-bit identical.
        assert_eq!(out_img.get_pixel(0, 0), target.get_pixel(0, 0));
        // Center of ROI changed smoothly.
        assert_ne!(out_img.get_pixel(64, 64), target.get_pixel(64, 64));
        assert_eq!(result.run.metadata.version, 1);
        assert_eq!(
            result.run.metadata.qa_reports.color_qa_max_delta_e2000,
            Some(0.0)
        );
    }

    #[rstest]
    #[case::passing_qa_records_run([150, 155, 160], true)]
    #[case::shadow_crush_fails_closed_without_mutating_image([4, 4, 6], false)]
    #[tokio::test]
    async fn whole_image_edit_gates_fail_closed_and_never_regrades(
        #[case] backend_fill: [u8; 3],
        #[case] expect_ok: bool,
    ) {
        let root = TempDir::new().expect("tempdir");
        let run_store = RunStore::new(root.path().join("runs"));
        let target_path = root.path().join("scene.png");
        let target_img: RgbaImage = ImageBuffer::from_pixel(64, 64, Rgba([150, 155, 160, 255]));
        write_test_png(&target_path, &target_img);

        let backend = StubEditBackend {
            response_fill: backend_fill,
            seen_requests: Arc::new(Mutex::new(Vec::new())),
        };

        let res = execute_whole_image_edit(
            &backend,
            &run_store,
            &WholeImageEditRequest {
                slug: "harbor-whole".to_string(),
                visibility: Visibility::Public,
                target_image_path: target_path,
                compiled_prompt: "preserve crisp daylight exposure".to_string(),
                reference_images: Vec::new(),
                exposure_floor: Some((
                    ExposureFloor {
                        min_mean_l_star: 35.0,
                        min_p10_l_star: 12.0,
                    },
                    ExposureQaConfig::default(),
                )),
                subject_rois: vec![OwnedSubjectRoi {
                    subject_slug: "stone-wall".to_string(),
                    palette_lock: PaletteLock {
                        swatches: vec![PaletteSwatch {
                            name: "stone".to_string(),
                            color_hex: "#969BA0".to_string(),
                            surface_slot: None,
                        }],
                    },
                    rois: Vec::new(),
                }],
                palette_config: PaletteQaConfig::default(),
                edit_mask: None,
                untouched_drift_config: UntouchedDriftConfig::default(),
                allowed_sparkle_mask: None,
                sparkle_config: SparkleConfig::default(),
                fail_closed_on_qa: true,
            },
        )
        .await;

        assert_eq!(res.is_ok(), expect_ok);
        if let Ok(ok) = res {
            // Confirm whole image was NOT silently re-graded: pixels remain exact backend output.
            let decoded = image::load_from_memory(&ok.png_bytes).unwrap().to_rgba8();
            assert_eq!(decoded.get_pixel(10, 10).0, [150, 155, 160, 255]);
        } else {
            assert!(matches!(res.unwrap_err(), CoreError::ColorQaFailed(_)));
            // Draft run is still preserved for inspection.
            assert_eq!(run_store.list_runs("harbor-whole").unwrap().len(), 1);
        }
    }
}
