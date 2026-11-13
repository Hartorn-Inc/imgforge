//! === FOREGROUND COMPONENT BOUNDING-BOX SLICER ===
//!
//! PURPOSE:
//! Segments foreground components against the `#252830` studio backdrop using
//! color-distance masking, separable morphological closing, connected-component
//! extraction, optional grid priors, and manual `RectOverride` boxes, exporting
//! `slices/<name>.png` and `slices.json` (ADR 0005).
//!
//! THREAT MODEL / DESIGN RATIONALE ("WHY"):
//! 1. Blind fixed-grid slicing clips character silhouettes whenever generative
//!    models shift pose spacing across a `ReferencePack` sheet. Foreground
//!    bounding-box detection extracts each pose/item tightly without clipping.
//! 2. Thin detached strokes or floating accessories (e.g. lantern glow or boot
//!    tip) are bridged via `merge_gap_px` morphological dilation, while the final
//!    bounding box is computed over the original un-dilated foreground mask so
//!    bounds stay crisp.
//! 3. Calibration strip rows at the bottom of a sheet are excluded before slicing
//!    so color swatches are never mistaken for subject components.
//!
//! EXECUTION LIFECYCLE / DATA FLOW:
//! 1. Compute foreground mask against `#252830` inside `0..active_height`.
//! 2. Bridge gaps (`merge_gap_px`), label 8-connected components, and optionally
//!    group by `(cols, rows)` grid priors.
//! 3. Apply `RectOverride`s (`ReplaceAll` or `MergeById`), crop PNG slices,
//!    and write `slices/<id>.png` + `slices.json` atomically.

use std::collections::{BTreeMap, HashSet, VecDeque};
use std::fs;
use std::io::Cursor;
use std::path::Path;

use image::imageops::crop_imm;
use image::{DynamicImage, ImageFormat, RgbaImage};
use serde::{Deserialize, Serialize};

use crate::calibration::{STUDIO_BG_RGB, studio_bg_distance};
use crate::error::CoreError;
use crate::lifecycle::sha256_hex;
use crate::store::{
    decode_png_checked, read_regular_file_nofollow, verify_contained_path_no_symlinks,
    verify_existing_dir_prefix_no_symlinks, write_atomic,
};

/// Supported `SlicesManifest` schema version.
pub const SUPPORTED_SLICES_SCHEMA_VERSION: u32 = 1;

/// Axis-aligned pixel rectangle inside a raster image.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PixelRect {
    /// Left X coordinate in pixels.
    pub x: u32,
    /// Top Y coordinate in pixels.
    pub y: u32,
    /// Rectangle width in pixels (`> 0`).
    pub width: u32,
    /// Rectangle height in pixels (`> 0`).
    pub height: u32,
}

impl PixelRect {
    /// Exclusive right X boundary (`x + width`).
    #[must_use]
    pub const fn right(self) -> u32 {
        self.x.saturating_add(self.width)
    }

    /// Exclusive bottom Y boundary (`y + height`).
    #[must_use]
    pub const fn bottom(self) -> u32 {
        self.y.saturating_add(self.height)
    }

    /// Return `true` when `self` and `other` have non-zero area overlap.
    #[must_use]
    pub const fn intersects(self, other: Self) -> bool {
        self.x < other.right()
            && other.x < self.right()
            && self.y < other.bottom()
            && other.y < self.bottom()
    }
}

/// Manual bounding-box override supplied by a user or the Web Canvas UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RectOverride {
    /// Stable slice identifier (for example `"front-turnaround"` or `"slice_001"`).
    pub id: String,
    /// Exact pixel rectangle to crop.
    pub rect: PixelRect,
    /// Optional human-readable label.
    #[serde(default)]
    pub label: Option<String>,
}

/// How manual `RectOverride` entries interact with automatic foreground detection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OverrideMode {
    /// Replace auto-detected components completely with explicit manual overrides when provided.
    #[default]
    ReplaceAll,
    /// Keep auto-detected components and replace/append entries matching `RectOverride.id`.
    MergeById,
}

