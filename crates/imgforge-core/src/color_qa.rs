//! === FAIL-CLOSED OKLAB / CIELAB DELTA-E 2000 COLOR QA ===
//!
//! PURPOSE:
//! Implements `ColorQaEngine` using `palette` (`Oklab`, `Lab`, `Ciede2000`) to
//! enforce fail-closed perceptual color quality gates across `ReferencePack`
//! outputs, `WholeImageEdit`, and `PatchEdit` (ADR 0005).
//!
//! THREAT MODEL / DESIGN RATIONALE ("WHY"):
//! 1. Generative edits can crush shadow floors, bleed `PaletteLock` colors
//!    across garment boundaries, introduce isolated yellow sparkle artifacts,
//!    or silently drift untouched pixels outside the intended edit ROI.
//! 2. Whole-image Color QA strictly flags/fails; it NEVER mutates or re-grades
//!    whole images (`ADR 0005`). Local harmonization belongs exclusively to
//!    `PatchEdit` collar transfer, and explicit LUT/CDF grading belongs strictly
//!    to `export --match-grade`.
//!
//! EXECUTION LIFECYCLE / DATA FLOW:
//! 1. `evaluate_exposure`: computes interior CIE `L*` mean, `P10`, `P90`, and
//!    clipped-black ratio (`L* < 5.0`) against `Atmosphere.exposure_floor` and
//!    optional anchor tolerances.
//! 2. `evaluate_palette_adherence`: runs deterministic OKLab k-means (`K = 6`)
//!    inside subject ROI(s), computes closest `DeltaE_2000` to `PaletteLock`
//!    swatches, checks lightness/chroma shifts, and detects forbidden-hue
//!    sector intrusions.
//! 3. `evaluate_untouched_drift`: measures per-pixel `DeltaE_2000` outside an
//!    edit mask (`1 - M`) against mean, P95, max, and fraction thresholds.
//! 4. `detect_sparkles`: runs a high-pass Difference-of-Gaussians (DoG)
//!    residual detector for isolated `1..=64 px^2` saturated yellow blobs
//!    outside allowed masks.
//! 5. `render_heatmap_rgba` / `render_heatmap_png`: generates RGBA diagnostic
//!    overlays (`drift`, `bleed`, `sparkle`, `out_of_roi`) for the Web Canvas UI.

use std::collections::VecDeque;
use std::io::Cursor;

use image::{DynamicImage, ImageFormat, Rgba, RgbaImage};
use palette::color_difference::Ciede2000;
use palette::white_point::D65;
use palette::{Clamp, IntoColor, Lab, LinSrgb, Oklab, Srgb};
use serde::{Deserialize, Serialize};

use crate::atmosphere::ExposureFloor;
use crate::calibration::parse_hex_rgb;
use crate::error::CoreError;
use crate::slicer::PixelRect;
use crate::subject::PaletteLock;

/// Default number of OKLab k-means clusters evaluated per Subject ROI.
pub const DEFAULT_KMEANS_K: usize = 6;

/// Default maximum `L*` threshold defining clipped-black shadow pixels.
pub const DEFAULT_CLIPPED_BLACK_L_STAR: f64 = 5.0;

/// Default minimum area (inclusive) for sparkle/yellow-dot blob detection.
pub const DEFAULT_SPARKLE_MIN_AREA_PX: usize = 1;

/// Default maximum area (inclusive) for sparkle/yellow-dot blob detection.
pub const DEFAULT_SPARKLE_MAX_AREA_PX: usize = 64;

/// Default bin count (`20` bins of width `5.0` covering CIE `L*` `0..=100`).
pub const DEFAULT_L_STAR_HISTOGRAM_BINS: usize = 20;

// === 1. Color Space Conversion Helpers ===

/// Convert an sRGB `u8` triple to perceptual `Oklab<f32>`.
#[must_use]
pub fn srgb_u8_to_oklab(rgb: [u8; 3]) -> Oklab<f32> {
    let srgb = Srgb::new(
        f32::from(rgb[0]) / 255.0,
        f32::from(rgb[1]) / 255.0,
        f32::from(rgb[2]) / 255.0,
    );
    srgb.into_linear::<f32>().into_color()
}

/// Convert an `Oklab<f32>` color back to clamped sRGB `u8`.
#[must_use]
pub fn oklab_to_srgb_u8(oklab: Oklab<f32>) -> [u8; 3] {
    let lin: LinSrgb<f32> = oklab.into_color();
    let srgb: Srgb<u8> = Srgb::<f32>::from_linear(lin).clamp().into_format();
    [srgb.red, srgb.green, srgb.blue]
}

/// Convert an sRGB `u8` triple to CIE `Lab<D65, f32>`.
#[must_use]
pub fn srgb_u8_to_lab(rgb: [u8; 3]) -> Lab<D65, f32> {
    let srgb = Srgb::new(
        f32::from(rgb[0]) / 255.0,
        f32::from(rgb[1]) / 255.0,
        f32::from(rgb[2]) / 255.0,
    );
    srgb.into_linear::<f32>().into_color()
}

/// Convert an `Oklab<f32>` color to CIE `Lab<D65, f32>`.
#[must_use]
pub fn oklab_to_lab(oklab: Oklab<f32>) -> Lab<D65, f32> {
    let lin: LinSrgb<f32> = oklab.into_color();
    lin.clamp().into_color()
}

/// Compute perceptual CIEDE2000 (`DeltaE_2000`) color difference between two CIE `Lab` colors.
#[must_use]
pub fn delta_e_2000(a: Lab<D65, f32>, b: Lab<D65, f32>) -> f64 {
    f64::from(a.difference(b))
}

/// Compute perceptual CIEDE2000 (`DeltaE_2000`) between two sRGB `u8` pixels.
#[must_use]
pub fn srgb_delta_e_2000(a: [u8; 3], b: [u8; 3]) -> f64 {
    delta_e_2000(srgb_u8_to_lab(a), srgb_u8_to_lab(b))
}

#[must_use]
fn lab_chroma(lab: Lab<D65, f32>) -> f64 {
    let a = f64::from(lab.a);
    let b = f64::from(lab.b);
    (a * a + b * b).sqrt()
}

#[must_use]
fn lab_hue_deg(lab: Lab<D65, f32>) -> f64 {
    f64::from(lab.b)
        .atan2(f64::from(lab.a))
        .to_degrees()
        .rem_euclid(360.0)
}

#[must_use]
fn format_hex_rgb(rgb: [u8; 3]) -> String {
    format!("#{:02X}{:02X}{:02X}", rgb[0], rgb[1], rgb[2])
}

// === 2. Exposure / Shadow-Crush Drift ===

/// Summary luminance statistics computed over interior pixels of an image.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ExposureMetrics {
    /// Mean CIE `L*` luminance (`0.0..=100.0`).
    pub mean_l_star: f64,
    /// 10th-percentile CIE `L*` luminance (`0.0..=100.0`).
    pub p10_l_star: f64,
    /// 90th-percentile CIE `L*` luminance (`0.0..=100.0`).
    pub p90_l_star: f64,
    /// Fraction of interior pixels with `L* < clipped_black_l_star_threshold`.
    pub clipped_black_ratio: f64,
    /// Total interior pixels sampled.
    pub pixel_count: usize,
}

/// Configuration thresholds for exposure and shadow-crush QA.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ExposureQaConfig {
    /// Inset border width excluded when image dimensions allow (`> 2 * inset`).
    pub border_inset_px: u32,
    /// CIE `L*` threshold below which pixels count as clipped shadow black (`5.0`).
    pub clipped_black_l_star_threshold: f64,
    /// Maximum allowed fraction of clipped-black pixels (`0.0..=1.0`).
    pub max_clipped_black_ratio: f64,
    /// Optional anchor exposure metrics for drift comparison.
    pub anchor_exposure: Option<ExposureMetrics>,
    /// Maximum allowed mean `L*` drift from anchor when `anchor_exposure` is provided.
    pub anchor_mean_l_star_tolerance: f64,
    /// Maximum allowed `P10 L*` drift from anchor when `anchor_exposure` is provided.
    pub anchor_p10_l_star_tolerance: f64,
}

impl Default for ExposureQaConfig {
    fn default() -> Self {
        Self {
            border_inset_px: 2,
            clipped_black_l_star_threshold: DEFAULT_CLIPPED_BLACK_L_STAR,
            max_clipped_black_ratio: 0.15,
            anchor_exposure: None,
            anchor_mean_l_star_tolerance: 12.0,
            anchor_p10_l_star_tolerance: 8.0,
        }
    }
}

/// Pass/fail exposure and shadow-crush evaluation report.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExposureReport {
    /// `true` when all exposure floor, clipped-black, and anchor tolerances pass.
    pub passed: bool,
    /// Computed interior CIE `L*` statistics.
    pub metrics: ExposureMetrics,
    /// Evaluated `Atmosphere.exposure_floor` thresholds.
    pub floor: ExposureFloor,
    /// Human-readable failure reasons (empty when `passed == true`).
    pub reasons: Vec<String>,
}

