//! === DETERMINISTIC EXPORT PIPELINE ===
//!
//! PURPOSE:
//! Derives reproducible, post-processed `Export` deliverables (`vtt_token`,
//! `vtt_map`, `web_webp`, `grid_2x2`) directly from a `Run` or `canonical` asset
//! without creating new `Lifecycle` versions (`GLOSSARY.md`).
//!
//! THREAT MODEL / DESIGN RATIONALE ("WHY"):
//! 1. Spoiler-sensitive (`internal` or `restricted`) assets must never leak into
//!    `public` deliverables; `Export` therefore enforces `Visibility`
//!    compatibility (`internal` > `restricted` > `public`) fail-closed before
//!    writing any output bytes.
//! 2. Color grading (`--match-grade`: damped `L*` CDF matching in `25%..=45%`
//!    and `.cube` 3D LUT trilinear application) is strictly isolated to explicit
//!    `Export` requests and is NEVER applied silently during `generate` or `edit`
//!    (`ADR 0005`).
//! 3. Inputs are read via `read_regular_file_nofollow` and outputs are written
//!    atomically via `write_atomic` with SHA-256 checksums.
//!
//! EXECUTION LIFECYCLE / DATA FLOW:
//! 1. Resolve source PNG and `Visibility` from `RunStore` or explicit PNG path.
//! 2. Enforce `check_visibility_compatibility(profile_visibility, ...)`.
//! 3. If `match_grade` is explicitly requested, apply damped `L*` CDF matching
//!    and/or `.cube` 3D LUT trilinear grading.
//! 4. Run the requested `ExportProfile` (`vtt_token`, `vtt_map`, `web_webp`,
//!    or `grid_2x2`) and write deterministic output files atomically.

use std::io::Cursor;
use std::path::PathBuf;

use image::codecs::jpeg::JpegEncoder;
use image::imageops::{FilterType, crop_imm};
use image::{DynamicImage, ImageFormat, Rgb, RgbImage, Rgba, RgbaImage};
use palette::white_point::D65;
use palette::{Clamp, IntoColor, Lab, LinSrgb, Srgb};
use serde::{Deserialize, Serialize};

use crate::calibration::{STUDIO_BG_RGB, parse_hex_rgb};
use crate::color_qa::srgb_u8_to_lab;
use crate::error::CoreError;
use crate::lifecycle::{sha256_hex, validate_slug};
use crate::scene_spec::{Visibility, check_visibility_compatibility};
use crate::store::{
    MAX_CUBE_LUT_BYTES, MAX_IMAGE_DIM, MAX_IMAGE_TOTAL_PIXELS, RunStore, decode_png_checked,
    read_regular_file_nofollow, read_regular_file_nofollow_bounded, write_atomic,
};

/// Minimum square resolution for `vtt_token` exports (`16x16`).
pub const MIN_VTT_TOKEN_SIZE_PX: u32 = 16;

/// Maximum square resolution for `vtt_token` exports (`4096x4096`).
pub const MAX_VTT_TOKEN_SIZE_PX: u32 = 4_096;

/// Default square resolution for `vtt_token` exports (`512x512`).
pub const DEFAULT_VTT_TOKEN_SIZE_PX: u32 = 512;

/// Default outer ring color (`#D4A843`) for `vtt_token` exports.
pub const DEFAULT_TOKEN_RING_HEX: &str = "#D4A843";

/// Default grid cell size in pixels (`140 px`) for `vtt_map` exports.
pub const DEFAULT_VTT_MAP_GRID_PX: u32 = 140;

/// Minimum allowed damped `L*` CDF matching strength (`25%`).
pub const MIN_CDF_DAMP_STRENGTH: f64 = 0.25;

/// Maximum allowed damped `L*` CDF matching strength (`45%`).
pub const MAX_CDF_DAMP_STRENGTH: f64 = 0.45;

/// Default damped `L*` CDF matching strength (`35%`).
pub const DEFAULT_CDF_DAMP_STRENGTH: f64 = 0.35;

// === 1. Export Contracts ===

/// Source image and `Visibility` input for `run_export`.
#[derive(Debug, Clone)]
pub enum ExportSource {
    /// Load `vNNN.png` and `Visibility` directly from a recorded `Run`.
    Run {
        /// `Run` stream slug.
        slug: String,
        /// `Run` version number (`1`, `2`, ...).
        version: u32,
    },
    /// Load a regular PNG file with explicit identifier and `Visibility`.
    PngPath {
        /// Source PNG path.
        path: PathBuf,
        /// Identifier used for `VisibilityViolation` error reporting.
        asset_id: String,
        /// Access classification of the source asset.
        visibility: Visibility,
    },
}

/// Configuration for `vtt_token` circular anti-aliased VTT token exports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VttTokenOptions {
    /// Output token square width and height in pixels (default `512`).
    pub size_px: u32,
    /// Role-colored outer ring `#RRGGBB` hex color.
    pub ring_color_hex: String,
    /// Outer ring thickness in pixels.
    pub ring_width_px: u32,
    /// Dark outer/inner rim thickness in pixels.
    pub dark_rim_px: u32,
    /// Optional flat-backdrop `#RRGGBB` hex color emitting an accompanying `.jpg` copy.
    pub jpeg_backdrop_hex: Option<String>,
    /// JPEG quality (`1..=100`) when `jpeg_backdrop_hex` is `Some`.
    pub jpeg_quality: u8,
}

impl Default for VttTokenOptions {
    fn default() -> Self {
        Self {
            size_px: DEFAULT_VTT_TOKEN_SIZE_PX,
            ring_color_hex: DEFAULT_TOKEN_RING_HEX.to_string(),
            ring_width_px: 16,
            dark_rim_px: 6,
            jpeg_backdrop_hex: None,
            jpeg_quality: 90,
        }
    }
}

/// Configuration for `vtt_map` Lanczos3-upscaled battlemap JPEG exports.
#[derive(Debug, Clone, PartialEq)]
pub struct VttMapOptions {
    /// Grid column count (`>= 1`) included in `<slug>_<cols>x<rows>.jpg`.
    pub cols: u32,
    /// Grid row count (`>= 1`) included in `<slug>_<cols>x<rows>.jpg`.
    pub rows: u32,
    /// Target pixels per grid cell (`140` by default) when `scale_factor` is `None`.
    pub grid_px: u32,
    /// Optional explicit Lanczos3 upscale multiplier (`> 0.0`) overriding `cols * grid_px`.
    pub scale_factor: Option<f64>,
    /// JPEG encoding quality (`1..=100`).
    pub jpeg_quality: u8,
}

impl Default for VttMapOptions {
    fn default() -> Self {
        Self {
            cols: 24,
            rows: 18,
            grid_px: DEFAULT_VTT_MAP_GRID_PX,
            scale_factor: None,
            jpeg_quality: 90,
        }
    }
}

/// Deterministic `Export` output profile (`GLOSSARY.md`).
#[derive(Debug, Clone, PartialEq)]
pub enum ExportProfile {
    /// Center square crop + 4x supersampled AA circular alpha mask + dark rim + role ring.
    VttToken(VttTokenOptions),
    /// Lanczos3-upscaled JPEG named `<slug>_<cols>x<rows>.jpg`.
    VttMap(VttMapOptions),
    /// Deterministic lossless WebP deliverable `<slug>.webp`.
    WebWebp,
    /// Deterministic 4-quadrant split (`_tl.png`, `_tr.png`, `_bl.png`, `_br.png`).
    Grid2x2,
}

