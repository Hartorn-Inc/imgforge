//! === HOST-COMPOSITING CALIBRATION STRIP & STRUCTURAL SHEET VALIDATION ===
//!
//! PURPOSE:
//! Deterministically composites the `#252830` calibration strip (6-step `L*`
//! neutral gray wedge + exact `48x48 px` `PaletteLock` swatches) onto the bottom
//! of `ReferencePack` sheets on the Rust host, and validates sheet dimensions and
//! `#252830` studio background coverage (ADR 0005).
//!
//! THREAT MODEL / DESIGN RATIONALE ("WHY"):
//! Generative models alter unshaded swatch RGB values when asked to draw hex
//! swatches directly inside an artwork prompt. Compositing the `#252830`
//! calibration strip deterministically on the host guarantees pixel-exact
//! `PaletteLock` RGB ground truth for downstream OKLab/`DeltaE_2000` QA without
//! trusting model-drawn color bars.
//!
//! EXECUTION LIFECYCLE / DATA FLOW:
//! 1. `validate_reference_sheet_structure` verifies exact `1536x1024` dimensions
//!    and `#252830` studio backdrop coverage before calibration compositing.
//! 2. `composite_calibration_strip` computes the 6-step `L*` neutral gray wedge via
//!    `palette::Lab -> Srgb<u8>`, paints `#252830` across the bottom strip, and
//!    stamps exact `48x48 px` `PaletteLock` swatches pixel-for-pixel.

use std::io::Cursor;
use std::path::Path;

use image::{DynamicImage, ImageFormat, Rgba, RgbaImage};
use palette::{Clamp, IntoColor, Lab, Srgb};
use serde::{Deserialize, Serialize};

use crate::backend::PNG_SIGNATURE;
use crate::error::CoreError;
use crate::slicer::PixelRect;
use crate::subject::PaletteLock;

/// Canonical neutral studio backdrop color (`#252830`) used across `ReferencePack` and `Atlas` sheets.
pub const STUDIO_BG_RGB: [u8; 3] = [0x25, 0x28, 0x30];

/// Canonical `#252830` RGBA pixel value.
pub const STUDIO_BG_RGBA: [u8; 4] = [0x25, 0x28, 0x30, 0xFF];

/// Canonical hex string for the neutral studio backdrop.
pub const STUDIO_BG_HEX: &str = "#252830";

/// Standard cell side length (`48x48 px`) for calibration strip wedge steps and `PaletteLock` swatches.
pub const CALIBRATION_CELL_SIZE_PX: u32 = 48;

/// Standard calibration strip height (`72 px`: `12 px` top/bottom margin around `48 px` cells).
pub const CALIBRATION_STRIP_HEIGHT_PX: u32 = 72;

/// Six perceptually uniform CIE `L*` neutral gray steps (`a*=0, b*=0`).
pub const GRAY_WEDGE_L_STAR_STEPS: [f32; 6] = [15.0, 30.0, 45.0, 60.0, 75.0, 90.0];

/// Configuration thresholds for `#252830` sheet structural validation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StructuralValidationConfig {
    /// Maximum Euclidean RGB distance from `#252830` considered studio background.
    pub bg_color_tolerance: f64,
    /// Minimum overall fraction (`0.0..=1.0`) of `#252830` studio backdrop pixels.
    pub min_bg_coverage: f64,
    /// Minimum fraction (`0.0..=1.0`) of 1 px border pixels matching `#252830`.
    pub min_border_bg_coverage: f64,
    /// Minimum non-background foreground pixels required so blank sheets fail closed.
    pub min_foreground_pixels: u64,
}

impl Default for StructuralValidationConfig {
    fn default() -> Self {
        Self {
            bg_color_tolerance: 28.0,
            min_bg_coverage: 0.15,
            min_border_bg_coverage: 0.50,
            min_foreground_pixels: 256,
        }
    }
}