// === 3. PaletteLock Adherence & Bleed ===

/// Hue-degree sector forbidden inside a Subject ROI unless within `max_fraction`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ForbiddenHueSector {
    /// Descriptive sector name (for example `"neon-magenta"`).
    pub name: String,
    /// Inclusive start hue angle in degrees `[0, 360)`.
    pub min_hue_deg: f64,
    /// Inclusive end hue angle in degrees `[0, 360)` (supports wrap-around when `min > max`).
    pub max_hue_deg: f64,
    /// Minimum CIELAB chroma required before hue angle is evaluated.
    pub min_chroma: f64,
    /// Maximum allowed fraction (`0.0..=1.0`) of ROI pixels falling inside this sector.
    pub max_fraction: f64,
}

impl ForbiddenHueSector {
    /// Returns `true` when `hue_deg` (normalized `[0, 360)`) and `chroma` fall in this sector.
    #[must_use]
    pub fn contains(&self, hue_deg: f64, chroma: f64) -> bool {
        if chroma < self.min_chroma {
            return false;
        }
        let min = self.min_hue_deg.rem_euclid(360.0);
        let max = self.max_hue_deg.rem_euclid(360.0);
        let h = hue_deg.rem_euclid(360.0);
        if min <= max {
            h >= min && h <= max
        } else {
            h >= min || h <= max
        }
    }
}

/// Configuration thresholds for `PaletteLock` OKLab k-means adherence and bleed QA.
#[derive(Debug, Clone, PartialEq)]
pub struct PaletteQaConfig {
    /// Number of deterministic OKLab clusters (`K = 6`).
    pub kmeans_k: usize,
    /// Maximum Lloyd iterations for deterministic k-means convergence.
    pub kmeans_max_iters: usize,
    /// Minimum cluster weight (`0.0..=1.0`) checked against `PaletteLock` swatches.
    pub min_cluster_weight: f64,
    /// Maximum allowed `DeltaE_2000` from a dominant cluster to its closest `PaletteLock` swatch.
    pub max_swatch_delta_e2000: f64,
    /// Maximum allowed absolute `L*` shift between a dominant cluster and its closest swatch.
    pub max_lightness_shift: f64,
    /// Maximum allowed absolute chroma `C*` shift between a dominant cluster and its closest swatch.
    pub max_chroma_shift: f64,
    /// Optional forbidden-hue sector exclusions checked inside ROI pixels.
    pub forbidden_hue_sectors: Vec<ForbiddenHueSector>,
}

impl Default for PaletteQaConfig {
    fn default() -> Self {
        Self {
            kmeans_k: DEFAULT_KMEANS_K,
            kmeans_max_iters: 25,
            min_cluster_weight: 0.03,
            max_swatch_delta_e2000: 22.0,
            max_lightness_shift: 28.0,
            max_chroma_shift: 28.0,
            forbidden_hue_sectors: Vec::new(),
        }
    }
}

/// Single deterministic OKLab k-means cluster evaluated against `PaletteLock`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OklabClusterSummary {
    /// Cluster centroid in `Oklab` `(L, a, b)`.
    pub centroid_oklab: [f32; 3],
    /// Clamped uppercase `#RRGGBB` representation of the centroid.
    pub centroid_hex: String,
    /// Fraction of ROI pixels assigned to this cluster (`0.0..=1.0`).
    pub weight: f64,
    /// Absolute count of ROI pixels assigned to this cluster.
    pub pixel_count: usize,
    /// Name of the closest `PaletteLock` swatch by `DeltaE_2000`.
    pub closest_swatch_name: String,
    /// Hex color of the closest `PaletteLock` swatch.
    pub closest_swatch_hex: String,
    /// `DeltaE_2000` distance to `closest_swatch_hex`.
    pub delta_e2000: f64,
    /// Absolute CIE `L*` difference relative to `closest_swatch_hex`.
    pub lightness_shift: f64,
    /// Absolute CIE `C*` chroma difference relative to `closest_swatch_hex`.
    pub chroma_shift: f64,
}

/// Recorded intrusion into a configured `ForbiddenHueSector`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ForbiddenHueIntrusion {
    /// Sector name.
    pub sector_name: String,
    /// Fraction of ROI pixels falling inside the forbidden sector.
    pub fraction: f64,
    /// Pixel count inside the forbidden sector.
    pub pixel_count: usize,
}

/// Input specification tying a `Subject` `PaletteLock` to one or more target ROIs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubjectRoiSpec<'a> {
    /// `Subject` slug for reporting context.
    pub subject_slug: &'a str,
    /// Bound `PaletteLock` swatches.
    pub palette_lock: &'a PaletteLock,
    /// Pixel bounding boxes inside the target image (empty selects entire image).
    pub rois: Vec<PixelRect>,
}

/// Pass/fail `PaletteLock` adherence, shift, and forbidden-hue bleed report.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PaletteAdherenceReport {
    /// `true` when dominant clusters, shifts, and forbidden hue sectors pass.
    pub passed: bool,
    /// Evaluated `Subject` identifier.
    pub subject_slug: String,
    /// Dominant OKLab k-means clusters ordered by weight descending.
    pub clusters: Vec<OklabClusterSummary>,
    /// Maximum `DeltaE_2000` among clusters meeting `min_cluster_weight`.
    pub max_swatch_delta_e2000: f64,
    /// Pixel-weighted mean `DeltaE_2000` across evaluated clusters.
    pub weighted_mean_delta_e2000: f64,
    /// Any forbidden hue sectors exceeding `max_fraction`.
    pub forbidden_hue_intrusions: Vec<ForbiddenHueIntrusion>,
    /// Human-readable failure reasons.
    pub reasons: Vec<String>,
}

// === 4. Untouched-Region Drift ===

/// Thresholds governing `DeltaE_2000` pixel drift outside an edit mask (`1 - M`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UntouchedDriftConfig {
    /// Maximum allowed mean `DeltaE_2000` across untouched pixels.
    pub max_mean_delta_e2000: f64,
    /// Maximum allowed 95th-percentile `DeltaE_2000` across untouched pixels.
    pub max_p95_delta_e2000: f64,
    /// Per-pixel `DeltaE_2000` threshold used for `fraction_exceeding_threshold` and max gate.
    pub max_pixel_delta_e2000: f64,
    /// Maximum allowed fraction (`0.0..=1.0`) of untouched pixels exceeding `max_pixel_delta_e2000`.
    pub max_fraction_exceeding: f64,
    /// Mask value threshold at or below which a pixel is considered untouched (`1e-4`).
    pub untouched_mask_epsilon: f32,
}

impl Default for UntouchedDriftConfig {
    fn default() -> Self {
        Self {
            max_mean_delta_e2000: 2.0,
            max_p95_delta_e2000: 5.0,
            max_pixel_delta_e2000: 10.0,
            max_fraction_exceeding: 0.01,
            untouched_mask_epsilon: 1e-4,
        }
    }
}

/// Pass/fail untouched-region (`1 - M`) `DeltaE_2000` drift report.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UntouchedDriftReport {
    /// `true` when untouched pixels remain within mean, P95, max, and fraction bounds.
    pub passed: bool,
    /// Count of untouched pixels evaluated (`mask[i] <= untouched_mask_epsilon`).
    pub untouched_pixel_count: usize,
    /// Mean `DeltaE_2000` across untouched pixels.
    pub mean_delta_e2000: f64,
    /// 95th-percentile `DeltaE_2000` across untouched pixels.
    pub p95_delta_e2000: f64,
    /// Maximum `DeltaE_2000` across untouched pixels.
    pub max_delta_e2000: f64,
    /// Fraction of untouched pixels exceeding `threshold_delta_e2000`.
    pub fraction_exceeding_threshold: f64,
    /// Configured `max_pixel_delta_e2000` threshold.
    pub threshold_delta_e2000: f64,
    /// Human-readable failure reasons.
    pub reasons: Vec<String>,
}

// === 5. Sparkle / Yellow-Dot Creep ===

/// Configuration for the high-pass Difference-of-Gaussians (DoG) yellow-sparkle detector.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SparkleConfig {
    /// Inner Gaussian blur sigma (`0.8 px`).
    pub dog_sigma_small: f64,
    /// Outer Gaussian blur sigma (`2.4 px`).
    pub dog_sigma_large: f64,
    /// Minimum positive DoG yellow-excess residual (`0.06`).
    pub min_dog_residual: f64,
    /// Minimum CIELAB `b*` coordinate (`22.0`) indicating yellow bias.
    pub min_yellow_b_star: f64,
    /// Minimum CIELAB chroma `C*` (`28.0`) indicating saturated yellow.
    pub min_yellow_chroma: f64,
    /// Minimum CIELAB hue angle in degrees (`60.0`) for yellow sector.
    pub yellow_hue_min_deg: f64,
    /// Maximum CIELAB hue angle in degrees (`125.0`) for yellow sector.
    pub yellow_hue_max_deg: f64,
    /// Minimum connected blob area in pixels (`1`).
    pub min_blob_area_px: usize,
    /// Maximum connected blob area in pixels (`64`).
    pub max_blob_area_px: usize,
}