/// Explicit post-export color grading options (`export --match-grade` ONLY).
#[derive(Debug, Clone, PartialEq)]
pub struct MatchGradeOptions {
    /// Optional reference PNG path for damped CIE `L*` CDF matching.
    pub reference_image_path: Option<PathBuf>,
    /// Damped `L*` CDF blend factor strictly within `0.25..=0.45` (default `0.35`).
    pub cdf_damp_strength: f64,
    /// Optional Adobe `.cube` 3D LUT file path.
    pub cube_lut_path: Option<PathBuf>,
}

impl Default for MatchGradeOptions {
    fn default() -> Self {
        Self {
            reference_image_path: None,
            cdf_damp_strength: DEFAULT_CDF_DAMP_STRENGTH,
            cube_lut_path: None,
        }
    }
}

/// Request payload for `execute_export`.
#[derive(Debug, Clone)]
pub struct ExportRequest {
    /// Input `Run` or PNG path with `Visibility`.
    pub source: ExportSource,
    /// Output basename slug used for deterministic filenames.
    pub output_slug: String,
    /// Allowed `Visibility` of the target export destination (`public`, `restricted`, `internal`).
    pub profile_visibility: Visibility,
    /// Target output directory receiving derived deliverables.
    pub out_dir: PathBuf,
    /// Selected export profile.
    pub profile: ExportProfile,
    /// Explicit `--match-grade` options (never applied automatically).
    pub match_grade: Option<MatchGradeOptions>,
}

/// Single deterministic output file produced by `execute_export`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportedArtifact {
    /// Role label (`"token_png"`, `"token_jpeg"`, `"vtt_map"`, `"web_webp"`, `"quadrant_tl"`, ...).
    pub role: String,
    /// Filesystem path to the emitted deliverable.
    pub path: PathBuf,
    /// Output width in pixels.
    pub width: u32,
    /// Output height in pixels.
    pub height: u32,
    /// Lowercase SHA-256 hex digest of the written file bytes.
    pub sha256: String,
}

/// Structured summary returned by `execute_export`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportResult {
    /// Source identifier (`"<slug>/vNNN"` or asset id).
    pub source_id: String,
    /// Verified source `Visibility`.
    pub source_visibility: Visibility,
    /// Target export profile `Visibility`.
    pub profile_visibility: Visibility,
    /// Whether explicit `--match-grade` LUT/CDF grading was applied.
    pub match_grade_applied: bool,
    /// Deterministic exported files.
    pub artifacts: Vec<ExportedArtifact>,
}

// === 2. Export Execution ===