/// Source indicating whether a slice bounding box was auto-detected or manually overridden.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SliceSource {
    /// Automatically extracted via `#252830` foreground component segmentation.
    Auto,
    /// Explicitly specified via `RectOverride`.
    ManualOverride,
}

/// Tunable parameters for foreground bounding-box slicing against `#252830`.
#[derive(Debug, Clone, PartialEq)]
pub struct SlicerConfig {
    /// Background RGB color (`#252830`).
    pub bg_rgb: [u8; 3],
    /// Euclidean RGB distance above which a pixel is treated as foreground.
    pub bg_distance_threshold: f64,
    /// Bottom rows excluded from foreground detection (for example `72` for calibration strip).
    pub exclude_bottom_rows: u32,
    /// Gap-bridging dilation diameter in pixels (`0` disables bridging).
    pub merge_gap_px: u32,
    /// Minimum detected component width in pixels.
    pub min_component_width: u32,
    /// Minimum detected component height in pixels.
    pub min_component_height: u32,
    /// Minimum detected foreground pixel count inside the component.
    pub min_component_area: u32,
    /// Margin added around each detected bounding box (clamped to active sheet bounds).
    pub padding_px: u32,
    /// Optional `(cols, rows)` grid prior merging components whose centers fall in the same cell.
    pub grid_priors: Option<(u32, u32)>,
    /// Optional prefix applied to auto-generated slice IDs (for example `"turnaround"`).
    pub id_prefix: Option<String>,
    /// Optional manual `RectOverride` entries.
    pub overrides: Vec<RectOverride>,
    /// Interaction mode for `overrides`.
    pub override_mode: OverrideMode,
}

impl Default for SlicerConfig {
    fn default() -> Self {
        Self {
            bg_rgb: STUDIO_BG_RGB,
            bg_distance_threshold: 28.0,
            exclude_bottom_rows: 0,
            merge_gap_px: 8,
            min_component_width: 16,
            min_component_height: 16,
            min_component_area: 128,
            padding_px: 4,
            grid_priors: None,
            id_prefix: None,
            overrides: Vec::new(),
            override_mode: OverrideMode::ReplaceAll,
        }
    }
}

/// Single exported slice entry recorded inside `slices.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SliceRecord {
    /// Stable slice identifier (unique within the pack/manifest).
    pub id: String,
    /// Relative path to the cropped slice PNG (for example `"slices/turnaround_001.png"`).
    pub file: String,
    /// Exact pixel crop rectangle on the source sheet.
    pub rect: PixelRect,
    /// How this slice rectangle was determined.
    pub source: SliceSource,
    /// Optional descriptive label.
    #[serde(default)]
    pub label: Option<String>,
    /// Lowercase SHA-256 hex digest of the exported slice PNG bytes.
    pub sha256: String,
}

/// Persisted `slices.json` manifest describing all sliced components of a sheet or pack.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SlicesManifest {
    /// Schema version; must equal `1`.
    pub schema_version: u32,
    /// Source sheet filename (for example `"turnaround.png"`).
    pub sheet_filename: String,
    /// Source sheet width in pixels.
    pub sheet_width: u32,
    /// Source sheet height in pixels.
    pub sheet_height: u32,
    /// Number of bottom calibration strip rows excluded during slicing.
    pub calibration_strip_height: u32,
    /// Ordered list of extracted slice records.
    pub slices: Vec<SliceRecord>,
}

/// In-memory sliced component paired with its encoded PNG bytes before disk export.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractedSlice {
    /// Slice metadata record (`file` reflects `"slices/<id>.png"`).
    pub record: SliceRecord,
    /// Cropped PNG bytes.
    pub png_bytes: Vec<u8>,
}