impl Default for SparkleConfig {
    fn default() -> Self {
        Self {
            dog_sigma_small: 0.8,
            dog_sigma_large: 2.4,
            min_dog_residual: 0.06,
            min_yellow_b_star: 22.0,
            min_yellow_chroma: 28.0,
            yellow_hue_min_deg: 60.0,
            yellow_hue_max_deg: 125.0,
            min_blob_area_px: DEFAULT_SPARKLE_MIN_AREA_PX,
            max_blob_area_px: DEFAULT_SPARKLE_MAX_AREA_PX,
        }
    }
}

/// Detected isolated yellow-sparkle blob outside allowed masks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SparkleBlob {
    /// Tight pixel bounding box of the connected blob.
    pub bounds: PixelRect,
    /// Connected pixel area (`1..=64 px^2`).
    pub area_px: usize,
    /// Peak high-pass DoG yellow-excess residual inside the blob.
    pub peak_residual: f64,
}

/// Pass/fail report from the high-pass DoG yellow-sparkle detector.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SparkleReport {
    /// `true` when zero isolated yellow-dot blobs are flagged outside allowed masks.
    pub passed: bool,
    /// Flagged connected yellow-dot blobs.
    pub flagged_blobs: Vec<SparkleBlob>,
    /// Total pixel count across all flagged blobs.
    pub total_flagged_pixels: usize,
    /// Human-readable failure reasons.
    pub reasons: Vec<String>,
}

// === 6. Combined ColorQaReport & Heatmap Kinds ===

/// Combined fail-closed Color QA report across exposure, palette, untouched drift, and sparkles.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ColorQaReport {
    /// `true` only when every evaluated gate passed.
    pub passed: bool,
    /// Optional exposure/shadow-crush check result.
    pub exposure: Option<ExposureReport>,
    /// Per-subject `PaletteLock` adherence & bleed check results.
    pub palette_adherence: Vec<PaletteAdherenceReport>,
    /// Optional untouched-region (`1 - M`) drift check result.
    pub untouched_drift: Option<UntouchedDriftReport>,
    /// Optional high-pass DoG yellow-sparkle check result.
    pub sparkle: Option<SparkleReport>,
    /// Maximum `DeltaE_2000` observed across palette and untouched-drift checks.
    pub max_delta_e2000: f64,
    /// Concise summary notes suitable for `RunQaReports.structural_checks`.
    pub notes: Vec<String>,
}

/// Diagnostic heatmap overlay mode exported for the Web Canvas UI inspector.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HeatmapKind {
    /// Per-pixel `DeltaE_2000` drift across the image.
    Drift,
    /// `PaletteLock` deviation / forbidden-hue bleed inside ROIs.
    Bleed,
    /// High-pass DoG yellow-sparkle residual overlay.
    Sparkle,
    /// Out-of-ROI / untouched-region (`1 - M`) drift overlay.
    OutOfRoi,
}

/// Stateless, fail-closed OKLab / CIELAB `DeltaE_2000` Color QA engine.
///
/// Whole-image QA methods borrow immutable `&RgbaImage` inputs and never mutate
/// or re-grade whole images (`ADR 0005`).
#[derive(Debug, Clone, Default)]
pub struct ColorQaEngine;

impl ColorQaEngine {
    /// Construct a new `ColorQaEngine`.
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    /// Compute interior CIE `L*` statistics (`mean`, `P10`, `P90`, clipped black ratio).
    pub fn compute_exposure_metrics(
        &self,
        image: &RgbaImage,
        config: &ExposureQaConfig,
    ) -> Result<ExposureMetrics, CoreError> {
        let w = image.width();
        let h = image.height();
        if w == 0 || h == 0 {
            return Err(CoreError::ColorQaFailed(
                "cannot evaluate exposure on a 0-pixel image".to_string(),
            ));
        }

        let inset = if w > config.border_inset_px * 2 && h > config.border_inset_px * 2 {
            config.border_inset_px
        } else {
            0
        };

        let mut l_values = Vec::with_capacity(((w - 2 * inset) * (h - 2 * inset)) as usize);
        let mut clipped_count = 0usize;

        for y in inset..(h - inset) {
            for x in inset..(w - inset) {
                let px = image.get_pixel(x, y).0;
                if px[3] == 0 {
                    continue;
                }
                let lab = srgb_u8_to_lab([px[0], px[1], px[2]]);
                let l_star = f64::from(lab.l);
                if l_star < config.clipped_black_l_star_threshold {
                    clipped_count += 1;
                }
                l_values.push(l_star);
            }
        }

        if l_values.is_empty() {
            return Err(CoreError::ColorQaFailed(
                "exposure evaluation found zero opaque interior pixels".to_string(),
            ));
        }

        let count = l_values.len();
        let sum: f64 = l_values.iter().sum();
        let mean_l_star = sum / count as f64;
        l_values.sort_by(f64::total_cmp);
        let p10_l_star = percentile_sorted(&l_values, 0.10);
        let p90_l_star = percentile_sorted(&l_values, 0.90);
        let clipped_black_ratio = clipped_count as f64 / count as f64;

        Ok(ExposureMetrics {
            mean_l_star,
            p10_l_star,
            p90_l_star,
            clipped_black_ratio,
            pixel_count: count,
        })
    }

    /// Evaluate exposure and shadow-crush drift against `Atmosphere.exposure_floor`.
    pub fn evaluate_exposure(
        &self,
        image: &RgbaImage,
        floor: ExposureFloor,
        config: &ExposureQaConfig,
    ) -> Result<ExposureReport, CoreError> {
        let metrics = self.compute_exposure_metrics(image, config)?;
        let mut reasons = Vec::new();

        if metrics.mean_l_star < floor.min_mean_l_star {
            reasons.push(format!(
                "interior mean L* {:.2} is below Atmosphere.exposure_floor.min_mean_l_star {:.2}",
                metrics.mean_l_star, floor.min_mean_l_star
            ));
        }
        if metrics.p10_l_star < floor.min_p10_l_star {
            reasons.push(format!(
                "interior P10 L* {:.2} is below Atmosphere.exposure_floor.min_p10_l_star {:.2}",
                metrics.p10_l_star, floor.min_p10_l_star
            ));
        }
        if metrics.clipped_black_ratio > config.max_clipped_black_ratio {
            reasons.push(format!(
                "clipped shadow black ratio (L* < {:.1}) {:.3} exceeds max {:.3}",
                config.clipped_black_l_star_threshold,
                metrics.clipped_black_ratio,
                config.max_clipped_black_ratio
            ));
        }
        if let Some(anchor) = config.anchor_exposure {
            let mean_drift = (metrics.mean_l_star - anchor.mean_l_star).abs();
            if mean_drift > config.anchor_mean_l_star_tolerance {
                reasons.push(format!(
                    "mean L* drift {mean_drift:.2} relative to anchor {:.2} exceeds tolerance {:.2}",
                    anchor.mean_l_star, config.anchor_mean_l_star_tolerance
                ));
            }
            let p10_drift = (metrics.p10_l_star - anchor.p10_l_star).abs();
            if p10_drift > config.anchor_p10_l_star_tolerance {
                reasons.push(format!(
                    "P10 L* drift {p10_drift:.2} relative to anchor {:.2} exceeds tolerance {:.2}",
                    anchor.p10_l_star, config.anchor_p10_l_star_tolerance
                ));
            }
        }

        Ok(ExposureReport {
            passed: reasons.is_empty(),
            metrics,
            floor,
            reasons,
        })
    }