/// Derive deterministic `Export` artifacts from a `Run` or PNG input without creating new
/// `Lifecycle` versions, enforcing `Visibility` boundaries fail-closed.
pub fn execute_export(
    run_store: Option<&RunStore>,
    request: &ExportRequest,
) -> Result<ExportResult, CoreError> {
    validate_slug(&request.output_slug, "Export").map_err(CoreError::ExportFailed)?;
    validate_export_profile_preflight(&request.profile)?;

    let (source_id, source_visibility, source_png_path) = match &request.source {
        ExportSource::Run { slug, version } => {
            let store = run_store.ok_or_else(|| {
                CoreError::ExportFailed(
                    "ExportSource::Run requires a resolved workspace RunStore".to_string(),
                )
            })?;
            let meta = store.load_run(slug, *version)?;
            (
                meta.run_id.clone(),
                meta.visibility,
                store.run_image_path(slug, *version),
            )
        }
        ExportSource::PngPath {
            path,
            asset_id,
            visibility,
        } => {
            let mut effective_vis = *visibility;
            if let Some(store) = run_store
                && let Some(run_vis) = store.inspect_run_image_visibility(path)?
            {
                effective_vis = std::cmp::max(effective_vis, run_vis);
            }
            (asset_id.clone(), effective_vis, path.clone())
        }
    };

    check_visibility_compatibility(
        request.profile_visibility,
        "Export",
        [(source_id.as_str(), source_visibility)],
    )?;

    let raw_bytes = read_regular_file_nofollow(&source_png_path, "Export source PNG")?;
    let mut working_img = decode_png_checked(&source_png_path, &raw_bytes)?.to_rgba8();

    let mut match_grade_applied = false;
    if let Some(ref grade_opts) = request.match_grade {
        if grade_opts.reference_image_path.is_none() && grade_opts.cube_lut_path.is_none() {
            return Err(CoreError::ExportFailed(
                "--match-grade requires at least a reference image (--grade-ref) or a .cube LUT (--lut)".to_string(),
            ));
        }
        if let Some(ref ref_path) = grade_opts.reference_image_path {
            let ref_bytes = read_regular_file_nofollow(ref_path, "Export grade reference PNG")?;
            let ref_img = decode_png_checked(ref_path, &ref_bytes)?.to_rgba8();
            working_img = apply_damped_l_star_cdf_match(
                &working_img,
                &ref_img,
                grade_opts.cdf_damp_strength,
            )?;
            match_grade_applied = true;
        }
        if let Some(ref lut_path) = grade_opts.cube_lut_path {
            let lut_bytes = read_regular_file_nofollow_bounded(
                lut_path,
                "Export .cube LUT",
                MAX_CUBE_LUT_BYTES,
            )?;
            let lut_str = std::str::from_utf8(&lut_bytes).map_err(|err| {
                CoreError::ExportFailed(format!(".cube LUT is not valid UTF-8: {err}"))
            })?;
            let lut = CubeLut::parse(lut_str)?;
            working_img = lut.apply_to_image(&working_img);
            match_grade_applied = true;
        }
    }

    let mut artifacts = Vec::new();
    match &request.profile {
        ExportProfile::VttToken(opts) => {
            let (token_rgba, jpeg_opt) = render_vtt_token(&working_img, opts)?;
            let mut png_bytes = Vec::new();
            DynamicImage::ImageRgba8(token_rgba)
                .write_to(&mut Cursor::new(&mut png_bytes), ImageFormat::Png)
                .map_err(|err| CoreError::ExportFailed(format!("encoding vtt_token PNG: {err}")))?;
            let png_path = request
                .out_dir
                .join(format!("{}_token.png", request.output_slug));
            write_atomic(&png_path, &png_bytes, "vtt_token PNG")?;
            artifacts.push(ExportedArtifact {
                role: "vtt_token_png".to_string(),
                path: png_path,
                width: opts.size_px,
                height: opts.size_px,
                sha256: sha256_hex(&png_bytes),
            });

            if let Some(jpg_rgb) = jpeg_opt {
                let jpg_bytes = encode_jpeg_rgb(&jpg_rgb, opts.jpeg_quality)?;
                let jpg_path = request
                    .out_dir
                    .join(format!("{}_token.jpg", request.output_slug));
                write_atomic(&jpg_path, &jpg_bytes, "vtt_token JPEG")?;
                artifacts.push(ExportedArtifact {
                    role: "vtt_token_jpeg".to_string(),
                    path: jpg_path,
                    width: opts.size_px,
                    height: opts.size_px,
                    sha256: sha256_hex(&jpg_bytes),
                });
            }
        }
        ExportProfile::VttMap(opts) => {
            if opts.cols == 0 || opts.rows == 0 {
                return Err(CoreError::ExportFailed(
                    "vtt_map cols and rows must both be >= 1".to_string(),
                ));
            }
            let (target_w, target_h) = if let Some(factor) = opts.scale_factor {
                if !factor.is_finite() || factor <= 0.0 {
                    return Err(CoreError::ExportFailed(format!(
                        "vtt_map scale_factor ({factor}) must be finite and > 0.0"
                    )));
                }
                let raw_w = f64::from(working_img.width()) * factor;
                let raw_h = f64::from(working_img.height()) * factor;
                if !raw_w.is_finite()
                    || !raw_h.is_finite()
                    || raw_w > f64::from(MAX_IMAGE_DIM)
                    || raw_h > f64::from(MAX_IMAGE_DIM)
                {
                    return Err(CoreError::ExportFailed(format!(
                        "vtt_map scaled dimensions ({raw_w}x{raw_h}) exceed maximum {MAX_IMAGE_DIM}"
                    )));
                }
                let rounded_w = raw_w.round();
                let rounded_h = raw_h.round();
                if rounded_w < 1.0
                    || rounded_h < 1.0
                    || rounded_w > f64::from(MAX_IMAGE_DIM)
                    || rounded_h > f64::from(MAX_IMAGE_DIM)
                {
                    return Err(CoreError::ExportFailed(format!(
                        "vtt_map rounded scaled dimensions ({rounded_w}x{rounded_h}) out of bounds 1..={MAX_IMAGE_DIM}"
                    )));
                }
                (rounded_w as u32, rounded_h as u32)
            } else {
                if opts.grid_px == 0 {
                    return Err(CoreError::ExportFailed(
                        "vtt_map grid_px must be >= 1".to_string(),
                    ));
                }
                let target_w = opts.cols.checked_mul(opts.grid_px).ok_or_else(|| {
                    CoreError::ExportFailed(format!(
                        "vtt_map width overflow: cols ({}) * grid_px ({})",
                        opts.cols, opts.grid_px
                    ))
                })?;
                let target_h = opts.rows.checked_mul(opts.grid_px).ok_or_else(|| {
                    CoreError::ExportFailed(format!(
                        "vtt_map height overflow: rows ({}) * grid_px ({})",
                        opts.rows, opts.grid_px
                    ))
                })?;
                (target_w, target_h)
            };
            validate_export_dimensions(target_w, target_h, true, "vtt_map")?;

            let resized = if working_img.width() == target_w && working_img.height() == target_h {
                working_img
            } else {
                DynamicImage::ImageRgba8(working_img)
                    .resize_exact(target_w, target_h, FilterType::Lanczos3)
                    .to_rgba8()
            };
            let rgb = flatten_rgba_to_rgb(&resized, STUDIO_BG_RGB);
            let jpg_bytes = encode_jpeg_rgb(&rgb, opts.jpeg_quality)?;
            let jpg_path = request.out_dir.join(format!(
                "{}_{}x{}.jpg",
                request.output_slug, opts.cols, opts.rows
            ));
            write_atomic(&jpg_path, &jpg_bytes, "vtt_map JPEG")?;
            artifacts.push(ExportedArtifact {
                role: "vtt_map".to_string(),
                path: jpg_path,
                width: target_w,
                height: target_h,
                sha256: sha256_hex(&jpg_bytes),
            });
        }
        ExportProfile::WebWebp => {
            let mut webp_bytes = Vec::new();
            let w = working_img.width();
            let h = working_img.height();
            validate_export_dimensions(w, h, false, "web_webp")?;
            DynamicImage::ImageRgba8(working_img)
                .write_to(&mut Cursor::new(&mut webp_bytes), ImageFormat::WebP)
                .map_err(|err| CoreError::ExportFailed(format!("encoding web_webp: {err}")))?;
            let webp_path = request
                .out_dir
                .join(format!("{}.webp", request.output_slug));
            write_atomic(&webp_path, &webp_bytes, "web_webp")?;
            artifacts.push(ExportedArtifact {
                role: "web_webp".to_string(),
                path: webp_path,
                width: w,
                height: h,
                sha256: sha256_hex(&webp_bytes),
            });
        }
        ExportProfile::Grid2x2 => {
            let quadrants = split_grid_2x2(&working_img)?;
            for (tag, quad_img) in quadrants {
                let qw = quad_img.width();
                let qh = quad_img.height();
                let mut png_bytes = Vec::new();
                DynamicImage::ImageRgba8(quad_img)
                    .write_to(&mut Cursor::new(&mut png_bytes), ImageFormat::Png)
                    .map_err(|err| {
                        CoreError::ExportFailed(format!("encoding grid_2x2 {tag} PNG: {err}"))
                    })?;
                let q_path = request
                    .out_dir
                    .join(format!("{}_{tag}.png", request.output_slug));
                write_atomic(&q_path, &png_bytes, "grid_2x2 PNG")?;
                artifacts.push(ExportedArtifact {
                    role: format!("grid_2x2_{tag}"),
                    path: q_path,
                    width: qw,
                    height: qh,
                    sha256: sha256_hex(&png_bytes),
                });
            }
        }
    }

    Ok(ExportResult {
        source_id,
        source_visibility,
        profile_visibility: request.profile_visibility,
        match_grade_applied,
        artifacts,
    })
}

// === 3. Profile Implementations ===