/// Structural validation metrics returned by `validate_reference_sheet_structure`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SheetStructuralReport {
    /// Verified image width in pixels.
    pub width: u32,
    /// Verified image height in pixels.
    pub height: u32,
    /// Fraction of pixels within tolerance of `#252830`.
    pub bg_coverage: f64,
    /// Fraction of 1 px perimeter pixels within tolerance of `#252830`.
    pub border_bg_coverage: f64,
    /// Number of non-background foreground pixels.
    pub foreground_pixels: u64,
}

/// Recorded placement of a single `PaletteLock` swatch inside the calibration strip.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CalibrationSwatchCell {
    /// Swatch name from `Subject.palette_lock`.
    pub name: String,
    /// Normalized uppercase `#RRGGBB` hex color stamped pixel-for-pixel.
    pub color_hex: String,
    /// Exact pixel rectangle inside the sheet image.
    pub rect: PixelRect,
}

/// Exact pixel geometry of the host-composited `#252830` calibration strip.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CalibrationStripLayout {
    /// Top Y row where the `#252830` calibration strip begins.
    pub strip_y: u32,
    /// Total height in pixels of the bottom calibration strip.
    pub strip_height: u32,
    /// Side length of each gray wedge / swatch cell in pixels (`48` when single-row fits).
    pub cell_size_px: u32,
    /// Pixel rectangles and RGB values of the 6-step `L*` neutral gray wedge.
    pub gray_wedge_cells: Vec<(PixelRect, [u8; 3])>,
    /// Pixel rectangles and metadata of each stamped `PaletteLock` swatch.
    pub palette_swatch_cells: Vec<CalibrationSwatchCell>,
}

/// Parse a `#RRGGBB` hex color string into `[r, g, b]`.
pub fn parse_hex_rgb(hex: &str) -> Result<[u8; 3], CoreError> {
    let trimmed = hex.trim();
    let Some(body) = trimmed.strip_prefix('#') else {
        return Err(CoreError::SubjectInvalid(format!(
            "hex color '{hex}' must start with '#'"
        )));
    };
    if body.len() != 6 || !body.is_ascii() {
        return Err(CoreError::SubjectInvalid(format!(
            "hex color '{hex}' must be 6 hex digits (#RRGGBB)"
        )));
    }
    let r = u8::from_str_radix(&body[0..2], 16)
        .map_err(|err| CoreError::SubjectInvalid(format!("invalid red hex in '{hex}': {err}")))?;
    let g = u8::from_str_radix(&body[2..4], 16)
        .map_err(|err| CoreError::SubjectInvalid(format!("invalid green hex in '{hex}': {err}")))?;
    let b = u8::from_str_radix(&body[4..6], 16)
        .map_err(|err| CoreError::SubjectInvalid(format!("invalid blue hex in '{hex}': {err}")))?;
    Ok([r, g, b])
}

/// Compute the 6-step `L*` neutral gray wedge RGB triples via `palette::Lab`.
#[must_use]
pub fn compute_l_star_gray_wedge() -> Vec<[u8; 3]> {
    GRAY_WEDGE_L_STAR_STEPS
        .iter()
        .map(|&l_star| {
            let lab = Lab::new(l_star, 0.0, 0.0);
            let srgb_f: Srgb<f32> = lab.into_color();
            let srgb_u8: Srgb<u8> = srgb_f.clamp().into_format();
            [srgb_u8.red, srgb_u8.green, srgb_u8.blue]
        })
        .collect()
}

/// Compute Euclidean RGB distance between an `[r, g, b]` pixel and `#252830`.
#[must_use]
pub fn studio_bg_distance(rgb: [u8; 3]) -> f64 {
    let dr = f64::from(rgb[0]) - f64::from(STUDIO_BG_RGB[0]);
    let dg = f64::from(rgb[1]) - f64::from(STUDIO_BG_RGB[1]);
    let db = f64::from(rgb[2]) - f64::from(STUDIO_BG_RGB[2]);
    (dr * dr + dg * dg + db * db).sqrt()
}