    /// Run deterministic OKLab k-means (`K = 6`) inside `Subject` ROI(s) and check `PaletteLock`
    /// adherence, lightness/chroma shift, and forbidden-hue sector intrusions.
    pub fn evaluate_palette_adherence(
        &self,
        image: &RgbaImage,
        spec: &SubjectRoiSpec<'_>,
        config: &PaletteQaConfig,
    ) -> Result<(PaletteAdherenceReport, Vec<f64>), CoreError> {
        let w = image.width();
        let h = image.height();
        if w == 0 || h == 0 {
            return Err(CoreError::ColorQaFailed(
                "cannot evaluate PaletteLock adherence on a 0-pixel image".to_string(),
            ));
        }
        if spec.palette_lock.swatches.is_empty() {
            return Err(CoreError::ColorQaFailed(format!(
                "Subject '{}' has empty PaletteLock swatches",
                spec.subject_slug
            )));
        }
        if config.kmeans_k == 0 {
            return Err(CoreError::ColorQaFailed(
                "PaletteQaConfig.kmeans_k must be >= 1".to_string(),
            ));
        }

        // Resolve swatch reference colors in CIELAB.
        let mut swatches_lab = Vec::with_capacity(spec.palette_lock.swatches.len());
        for sw in &spec.palette_lock.swatches {
            let rgb = parse_hex_rgb(&sw.color_hex)?;
            swatches_lab.push((sw.name.clone(), sw.color_hex.clone(), srgb_u8_to_lab(rgb)));
        }

        // Build ROI inclusion mask.
        let total = (w as usize) * (h as usize);
        let mut in_roi = vec![false; total];
        if spec.rois.is_empty() {
            in_roi.fill(true);
        } else {
            for rect in &spec.rois {
                if rect.width == 0 || rect.height == 0 || rect.right() > w || rect.bottom() > h {
                    return Err(CoreError::ColorQaFailed(format!(
                        "Subject '{}' ROI {:?} is empty or out of bounds {}x{}",
                        spec.subject_slug, rect, w, h
                    )));
                }
                for y in rect.y..rect.bottom() {
                    for x in rect.x..rect.right() {
                        in_roi[(y as usize) * (w as usize) + (x as usize)] = true;
                    }
                }
            }
        }

        let mut samples_oklab = Vec::new();
        let mut bleed_map = vec![0.0f64; total];
        let mut sector_counts = vec![0usize; config.forbidden_hue_sectors.len()];
        let mut roi_pixel_count = 0usize;

        for y in 0..h {
            for x in 0..w {
                let idx = (y as usize) * (w as usize) + (x as usize);
                if !in_roi[idx] {
                    continue;
                }
                let px = image.get_pixel(x, y).0;
                if px[3] == 0 {
                    continue;
                }
                roi_pixel_count += 1;
                let rgb = [px[0], px[1], px[2]];
                let oklab = srgb_u8_to_oklab(rgb);
                samples_oklab.push(oklab);

                let lab = srgb_u8_to_lab(rgb);
                let min_de = swatches_lab
                    .iter()
                    .map(|(_, _, sw_lab)| delta_e_2000(lab, *sw_lab))
                    .fold(f64::INFINITY, f64::min);
                bleed_map[idx] = min_de;

                let chroma = lab_chroma(lab);
                let hue = lab_hue_deg(lab);
                for (s_idx, sector) in config.forbidden_hue_sectors.iter().enumerate() {
                    if sector.contains(hue, chroma) {
                        sector_counts[s_idx] += 1;
                    }
                }
            }
        }

        if roi_pixel_count == 0 {
            return Err(CoreError::ColorQaFailed(format!(
                "Subject '{}' ROI contains zero opaque pixels",
                spec.subject_slug
            )));
        }

        let raw_clusters =
            deterministic_oklab_kmeans(&samples_oklab, config.kmeans_k, config.kmeans_max_iters);

        let mut clusters = Vec::with_capacity(raw_clusters.len());
        let mut max_swatch_delta_e2000 = 0.0f64;
        let mut weighted_de_sum = 0.0f64;
        let mut reasons = Vec::new();

        for (centroid, count) in raw_clusters {
            let weight = count as f64 / roi_pixel_count as f64;
            let centroid_lab = oklab_to_lab(centroid);
            let centroid_hex = format_hex_rgb(oklab_to_srgb_u8(centroid));

            let (closest_name, closest_hex, closest_lab, best_de) = swatches_lab
                .iter()
                .map(|(name, hex, sw_lab)| {
                    let de = delta_e_2000(centroid_lab, *sw_lab);
                    (name, hex, *sw_lab, de)
                })
                .min_by(|a, b| a.3.total_cmp(&b.3))
                .expect("swatches_lab is non-empty");

            let lightness_shift = (f64::from(centroid_lab.l) - f64::from(closest_lab.l)).abs();
            let chroma_shift = (lab_chroma(centroid_lab) - lab_chroma(closest_lab)).abs();
            weighted_de_sum += weight * best_de;

            if weight >= config.min_cluster_weight {
                max_swatch_delta_e2000 = max_swatch_delta_e2000.max(best_de);
                if best_de > config.max_swatch_delta_e2000 {
                    reasons.push(format!(
                        "Subject '{}' cluster {} ({:.1}% of ROI) deviates DeltaE_2000={:.2} from closest PaletteLock swatch '{}' ({}, max {:.2})",
                        spec.subject_slug,
                        centroid_hex,
                        weight * 100.0,
                        best_de,
                        closest_name,
                        closest_hex,
                        config.max_swatch_delta_e2000
                    ));
                }
                if lightness_shift > config.max_lightness_shift {
                    reasons.push(format!(
                        "Subject '{}' cluster {} lightness shift |dL*|={:.2} exceeds max {:.2} vs '{}'",
                        spec.subject_slug,
                        centroid_hex,
                        lightness_shift,
                        config.max_lightness_shift,
                        closest_name
                    ));
                }
                if chroma_shift > config.max_chroma_shift {
                    reasons.push(format!(
                        "Subject '{}' cluster {} chroma shift |dC*|={:.2} exceeds max {:.2} vs '{}'",
                        spec.subject_slug,
                        centroid_hex,
                        chroma_shift,
                        config.max_chroma_shift,
                        closest_name
                    ));
                }
            }

            clusters.push(OklabClusterSummary {
                centroid_oklab: [centroid.l, centroid.a, centroid.b],
                centroid_hex,
                weight,
                pixel_count: count,
                closest_swatch_name: closest_name.clone(),
                closest_swatch_hex: closest_hex.clone(),
                delta_e2000: best_de,
                lightness_shift,
                chroma_shift,
            });
        }

        let mut forbidden_hue_intrusions = Vec::new();
        for (s_idx, sector) in config.forbidden_hue_sectors.iter().enumerate() {
            let count = sector_counts[s_idx];
            let frac = count as f64 / roi_pixel_count as f64;
            if frac > sector.max_fraction {
                forbidden_hue_intrusions.push(ForbiddenHueIntrusion {
                    sector_name: sector.name.clone(),
                    fraction: frac,
                    pixel_count: count,
                });
                reasons.push(format!(
                    "Subject '{}' forbidden hue sector '{}' covers {:.2}% of ROI (max {:.2}%)",
                    spec.subject_slug,
                    sector.name,
                    frac * 100.0,
                    sector.max_fraction * 100.0
                ));
            }
        }

        let report = PaletteAdherenceReport {
            passed: reasons.is_empty(),
            subject_slug: spec.subject_slug.to_string(),
            clusters,
            max_swatch_delta_e2000,
            weighted_mean_delta_e2000: weighted_de_sum,
            forbidden_hue_intrusions,
            reasons,
        };
        Ok((report, bleed_map))
    }

    /// Measure per-pixel `DeltaE_2000` outside an edit mask (`1 - M`) and return both the
    /// structured report and the full-canvas per-pixel `DeltaE_2000` map (zero inside mask).
    pub fn evaluate_untouched_drift(
        &self,
        before: &RgbaImage,
        after: &RgbaImage,
        edit_mask: &[f32],
        config: &UntouchedDriftConfig,
    ) -> Result<(UntouchedDriftReport, Vec<f64>), CoreError> {
        let w = before.width();
        let h = before.height();
        if w == 0 || h == 0 {
            return Err(CoreError::ColorQaFailed(
                "cannot evaluate untouched-region drift on a 0-pixel image".to_string(),
            ));
        }
        if (after.width(), after.height()) != (w, h) {
            return Err(CoreError::ColorQaFailed(format!(
                "before ({w}x{h}) and after ({}x{}) image dimensions must match",
                after.width(),
                after.height()
            )));
        }
        let expected_len = (w as usize) * (h as usize);
        if edit_mask.len() != expected_len {
            return Err(CoreError::ColorQaFailed(format!(
                "edit_mask length {} does not match image pixel count {expected_len}",
                edit_mask.len()
            )));
        }

        let mut delta_map = vec![0.0f64; expected_len];
        let mut untouched_deltas = Vec::new();
        let mut exceeding_count = 0usize;
        let mut max_de = 0.0f64;
        let mut sum_de = 0.0f64;

        for y in 0..h {
            for x in 0..w {
                let idx = (y as usize) * (w as usize) + (x as usize);
                let m = edit_mask[idx];
                if !m.is_finite() || !(0.0..=1.0).contains(&m) {
                    return Err(CoreError::ColorQaFailed(format!(
                        "edit_mask[{idx}]={m} must be finite within [0.0, 1.0]"
                    )));
                }
                if m <= config.untouched_mask_epsilon {
                    let pa = before.get_pixel(x, y).0;
                    let pb = after.get_pixel(x, y).0;
                    let de = srgb_delta_e_2000([pa[0], pa[1], pa[2]], [pb[0], pb[1], pb[2]]);
                    delta_map[idx] = de;
                    untouched_deltas.push(de);
                    sum_de += de;
                    max_de = max_de.max(de);
                    if de > config.max_pixel_delta_e2000 {
                        exceeding_count += 1;
                    }
                }
            }
        }

        if untouched_deltas.is_empty() {
            return Err(CoreError::ColorQaFailed(
                "edit_mask covers 100% of pixels; zero untouched (1 - M) pixels remain".to_string(),
            ));
        }

        let count = untouched_deltas.len();
        let mean_delta_e2000 = sum_de / count as f64;
        untouched_deltas.sort_by(f64::total_cmp);
        let p95_delta_e2000 = percentile_sorted(&untouched_deltas, 0.95);
        let fraction_exceeding_threshold = exceeding_count as f64 / count as f64;

        let mut reasons = Vec::new();
        if mean_delta_e2000 > config.max_mean_delta_e2000 {
            reasons.push(format!(
                "untouched-region mean DeltaE_2000={mean_delta_e2000:.3} exceeds max {:.3}",
                config.max_mean_delta_e2000
            ));
        }
        if p95_delta_e2000 > config.max_p95_delta_e2000 {
            reasons.push(format!(
                "untouched-region P95 DeltaE_2000={p95_delta_e2000:.3} exceeds max {:.3}",
                config.max_p95_delta_e2000
            ));
        }
        if max_de > config.max_pixel_delta_e2000 {
            reasons.push(format!(
                "untouched-region max DeltaE_2000={max_de:.3} exceeds max {:.3}",
                config.max_pixel_delta_e2000
            ));
        }
        if fraction_exceeding_threshold > config.max_fraction_exceeding {
            reasons.push(format!(
                "untouched-region fraction exceeding DeltaE_2000>{:.2} is {:.4} (max {:.4})",
                config.max_pixel_delta_e2000,
                fraction_exceeding_threshold,
                config.max_fraction_exceeding
            ));
        }

        let report = UntouchedDriftReport {
            passed: reasons.is_empty(),
            untouched_pixel_count: count,
            mean_delta_e2000,
            p95_delta_e2000,
            max_delta_e2000: max_de,
            fraction_exceeding_threshold,
            threshold_delta_e2000: config.max_pixel_delta_e2000,
            reasons,
        };
        Ok((report, delta_map))
    }