/// Detect and crop foreground components from an in-memory `RgbaImage`.
pub fn slice_sheet_image(
    sheet: &RgbaImage,
    config: &SlicerConfig,
) -> Result<Vec<ExtractedSlice>, CoreError> {
    let width = sheet.width();
    let height = sheet.height();
    if width == 0 || height == 0 {
        return Err(CoreError::SlicerFailed(
            "cannot slice an empty 0-pixel image".to_string(),
        ));
    }
    if config.exclude_bottom_rows >= height {
        return Err(CoreError::SlicerFailed(format!(
            "exclude_bottom_rows ({}) must be less than sheet height ({height})",
            config.exclude_bottom_rows
        )));
    }
    let validated_prefix = match config.id_prefix {
        Some(ref raw_prefix) => Some(validate_slice_identifier(
            raw_prefix,
            "SlicerConfig::id_prefix",
        )?),
        None => None,
    };
    let active_height = height - config.exclude_bottom_rows;

    let mut planned: Vec<(String, PixelRect, SliceSource, Option<String>)> = Vec::new();
    let use_only_overrides =
        !config.overrides.is_empty() && config.override_mode == OverrideMode::ReplaceAll;

    if use_only_overrides {
        let mut seen_ids = HashSet::with_capacity(config.overrides.len());
        for ov in &config.overrides {
            validate_rect_override(ov, width, height, &mut seen_ids)?;
            planned.push((
                ov.id.trim().to_string(),
                ov.rect,
                SliceSource::ManualOverride,
                ov.label.clone(),
            ));
        }
    } else {
        let auto_rects = detect_foreground_components(sheet, active_height, config)?;

        for (idx, rect) in auto_rects.into_iter().enumerate() {
            let id = match validated_prefix {
                Some(ref p) => format!("{p}_{:03}", idx + 1),
                None => format!("slice_{:03}", idx + 1),
            };
            planned.push((id, rect, SliceSource::Auto, None));
        }

        if !config.overrides.is_empty() {
            let mut by_id: BTreeMap<String, usize> = BTreeMap::new();
            for (idx, (id, _, _, _)) in planned.iter().enumerate() {
                by_id.insert(id.clone(), idx);
            }
            let mut seen_override_ids = HashSet::with_capacity(config.overrides.len());
            for ov in &config.overrides {
                validate_rect_override(ov, width, height, &mut seen_override_ids)?;
                let clean_id = ov.id.trim().to_string();
                if let Some(&existing_idx) = by_id.get(&clean_id) {
                    planned[existing_idx] = (
                        clean_id,
                        ov.rect,
                        SliceSource::ManualOverride,
                        ov.label.clone(),
                    );
                } else {
                    by_id.insert(clean_id.clone(), planned.len());
                    planned.push((
                        clean_id,
                        ov.rect,
                        SliceSource::ManualOverride,
                        ov.label.clone(),
                    ));
                }
            }
        }
    }

    if planned.is_empty() {
        return Err(CoreError::SlicerFailed(
            "no foreground components or manual overrides produced any slices".to_string(),
        ));
    }

    let mut extracted = Vec::with_capacity(planned.len());
    for (id, rect, source, label) in planned {
        let sub = crop_imm(sheet, rect.x, rect.y, rect.width, rect.height).to_image();
        let mut png_bytes = Vec::new();
        DynamicImage::ImageRgba8(sub)
            .write_to(&mut Cursor::new(&mut png_bytes), ImageFormat::Png)
            .map_err(|err| CoreError::SlicerFailed(format!("encoding slice '{id}' PNG: {err}")))?;
        let sha256 = sha256_hex(&png_bytes);
        extracted.push(ExtractedSlice {
            record: SliceRecord {
                file: format!("slices/{id}.png"),
                id,
                rect,
                source,
                label,
                sha256,
            },
            png_bytes,
        });
    }

    Ok(extracted)
}