/// Center-crop to `1:1`, Lanczos3-resize to `size_px`, and composite a 4x supersampled
/// anti-aliased circular mask with dark rim and role-colored outer ring.
pub fn render_vtt_token(
    src: &RgbaImage,
    opts: &VttTokenOptions,
) -> Result<(RgbaImage, Option<RgbImage>), CoreError> {
    if !(MIN_VTT_TOKEN_SIZE_PX..=MAX_VTT_TOKEN_SIZE_PX).contains(&opts.size_px) {
        return Err(CoreError::ExportFailed(format!(
            "vtt_token size_px ({}) must be within {MIN_VTT_TOKEN_SIZE_PX}..={MAX_VTT_TOKEN_SIZE_PX}",
            opts.size_px
        )));
    }
    let ring_rgb = parse_hex_rgb(&opts.ring_color_hex)?;
    let backdrop_rgb_opt = opts
        .jpeg_backdrop_hex
        .as_deref()
        .map(parse_hex_rgb)
        .transpose()?;
    validate_export_dimensions(src.width(), src.height(), false, "vtt_token source image")?;
    if backdrop_rgb_opt.is_some() {
        validate_export_dimensions(opts.size_px, opts.size_px, true, "vtt_token JPEG")?;
    }

    let side = src.width().min(src.height());
    if side == 0 {
        return Err(CoreError::ExportFailed(
            "vtt_token source image has zero width or height".to_string(),
        ));
    }
    let crop_x = (src.width() - side) / 2;
    let crop_y = (src.height() - side) / 2;
    let square = crop_imm(src, crop_x, crop_y, side, side).to_image();
    let base = if side == opts.size_px {
        square
    } else {
        DynamicImage::ImageRgba8(square)
            .resize_exact(opts.size_px, opts.size_px, FilterType::Lanczos3)
            .to_rgba8()
    };

    let size = opts.size_px;
    let center = f64::from(size) * 0.5;
    let outer_r = center - 1.0;
    let dark_rim = f64::from(opts.dark_rim_px);
    let ring_w = f64::from(opts.ring_width_px);
    let inner_rim = dark_rim.clamp(2.0, 4.0);

    let r_ring_outer = (outer_r - dark_rim).max(0.0);
    let r_ring_inner = (r_ring_outer - ring_w).max(0.0);
    let r_portrait = (r_ring_inner - inner_rim).max(0.0);

    let dark_rim_rgb = [20u8, 22u8, 28u8];
    let mut out = RgbaImage::new(size, size);

    // 4x supersampled anti-aliased subpixel grid (4x4 = 16 samples per output pixel).
    const SS: usize = 4;
    const INV_SAMPLES: f64 = 1.0 / ((SS * SS) as f64);

    for y in 0..size {
        for x in 0..size {
            let portrait_px = base.get_pixel(x, y).0;
            let mut sum_r = 0.0f64;
            let mut sum_g = 0.0f64;
            let mut sum_b = 0.0f64;
            let mut sum_a = 0.0f64;

            for sy in 0..SS {
                let py = f64::from(y) + (sy as f64 + 0.5) / (SS as f64);
                let dy = py - center;
                for sx in 0..SS {
                    let px = f64::from(x) + (sx as f64 + 0.5) / (SS as f64);
                    let dx = px - center;
                    let dist = (dx * dx + dy * dy).sqrt();

                    if dist > outer_r {
                        continue;
                    }
                    let (sr, sg, sb, sa) =
                        if dist > r_ring_outer || dist > r_portrait && dist <= r_ring_inner {
                            (
                                f64::from(dark_rim_rgb[0]),
                                f64::from(dark_rim_rgb[1]),
                                f64::from(dark_rim_rgb[2]),
                                1.0,
                            )
                        } else if dist > r_ring_inner {
                            (
                                f64::from(ring_rgb[0]),
                                f64::from(ring_rgb[1]),
                                f64::from(ring_rgb[2]),
                                1.0,
                            )
                        } else {
                            let a = f64::from(portrait_px[3]) / 255.0;
                            (
                                f64::from(portrait_px[0]) * a,
                                f64::from(portrait_px[1]) * a,
                                f64::from(portrait_px[2]) * a,
                                a,
                            )
                        };
                    sum_r += sr;
                    sum_g += sg;
                    sum_b += sb;
                    sum_a += sa;
                }
            }

            let avg_a = (sum_a * INV_SAMPLES).clamp(0.0, 1.0);
            if avg_a <= 1e-6 {
                out.put_pixel(x, y, Rgba([0, 0, 0, 0]));
            } else {
                let r = ((sum_r * INV_SAMPLES) / avg_a).round().clamp(0.0, 255.0) as u8;
                let g = ((sum_g * INV_SAMPLES) / avg_a).round().clamp(0.0, 255.0) as u8;
                let b = ((sum_b * INV_SAMPLES) / avg_a).round().clamp(0.0, 255.0) as u8;
                let a = (avg_a * 255.0).round().clamp(0.0, 255.0) as u8;
                out.put_pixel(x, y, Rgba([r, g, b, a]));
            }
        }
    }

    let jpeg_img = backdrop_rgb_opt.map(|bg| flatten_rgba_to_rgb(&out, bg));
    Ok((out, jpeg_img))
}