    /// Detect isolated saturated yellow blobs (`1..=64 px^2`) outside `allowed_mask`
    /// using a high-pass Difference-of-Gaussians (DoG) yellow-excess residual.
    pub fn detect_sparkles(
        &self,
        image: &RgbaImage,
        allowed_mask: Option<&[f32]>,
        config: &SparkleConfig,
    ) -> Result<(SparkleReport, Vec<f64>), CoreError> {
        let w = image.width() as usize;
        let h = image.height() as usize;
        if w == 0 || h == 0 {
            return Err(CoreError::ColorQaFailed(
                "cannot run sparkle detector on a 0-pixel image".to_string(),
            ));
        }
        let total = w * h;
        if let Some(mask) = allowed_mask
            && mask.len() != total
        {
            return Err(CoreError::ColorQaFailed(format!(
                "allowed_mask length {} does not match image pixel count {total}",
                mask.len()
            )));
        }

        // Compute normalized yellow-excess scalar per pixel: ((R + G) / 2 - B).max(0).
        let mut yellow_excess = vec![0.0f64; total];
        for y in 0..h {
            for x in 0..w {
                let px = image.get_pixel(x as u32, y as u32).0;
                if px[3] == 0 {
                    continue;
                }
                let r = f64::from(px[0]) / 255.0;
                let g = f64::from(px[1]) / 255.0;
                let b = f64::from(px[2]) / 255.0;
                yellow_excess[y * w + x] = ((r + g) * 0.5 - b).max(0.0);
            }
        }

        let blur_small = gaussian_blur_2d(&yellow_excess, w, h, config.dog_sigma_small);
        let blur_large = gaussian_blur_2d(&yellow_excess, w, h, config.dog_sigma_large);

        let mut candidate = vec![false; total];
        let mut residual_map = vec![0.0f64; total];

        for y in 0..h {
            for x in 0..w {
                let idx = y * w + x;
                if let Some(mask) = allowed_mask
                    && mask[idx] > 0.5
                {
                    continue;
                }
                let px = image.get_pixel(x as u32, y as u32).0;
                if px[3] == 0 {
                    continue;
                }
                let dog = (blur_small[idx] - blur_large[idx]).max(0.0);
                if dog < config.min_dog_residual {
                    continue;
                }

                let lab = srgb_u8_to_lab([px[0], px[1], px[2]]);
                let chroma = lab_chroma(lab);
                let hue = lab_hue_deg(lab);
                if f64::from(lab.b) >= config.min_yellow_b_star
                    && chroma >= config.min_yellow_chroma
                    && hue >= config.yellow_hue_min_deg
                    && hue <= config.yellow_hue_max_deg
                {
                    candidate[idx] = true;
                    residual_map[idx] = dog;
                }
            }
        }

        // Connected component labeling (8-connected) to isolate 1..=64 px^2 blobs.
        let mut visited = vec![false; total];
        let mut queue = VecDeque::new();
        let mut flagged_blobs = Vec::new();
        let mut flagged_pixel_mask = vec![0.0f64; total];
        let mut total_flagged_pixels = 0usize;

        for sy in 0..h {
            for sx in 0..w {
                let start = sy * w + sx;
                if !candidate[start] || visited[start] {
                    continue;
                }
                visited[start] = true;
                queue.push_back((sx, sy));

                let mut coords = Vec::new();
                let mut min_x = sx;
                let mut min_y = sy;
                let mut max_x = sx;
                let mut max_y = sy;
                let mut peak = 0.0f64;

                while let Some((cx, cy)) = queue.pop_front() {
                    let cidx = cy * w + cx;
                    coords.push(cidx);
                    min_x = min_x.min(cx);
                    min_y = min_y.min(cy);
                    max_x = max_x.max(cx);
                    max_y = max_y.max(cy);
                    peak = peak.max(residual_map[cidx]);

                    let y0 = cy.saturating_sub(1);
                    let y1 = (cy + 1).min(h - 1);
                    let x0 = cx.saturating_sub(1);
                    let x1 = (cx + 1).min(w - 1);
                    for ny in y0..=y1 {
                        for nx in x0..=x1 {
                            let nidx = ny * w + nx;
                            if candidate[nidx] && !visited[nidx] {
                                visited[nidx] = true;
                                queue.push_back((nx, ny));
                            }
                        }
                    }
                }

                let area = coords.len();
                if area >= config.min_blob_area_px && area <= config.max_blob_area_px {
                    for cidx in coords {
                        flagged_pixel_mask[cidx] = residual_map[cidx];
                    }
                    total_flagged_pixels += area;
                    flagged_blobs.push(SparkleBlob {
                        bounds: PixelRect {
                            x: min_x as u32,
                            y: min_y as u32,
                            width: (max_x - min_x + 1) as u32,
                            height: (max_y - min_y + 1) as u32,
                        },
                        area_px: area,
                        peak_residual: peak,
                    });
                }
            }
        }

        let mut reasons = Vec::new();
        if !flagged_blobs.is_empty() {
            reasons.push(format!(
                "detected {} isolated yellow-sparkle blob(s) ({total_flagged_pixels} px^2 total) outside allowed masks",
                flagged_blobs.len()
            ));
        }

        Ok((
            SparkleReport {
                passed: flagged_blobs.is_empty(),
                flagged_blobs,
                total_flagged_pixels,
                reasons,
            },
            flagged_pixel_mask,
        ))
    }