/// Detect foreground components on `sheet_path`, write `slices/<id>.png`, and write `slices.json`.
pub fn slice_sheet_to_dir(
    sheet_path: &Path,
    out_dir: &Path,
    config: &SlicerConfig,
) -> Result<SlicesManifest, CoreError> {
    let raw = read_regular_file_nofollow(sheet_path, "sheet PNG")?;
    let decoded = decode_png_checked(sheet_path, &raw)?;
    let rgba = decoded.to_rgba8();
    let extracted = slice_sheet_image(&rgba, config)?;

    ensure_out_dir_no_symlinks(out_dir)?;
    verify_existing_dir_prefix_no_symlinks(
        out_dir,
        Path::new("slices"),
        "slices output directory",
    )?;

    let slices_dir = out_dir.join("slices");
    fs::create_dir_all(&slices_dir).map_err(|err| CoreError::Io {
        context: format!("creating slices directory {}", slices_dir.display()),
        source: err,
    })?;
    verify_contained_path_no_symlinks(
        out_dir,
        Path::new("slices"),
        true,
        "slices output directory",
    )?;

    let mut records = Vec::with_capacity(extracted.len());
    for slice in extracted {
        let dest = out_dir.join(&slice.record.file);
        reject_existing_symlink(&dest, "slice PNG")?;
        write_atomic(&dest, &slice.png_bytes, "slice PNG")?;
        records.push(slice.record);
    }

    let sheet_filename = sheet_path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("sheet.png")
        .to_string();
    let manifest = SlicesManifest {
        schema_version: SUPPORTED_SLICES_SCHEMA_VERSION,
        sheet_filename,
        sheet_width: rgba.width(),
        sheet_height: rgba.height(),
        calibration_strip_height: config.exclude_bottom_rows,
        slices: records,
    };

    let manifest_path = out_dir.join("slices.json");
    reject_existing_symlink(&manifest_path, "slices.json")?;
    let manifest_bytes = serde_json::to_vec_pretty(&manifest)
        .map_err(|err| CoreError::SlicerFailed(err.to_string()))?;
    write_atomic(&manifest_path, &manifest_bytes, "slices.json")?;
    Ok(manifest)
}

fn ensure_out_dir_no_symlinks(out_dir: &Path) -> Result<(), CoreError> {
    match fs::symlink_metadata(out_dir) {
        Ok(meta) => {
            if meta.file_type().is_symlink() || !meta.is_dir() {
                return Err(CoreError::Io {
                    context: format!("inspecting slicer out_dir {}", out_dir.display()),
                    source: std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        format!(
                            "refusing symlink or non-directory slicer out_dir: {}",
                            out_dir.display()
                        ),
                    ),
                });
            }
            return Ok(());
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => {
            return Err(CoreError::Io {
                context: format!("inspecting slicer out_dir {}", out_dir.display()),
                source: err,
            });
        }
    }

    for ancestor in out_dir.ancestors().skip(1) {
        if ancestor.as_os_str().is_empty() {
            break;
        }
        match fs::symlink_metadata(ancestor) {
            Ok(meta) => {
                if meta.file_type().is_symlink() || !meta.is_dir() {
                    return Err(CoreError::Io {
                        context: format!(
                            "inspecting slicer out_dir ancestor {}",
                            ancestor.display()
                        ),
                        source: std::io::Error::new(
                            std::io::ErrorKind::InvalidInput,
                            format!(
                                "refusing symlink or non-directory ancestor in slicer out_dir: {}",
                                ancestor.display()
                            ),
                        ),
                    });
                }
                break;
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
            Err(err) => {
                return Err(CoreError::Io {
                    context: format!("inspecting slicer out_dir ancestor {}", ancestor.display()),
                    source: err,
                });
            }
        }
    }
    Ok(())
}

fn reject_existing_symlink(path: &Path, label: &str) -> Result<(), CoreError> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => Err(CoreError::Io {
            context: format!("inspecting {label} at {}", path.display()),
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!(
                    "refusing symlink at {label} destination: {}",
                    path.display()
                ),
            ),
        }),
        Ok(_) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(CoreError::Io {
            context: format!("inspecting {label} at {}", path.display()),
            source: err,
        }),
    }
}

fn validate_slice_identifier(raw: &str, field_name: &str) -> Result<String, CoreError> {
    if raw.is_empty()
        || raw != raw.trim()
        || !raw
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err(CoreError::SlicerFailed(format!(
            "{field_name} '{raw}' must be non-empty ASCII alphanumeric, '-', or '_'"
        )));
    }
    Ok(raw.to_string())
}