/// Validate sheet PNG signature, exact dimensions, `#252830` backdrop coverage, and non-blank foreground.
pub fn validate_reference_sheet_structure(
    png_bytes: &[u8],
    expected_width: u32,
    expected_height: u32,
    config: &StructuralValidationConfig,
) -> Result<SheetStructuralReport, CoreError> {
    if png_bytes.len() < PNG_SIGNATURE.len() || png_bytes[..PNG_SIGNATURE.len()] != PNG_SIGNATURE {
        return Err(CoreError::ReferencePackFailed(
            "generated sheet is missing valid 8-byte PNG signature".to_string(),
        ));
    }

    let decoded =
        image::load_from_memory_with_format(png_bytes, ImageFormat::Png).map_err(|err| {
            CoreError::InvalidPng {
                path: Path::new("reference_sheet.png").to_path_buf(),
                reason: format!("PNG decode failed: {err}"),
            }
        })?;

    let width = decoded.width();
    let height = decoded.height();
    if width != expected_width || height != expected_height {
        return Err(CoreError::ReferencePackFailed(format!(
            "sheet dimensions {width}x{height} do not match required {expected_width}x{expected_height}"
        )));
    }

    let rgba = decoded.to_rgba8();
    let total_pixels = u64::from(width) * u64::from(height);
    let mut bg_pixels = 0u64;
    let mut fg_pixels = 0u64;
    let mut border_total = 0u64;
    let mut border_bg = 0u64;

    for (x, y, px) in rgba.enumerate_pixels() {
        let is_bg = px.0[3] > 0
            && studio_bg_distance([px.0[0], px.0[1], px.0[2]]) <= config.bg_color_tolerance;
        if is_bg {
            bg_pixels += 1;
        } else {
            fg_pixels += 1;
        }
        if x == 0 || y == 0 || x == width - 1 || y == height - 1 {
            border_total += 1;
            if is_bg {
                border_bg += 1;
            }
        }
    }

    let bg_coverage = bg_pixels as f64 / total_pixels.max(1) as f64;
    let border_bg_coverage = border_bg as f64 / border_total.max(1) as f64;

    if bg_coverage < config.min_bg_coverage {
        return Err(CoreError::ReferencePackFailed(format!(
            "studio #252830 background coverage {:.1}% is below minimum {:.1}%",
            bg_coverage * 100.0,
            config.min_bg_coverage * 100.0
        )));
    }
    if border_bg_coverage < config.min_border_bg_coverage {
        return Err(CoreError::ReferencePackFailed(format!(
            "sheet perimeter #252830 coverage {:.1}% is below minimum {:.1}%",
            border_bg_coverage * 100.0,
            config.min_border_bg_coverage * 100.0
        )));
    }
    if fg_pixels < config.min_foreground_pixels {
        return Err(CoreError::ReferencePackFailed(format!(
            "sheet foreground has only {fg_pixels} pixels (minimum {} required)",
            config.min_foreground_pixels
        )));
    }

    Ok(SheetStructuralReport {
        width,
        height,
        bg_coverage,
        border_bg_coverage,
        foreground_pixels: fg_pixels,
    })
}

/// Compute exact horizontal width required to render `total_cells` calibration cells.
fn required_strip_width(
    total_cells: usize,
    margin_x: u32,
    cell_size: u32,
    gap: u32,
    group_gap: u32,
) -> u64 {
    let n = total_cells as u64;
    let gaps = n.saturating_sub(2);
    u64::from(margin_x) * 2
        + n.saturating_mul(u64::from(cell_size))
        + gaps.saturating_mul(u64::from(gap))
        + u64::from(group_gap)
}