    /// Run a combined fail-closed whole-image Color QA evaluation without mutating `image`.
    #[allow(clippy::too_many_arguments)]
    pub fn evaluate_whole_image(
        &self,
        image: &RgbaImage,
        exposure_floor: Option<(ExposureFloor, &ExposureQaConfig)>,
        subjects: &[SubjectRoiSpec<'_>],
        palette_config: &PaletteQaConfig,
        untouched_comparison: Option<(&RgbaImage, &[f32], &UntouchedDriftConfig)>,
        sparkle_check: Option<(Option<&[f32]>, &SparkleConfig)>,
    ) -> Result<ColorQaReport, CoreError> {
        let mut passed = true;
        let mut notes = Vec::new();
        let mut max_delta_e2000 = 0.0f64;

        let exposure = if let Some((floor, exp_cfg)) = exposure_floor {
            let rep = self.evaluate_exposure(image, floor, exp_cfg)?;
            if !rep.passed {
                passed = false;
                notes.extend(rep.reasons.iter().cloned());
            } else {
                notes.push(format!(
                    "exposure OK (mean L*={:.1}, P10 L*={:.1})",
                    rep.metrics.mean_l_star, rep.metrics.p10_l_star
                ));
            }
            Some(rep)
        } else {
            None
        };

        let mut palette_adherence = Vec::with_capacity(subjects.len());
        for spec in subjects {
            let (rep, _) = self.evaluate_palette_adherence(image, spec, palette_config)?;
            max_delta_e2000 = max_delta_e2000.max(rep.max_swatch_delta_e2000);
            if !rep.passed {
                passed = false;
                notes.extend(rep.reasons.iter().cloned());
            } else {
                notes.push(format!(
                    "palette '{}' OK (max DeltaE_2000={:.2})",
                    rep.subject_slug, rep.max_swatch_delta_e2000
                ));
            }
            palette_adherence.push(rep);
        }

        let untouched_drift = if let Some((before, mask, drift_cfg)) = untouched_comparison {
            let (rep, _) = self.evaluate_untouched_drift(before, image, mask, drift_cfg)?;
            max_delta_e2000 = max_delta_e2000.max(rep.max_delta_e2000);
            if !rep.passed {
                passed = false;
                notes.extend(rep.reasons.iter().cloned());
            } else {
                notes.push(format!(
                    "untouched drift OK (mean={:.2}, P95={:.2}, max={:.2})",
                    rep.mean_delta_e2000, rep.p95_delta_e2000, rep.max_delta_e2000
                ));
            }
            Some(rep)
        } else {
            None
        };

        let sparkle = if let Some((allowed_mask, sparkle_cfg)) = sparkle_check {
            let (rep, _) = self.detect_sparkles(image, allowed_mask, sparkle_cfg)?;
            if !rep.passed {
                passed = false;
                notes.extend(rep.reasons.iter().cloned());
            } else {
                notes.push("sparkle detector OK (0 blobs)".to_string());
            }
            Some(rep)
        } else {
            None
        };

        Ok(ColorQaReport {
            passed,
            exposure,
            palette_adherence,
            untouched_drift,
            sparkle,
            max_delta_e2000,
            notes,
        })
    }

    /// Render a diagnostic RGBA overlay heatmap for `drift`, `bleed`, `sparkle`, or `out_of_roi`.
    pub fn render_heatmap_rgba(
        &self,
        width: u32,
        height: u32,
        values: &[f64],
        threshold: f64,
        max_scale: f64,
        kind: HeatmapKind,
    ) -> Result<RgbaImage, CoreError> {
        let expected = (width as usize) * (height as usize);
        if width == 0 || height == 0 || values.len() != expected {
            return Err(CoreError::ColorQaFailed(format!(
                "heatmap value slice length {} does not match {width}x{height}",
                values.len()
            )));
        }
        let span = (max_scale - threshold).max(1e-6);
        let mut out = RgbaImage::new(width, height);

        for y in 0..height {
            for x in 0..width {
                let idx = (y as usize) * (width as usize) + (x as usize);
                let val = values[idx];
                if !val.is_finite() || val <= threshold {
                    out.put_pixel(x, y, Rgba([0, 0, 0, 0]));
                    continue;
                }
                let t = ((val - threshold) / span).clamp(0.0, 1.0);
                let alpha = (96.0 + 144.0 * t).round() as u8;
                let rgba = match kind {
                    HeatmapKind::Drift | HeatmapKind::OutOfRoi => {
                        // Amber -> Crimson -> Magenta ramp
                        let r = 255u8;
                        let g = (190.0 * (1.0 - t)).round() as u8;
                        let b = if t > 0.65 {
                            ((t - 0.65) / 0.35 * 200.0).round() as u8
                        } else {
                            20
                        };
                        [r, g, b, alpha]
                    }
                    HeatmapKind::Bleed => {
                        // Violet -> Hot Red ramp
                        let r = (180.0 + 75.0 * t).round() as u8;
                        let g = 40u8;
                        let b = (220.0 * (1.0 - t)).round() as u8;
                        [r, g, b, alpha]
                    }
                    HeatmapKind::Sparkle => {
                        // Electric Yellow -> Red-Orange highlight
                        let r = 255u8;
                        let g = (240.0 * (1.0 - 0.65 * t)).round() as u8;
                        let b = 0u8;
                        [r, g, b, 235]
                    }
                };
                out.put_pixel(x, y, Rgba(rgba));
            }
        }
        Ok(out)
    }

    /// Encode a diagnostic RGBA heatmap overlay as PNG bytes.
    pub fn render_heatmap_png(
        &self,
        width: u32,
        height: u32,
        values: &[f64],
        threshold: f64,
        max_scale: f64,
        kind: HeatmapKind,
    ) -> Result<Vec<u8>, CoreError> {
        let rgba = self.render_heatmap_rgba(width, height, values, threshold, max_scale, kind)?;
        let mut buf = Vec::new();
        DynamicImage::ImageRgba8(rgba)
            .write_to(&mut Cursor::new(&mut buf), ImageFormat::Png)
            .map_err(|err| CoreError::ColorQaFailed(format!("encoding heatmap PNG: {err}")))?;
        Ok(buf)
    }

    /// Convenience helper producing an `out_of_roi` or `drift` PNG heatmap from `before`, `after`,
    /// and an edit mask (`1 - M`).
    pub fn export_drift_heatmap_png(
        &self,
        before: &RgbaImage,
        after: &RgbaImage,
        edit_mask: &[f32],
        config: &UntouchedDriftConfig,
        kind: HeatmapKind,
    ) -> Result<Vec<u8>, CoreError> {
        let (_, delta_map) = self.evaluate_untouched_drift(before, after, edit_mask, config)?;
        self.render_heatmap_png(
            before.width(),
            before.height(),
            &delta_map,
            1.0,
            config.max_pixel_delta_e2000.max(15.0),
            kind,
        )
    }

    /// Compute binned CIE `L*` histogram and downsampled OKLab vectorscope points for UI inspection.
    pub fn compute_l_star_histogram_and_vectorscope(
        &self,
        image: &RgbaImage,
        max_samples: usize,
    ) -> Result<ColorInspectionTelemetry, CoreError> {
        compute_l_star_histogram_and_vectorscope(image, max_samples)
    }
}

/// Single bin of a CIE `L*` luminance histogram (`0..=100`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LStarHistogramBin {
    /// Zero-based bin index (`0..DEFAULT_L_STAR_HISTOGRAM_BINS`).
    pub bin_index: usize,
    /// Inclusive lower CIE `L*` bound for this bin.
    pub min_l_star: f64,
    /// Upper CIE `L*` bound for this bin.
    pub max_l_star: f64,
    /// Number of opaque pixels falling into this bin.
    pub count: usize,
    /// Fraction of opaque pixels (`0.0..=1.0`) falling into this bin.
    pub fraction: f64,
}

/// Single downsampled OKLab + CIE `L*` point rendered in the Web Canvas vectorscope.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OklabVectorscopePoint {
    /// OKLab `L` perceptual lightness (`0.0..=1.0`).
    pub oklab_l: f32,
    /// OKLab `a` green-red axis.
    pub a: f32,
    /// OKLab `b` blue-yellow axis.
    pub b: f32,
    /// CIE `L*` luminance (`0.0..=100.0`).
    pub l_star: f64,
    /// Uppercase `#RRGGBB` sRGB representation of the sampled pixel.
    pub hex: String,
}

/// Combined CIE `L*` histogram and OKLab vectorscope payload computed in Rust core.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ColorInspectionTelemetry {
    /// Source image width in pixels.
    pub width: u32,
    /// Source image height in pixels.
    pub height: u32,
    /// Total number of non-transparent pixels evaluated (`alpha > 0`).
    pub opaque_pixel_count: usize,
    /// 20-bin CIE `L*` histogram across `0.0..=100.0`.
    pub l_star_bins: Vec<LStarHistogramBin>,
    /// Deterministically downsampled OKLab `(a, b, lStar, hex)` vectorscope points.
    pub vectorscope_points: Vec<OklabVectorscopePoint>,
}

/// Compute a 20-bin CIE `L*` histogram (`0..100`) and deterministically downsampled OKLab
/// `(a*, b*, L*, hex)` vectorscope points for the Web Canvas Run Inspector.
pub fn compute_l_star_histogram_and_vectorscope(
    image: &RgbaImage,
    max_samples: usize,
) -> Result<ColorInspectionTelemetry, CoreError> {
    let width = image.width();
    let height = image.height();
    if width == 0 || height == 0 {
        return Err(CoreError::ColorQaFailed(
            "cannot compute L* histogram and vectorscope on a 0-pixel image".to_string(),
        ));
    }

    let bin_width = 100.0 / (DEFAULT_L_STAR_HISTOGRAM_BINS as f64);
    let mut counts = [0usize; DEFAULT_L_STAR_HISTOGRAM_BINS];
    let mut opaque_pixel_count = 0usize;

    for px in image.pixels() {
        if px.0[3] == 0 {
            continue;
        }
        opaque_pixel_count += 1;
        let lab = srgb_u8_to_lab([px.0[0], px.0[1], px.0[2]]);
        let l_star = f64::from(lab.l).clamp(0.0, 100.0);
        let bin = ((l_star / bin_width).floor() as usize).min(DEFAULT_L_STAR_HISTOGRAM_BINS - 1);
        counts[bin] += 1;
    }

    let l_star_bins = (0..DEFAULT_L_STAR_HISTOGRAM_BINS)
        .map(|bin_index| {
            let count = counts[bin_index];
            let fraction = if opaque_pixel_count > 0 {
                count as f64 / opaque_pixel_count as f64
            } else {
                0.0
            };
            LStarHistogramBin {
                bin_index,
                min_l_star: bin_index as f64 * bin_width,
                max_l_star: (bin_index + 1) as f64 * bin_width,
                count,
                fraction,
            }
        })
        .collect();

    let target_samples = if opaque_pixel_count == 0 || max_samples == 0 {
        0
    } else {
        max_samples.min(opaque_pixel_count)
    };
    let mut vectorscope_points = Vec::with_capacity(target_samples);

    if target_samples > 0 {
        let mut opaque_idx = 0usize;
        let mut sampled = 0usize;
        let mut next_pick = 0usize;

        for px in image.pixels() {
            if px.0[3] == 0 {
                continue;
            }
            if opaque_idx == next_pick {
                let rgb = [px.0[0], px.0[1], px.0[2]];
                let ok = srgb_u8_to_oklab(rgb);
                let lab = srgb_u8_to_lab(rgb);
                vectorscope_points.push(OklabVectorscopePoint {
                    oklab_l: ok.l,
                    a: ok.a,
                    b: ok.b,
                    l_star: f64::from(lab.l),
                    hex: format_hex_rgb(rgb),
                });
                sampled += 1;
                if sampled >= target_samples {
                    break;
                }
                next_pick = (sampled * opaque_pixel_count) / target_samples;
            }
            opaque_idx += 1;
        }
    }

    Ok(ColorInspectionTelemetry {
        width,
        height,
        opaque_pixel_count,
        l_star_bins,
        vectorscope_points,
    })
}