fn validate_rect_override(
    ov: &RectOverride,
    sheet_w: u32,
    sheet_h: u32,
    seen_ids: &mut HashSet<String>,
) -> Result<(), CoreError> {
    let id = ov.id.trim();
    if id.is_empty()
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err(CoreError::SlicerFailed(format!(
            "RectOverride id '{}' must be non-empty alphanumeric/hyphen/underscore",
            ov.id
        )));
    }
    if !seen_ids.insert(id.to_string()) {
        return Err(CoreError::SlicerFailed(format!(
            "duplicate RectOverride id '{id}'"
        )));
    }
    if ov.rect.width == 0 || ov.rect.height == 0 {
        return Err(CoreError::SlicerFailed(format!(
            "RectOverride '{id}' must have positive width and height"
        )));
    }
    if ov.rect.right() > sheet_w || ov.rect.bottom() > sheet_h {
        return Err(CoreError::SlicerFailed(format!(
            "RectOverride '{id}' ({:?}) exceeds sheet bounds {sheet_w}x{sheet_h}",
            ov.rect
        )));
    }
    Ok(())
}

fn detect_foreground_components(
    sheet: &RgbaImage,
    active_height: u32,
    config: &SlicerConfig,
) -> Result<Vec<PixelRect>, CoreError> {
    let w = sheet.width() as usize;
    let h = active_height as usize;
    let mut fg_mask = vec![false; w * h];

    let custom_bg = config.bg_rgb != STUDIO_BG_RGB;
    for y in 0..h {
        for x in 0..w {
            let px = sheet.get_pixel(x as u32, y as u32).0;
            if px[3] == 0 {
                continue;
            }
            let dist = if custom_bg {
                let dr = f64::from(px[0]) - f64::from(config.bg_rgb[0]);
                let dg = f64::from(px[1]) - f64::from(config.bg_rgb[1]);
                let db = f64::from(px[2]) - f64::from(config.bg_rgb[2]);
                (dr * dr + dg * dg + db * db).sqrt()
            } else {
                studio_bg_distance([px[0], px[1], px[2]])
            };
            if dist > config.bg_distance_threshold {
                fg_mask[y * w + x] = true;
            }
        }
    }

    let radius = (config.merge_gap_px / 2) as usize;
    let bridged = if radius > 0 {
        dilate_mask(&fg_mask, w, h, radius)
    } else {
        fg_mask.clone()
    };

    let mut visited = vec![false; w * h];
    let mut raw_components: Vec<(u32, u32, u32, u32, u32)> = Vec::new();
    let mut queue = VecDeque::new();

    for start_y in 0..h {
        for start_x in 0..w {
            let start_idx = start_y * w + start_x;
            if !bridged[start_idx] || visited[start_idx] {
                continue;
            }
            visited[start_idx] = true;
            queue.push_back((start_x, start_y));

            let mut min_x = u32::MAX;
            let mut min_y = u32::MAX;
            let mut max_x = 0u32;
            let mut max_y = 0u32;
            let mut fg_area = 0u32;

            while let Some((cx, cy)) = queue.pop_front() {
                let idx = cy * w + cx;
                if fg_mask[idx] {
                    fg_area += 1;
                    let ux = cx as u32;
                    let uy = cy as u32;
                    min_x = min_x.min(ux);
                    min_y = min_y.min(uy);
                    max_x = max_x.max(ux);
                    max_y = max_y.max(uy);
                }

                let y0 = cy.saturating_sub(1);
                let y1 = (cy + 1).min(h - 1);
                let x0 = cx.saturating_sub(1);
                let x1 = (cx + 1).min(w - 1);

                for ny in y0..=y1 {
                    for nx in x0..=x1 {
                        let nidx = ny * w + nx;
                        if bridged[nidx] && !visited[nidx] {
                            visited[nidx] = true;
                            queue.push_back((nx, ny));
                        }
                    }
                }
            }

            if fg_area > 0 && min_x <= max_x && min_y <= max_y {
                raw_components.push((min_x, min_y, max_x, max_y, fg_area));
            }
        }
    }

    // Optional grid priors: merge components whose centroids lie in the same (col, row) grid cell.
    let merged_components: Vec<(u32, u32, u32, u32, u32)> =
        if let Some((cols, rows)) = config.grid_priors {
            if cols == 0 || rows == 0 {
                return Err(CoreError::SlicerFailed(
                    "grid_priors cols and rows must both be > 0".to_string(),
                ));
            }
            type BoundsAccum = (u32, u32, u32, u32, u32);
            let mut cells: BTreeMap<(u32, u32), BoundsAccum> = BTreeMap::new();
            for (min_x, min_y, max_x, max_y, area) in raw_components {
                let cx = (min_x + max_x) / 2;
                let cy = (min_y + max_y) / 2;
                let col = (cx * cols / sheet.width()).min(cols - 1);
                let row = (cy * rows / active_height).min(rows - 1);
                cells
                    .entry((row, col))
                    .and_modify(|e| {
                        e.0 = e.0.min(min_x);
                        e.1 = e.1.min(min_y);
                        e.2 = e.2.max(max_x);
                        e.3 = e.3.max(max_y);
                        e.4 = e.4.saturating_add(area);
                    })
                    .or_insert((min_x, min_y, max_x, max_y, area));
            }
            cells.into_values().collect()
        } else {
            raw_components
        };

    let mut rects = Vec::new();
    for (min_x, min_y, max_x, max_y, area) in merged_components {
        let comp_w = max_x - min_x + 1;
        let comp_h = max_y - min_y + 1;
        if comp_w < config.min_component_width
            || comp_h < config.min_component_height
            || area < config.min_component_area
        {
            continue;
        }
        let px = min_x.saturating_sub(config.padding_px);
        let py = min_y.saturating_sub(config.padding_px);
        let pr = (max_x + 1 + config.padding_px).min(sheet.width());
        let pb = (max_y + 1 + config.padding_px).min(active_height);
        rects.push(PixelRect {
            x: px,
            y: py,
            width: pr - px,
            height: pb - py,
        });
    }

    // Deterministic reading-order sort: coarse row bands (32px) then left-to-right X.
    rects.sort_by_key(|r| (r.y / 32, r.x, r.y));
    Ok(rects)
}