/// Composite the deterministic `#252830` bottom calibration strip onto an in-memory `RgbaImage`.
///
/// Renders a 6-step `L*` neutral gray wedge (`48x48 px` when fitting in `width`) followed
/// by exact `48x48 px` `PaletteLock` swatches matching `Subject.palette_lock` pixel-for-pixel.
pub fn composite_calibration_strip(
    sheet: &mut RgbaImage,
    palette_lock: &PaletteLock,
) -> Result<CalibrationStripLayout, CoreError> {
    if palette_lock.swatches.is_empty() {
        return Err(CoreError::ReferencePackFailed(
            "cannot composite calibration strip without PaletteLock swatches".to_string(),
        ));
    }

    let width = sheet.width();
    let height = sheet.height();
    let margin_x = 16u32;
    let gap = 8u32;
    let group_gap = 24u32;
    let wedge = compute_l_star_gray_wedge();
    let total_cells = wedge.len() + palette_lock.swatches.len();

    // Use exact 48x48 cells whenever they fit inside the sheet width; otherwise scale cells
    // down to at least 8 px and fail closed if the required strip width still exceeds `width`.
    let required_48 = required_strip_width(
        total_cells,
        margin_x,
        CALIBRATION_CELL_SIZE_PX,
        gap,
        group_gap,
    );
    let (cell_size, effective_gap, effective_group_gap) = if required_48 <= u64::from(width) {
        (CALIBRATION_CELL_SIZE_PX, gap, group_gap)
    } else {
        let compact_gap = 4u32;
        let compact_group_gap = 12u32;
        let fixed_overhead = u64::from(margin_x) * 2
            + (total_cells as u64).saturating_sub(2) * u64::from(compact_gap)
            + u64::from(compact_group_gap);
        let usable = u64::from(width).saturating_sub(fixed_overhead);
        let scaled =
            ((usable / (total_cells as u64).max(1)) as u32).clamp(8, CALIBRATION_CELL_SIZE_PX);
        (scaled, compact_gap, compact_group_gap)
    };

    let required_width = required_strip_width(
        total_cells,
        margin_x,
        cell_size,
        effective_gap,
        effective_group_gap,
    );
    if required_width > u64::from(width) {
        return Err(CoreError::ReferencePackFailed(format!(
            "calibration strip requires {required_width}px width for {total_cells} cells (cell_size={cell_size}px), exceeding sheet width {width}px"
        )));
    }

    let pad_y = 12u32;
    let strip_height = cell_size + pad_y * 2;
    if height <= strip_height {
        return Err(CoreError::ReferencePackFailed(format!(
            "sheet {width}x{height} is too short for calibration strip ({strip_height}px tall)"
        )));
    }
    let strip_y = height - strip_height;

    // 1. Fill the entire calibration strip with exact #252830 studio backdrop.
    for y in strip_y..height {
        for x in 0..width {
            sheet.put_pixel(x, y, Rgba(STUDIO_BG_RGBA));
        }
    }

    let cell_y = strip_y + pad_y;
    let mut cursor_x = margin_x;
    let mut gray_wedge_cells = Vec::with_capacity(wedge.len());

    // 2. Paint 6-step L* neutral gray wedge.
    for rgb in wedge {
        let rect = PixelRect {
            x: cursor_x,
            y: cell_y,
            width: cell_size,
            height: cell_size,
        };
        fill_rect(sheet, rect, rgb);
        gray_wedge_cells.push((rect, rgb));
        cursor_x += cell_size + effective_gap;
    }

    cursor_x += effective_group_gap.saturating_sub(effective_gap);

    // 3. Paint exact PaletteLock swatches pixel-for-pixel.
    let mut palette_swatch_cells = Vec::with_capacity(palette_lock.swatches.len());
    for swatch in &palette_lock.swatches {
        let rgb = parse_hex_rgb(&swatch.color_hex)?;
        let rect = PixelRect {
            x: cursor_x,
            y: cell_y,
            width: cell_size,
            height: cell_size,
        };
        fill_rect(sheet, rect, rgb);
        palette_swatch_cells.push(CalibrationSwatchCell {
            name: swatch.name.clone(),
            color_hex: swatch.color_hex.clone(),
            rect,
        });
        cursor_x += cell_size + effective_gap;
    }

    Ok(CalibrationStripLayout {
        strip_y,
        strip_height,
        cell_size_px: cell_size,
        gray_wedge_cells,
        palette_swatch_cells,
    })
}