// === 7. Deterministic Internal Helpers ===

fn percentile_sorted(sorted: &[f64], q: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    if sorted.len() == 1 {
        return sorted[0];
    }
    let pos = q.clamp(0.0, 1.0) * (sorted.len() - 1) as f64;
    let lo = pos.floor() as usize;
    let hi = pos.ceil() as usize;
    let frac = pos - lo as f64;
    sorted[lo] * (1.0 - frac) + sorted[hi] * frac
}

fn oklab_sq_dist(a: Oklab<f32>, b: Oklab<f32>) -> f32 {
    let dl = a.l - b.l;
    let da = a.a - b.a;
    let db = a.b - b.b;
    dl * dl + da * da + db * db
}

/// Deterministic OKLab k-means using farthest-first initialization (tie-broken by scanline index).
fn deterministic_oklab_kmeans(
    samples: &[Oklab<f32>],
    k: usize,
    max_iters: usize,
) -> Vec<(Oklab<f32>, usize)> {
    if samples.is_empty() || k == 0 {
        return Vec::new();
    }
    let effective_k = k.min(samples.len());

    // Seed 0: deterministic sample closest to the global mean.
    let mut mean_l = 0.0f64;
    let mut mean_a = 0.0f64;
    let mut mean_b = 0.0f64;
    for s in samples {
        mean_l += f64::from(s.l);
        mean_a += f64::from(s.a);
        mean_b += f64::from(s.b);
    }
    let n = samples.len() as f64;
    let global_mean = Oklab::new(
        (mean_l / n) as f32,
        (mean_a / n) as f32,
        (mean_b / n) as f32,
    );

    let mut centers = Vec::with_capacity(effective_k);
    let mut first_idx = 0usize;
    let mut best_mean_dist = f32::INFINITY;
    for (idx, &s) in samples.iter().enumerate() {
        let d = oklab_sq_dist(s, global_mean);
        if d < best_mean_dist {
            best_mean_dist = d;
            first_idx = idx;
        }
    }
    centers.push(samples[first_idx]);

    let mut min_dist = vec![f32::INFINITY; samples.len()];
    for _ in 1..effective_k {
        let last_center = *centers.last().expect("at least one center");
        let mut farthest_idx = 0usize;
        let mut farthest_dist = -1.0f32;
        for (idx, &s) in samples.iter().enumerate() {
            let d = oklab_sq_dist(s, last_center);
            if d < min_dist[idx] {
                min_dist[idx] = d;
            }
            if min_dist[idx] > farthest_dist {
                farthest_dist = min_dist[idx];
                farthest_idx = idx;
            }
        }
        if farthest_dist <= 1e-10 {
            break;
        }
        centers.push(samples[farthest_idx]);
    }

    let c_count = centers.len();
    let mut assignments = vec![0usize; samples.len()];

    for _ in 0..max_iters.max(1) {
        let mut changed = false;
        let mut accum_l = vec![0.0f64; c_count];
        let mut accum_a = vec![0.0f64; c_count];
        let mut accum_b = vec![0.0f64; c_count];
        let mut counts = vec![0usize; c_count];

        for (i, &s) in samples.iter().enumerate() {
            let mut best_c = 0usize;
            let mut best_d = f32::INFINITY;
            for (c_idx, &center) in centers.iter().enumerate() {
                let d = oklab_sq_dist(s, center);
                if d < best_d {
                    best_d = d;
                    best_c = c_idx;
                }
            }
            if assignments[i] != best_c {
                assignments[i] = best_c;
                changed = true;
            }
            accum_l[best_c] += f64::from(s.l);
            accum_a[best_c] += f64::from(s.a);
            accum_b[best_c] += f64::from(s.b);
            counts[best_c] += 1;
        }

        for c_idx in 0..c_count {
            if counts[c_idx] > 0 {
                let cnt = counts[c_idx] as f64;
                centers[c_idx] = Oklab::new(
                    (accum_l[c_idx] / cnt) as f32,
                    (accum_a[c_idx] / cnt) as f32,
                    (accum_b[c_idx] / cnt) as f32,
                );
            }
        }

        if !changed {
            break;
        }
    }

    let mut final_counts = vec![0usize; c_count];
    for &c in &assignments {
        final_counts[c] += 1;
    }

    let mut out: Vec<(Oklab<f32>, usize)> = centers
        .into_iter()
        .zip(final_counts)
        .filter(|(_, cnt)| *cnt > 0)
        .collect();
    out.sort_by(|a, b| {
        b.1.cmp(&a.1)
            .then_with(|| a.0.l.total_cmp(&b.0.l))
            .then_with(|| a.0.a.total_cmp(&b.0.a))
            .then_with(|| a.0.b.total_cmp(&b.0.b))
    });
    out
}