/// Fast separable 1D-sliding-window binary mask dilation (`O(W * H)`).
fn dilate_mask(mask: &[bool], w: usize, h: usize, radius: usize) -> Vec<bool> {
    let mut horiz = vec![false; w * h];
    for y in 0..h {
        let row = &mask[y * w..(y + 1) * w];
        let mut count = 0usize;
        let init_end = radius.min(w - 1);
        for &val in &row[0..=init_end] {
            if val {
                count += 1;
            }
        }
        for x in 0..w {
            if count > 0 {
                horiz[y * w + x] = true;
            }
            let remove_idx = x.checked_sub(radius);
            if let Some(ri) = remove_idx
                && row[ri]
            {
                count -= 1;
            }
            let add_idx = x + radius + 1;
            if add_idx < w && row[add_idx] {
                count += 1;
            }
        }
    }

    let mut out = vec![false; w * h];
    for x in 0..w {
        let mut count = 0usize;
        let init_end = radius.min(h - 1);
        for y in 0..=init_end {
            if horiz[y * w + x] {
                count += 1;
            }
        }
        for y in 0..h {
            if count > 0 {
                out[y * w + x] = true;
            }
            let remove_y = y.checked_sub(radius);
            if let Some(ry) = remove_y
                && horiz[ry * w + x]
            {
                count -= 1;
            }
            let add_y = y + radius + 1;
            if add_y < h && horiz[add_y * w + x] {
                count += 1;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use image::{ImageBuffer, Rgba};
    use rstest::rstest;
    use tempfile::TempDir;

    use super::*;
    use crate::calibration::STUDIO_BG_RGBA;

    fn paint_box(img: &mut RgbaImage, x: u32, y: u32, w: u32, h: u32, color: [u8; 3]) {
        for py in y..(y + h) {
            for px in x..(x + w) {
                img.put_pixel(px, py, Rgba([color[0], color[1], color[2], 255]));
            }
        }
    }

    #[test]
    fn slices_distinct_foreground_components_and_excludes_calibration_strip() {
        let mut img: RgbaImage = ImageBuffer::from_pixel(640, 480, Rgba(STUDIO_BG_RGBA));
        // Left pose, center pose, right pose (plus a small nearby accessory within 4px of center pose)
        paint_box(&mut img, 40, 60, 100, 220, [210, 180, 140]);
        paint_box(&mut img, 250, 60, 110, 220, [140, 190, 210]);
        paint_box(&mut img, 363, 120, 20, 30, [220, 200, 90]); // within merge_gap_px=8 of center
        paint_box(&mut img, 480, 70, 100, 210, [190, 120, 150]);

        // Simulated calibration swatch inside bottom 72px strip — must be excluded!
        paint_box(&mut img, 20, 420, 48, 48, [255, 0, 0]);

        let dir = TempDir::new().expect("tempdir");
        let sheet_path = dir.path().join("turnaround.png");
        DynamicImage::ImageRgba8(img)
            .save_with_format(&sheet_path, ImageFormat::Png)
            .unwrap();

        let manifest = slice_sheet_to_dir(
            &sheet_path,
            dir.path(),
            &SlicerConfig {
                exclude_bottom_rows: 72,
                id_prefix: Some("turnaround".to_string()),
                ..SlicerConfig::default()
            },
        )
        .expect("slicer should succeed");

        assert_eq!(manifest.slices.len(), 3);
        assert_eq!(manifest.slices[0].id, "turnaround_001");
        assert_eq!(manifest.slices[1].id, "turnaround_002");
        assert_eq!(manifest.slices[2].id, "turnaround_003");
        // Verify calibration swatch was excluded
        for s in &manifest.slices {
            assert!(s.rect.bottom() <= 480 - 72);
            assert!(dir.path().join(&s.file).is_file());
        }
        // Center pose merged the nearby accessory
        assert!(manifest.slices[1].rect.right() >= 383);
        assert!(dir.path().join("slices.json").is_file());
    }

    #[rstest]
    #[case::replace_all(OverrideMode::ReplaceAll, 1)]
    #[case::merge_by_id_replaces_existing_and_appends_new(OverrideMode::MergeById, 3)]
    fn supports_manual_rect_overrides(#[case] mode: OverrideMode, #[case] expected_count: usize) {
        let mut img: RgbaImage = ImageBuffer::from_pixel(400, 300, Rgba(STUDIO_BG_RGBA));
        paint_box(&mut img, 20, 20, 80, 120, [200, 150, 100]);
        paint_box(&mut img, 220, 20, 80, 120, [100, 150, 200]);

        let overrides = vec![
            RectOverride {
                id: "slice_001".to_string(),
                rect: PixelRect {
                    x: 10,
                    y: 10,
                    width: 100,
                    height: 140,
                },
                label: Some("front-view".to_string()),
            },
            RectOverride {
                id: "custom_extra".to_string(),
                rect: PixelRect {
                    x: 150,
                    y: 150,
                    width: 50,
                    height: 50,
                },
                label: None,
            },
        ];

        let cfg = SlicerConfig {
            overrides: if mode == OverrideMode::ReplaceAll {
                overrides[..1].to_vec()
            } else {
                overrides
            },
            override_mode: mode,
            ..SlicerConfig::default()
        };
        let slices = slice_sheet_image(&img, &cfg).expect("override slicing succeeds");
        assert_eq!(slices.len(), expected_count);
        assert_eq!(slices[0].record.source, SliceSource::ManualOverride);
        assert_eq!(slices[0].record.rect.width, 100);
    }

    #[test]
    fn grid_priors_group_fragmented_components_within_same_cell() {
        let mut img: RgbaImage = ImageBuffer::from_pixel(400, 200, Rgba(STUDIO_BG_RGBA));
        // Two separated parts inside left cell (x < 200)
        paint_box(&mut img, 30, 30, 40, 40, [220, 120, 80]);
        paint_box(&mut img, 30, 120, 40, 40, [220, 120, 80]);
        // One part inside right cell (x >= 200)
        paint_box(&mut img, 260, 50, 60, 80, [80, 180, 220]);

        let slices = slice_sheet_image(
            &img,
            &SlicerConfig {
                merge_gap_px: 0,
                grid_priors: Some((2, 1)),
                ..SlicerConfig::default()
            },
        )
        .expect("grid prior slicing succeeds");
        assert_eq!(slices.len(), 2);
        assert!(slices[0].record.rect.height >= 130);
    }

    #[rstest]
    #[case::traversal_prefix("../evil", false)]
    #[case::slash_prefix("sub/dir", false)]
    #[case::backslash_prefix("sub\\dir", false)]
    #[case::whitespace_in_prefix("bad prefix", false)]
    #[case::leading_whitespace(" turnaround", false)]
    #[case::empty_prefix("", false)]
    #[case::whitespace_only_prefix("   ", false)]
    #[case::control_char_prefix("turn\naround", false)]
    #[case::valid_prefix("turnaround-01_a", true)]
    fn validates_slicer_id_prefix_allowlist(#[case] prefix: &str, #[case] expect_ok: bool) {
        let mut img: RgbaImage = ImageBuffer::from_pixel(200, 200, Rgba(STUDIO_BG_RGBA));
        paint_box(&mut img, 40, 40, 60, 60, [200, 160, 120]);

        let res = slice_sheet_image(
            &img,
            &SlicerConfig {
                id_prefix: Some(prefix.to_string()),
                ..SlicerConfig::default()
            },
        );
        assert_eq!(res.is_ok(), expect_ok);
        if !expect_ok {
            assert!(matches!(res.unwrap_err(), CoreError::SlicerFailed(_)));
        }
    }

    #[cfg(unix)]
    #[rstest]
    #[case::symlinked_out_dir("out_dir")]
    #[case::symlinked_slices_subdir("slices_subdir")]
    #[case::symlinked_parent_dir("parent_dir")]
    fn slice_sheet_to_dir_rejects_symlinks_in_out_dir_and_slices(#[case] mode: &str) {
        use std::os::unix::fs::symlink;

        let root = TempDir::new().unwrap();
        let mut img: RgbaImage = ImageBuffer::from_pixel(200, 200, Rgba(STUDIO_BG_RGBA));
        paint_box(&mut img, 40, 40, 60, 60, [200, 160, 120]);
        let sheet_path = root.path().join("sheet.png");
        DynamicImage::ImageRgba8(img)
            .save_with_format(&sheet_path, ImageFormat::Png)
            .unwrap();

        let escape_dir = root.path().join("escape");
        fs::create_dir_all(&escape_dir).unwrap();

        let out_dir = match mode {
            "out_dir" => {
                let link = root.path().join("out_link");
                symlink(&escape_dir, &link).unwrap();
                link
            }
            "slices_subdir" => {
                let od = root.path().join("out_real");
                fs::create_dir_all(&od).unwrap();
                symlink(&escape_dir, od.join("slices")).unwrap();
                od
            }
            "parent_dir" => {
                let link_parent = root.path().join("parent_link");
                symlink(&escape_dir, &link_parent).unwrap();
                link_parent.join("nested_out")
            }
            _ => unreachable!(),
        };

        let err = slice_sheet_to_dir(&sheet_path, &out_dir, &SlicerConfig::default())
            .expect_err("symlinked out_dir or slices dir must fail closed");
        assert!(matches!(err, CoreError::Io { .. }));
        assert!(fs::read_dir(&escape_dir).unwrap().next().is_none());
    }
}