/// Deterministic 4-quadrant split (`tl`, `tr`, `bl`, `br`).
pub fn split_grid_2x2(src: &RgbaImage) -> Result<[(&'static str, RgbaImage); 4], CoreError> {
    let w = src.width();
    let h = src.height();
    validate_export_dimensions(w, h, false, "grid_2x2")?;
    if w < 2 || h < 2 {
        return Err(CoreError::ExportFailed(format!(
            "grid_2x2 requires at least 2x2 image dimensions (got {w}x{h})"
        )));
    }
    let left_w = w / 2;
    let right_w = w - left_w;
    let top_h = h / 2;
    let bottom_h = h - top_h;

    Ok([
        ("tl", crop_imm(src, 0, 0, left_w, top_h).to_image()),
        ("tr", crop_imm(src, left_w, 0, right_w, top_h).to_image()),
        ("bl", crop_imm(src, 0, top_h, left_w, bottom_h).to_image()),
        (
            "br",
            crop_imm(src, left_w, top_h, right_w, bottom_h).to_image(),
        ),
    ])
}

// === 4. Explicit --match-grade: Damped L* CDF Matching & .cube 3D LUT ===

/// Apply damped CIE `L*` cumulative-distribution matching (`damp_strength` strictly in
/// `0.25..=0.45`) from `reference` onto `source` while preserving `a*, b*` chroma.
pub fn apply_damped_l_star_cdf_match(
    source: &RgbaImage,
    reference: &RgbaImage,
    damp_strength: f64,
) -> Result<RgbaImage, CoreError> {
    if !damp_strength.is_finite()
        || !(MIN_CDF_DAMP_STRENGTH..=MAX_CDF_DAMP_STRENGTH).contains(&damp_strength)
    {
        return Err(CoreError::ExportFailed(format!(
            "cdf_damp_strength ({damp_strength}) must be finite within {MIN_CDF_DAMP_STRENGTH}..={MAX_CDF_DAMP_STRENGTH}"
        )));
    }
    if source.width() == 0
        || source.height() == 0
        || reference.width() == 0
        || reference.height() == 0
    {
        return Err(CoreError::ExportFailed(
            "damped L* CDF match requires non-empty source and reference images".to_string(),
        ));
    }

    const BINS: usize = 1001; // 0.1 L* resolution across 0.0..=100.0
    let mut src_hist = vec![0u64; BINS];
    let mut ref_hist = vec![0u64; BINS];
    let mut src_total = 0u64;
    let mut ref_total = 0u64;

    for px in source.pixels() {
        if px.0[3] == 0 {
            continue;
        }
        let lab = srgb_u8_to_lab([px.0[0], px.0[1], px.0[2]]);
        let bin = (f64::from(lab.l) * 10.0).round().clamp(0.0, 1000.0) as usize;
        src_hist[bin] += 1;
        src_total += 1;
    }
    for px in reference.pixels() {
        if px.0[3] == 0 {
            continue;
        }
        let lab = srgb_u8_to_lab([px.0[0], px.0[1], px.0[2]]);
        let bin = (f64::from(lab.l) * 10.0).round().clamp(0.0, 1000.0) as usize;
        ref_hist[bin] += 1;
        ref_total += 1;
    }

    if src_total == 0 || ref_total == 0 {
        return Err(CoreError::ExportFailed(
            "damped L* CDF match found zero opaque pixels".to_string(),
        ));
    }

    let mut src_cdf = vec![0.0f64; BINS];
    let mut ref_cdf = vec![0.0f64; BINS];
    let mut s_acc = 0u64;
    let mut r_acc = 0u64;
    for i in 0..BINS {
        s_acc += src_hist[i];
        r_acc += ref_hist[i];
        src_cdf[i] = s_acc as f64 / src_total as f64;
        ref_cdf[i] = r_acc as f64 / ref_total as f64;
    }

    let mut lut_l = vec![0.0f64; BINS];
    let mut r_idx = 0usize;
    for (s_idx, &p) in src_cdf.iter().enumerate() {
        while r_idx + 1 < BINS && ref_cdf[r_idx] < p {
            r_idx += 1;
        }
        lut_l[s_idx] = (r_idx as f64) * 0.1;
    }

    let mut out = RgbaImage::new(source.width(), source.height());
    for y in 0..source.height() {
        for x in 0..source.width() {
            let px = source.get_pixel(x, y).0;
            if px[3] == 0 {
                out.put_pixel(x, y, Rgba(px));
                continue;
            }
            let lab = srgb_u8_to_lab([px[0], px[1], px[2]]);
            let bin = (f64::from(lab.l) * 10.0).round().clamp(0.0, 1000.0) as usize;
            let target_l = lut_l[bin];
            let damped_l = (1.0 - damp_strength) * f64::from(lab.l) + damp_strength * target_l;
            let adjusted = Lab::<D65, f32>::new(damped_l as f32, lab.a, lab.b);
            let lin: LinSrgb<f32> = adjusted.into_color();
            let srgb: Srgb<u8> = Srgb::<f32>::from_linear(lin).clamp().into_format();
            out.put_pixel(x, y, Rgba([srgb.red, srgb.green, srgb.blue, px[3]]));
        }
    }

    Ok(out)
}

/// Parsed Adobe `.cube` 3D LUT supporting trilinear RGB interpolation (typically `33^3`).
#[derive(Debug, Clone, PartialEq)]
pub struct CubeLut {
    /// Optional `TITLE` header string.
    pub title: Option<String>,
    /// Side dimension `N` (`LUT_3D_SIZE N`, for example `33`).
    pub size: usize,
    /// Minimum RGB input domain (`[0.0, 0.0, 0.0]` by default).
    pub domain_min: [f32; 3],
    /// Maximum RGB input domain (`[1.0, 1.0, 1.0]` by default).
    pub domain_max: [f32; 3],
    /// Row-major `N * N * N` entries ordered `B` slowest, `G` middle, `R` fastest.
    pub table: Vec<[f32; 3]>,
}

impl CubeLut {
    /// Parse and validate an Adobe `.cube` 3D LUT text payload fail-closed.
    pub fn parse(raw: &str) -> Result<Self, CoreError> {
        let mut title: Option<String> = None;
        let mut size: Option<usize> = None;
        let mut domain_min = [0.0f32, 0.0, 0.0];
        let mut domain_max = [1.0f32, 1.0, 1.0];
        let mut table: Vec<[f32; 3]> = Vec::new();

        for (line_idx, raw_line) in raw.lines().enumerate() {
            let line = raw_line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some(rest) = line.strip_prefix("TITLE") {
                let cleaned = rest.trim().trim_matches('"').to_string();
                if !cleaned.is_empty() {
                    title = Some(cleaned);
                }
                continue;
            }
            if let Some(rest) = line.strip_prefix("LUT_3D_SIZE") {
                let n: usize = rest.trim().parse().map_err(|_| {
                    CoreError::ExportFailed(format!(
                        ".cube line {}: invalid LUT_3D_SIZE '{}'",
                        line_idx + 1,
                        rest.trim()
                    ))
                })?;
                if !(2..=65).contains(&n) {
                    return Err(CoreError::ExportFailed(format!(
                        ".cube LUT_3D_SIZE {n} must be within 2..=65"
                    )));
                }
                size = Some(n);
                continue;
            }
            if let Some(rest) = line.strip_prefix("DOMAIN_MIN") {
                domain_min = parse_rgb_triple(rest, line_idx + 1)?;
                continue;
            }
            if let Some(rest) = line.strip_prefix("DOMAIN_MAX") {
                domain_max = parse_rgb_triple(rest, line_idx + 1)?;
                continue;
            }
            if line.starts_with("LUT_1D_SIZE") {
                return Err(CoreError::ExportFailed(
                    "1D .cube LUTs are not supported; expected LUT_3D_SIZE".to_string(),
                ));
            }
            table.push(parse_rgb_triple(line, line_idx + 1)?);
        }

        let n = size.ok_or_else(|| {
            CoreError::ExportFailed(".cube LUT missing required LUT_3D_SIZE header".to_string())
        })?;
        for c in 0..3 {
            if domain_max[c] <= domain_min[c] {
                return Err(CoreError::ExportFailed(format!(
                    ".cube DOMAIN_MAX[{c}] ({}) must exceed DOMAIN_MIN[{c}] ({})",
                    domain_max[c], domain_min[c]
                )));
            }
        }
        let expected_entries = n * n * n;
        if table.len() != expected_entries {
            return Err(CoreError::ExportFailed(format!(
                ".cube LUT_3D_SIZE {n} requires {expected_entries} entries, found {}",
                table.len()
            )));
        }

        Ok(Self {
            title,
            size: n,
            domain_min,
            domain_max,
            table,
        })
    }

    /// Evaluate the 3D LUT at normalized sRGB `[r, g, b]` in `[0.0, 1.0]` via trilinear interpolation.
    #[must_use]
    pub fn sample_trilinear(&self, rgb: [f32; 3]) -> [f32; 3] {
        let max_idx = (self.size - 1) as f32;
        let nr = ((rgb[0] - self.domain_min[0]) / (self.domain_max[0] - self.domain_min[0]))
            .clamp(0.0, 1.0)
            * max_idx;
        let ng = ((rgb[1] - self.domain_min[1]) / (self.domain_max[1] - self.domain_min[1]))
            .clamp(0.0, 1.0)
            * max_idx;
        let nb = ((rgb[2] - self.domain_min[2]) / (self.domain_max[2] - self.domain_min[2]))
            .clamp(0.0, 1.0)
            * max_idx;

        let r0 = (nr.floor() as usize).min(self.size - 1);
        let g0 = (ng.floor() as usize).min(self.size - 1);
        let b0 = (nb.floor() as usize).min(self.size - 1);
        let r1 = (r0 + 1).min(self.size - 1);
        let g1 = (g0 + 1).min(self.size - 1);
        let b1 = (b0 + 1).min(self.size - 1);

        let fr = nr - r0 as f32;
        let fg = ng - g0 as f32;
        let fb = nb - b0 as f32;

        let idx = |r: usize, g: usize, b: usize| -> [f32; 3] {
            self.table[(b * self.size + g) * self.size + r]
        };

        let c000 = idx(r0, g0, b0);
        let c100 = idx(r1, g0, b0);
        let c010 = idx(r0, g1, b0);
        let c110 = idx(r1, g1, b0);
        let c001 = idx(r0, g0, b1);
        let c101 = idx(r1, g0, b1);
        let c011 = idx(r0, g1, b1);
        let c111 = idx(r1, g1, b1);

        let mut out = [0.0f32; 3];
        for ch in 0..3 {
            let c00 = c000[ch] * (1.0 - fr) + c100[ch] * fr;
            let c10 = c010[ch] * (1.0 - fr) + c110[ch] * fr;
            let c01 = c001[ch] * (1.0 - fr) + c101[ch] * fr;
            let c11 = c011[ch] * (1.0 - fr) + c111[ch] * fr;

            let c0 = c00 * (1.0 - fg) + c10 * fg;
            let c1 = c01 * (1.0 - fg) + c11 * fg;

            out[ch] = (c0 * (1.0 - fb) + c1 * fb).clamp(0.0, 1.0);
        }
        out
    }

    /// Apply this 3D LUT across every pixel of `src`, preserving alpha.
    #[must_use]
    pub fn apply_to_image(&self, src: &RgbaImage) -> RgbaImage {
        let mut out = RgbaImage::new(src.width(), src.height());
        for y in 0..src.height() {
            for x in 0..src.width() {
                let px = src.get_pixel(x, y).0;
                let mapped = self.sample_trilinear([
                    f32::from(px[0]) / 255.0,
                    f32::from(px[1]) / 255.0,
                    f32::from(px[2]) / 255.0,
                ]);
                out.put_pixel(
                    x,
                    y,
                    Rgba([
                        (mapped[0] * 255.0).round().clamp(0.0, 255.0) as u8,
                        (mapped[1] * 255.0).round().clamp(0.0, 255.0) as u8,
                        (mapped[2] * 255.0).round().clamp(0.0, 255.0) as u8,
                        px[3],
                    ]),
                );
            }
        }
        out
    }
}

fn parse_rgb_triple(line: &str, line_num: usize) -> Result<[f32; 3], CoreError> {
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() != 3 {
        return Err(CoreError::ExportFailed(format!(
            ".cube line {line_num}: expected 3 floats, found '{}'",
            line
        )));
    }
    let mut vals = [0.0f32; 3];
    for (i, part) in parts.iter().enumerate() {
        let v: f32 = part.parse().map_err(|_| {
            CoreError::ExportFailed(format!(".cube line {line_num}: invalid float '{part}'"))
        })?;
        if !v.is_finite() {
            return Err(CoreError::ExportFailed(format!(
                ".cube line {line_num}: non-finite float '{part}'"
            )));
        }
        vals[i] = v;
    }
    Ok(vals)
}

fn validate_export_dimensions(
    w: u32,
    h: u32,
    for_jpeg: bool,
    context: &str,
) -> Result<(), CoreError> {
    if w == 0 || h == 0 || w > MAX_IMAGE_DIM || h > MAX_IMAGE_DIM {
        return Err(CoreError::ExportFailed(format!(
            "{context} dimensions {w}x{h} out of bounds 1..={MAX_IMAGE_DIM}"
        )));
    }
    if for_jpeg && (w > u32::from(u16::MAX) || h > u32::from(u16::MAX)) {
        return Err(CoreError::ExportFailed(format!(
            "{context} JPEG dimensions {w}x{h} exceed u16::MAX ({})",
            u16::MAX
        )));
    }
    let total = u64::from(w).checked_mul(u64::from(h)).ok_or_else(|| {
        CoreError::ExportFailed(format!("{context} pixel count overflow for {w}x{h}"))
    })?;
    if total > MAX_IMAGE_TOTAL_PIXELS {
        return Err(CoreError::ExportFailed(format!(
            "{context} total pixel count {total} ({w}x{h}) exceeds maximum {MAX_IMAGE_TOTAL_PIXELS}"
        )));
    }
    Ok(())
}

fn validate_export_profile_preflight(profile: &ExportProfile) -> Result<(), CoreError> {
    match profile {
        ExportProfile::VttToken(opts) => {
            if !(MIN_VTT_TOKEN_SIZE_PX..=MAX_VTT_TOKEN_SIZE_PX).contains(&opts.size_px) {
                return Err(CoreError::ExportFailed(format!(
                    "vtt_token size_px ({}) must be within {MIN_VTT_TOKEN_SIZE_PX}..={MAX_VTT_TOKEN_SIZE_PX}",
                    opts.size_px
                )));
            }
            let _ = parse_hex_rgb(&opts.ring_color_hex)?;
            if let Some(ref bg) = opts.jpeg_backdrop_hex {
                let _ = parse_hex_rgb(bg)?;
            }
            Ok(())
        }
        ExportProfile::VttMap(opts) => {
            if opts.cols == 0 || opts.rows == 0 {
                return Err(CoreError::ExportFailed(
                    "vtt_map cols and rows must both be >= 1".to_string(),
                ));
            }
            if let Some(factor) = opts.scale_factor {
                if !factor.is_finite() || factor <= 0.0 || factor > f64::from(MAX_IMAGE_DIM) {
                    return Err(CoreError::ExportFailed(format!(
                        "vtt_map scale_factor ({factor}) must be finite and within (0.0, {MAX_IMAGE_DIM}]"
                    )));
                }
            } else {
                if opts.grid_px == 0 {
                    return Err(CoreError::ExportFailed(
                        "vtt_map grid_px must be >= 1".to_string(),
                    ));
                }
                let target_w = opts.cols.checked_mul(opts.grid_px).ok_or_else(|| {
                    CoreError::ExportFailed(format!(
                        "vtt_map width overflow: cols ({}) * grid_px ({})",
                        opts.cols, opts.grid_px
                    ))
                })?;
                let target_h = opts.rows.checked_mul(opts.grid_px).ok_or_else(|| {
                    CoreError::ExportFailed(format!(
                        "vtt_map height overflow: rows ({}) * grid_px ({})",
                        opts.rows, opts.grid_px
                    ))
                })?;
                validate_export_dimensions(target_w, target_h, true, "vtt_map")?;
            }
            Ok(())
        }
        ExportProfile::WebWebp | ExportProfile::Grid2x2 => Ok(()),
    }
}

fn flatten_rgba_to_rgb(src: &RgbaImage, bg: [u8; 3]) -> RgbImage {
    let mut out = RgbImage::new(src.width(), src.height());
    for y in 0..src.height() {
        for x in 0..src.width() {
            let px = src.get_pixel(x, y).0;
            let a = f32::from(px[3]) / 255.0;
            let inv = 1.0 - a;
            let r = (f32::from(px[0]) * a + f32::from(bg[0]) * inv)
                .round()
                .clamp(0.0, 255.0) as u8;
            let g = (f32::from(px[1]) * a + f32::from(bg[1]) * inv)
                .round()
                .clamp(0.0, 255.0) as u8;
            let b = (f32::from(px[2]) * a + f32::from(bg[2]) * inv)
                .round()
                .clamp(0.0, 255.0) as u8;
            out.put_pixel(x, y, Rgb([r, g, b]));
        }
    }
    out
}

fn encode_jpeg_rgb(rgb: &RgbImage, quality: u8) -> Result<Vec<u8>, CoreError> {
    validate_export_dimensions(rgb.width(), rgb.height(), true, "JPEG export")?;
    let q = quality.clamp(1, 100);
    let mut buf = Vec::new();
    let mut encoder = JpegEncoder::new_with_quality(&mut buf, q);
    encoder
        .encode(
            rgb.as_raw(),
            rgb.width(),
            rgb.height(),
            image::ExtendedColorType::Rgb8,
        )
        .map_err(|err| CoreError::ExportFailed(format!("encoding JPEG: {err}")))?;
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fmt::Write as _;
    use std::fs;
    use std::path::Path;

    use image::ImageBuffer;
    use rstest::rstest;
    use tempfile::TempDir;

    use super::*;
    use crate::store::{RecordRunParams, RunBackendMetadata, RunQaReports};

    fn make_test_png(w: u32, h: u32, fill: [u8; 4]) -> Vec<u8> {
        let img: RgbaImage = ImageBuffer::from_pixel(w, h, Rgba(fill));
        let mut buf = Vec::new();
        DynamicImage::ImageRgba8(img)
            .write_to(&mut Cursor::new(&mut buf), ImageFormat::Png)
            .unwrap();
        buf
    }

    fn seed_run(
        ws: &Path,
        slug: &str,
        visibility: Visibility,
        w: u32,
        h: u32,
        fill: [u8; 4],
    ) -> RunStore {
        let store = RunStore::new(ws.join("runs"));
        store
            .record_run(RecordRunParams {
                slug: slug.to_string(),
                visibility,
                png_bytes: make_test_png(w, h, fill),
                input_hashes: BTreeMap::new(),
                compiled_prompt: "harbor scene".to_string(),
                backend: RunBackendMetadata {
                    name: "stub".to_string(),
                    width: w,
                    height: h,
                    raw_width: w,
                    raw_height: h,
                    thread_id: None,
                },
                seed: None,
                qa_reports: RunQaReports::default(),
            })
            .unwrap();
        store
    }

    fn build_33_cube_lut(scale: [f32; 3]) -> String {
        let n = 33usize;
        let mut s = format!("TITLE \"Test 33x33x33 LUT\"\nLUT_3D_SIZE {n}\n");
        for b in 0..n {
            for g in 0..n {
                for r in 0..n {
                    let rf = (r as f32 / (n - 1) as f32 * scale[0]).clamp(0.0, 1.0);
                    let gf = (g as f32 / (n - 1) as f32 * scale[1]).clamp(0.0, 1.0);
                    let bf = (b as f32 / (n - 1) as f32 * scale[2]).clamp(0.0, 1.0);
                    let _ = writeln!(s, "{rf:.6} {gf:.6} {bf:.6}");
                }
            }
        }
        s
    }

    #[rstest]
    #[case::internal_run_blocked_for_public_export(Visibility::Internal, Visibility::Public, false)]
    #[case::restricted_run_blocked_for_public_export(
        Visibility::Restricted,
        Visibility::Public,
        false
    )]
    #[case::internal_run_blocked_for_restricted_export(
        Visibility::Internal,
        Visibility::Restricted,
        false
    )]
    #[case::restricted_run_allowed_for_restricted_export(
        Visibility::Restricted,
        Visibility::Restricted,
        true
    )]
    #[case::public_run_allowed_for_public_export(Visibility::Public, Visibility::Public, true)]
    fn export_enforces_visibility_gate_fail_closed(
        #[case] run_vis: Visibility,
        #[case] export_vis: Visibility,
        #[case] expect_ok: bool,
    ) {
        let root = TempDir::new().unwrap();
        let store = seed_run(
            root.path(),
            "courier-ren",
            run_vis,
            128,
            128,
            [120, 140, 160, 255],
        );
        let res = execute_export(
            Some(&store),
            &ExportRequest {
                source: ExportSource::Run {
                    slug: "courier-ren".to_string(),
                    version: 1,
                },
                output_slug: "courier-ren".to_string(),
                profile_visibility: export_vis,
                out_dir: root.path().join("exports"),
                profile: ExportProfile::WebWebp,
                match_grade: None,
            },
        );
        assert_eq!(res.is_ok(), expect_ok);
        if !expect_ok {
            assert!(matches!(
                res.unwrap_err(),
                CoreError::VisibilityViolation { .. }
            ));
        }
    }

    #[test]
    fn vtt_token_exports_4x_aa_circular_png_and_optional_flat_jpeg() {
        let root = TempDir::new().unwrap();
        let store = seed_run(
            root.path(),
            "courier-ren",
            Visibility::Public,
            640,
            480,
            [110, 150, 190, 255],
        );
        let res = execute_export(
            Some(&store),
            &ExportRequest {
                source: ExportSource::Run {
                    slug: "courier-ren".to_string(),
                    version: 1,
                },
                output_slug: "courier-ren".to_string(),
                profile_visibility: Visibility::Public,
                out_dir: root.path().join("exports"),
                profile: ExportProfile::VttToken(VttTokenOptions {
                    size_px: 512,
                    ring_color_hex: "#D4A843".to_string(),
                    ring_width_px: 16,
                    dark_rim_px: 6,
                    jpeg_backdrop_hex: Some("#252830".to_string()),
                    jpeg_quality: 92,
                }),
                match_grade: None,
            },
        )
        .expect("vtt_token export succeeds");

        assert_eq!(res.artifacts.len(), 2);
        let token_png = image::open(&res.artifacts[0].path).unwrap().to_rgba8();
        assert_eq!((token_png.width(), token_png.height()), (512, 512));
        // Corners outside circular mask are fully transparent
        assert_eq!(token_png.get_pixel(0, 0).0[3], 0);
        // Center pixel matches source portrait RGB
        assert_eq!(token_png.get_pixel(256, 256).0, [110, 150, 190, 255]);
        // Outer ring pixel (~radius 240 from center 256) matches #D4A843
        assert_eq!(
            token_png.get_pixel(256 + 240, 256).0,
            [0xD4, 0xA8, 0x43, 255]
        );
        // Anti-aliased boundary pixel has partial alpha (0 < a < 255)
        let has_partial_alpha = (0..512).any(|x| {
            let a = token_png.get_pixel(x, 1).0[3];
            a > 0 && a < 255
        });
        assert!(
            has_partial_alpha,
            "4x SS circular mask must produce anti-aliased edge alpha"
        );
        assert!(res.artifacts[1].path.extension().unwrap() == "jpg");
    }

    #[test]
    fn vtt_map_webp_and_grid_2x2_export_deterministically() {
        let root = TempDir::new().unwrap();
        let store = seed_run(
            root.path(),
            "harbor-dock",
            Visibility::Public,
            200,
            100,
            [80, 120, 160, 255],
        );
        let out_dir = root.path().join("exports");

        let map_res = execute_export(
            Some(&store),
            &ExportRequest {
                source: ExportSource::Run {
                    slug: "harbor-dock".to_string(),
                    version: 1,
                },
                output_slug: "harbor-dock".to_string(),
                profile_visibility: Visibility::Public,
                out_dir: out_dir.clone(),
                profile: ExportProfile::VttMap(VttMapOptions {
                    cols: 12,
                    rows: 6,
                    grid_px: 100,
                    scale_factor: None,
                    jpeg_quality: 90,
                }),
                match_grade: None,
            },
        )
        .unwrap();
        assert!(map_res.artifacts[0].path.ends_with("harbor-dock_12x6.jpg"));
        assert_eq!(
            (map_res.artifacts[0].width, map_res.artifacts[0].height),
            (1200, 600)
        );

        let webp_res = execute_export(
            Some(&store),
            &ExportRequest {
                source: ExportSource::Run {
                    slug: "harbor-dock".to_string(),
                    version: 1,
                },
                output_slug: "harbor-dock".to_string(),
                profile_visibility: Visibility::Public,
                out_dir: out_dir.clone(),
                profile: ExportProfile::WebWebp,
                match_grade: None,
            },
        )
        .unwrap();
        assert!(webp_res.artifacts[0].path.ends_with("harbor-dock.webp"));

        let grid_res = execute_export(
            Some(&store),
            &ExportRequest {
                source: ExportSource::Run {
                    slug: "harbor-dock".to_string(),
                    version: 1,
                },
                output_slug: "harbor-dock".to_string(),
                profile_visibility: Visibility::Public,
                out_dir,
                profile: ExportProfile::Grid2x2,
                match_grade: None,
            },
        )
        .unwrap();
        assert_eq!(grid_res.artifacts.len(), 4);
        assert!(grid_res.artifacts[0].path.ends_with("harbor-dock_tl.png"));
        assert!(grid_res.artifacts[3].path.ends_with("harbor-dock_br.png"));
        assert_eq!(
            (grid_res.artifacts[0].width, grid_res.artifacts[0].height),
            (100, 50)
        );
    }

    #[rstest]
    #[case::valid_35_percent_damp(0.35, true)]
    #[case::too_low_damp_rejected(0.10, false)]
    #[case::too_high_damp_rejected(0.75, false)]
    fn explicit_match_grade_enforces_25_to_45_percent_cdf_bounds_and_33_cube_lut(
        #[case] strength: f64,
        #[case] expect_ok: bool,
    ) {
        let root = TempDir::new().unwrap();
        let store = seed_run(
            root.path(),
            "harbor",
            Visibility::Public,
            64,
            64,
            [60, 60, 60, 255],
        );
        let ref_path = root.path().join("grade_ref.png");
        fs::write(&ref_path, make_test_png(64, 64, [200, 200, 200, 255])).unwrap();
        let lut_path = root.path().join("warm.cube");
        fs::write(&lut_path, build_33_cube_lut([1.0, 0.8, 0.6])).unwrap();

        let res = execute_export(
            Some(&store),
            &ExportRequest {
                source: ExportSource::Run {
                    slug: "harbor".to_string(),
                    version: 1,
                },
                output_slug: "harbor".to_string(),
                profile_visibility: Visibility::Public,
                out_dir: root.path().join("exports"),
                profile: ExportProfile::WebWebp,
                match_grade: Some(MatchGradeOptions {
                    reference_image_path: Some(ref_path),
                    cdf_damp_strength: strength,
                    cube_lut_path: Some(lut_path),
                }),
            },
        );
        assert_eq!(res.is_ok(), expect_ok);
        if let Ok(ok) = res {
            assert!(ok.match_grade_applied);
            let webp = image::open(&ok.artifacts[0].path).unwrap().to_rgba8();
            let px = webp.get_pixel(10, 10).0;
            // Damped L* lifted brightness above 60 AND warm LUT attenuated B < G < R.
            assert!(px[0] > 80);
            assert!(px[0] > px[1] && px[1] > px[2]);
        }
    }

    #[test]
    fn cube_lut_parser_rejects_malformed_entry_counts_fail_closed() {
        let bad_lut = "LUT_3D_SIZE 33\n0.0 0.0 0.0\n1.0 1.0 1.0\n";
        let err = CubeLut::parse(bad_lut).expect_err("truncated .cube LUT must fail closed");
        assert!(matches!(err, CoreError::ExportFailed(_)));
    }

    #[rstest]
    #[case::vtt_map_cols_grid_overflow(ExportProfile::VttMap(VttMapOptions {
        cols: u32::MAX,
        rows: 10,
        grid_px: 140,
        scale_factor: None,
        jpeg_quality: 90,
    }))]
    #[case::vtt_map_rows_grid_overflow(ExportProfile::VttMap(VttMapOptions {
        cols: 10,
        rows: u32::MAX,
        grid_px: 140,
        scale_factor: None,
        jpeg_quality: 90,
    }))]
    #[case::vtt_map_exceeds_max_axis_dim(ExportProfile::VttMap(VttMapOptions {
        cols: 200,
        rows: 10,
        grid_px: 140,
        scale_factor: None,
        jpeg_quality: 90,
    }))]
    #[case::vtt_map_exceeds_total_pixels(ExportProfile::VttMap(VttMapOptions {
        cols: 100,
        rows: 100,
        grid_px: 100,
        scale_factor: None,
        jpeg_quality: 90,
    }))]
    #[case::vtt_map_huge_scale_factor_no_saturation(ExportProfile::VttMap(VttMapOptions {
        cols: 10,
        rows: 10,
        grid_px: 100,
        scale_factor: Some(1e12),
        jpeg_quality: 90,
    }))]
    #[case::vtt_map_infinite_scale_factor(ExportProfile::VttMap(VttMapOptions {
        cols: 10,
        rows: 10,
        grid_px: 100,
        scale_factor: Some(f64::INFINITY),
        jpeg_quality: 90,
    }))]
    #[case::vtt_map_scaled_axis_exceeds_max_dim(ExportProfile::VttMap(VttMapOptions {
        cols: 10,
        rows: 10,
        grid_px: 100,
        scale_factor: Some(500.0),
        jpeg_quality: 90,
    }))]
    #[case::vtt_token_size_below_min(ExportProfile::VttToken(VttTokenOptions {
        size_px: 15,
        ..VttTokenOptions::default()
    }))]
    #[case::vtt_token_size_above_max(ExportProfile::VttToken(VttTokenOptions {
        size_px: MAX_VTT_TOKEN_SIZE_PX + 1,
        ..VttTokenOptions::default()
    }))]
    #[case::vtt_token_size_u32_max(ExportProfile::VttToken(VttTokenOptions {
        size_px: u32::MAX,
        ..VttTokenOptions::default()
    }))]
    fn export_rejects_overflowing_and_oversized_dimensions_fail_closed(
        #[case] profile: ExportProfile,
    ) {
        let root = TempDir::new().unwrap();
        let store = seed_run(
            root.path(),
            "harbor-dock",
            Visibility::Public,
            64,
            64,
            [90, 110, 130, 255],
        );
        let err = execute_export(
            Some(&store),
            &ExportRequest {
                source: ExportSource::Run {
                    slug: "harbor-dock".to_string(),
                    version: 1,
                },
                output_slug: "harbor-dock".to_string(),
                profile_visibility: Visibility::Public,
                out_dir: root.path().join("exports"),
                profile,
                match_grade: None,
            },
        )
        .expect_err("oversized or overflowing Export profile must fail closed");
        assert!(matches!(err, CoreError::ExportFailed(_)));
    }

    #[rstest]
    #[case::internal_run_png_blocked_for_public_export(
        Visibility::Internal,
        Visibility::Public,
        false
    )]
    #[case::restricted_run_png_blocked_for_public_export(
        Visibility::Restricted,
        Visibility::Public,
        false
    )]
    #[case::internal_run_png_allowed_for_internal_export(
        Visibility::Internal,
        Visibility::Internal,
        true
    )]
    fn export_png_path_raises_effective_visibility_from_run_store(
        #[case] run_vis: Visibility,
        #[case] export_vis: Visibility,
        #[case] expect_ok: bool,
    ) {
        let root = TempDir::new().unwrap();
        let store = seed_run(
            root.path(),
            "secret-run",
            run_vis,
            64,
            64,
            [90, 110, 130, 255],
        );
        let run_png = store.run_image_path("secret-run", 1);

        // Programmatic caller specifies Visibility::Public on PngPath, but RunStore must raise it.
        let res = execute_export(
            Some(&store),
            &ExportRequest {
                source: ExportSource::PngPath {
                    path: run_png,
                    asset_id: "secret-run".to_string(),
                    visibility: Visibility::Public,
                },
                output_slug: "secret-run".to_string(),
                profile_visibility: export_vis,
                out_dir: root.path().join("exports"),
                profile: ExportProfile::WebWebp,
                match_grade: None,
            },
        );
        assert_eq!(res.is_ok(), expect_ok);
        if let Ok(ok) = res {
            assert_eq!(ok.source_visibility, run_vis);
        } else {
            assert!(matches!(
                res.unwrap_err(),
                CoreError::VisibilityViolation { .. }
            ));
        }
    }
}