/// Decode a sheet PNG, composite the `#252830` calibration strip, and re-encode as PNG.
pub fn composite_calibration_strip_png(
    png_bytes: &[u8],
    palette_lock: &PaletteLock,
) -> Result<(Vec<u8>, CalibrationStripLayout), CoreError> {
    let decoded =
        image::load_from_memory_with_format(png_bytes, ImageFormat::Png).map_err(|err| {
            CoreError::InvalidPng {
                path: Path::new("reference_sheet.png").to_path_buf(),
                reason: format!("PNG decode failed before calibration compositing: {err}"),
            }
        })?;
    let mut rgba = decoded.to_rgba8();
    let layout = composite_calibration_strip(&mut rgba, palette_lock)?;

    let mut out = Vec::new();
    DynamicImage::ImageRgba8(rgba)
        .write_to(&mut Cursor::new(&mut out), ImageFormat::Png)
        .map_err(|err| CoreError::ReferencePackFailed(format!("failed to encode PNG: {err}")))?;
    Ok((out, layout))
}

fn fill_rect(img: &mut RgbaImage, rect: PixelRect, rgb: [u8; 3]) {
    let pixel = Rgba([rgb[0], rgb[1], rgb[2], 255]);
    for y in rect.y..rect.bottom() {
        for x in rect.x..rect.right() {
            img.put_pixel(x, y, pixel);
        }
    }
}

#[cfg(test)]
mod tests {
    use image::ImageBuffer;
    use rstest::rstest;

    use super::*;
    use crate::subject::PaletteSwatch;

    fn make_sheet_png(
        width: u32,
        height: u32,
        bg: [u8; 3],
        fg_rect: Option<(u32, u32, [u8; 3])>,
    ) -> Vec<u8> {
        let mut img: ImageBuffer<Rgba<u8>, Vec<u8>> =
            ImageBuffer::from_pixel(width, height, Rgba([bg[0], bg[1], bg[2], 255]));
        if let Some((fw, fh, fg_rgb)) = fg_rect {
            let start_x = (width - fw) / 2;
            let start_y = (height - fh) / 2;
            for y in start_y..(start_y + fh) {
                for x in start_x..(start_x + fw) {
                    img.put_pixel(x, y, Rgba([fg_rgb[0], fg_rgb[1], fg_rgb[2], 255]));
                }
            }
        }
        let mut buf = Vec::new();
        DynamicImage::ImageRgba8(img)
            .write_to(&mut Cursor::new(&mut buf), ImageFormat::Png)
            .unwrap();
        buf
    }