/// Separable 2D Gaussian blur with clamp-to-edge boundary handling.
fn gaussian_blur_2d(input: &[f64], w: usize, h: usize, sigma: f64) -> Vec<f64> {
    if sigma <= 0.0 || w == 0 || h == 0 {
        return input.to_vec();
    }
    let radius = (sigma * 3.0).ceil() as isize;
    let mut kernel = Vec::with_capacity((2 * radius + 1) as usize);
    let two_sigma_sq = 2.0 * sigma * sigma;
    let mut sum = 0.0f64;
    for k in -radius..=radius {
        let val = (-((k * k) as f64) / two_sigma_sq).exp();
        kernel.push(val);
        sum += val;
    }
    for v in &mut kernel {
        *v /= sum;
    }

    let mut horiz = vec![0.0f64; w * h];
    for y in 0..h {
        for x in 0..w {
            let mut acc = 0.0f64;
            for (ki, k) in (-radius..=radius).enumerate() {
                let sx = (x as isize + k).clamp(0, w as isize - 1) as usize;
                acc += input[y * w + sx] * kernel[ki];
            }
            horiz[y * w + x] = acc;
        }
    }

    let mut out = vec![0.0f64; w * h];
    for y in 0..h {
        for x in 0..w {
            let mut acc = 0.0f64;
            for (ki, k) in (-radius..=radius).enumerate() {
                let sy = (y as isize + k).clamp(0, h as isize - 1) as usize;
                acc += horiz[sy * w + x] * kernel[ki];
            }
            out[y * w + x] = acc;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use image::ImageBuffer;
    use rstest::rstest;

    use super::*;
    use crate::subject::PaletteSwatch;

    fn solid_image(w: u32, h: u32, rgb: [u8; 3]) -> RgbaImage {
        ImageBuffer::from_pixel(w, h, Rgba([rgb[0], rgb[1], rgb[2], 255]))
    }

    fn paint_rect(img: &mut RgbaImage, x: u32, y: u32, w: u32, h: u32, rgb: [u8; 3]) {
        for py in y..(y + h).min(img.height()) {
            for px in x..(x + w).min(img.width()) {
                img.put_pixel(px, py, Rgba([rgb[0], rgb[1], rgb[2], 255]));
            }
        }
    }

    #[rstest]
    #[case::bright_daylight_passes([170, 175, 180], 30.0, 10.0, true)]
    #[case::crushed_shadows_fail_floor([6, 6, 8], 30.0, 10.0, false)]
    #[case::dim_midtone_fails_mean_floor([45, 45, 50], 35.0, 10.0, false)]
    fn exposure_floor_fails_closed_on_shadow_crush(
        #[case] fill: [u8; 3],
        #[case] min_mean: f64,
        #[case] min_p10: f64,
        #[case] expect_ok: bool,
    ) {
        let engine = ColorQaEngine::new();
        let img = solid_image(32, 32, fill);
        let floor = ExposureFloor {
            min_mean_l_star: min_mean,
            min_p10_l_star: min_p10,
        };
        let report = engine
            .evaluate_exposure(&img, floor, &ExposureQaConfig::default())
            .expect("exposure evaluation succeeds");
        assert_eq!(report.passed, expect_ok, "reasons: {:?}", report.reasons);
    }

    #[rstest]
    #[case::matching_swatches_pass([44, 62, 80], [184, 74, 57], true)]
    #[case::drifted_green_bleed_fails([40, 190, 50], [184, 74, 57], false)]
    fn palette_adherence_detects_swatch_drift_and_bleed(
        #[case] region_a: [u8; 3],
        #[case] region_b: [u8; 3],
        #[case] expect_ok: bool,
    ) {
        let engine = ColorQaEngine::new();
        let mut img = solid_image(64, 64, [37, 40, 48]);
        paint_rect(&mut img, 8, 8, 24, 48, region_a);
        paint_rect(&mut img, 32, 8, 24, 48, region_b);

        let lock = PaletteLock {
            swatches: vec![
                PaletteSwatch {
                    name: "coat".to_string(),
                    color_hex: "#2C3E50".to_string(),
                    surface_slot: Some("coat".to_string()),
                },
                PaletteSwatch {
                    name: "scarf".to_string(),
                    color_hex: "#B84A39".to_string(),
                    surface_slot: Some("scarf".to_string()),
                },
            ],
        };
        let spec = SubjectRoiSpec {
            subject_slug: "courier-ren",
            palette_lock: &lock,
            rois: vec![PixelRect {
                x: 8,
                y: 8,
                width: 48,
                height: 48,
            }],
        };

        let (report, bleed_map) = engine
            .evaluate_palette_adherence(&img, &spec, &PaletteQaConfig::default())
            .expect("palette evaluation succeeds");
        assert_eq!(report.passed, expect_ok, "reasons: {:?}", report.reasons);
        assert_eq!(bleed_map.len(), 64 * 64);
    }

    #[test]
    fn forbidden_hue_sector_flags_wraparound_magenta_intrusion() {
        let engine = ColorQaEngine::new();
        let mut img = solid_image(64, 64, [44, 62, 80]);
        paint_rect(&mut img, 16, 16, 20, 20, [230, 30, 190]);

        let lock = PaletteLock {
            swatches: vec![PaletteSwatch {
                name: "coat".to_string(),
                color_hex: "#2C3E50".to_string(),
                surface_slot: None,
            }],
        };
        let cfg = PaletteQaConfig {
            max_swatch_delta_e2000: 100.0,
            max_lightness_shift: 100.0,
            max_chroma_shift: 100.0,
            forbidden_hue_sectors: vec![ForbiddenHueSector {
                name: "neon-magenta".to_string(),
                min_hue_deg: 315.0,
                max_hue_deg: 15.0,
                min_chroma: 25.0,
                max_fraction: 0.02,
            }],
            ..PaletteQaConfig::default()
        };
        let spec = SubjectRoiSpec {
            subject_slug: "courier-ren",
            palette_lock: &lock,
            rois: Vec::new(),
        };

        let (report, _) = engine
            .evaluate_palette_adherence(&img, &spec, &cfg)
            .expect("palette evaluation succeeds");
        assert!(!report.passed);
        assert_eq!(report.forbidden_hue_intrusions.len(), 1);
        assert_eq!(
            report.forbidden_hue_intrusions[0].sector_name,
            "neon-magenta"
        );
    }

    #[rstest]
    #[case::zero_outside_drift_passes([44, 62, 80], true)]
    #[case::outside_drift_fails([220, 80, 50], false)]
    fn untouched_region_drift_gates_outside_edit_mask(
        #[case] outside_after_rgb: [u8; 3],
        #[case] expect_ok: bool,
    ) {
        let engine = ColorQaEngine::new();
        let before = solid_image(32, 32, [44, 62, 80]);
        let mut after = before.clone();
        // Edit inside ROI [8..24, 8..24]
        paint_rect(&mut after, 8, 8, 16, 16, [200, 180, 140]);
        // Optionally corrupt outside pixel (0, 0) and neighbors
        if !expect_ok {
            paint_rect(&mut after, 0, 0, 4, 4, outside_after_rgb);
        }

        let mut mask = vec![0.0f32; 32 * 32];
        for y in 8..24 {
            for x in 8..24 {
                mask[y * 32 + x] = 1.0;
            }
        }

        let (report, delta_map) = engine
            .evaluate_untouched_drift(&before, &after, &mask, &UntouchedDriftConfig::default())
            .expect("untouched drift evaluation succeeds");
        assert_eq!(report.passed, expect_ok, "reasons: {:?}", report.reasons);
        assert_eq!(report.untouched_pixel_count, 32 * 32 - 16 * 16);
        if expect_ok {
            assert_eq!(report.max_delta_e2000, 0.0);
        } else {
            assert!(report.max_delta_e2000 > 10.0);
            assert!(delta_map[0] > 10.0);
        }
    }

    #[rstest]
    #[case::clean_dark_canvas(false, false, true)]
    #[case::large_uniform_yellow_cloak_not_flagged(true, false, true)]
    #[case::isolated_yellow_sparkle_outside_mask_flagged(false, true, false)]
    fn sparkle_detector_isolates_small_yellow_blobs(
        #[case] add_large_yellow: bool,
        #[case] add_small_sparkle: bool,
        #[case] expect_ok: bool,
    ) {
        let engine = ColorQaEngine::new();
        let mut img = solid_image(64, 64, [37, 40, 48]);
        if add_large_yellow {
            // 20x20 = 400 px^2 (> 64 px^2) uniform yellow block should NOT be flagged as sparkle.
            paint_rect(&mut img, 20, 20, 20, 20, [245, 220, 40]);
        }
        if add_small_sparkle {
            // 3x3 = 9 px^2 isolated yellow dot outside allowed mask MUST be flagged.
            paint_rect(&mut img, 10, 10, 3, 3, [255, 240, 20]);
        }

        let (report, _) = engine
            .detect_sparkles(&img, None, &SparkleConfig::default())
            .expect("sparkle detection succeeds");
        assert_eq!(
            report.passed, expect_ok,
            "blobs: {:?}",
            report.flagged_blobs
        );
    }

    #[test]
    fn sparkle_detector_ignores_yellow_dots_inside_allowed_mask_and_renders_heatmap() {
        let engine = ColorQaEngine::new();
        let mut img = solid_image(48, 48, [37, 40, 48]);
        paint_rect(&mut img, 12, 12, 2, 2, [255, 235, 10]);

        let mut allowed = vec![0.0f32; 48 * 48];
        for y in 10..16 {
            for x in 10..16 {
                allowed[y * 48 + x] = 1.0;
            }
        }
        let (masked_report, _) = engine
            .detect_sparkles(&img, Some(&allowed), &SparkleConfig::default())
            .unwrap();
        assert!(masked_report.passed);

        let (unmasked_report, residual_mask) = engine
            .detect_sparkles(&img, None, &SparkleConfig::default())
            .unwrap();
        assert!(!unmasked_report.passed);
        assert_eq!(unmasked_report.flagged_blobs.len(), 1);
        assert!(unmasked_report.flagged_blobs[0].area_px <= 64);

        let heatmap_png = engine
            .render_heatmap_png(48, 48, &residual_mask, 0.0, 0.5, HeatmapKind::Sparkle)
            .unwrap();
        let decoded = image::load_from_memory(&heatmap_png).unwrap().to_rgba8();
        assert_eq!(decoded.get_pixel(0, 0).0, [0, 0, 0, 0]);
        assert!(decoded.get_pixel(12, 12).0[3] > 0);
    }

    #[rstest]
    #[case::solid_mid_tone_single_bin(32, 32, [120, 120, 120, 255], 16, 16, "#787878", true)]
    #[case::solid_white_clamps_to_last_bin(16, 16, [255, 255, 255, 255], 64, 64, "#FFFFFF", true)]
    #[case::zero_max_samples_returns_histogram_only(16, 16, [44, 62, 80, 255], 0, 0, "#2C3E50", true)]
    #[case::fully_transparent_returns_zero_opaque(16, 16, [0, 0, 0, 0], 32, 0, "#000000", true)]
    #[case::zero_pixel_image_rejected(0, 16, [0, 0, 0, 255], 32, 0, "#000000", false)]
    fn computes_l_star_histogram_and_downsampled_oklab_vectorscope(
        #[case] w: u32,
        #[case] h: u32,
        #[case] fill: [u8; 4],
        #[case] max_samples: usize,
        #[case] expected_points: usize,
        #[case] expected_hex: &str,
        #[case] expect_ok: bool,
    ) {
        let img: RgbaImage = ImageBuffer::from_pixel(w, h, Rgba(fill));
        let res = compute_l_star_histogram_and_vectorscope(&img, max_samples);
        assert_eq!(res.is_ok(), expect_ok);
        if let Ok(telemetry) = res {
            assert_eq!(telemetry.l_star_bins.len(), DEFAULT_L_STAR_HISTOGRAM_BINS);
            let total_binned: usize = telemetry.l_star_bins.iter().map(|b| b.count).sum();
            assert_eq!(total_binned, telemetry.opaque_pixel_count);
            assert_eq!(telemetry.vectorscope_points.len(), expected_points);
            if let Some(pt) = telemetry.vectorscope_points.first() {
                assert_eq!(pt.hex, expected_hex);
                assert!(pt.l_star >= 0.0 && pt.l_star <= 100.0);
            }
        }
    }
}