    #[test]
    fn composites_pixel_exact_48x48_palette_lock_swatches_and_6_step_gray_wedge() {
        let raw_png = make_sheet_png(1536, 1024, STUDIO_BG_RGB, Some((300, 500, [180, 120, 90])));
        let palette_lock = PaletteLock {
            swatches: vec![
                PaletteSwatch {
                    name: "coat_navy".to_string(),
                    color_hex: "#2C3E50".to_string(),
                    surface_slot: Some("coat".to_string()),
                },
                PaletteSwatch {
                    name: "scarf_rust".to_string(),
                    color_hex: "#B84A39".to_string(),
                    surface_slot: Some("scarf".to_string()),
                },
                PaletteSwatch {
                    name: "brass_trim".to_string(),
                    color_hex: "#D4A843".to_string(),
                    surface_slot: None,
                },
            ],
        };

        let (composited_png, layout) =
            composite_calibration_strip_png(&raw_png, &palette_lock).expect("composite succeeds");
        assert_eq!(layout.cell_size_px, 48);
        assert_eq!(layout.strip_height, 72);
        assert_eq!(layout.strip_y, 1024 - 72);
        assert_eq!(layout.gray_wedge_cells.len(), 6);
        assert_eq!(layout.palette_swatch_cells.len(), 3);

        // Gray wedge must be strictly monotonic from dark to light and neutral (R == G == B).
        let mut prev_luma = 0u8;
        for (idx, (rect, rgb)) in layout.gray_wedge_cells.iter().enumerate() {
            assert_eq!((rect.width, rect.height), (48, 48));
            assert!((rgb[0] as i16 - rgb[1] as i16).abs() <= 1);
            assert!((rgb[1] as i16 - rgb[2] as i16).abs() <= 1);
            if idx > 0 {
                assert!(rgb[0] > prev_luma);
            }
            prev_luma = rgb[0];
        }

        // Re-decode final PNG and verify pixel-for-pixel PaletteLock RGB values across every 48x48 cell.
        let decoded = image::load_from_memory(&composited_png).unwrap().to_rgba8();
        assert_eq!((decoded.width(), decoded.height()), (1536, 1024));

        for cell in &layout.palette_swatch_cells {
            assert_eq!((cell.rect.width, cell.rect.height), (48, 48));
            let expected_rgb = parse_hex_rgb(&cell.color_hex).unwrap();
            for y in cell.rect.y..cell.rect.bottom() {
                for x in cell.rect.x..cell.rect.right() {
                    let px = decoded.get_pixel(x, y).0;
                    assert_eq!(
                        [px[0], px[1], px[2]],
                        expected_rgb,
                        "pixel ({x},{y}) in swatch '{}' must match exact PaletteLock RGB",
                        cell.name
                    );
                }
            }
        }

        // Verify surrounding strip background pixels are exact #252830.
        let bg_px = decoded.get_pixel(4, layout.strip_y + 4).0;
        assert_eq!([bg_px[0], bg_px[1], bg_px[2]], STUDIO_BG_RGB);
    }

    #[rstest]
    #[case::valid_studio_sheet(1536, 1024, STUDIO_BG_RGB, Some((240, 400, [200, 180, 160])), true)]
    #[case::wrong_dimensions(1024, 1024, STUDIO_BG_RGB, Some((200, 200, [200, 180, 160])), false)]
    #[case::wrong_background_color(1536, 1024, [245, 245, 245], Some((240, 400, [20, 20, 20])), false)]
    #[case::blank_studio_sheet_without_foreground(1536, 1024, STUDIO_BG_RGB, None, false)]
    fn structural_validation_gates_dimensions_backdrop_and_foreground(
        #[case] width: u32,
        #[case] height: u32,
        #[case] bg: [u8; 3],
        #[case] fg: Option<(u32, u32, [u8; 3])>,
        #[case] expect_ok: bool,
    ) {
        let png = make_sheet_png(width, height, bg, fg);
        let res = validate_reference_sheet_structure(
            &png,
            1536,
            1024,
            &StructuralValidationConfig::default(),
        );
        assert_eq!(res.is_ok(), expect_ok);
    }

    #[rstest]
    #[case::excessive_swatch_count_on_standard_sheet(1536, 1024, 140)]
    #[case::narrow_sheet_with_multiple_swatches(128, 256, 8)]
    #[case::too_narrow_even_for_minimum_cells(96, 256, 1)]
    fn composite_calibration_strip_fails_closed_without_panicking_when_width_exceeded(
        #[case] width: u32,
        #[case] height: u32,
        #[case] swatch_count: usize,
    ) {
        let raw_png = make_sheet_png(width, height, STUDIO_BG_RGB, Some((32, 32, [180, 120, 90])));
        let swatches = (0..swatch_count)
            .map(|idx| PaletteSwatch {
                name: format!("swatch_{idx}"),
                color_hex: "#2C3E50".to_string(),
                surface_slot: None,
            })
            .collect();
        let palette_lock = PaletteLock { swatches };

        let err = composite_calibration_strip_png(&raw_png, &palette_lock)
            .expect_err("overflowing calibration strip must fail closed");
        assert!(matches!(err, CoreError::ReferencePackFailed(_)));
    }
}
