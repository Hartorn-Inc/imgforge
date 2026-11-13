//! === GUIDESPEC SCHEMA & DETERMINISTIC GUIDERENDERER ===
//!
//! PURPOSE:
//! Defines the Rust `GuideSpec` schema matching `frontend/src/types/guideSpec.ts`
//! 1:1 (`camelCase` JSON fields, `schemaVersion: "1.0"`, `OverlayGuide` vs
//! `Blockout`) and implements `GuideRenderer`, the deterministic vector-to-raster
//! engine (`tiny-skia` + `resvg`) that produces `SpatialGuide` PNGs (`GLOSSARY.md`).
//!
//! THREAT MODEL / DESIGN RATIONALE ("WHY"):
//! 1. `OverlayGuide` loads an existing `targetImagePath` from disk; trusting
//!    unvalidated paths would allow symlink escapes or arbitrary workspace reads.
//!    All target image paths therefore enforce strict relative-path validation,
//!    symlink-free workspace containment (`verify_contained_path_no_symlinks`),
//!    and `O_NOFOLLOW` regular-file reads (`read_regular_file_nofollow`).
//! 2. `GuideSpec` validation rejects unknown fields, out-of-range `[0, 1]` ratios,
//!    inverted depth bands, invalid hex colors, and variant/`targetImagePath`
//!    mismatches fail-closed before any raster allocation.
//! 3. Mannequin silhouettes are rendered via `resvg` from pure text-free SVG
//!    geometry so zero unintended text ever leaks into downstream generation.
//!
//! EXECUTION LIFECYCLE / DATA FLOW:
//! 1. `GuideSpec::from_json_str` parses JSON (`#[serde(deny_unknown_fields)]`)
//!    and validates all invariants matching `parseGuideSpecJson`.
//! 2. `GuideRenderer::render` dispatches `OverlayGuide` (target image copy +
//!    haloed `OverlayRegion`s + haloed `ActionVector`s) or `Blockout` (from-scratch
//!    `DepthBand` zones + proportionally scaled `Subject` mannequins via `resvg` +
//!    `OverlayRegion`s + `ActionVector`s) and encodes a verified PNG.

use std::fmt::Write as _;
use std::path::Path;

use image::codecs::png::PngEncoder;
use image::imageops::FilterType;
use image::{ExtendedColorType, ImageEncoder, RgbaImage};
use resvg::usvg;
use serde::{Deserialize, Serialize};
use tiny_skia::{
    Color, FillRule, LineCap, LineJoin, Paint, PathBuilder, Pixmap, Rect, Stroke, StrokeDash,
    Transform,
};

use crate::calibration::{STUDIO_BG_RGB, parse_hex_rgb};
use crate::error::CoreError;
use crate::lifecycle::normalize_hex_color;
use crate::scene_spec::{DepthPlane, SceneSpec};
use crate::store::{
    checked_pixel_count, decode_png_checked, read_regular_file_nofollow,
    validate_strict_relative_path, verify_contained_path_no_symlinks,
};

/// Supported `GuideSpec` schema version matching `frontend/src/types/guideSpec.ts`.
pub const SUPPORTED_GUIDE_SPEC_SCHEMA_VERSION: &str = "1.0";

/// Maximum permitted `SpatialGuide` canvas dimension in pixels (`1..=16384`).
pub const MAX_GUIDE_CANVAS_DIM: u32 = 16_384;

/// Maximum permitted `depthBands` entries in a `GuideSpec`.
pub const MAX_GUIDE_DEPTH_BANDS: usize = 64;

/// Maximum permitted `mannequins` entries in a `GuideSpec`.
pub const MAX_GUIDE_MANNEQUINS: usize = 128;

/// Maximum permitted `actionVectors` entries in a `GuideSpec`.
pub const MAX_GUIDE_ACTION_VECTORS: usize = 256;

/// Maximum permitted `overlayRegions` entries in a `GuideSpec`.
pub const MAX_GUIDE_OVERLAY_REGIONS: usize = 256;

/// Maximum permitted `points` per `OverlayRegion`.
pub const MAX_GUIDE_REGION_POINTS: usize = 1_024;

/// Discriminator selecting the `SpatialGuide` rendering mode in `GuideRenderer`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SpatialGuideVariant {
    /// Draws haloed vector cues onto a copy of an existing target image.
    #[serde(rename = "OverlayGuide")]
    OverlayGuide,
    /// Renders depth bands, scaled `Subject` mannequins, and cues from scratch.
    #[serde(rename = "Blockout")]
    Blockout,
}

/// Depth plane assignment for `Blockout` depth-band zones.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GuideDepthPlane {
    /// Distant horizon plane.
    Background,
    /// Intermediate staging plane.
    Midground,
    /// Near focal plane.
    Foreground,
}

/// Canonical facing orientation for a scaled `Subject` mannequin.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MannequinFacing {
    /// Facing viewer's left.
    Left,
    /// Facing viewer's right.
    Right,
    /// Facing toward the camera.
    Front,
    /// Facing away from the camera.
    Back,
}

/// Stroke pattern for an `ActionVector` inside a `SpatialGuide`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VectorStrokeStyle {
    /// Continuous solid trajectory line.
    Solid,
    /// Dashed trajectory line.
    Dashed,
}

/// Geometric primitive kind for an `OverlayRegion` cue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OverlayRegionKind {
    /// Axis-aligned rectangle spanned by `points`.
    Rect,
    /// Axis-aligned ellipse inscribed in the bounding box of `points`.
    Ellipse,
    /// Open polyline connecting `points` in sequence.
    Polyline,
}

/// Normalized 2D coordinate in `[0, 1]` relative to canvas width/height.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NormalizedPoint {
    /// Horizontal position in `[0.0, 1.0]`.
    pub x_ratio: f64,
    /// Vertical position in `[0.0, 1.0]`.
    pub y_ratio: f64,
}

/// Horizontal depth-band zone inside a `Blockout` `GuideSpec`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DepthBand {
    /// Stable band identifier.
    pub id: String,
    /// Descriptive label.
    pub label: String,
    /// Depth plane assignment.
    pub plane: GuideDepthPlane,
    /// Normalized top boundary in `[0.0, 1.0]`.
    pub y_start_ratio: f64,
    /// Normalized bottom boundary in `[0.0, 1.0]` (`> y_start_ratio`).
    pub y_end_ratio: f64,
    /// Six-digit hex fill color (`#RRGGBB`).
    pub fill_hex: String,
}

/// Proportionally scaled `Subject` mannequin placed on the spatial canvas.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SubjectMannequin {
    /// Stable mannequin identifier.
    pub id: String,
    /// Referenced `Subject` slug.
    pub subject_slug: String,
    /// Human-readable label.
    pub label: String,
    /// Normalized anchor X coordinate in `[0.0, 1.0]`.
    pub x_ratio: f64,
    /// Normalized ground-foot Y coordinate in `[0.0, 1.0]`.
    pub y_ratio: f64,
    /// Physical height in meters (`> 0.0`).
    pub height_meters: f64,
    /// Perspective/composition scale multiplier (`> 0.0`).
    pub scale_factor: f64,
    /// Facing orientation indicator.
    pub facing: MannequinFacing,
    /// Pose guidance metadata (not rendered as text onto the raster output).
    pub pose_hint: String,
    /// Six-digit hex silhouette tint color (`#RRGGBB`).
    pub palette_hex: String,
}

/// Haloed trajectory or facing action vector drawn onto a `SpatialGuide`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ActionVector {
    /// Stable vector identifier.
    pub id: String,
    /// Human-readable label.
    pub label: String,
    /// Normalized start X coordinate in `[0.0, 1.0]`.
    pub from_x_ratio: f64,
    /// Normalized start Y coordinate in `[0.0, 1.0]`.
    pub from_y_ratio: f64,
    /// Normalized tip X coordinate in `[0.0, 1.0]`.
    pub to_x_ratio: f64,
    /// Normalized tip Y coordinate in `[0.0, 1.0]`.
    pub to_y_ratio: f64,
    /// Six-digit inner stroke hex color (`#RRGGBB`).
    pub stroke_hex: String,
    /// Six-digit outer contrast halo hex color (`#RRGGBB`).
    pub halo_hex: String,
    /// Inner stroke width in pixels (`> 0.0`).
    pub stroke_width_px: f64,
    /// Solid or dashed stroke pattern.
    pub style: VectorStrokeStyle,
}

/// Haloed vector region or path drawn in `OverlayGuide` (or annotated in `Blockout`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OverlayRegion {
    /// Stable region identifier.
    pub id: String,
    /// Human-readable label.
    pub label: String,
    /// Geometric primitive kind (`rect`, `ellipse`, or `polyline`).
    pub kind: OverlayRegionKind,
    /// At least 2 normalized points defining the region geometry.
    pub points: Vec<NormalizedPoint>,
    /// Six-digit inner stroke hex color (`#RRGGBB`).
    pub stroke_hex: String,
    /// Six-digit outer contrast halo hex color (`#RRGGBB`).
    pub halo_hex: String,
    /// Six-digit fill hex color (`#RRGGBB`).
    pub fill_hex: String,
    /// Fill opacity in `[0.0, 1.0]`.
    pub opacity: f64,
}

/// Shared `GuideSpec` JSON schema matching `frontend/src/types/guideSpec.ts` 1:1.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GuideSpec {
    /// Schema version; must equal `"1.0"`.
    pub schema_version: String,
    /// Stable guide identifier.
    pub id: String,
    /// Human-readable guide title.
    pub title: String,
    /// Variant discriminator (`"OverlayGuide"` or `"Blockout"`).
    pub variant: SpatialGuideVariant,
    /// Target workspace-relative image path for `OverlayGuide`, or `null` for `Blockout`.
    pub target_image_path: Option<String>,
    /// Target canvas width in pixels (`1..=16384`).
    pub canvas_width: u32,
    /// Target canvas height in pixels (`1..=16384`).
    pub canvas_height: u32,
    /// Horizontal depth bands (typically used in `Blockout`).
    pub depth_bands: Vec<DepthBand>,
    /// Proportionally scaled `Subject` mannequins.
    pub mannequins: Vec<SubjectMannequin>,
    /// Haloed directional/trajectory action vectors.
    pub action_vectors: Vec<ActionVector>,
    /// Haloed overlay regions (`rect`, `ellipse`, `polyline`).
    pub overlay_regions: Vec<OverlayRegion>,
}

impl GuideSpec {
    /// Parse a `GuideSpec` from JSON (`#[serde(deny_unknown_fields)]`) and validate all invariants.
    pub fn from_json_str(raw: &str) -> Result<Self, CoreError> {
        let mut spec: Self = serde_json::from_str(raw).map_err(|err| {
            CoreError::GuideRenderFailed(format!("invalid GuideSpec JSON: {err}"))
        })?;
        spec.validate_and_normalize()?;
        Ok(spec)
    }

    /// Validate and serialize this `GuideSpec` to pretty `camelCase` JSON matching `guideSpec.ts`.
    pub fn to_json_string(&self) -> Result<String, CoreError> {
        let mut copy = self.clone();
        copy.validate_and_normalize()?;
        serde_json::to_string_pretty(&copy).map_err(|err| {
            CoreError::GuideRenderFailed(format!("serializing GuideSpec JSON: {err}"))
        })
    }

    /// Validate every geometric, color, and variant invariant and normalize hex colors uppercase.
    pub fn validate_and_normalize(&mut self) -> Result<(), CoreError> {
        if self.schema_version != SUPPORTED_GUIDE_SPEC_SCHEMA_VERSION {
            return Err(CoreError::GuideRenderFailed(format!(
                "GuideSpec schemaVersion '{}' must equal '{}'",
                self.schema_version, SUPPORTED_GUIDE_SPEC_SCHEMA_VERSION
            )));
        }
        require_non_empty_str("id", &self.id)?;
        require_non_empty_str("title", &self.title)?;

        if self.canvas_width == 0 || self.canvas_width > MAX_GUIDE_CANVAS_DIM {
            return Err(CoreError::GuideRenderFailed(format!(
                "canvasWidth {} must be an integer in 1..={MAX_GUIDE_CANVAS_DIM}",
                self.canvas_width
            )));
        }
        if self.canvas_height == 0 || self.canvas_height > MAX_GUIDE_CANVAS_DIM {
            return Err(CoreError::GuideRenderFailed(format!(
                "canvasHeight {} must be an integer in 1..={MAX_GUIDE_CANVAS_DIM}",
                self.canvas_height
            )));
        }

        match (self.variant, self.target_image_path.as_deref()) {
            (SpatialGuideVariant::OverlayGuide, Some(p)) if !p.trim().is_empty() => {}
            (SpatialGuideVariant::OverlayGuide, _) => {
                return Err(CoreError::GuideRenderFailed(
                    "OverlayGuide requires a non-empty targetImagePath string".to_string(),
                ));
            }
            (SpatialGuideVariant::Blockout, None) => {}
            (SpatialGuideVariant::Blockout, Some(_)) => {
                return Err(CoreError::GuideRenderFailed(
                    "Blockout must set targetImagePath to null".to_string(),
                ));
            }
        }

        if self.depth_bands.len() > MAX_GUIDE_DEPTH_BANDS {
            return Err(CoreError::GuideRenderFailed(format!(
                "depthBands count {} exceeds maximum {MAX_GUIDE_DEPTH_BANDS}",
                self.depth_bands.len()
            )));
        }
        if self.mannequins.len() > MAX_GUIDE_MANNEQUINS {
            return Err(CoreError::GuideRenderFailed(format!(
                "mannequins count {} exceeds maximum {MAX_GUIDE_MANNEQUINS}",
                self.mannequins.len()
            )));
        }
        if self.action_vectors.len() > MAX_GUIDE_ACTION_VECTORS {
            return Err(CoreError::GuideRenderFailed(format!(
                "actionVectors count {} exceeds maximum {MAX_GUIDE_ACTION_VECTORS}",
                self.action_vectors.len()
            )));
        }
        if self.overlay_regions.len() > MAX_GUIDE_OVERLAY_REGIONS {
            return Err(CoreError::GuideRenderFailed(format!(
                "overlayRegions count {} exceeds maximum {MAX_GUIDE_OVERLAY_REGIONS}",
                self.overlay_regions.len()
            )));
        }

        for (idx, band) in self.depth_bands.iter_mut().enumerate() {
            let pfx = format!("depthBands[{idx}]");
            require_non_empty_str(&format!("{pfx}.id"), &band.id)?;
            require_non_empty_str(&format!("{pfx}.label"), &band.label)?;
            require_unit_ratio(&format!("{pfx}.yStartRatio"), band.y_start_ratio)?;
            require_unit_ratio(&format!("{pfx}.yEndRatio"), band.y_end_ratio)?;
            if band.y_start_ratio >= band.y_end_ratio {
                return Err(CoreError::GuideRenderFailed(format!(
                    "{pfx}.yEndRatio ({}) must be strictly greater than yStartRatio ({})",
                    band.y_end_ratio, band.y_start_ratio
                )));
            }
            band.fill_hex = normalize_hex_color(&band.fill_hex, &format!("{pfx}.fillHex"))
                .map_err(CoreError::GuideRenderFailed)?;
        }

        for (idx, m) in self.mannequins.iter_mut().enumerate() {
            let pfx = format!("mannequins[{idx}]");
            require_non_empty_str(&format!("{pfx}.id"), &m.id)?;
            require_non_empty_str(&format!("{pfx}.subjectSlug"), &m.subject_slug)?;
            require_non_empty_str(&format!("{pfx}.label"), &m.label)?;
            require_unit_ratio(&format!("{pfx}.xRatio"), m.x_ratio)?;
            require_unit_ratio(&format!("{pfx}.yRatio"), m.y_ratio)?;
            require_pos_finite(&format!("{pfx}.heightMeters"), m.height_meters)?;
            require_pos_finite(&format!("{pfx}.scaleFactor"), m.scale_factor)?;
            m.palette_hex = normalize_hex_color(&m.palette_hex, &format!("{pfx}.paletteHex"))
                .map_err(CoreError::GuideRenderFailed)?;
        }

        for (idx, v) in self.action_vectors.iter_mut().enumerate() {
            let pfx = format!("actionVectors[{idx}]");
            require_non_empty_str(&format!("{pfx}.id"), &v.id)?;
            require_non_empty_str(&format!("{pfx}.label"), &v.label)?;
            require_unit_ratio(&format!("{pfx}.fromXRatio"), v.from_x_ratio)?;
            require_unit_ratio(&format!("{pfx}.fromYRatio"), v.from_y_ratio)?;
            require_unit_ratio(&format!("{pfx}.toXRatio"), v.to_x_ratio)?;
            require_unit_ratio(&format!("{pfx}.toYRatio"), v.to_y_ratio)?;
            require_pos_finite(&format!("{pfx}.strokeWidthPx"), v.stroke_width_px)?;
            if v.stroke_width_px > f64::from(MAX_GUIDE_CANVAS_DIM) {
                return Err(CoreError::GuideRenderFailed(format!(
                    "{pfx}.strokeWidthPx ({}) exceeds maximum {MAX_GUIDE_CANVAS_DIM}",
                    v.stroke_width_px
                )));
            }
            v.stroke_hex = normalize_hex_color(&v.stroke_hex, &format!("{pfx}.strokeHex"))
                .map_err(CoreError::GuideRenderFailed)?;
            v.halo_hex = normalize_hex_color(&v.halo_hex, &format!("{pfx}.haloHex"))
                .map_err(CoreError::GuideRenderFailed)?;
        }

        for (idx, r) in self.overlay_regions.iter_mut().enumerate() {
            let pfx = format!("overlayRegions[{idx}]");
            require_non_empty_str(&format!("{pfx}.id"), &r.id)?;
            require_non_empty_str(&format!("{pfx}.label"), &r.label)?;
            if r.points.len() < 2 || r.points.len() > MAX_GUIDE_REGION_POINTS {
                return Err(CoreError::GuideRenderFailed(format!(
                    "{pfx}.points must contain between 2 and {MAX_GUIDE_REGION_POINTS} points"
                )));
            }
            for (pidx, pt) in r.points.iter().enumerate() {
                require_unit_ratio(&format!("{pfx}.points[{pidx}].xRatio"), pt.x_ratio)?;
                require_unit_ratio(&format!("{pfx}.points[{pidx}].yRatio"), pt.y_ratio)?;
            }
            require_unit_ratio(&format!("{pfx}.opacity"), r.opacity)?;
            r.stroke_hex = normalize_hex_color(&r.stroke_hex, &format!("{pfx}.strokeHex"))
                .map_err(CoreError::GuideRenderFailed)?;
            r.halo_hex = normalize_hex_color(&r.halo_hex, &format!("{pfx}.haloHex"))
                .map_err(CoreError::GuideRenderFailed)?;
            r.fill_hex = normalize_hex_color(&r.fill_hex, &format!("{pfx}.fillHex"))
                .map_err(CoreError::GuideRenderFailed)?;
        }

        Ok(())
    }
}

/// Deterministic `SpatialGuide` raster output emitted by `GuideRenderer`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedSpatialGuide {
    /// Validated PNG bytes.
    pub png_bytes: Vec<u8>,
    /// Output width in pixels.
    pub width: u32,
    /// Output height in pixels.
    pub height: u32,
    /// Rendered `SpatialGuideVariant`.
    pub variant: SpatialGuideVariant,
}

/// Deterministic vector-to-raster `SpatialGuide` renderer (`tiny-skia` + `resvg`).
#[derive(Debug, Clone, Default)]
pub struct GuideRenderer;

/// Build a recommended `Blockout` `GuideSpec` scaffold from a validated `SceneSpec`
/// so CLI, MCP, and Web Canvas UI share one deterministic layout projection.
#[must_use]
pub fn build_recommended_blockout(spec: &SceneSpec) -> GuideSpec {
    let w = spec.width.unwrap_or(1024);
    let h = spec.height.unwrap_or(1024);

    let mut depth_bands = Vec::with_capacity(spec.depth_bands.len());
    let n_bands = spec.depth_bands.len().max(1);
    for (idx, b) in spec.depth_bands.iter().enumerate() {
        let y0 = (idx as f64) / (n_bands as f64);
        let y1 = ((idx + 1) as f64) / (n_bands as f64);
        let (plane, fill_hex) = match b.plane {
            DepthPlane::Background => (GuideDepthPlane::Background, "#2C3344".to_string()),
            DepthPlane::Midground => (GuideDepthPlane::Midground, "#3B4254".to_string()),
            DepthPlane::Foreground => (GuideDepthPlane::Foreground, "#4A5164".to_string()),
        };
        depth_bands.push(DepthBand {
            id: format!("band-{}", idx + 1),
            label: b.plane.as_str().to_string(),
            plane,
            y_start_ratio: y0,
            y_end_ratio: y1,
            fill_hex,
        });
    }

    let mannequins = spec
        .foreground_slots
        .iter()
        .enumerate()
        .map(|(idx, slot)| {
            let palette_hex = slot
                .exclusive_surface_map
                .bindings
                .first()
                .map(|b| b.color_hex.clone())
                .unwrap_or_else(|| "#63B3ED".to_string());
            SubjectMannequin {
                id: format!("mannequin-{}", idx + 1),
                subject_slug: slot.subject_ref.clone(),
                label: slot.subject_ref.clone(),
                x_ratio: slot.position_x_ratio,
                y_ratio: 0.85,
                height_meters: slot.height_meters,
                scale_factor: 1.0,
                facing: MannequinFacing::Front,
                pose_hint: slot.pose.clone(),
                palette_hex,
            }
        })
        .collect();

    let action_vectors = if spec.foreground_slots.len() >= 2 {
        vec![ActionVector {
            id: "vector-1".to_string(),
            label: "Primary interaction".to_string(),
            from_x_ratio: spec.foreground_slots[0].position_x_ratio,
            from_y_ratio: 0.65,
            to_x_ratio: spec.foreground_slots[1].position_x_ratio,
            to_y_ratio: 0.65,
            stroke_hex: "#F6AD55".to_string(),
            halo_hex: "#1A1D24".to_string(),
            stroke_width_px: 4.0,
            style: VectorStrokeStyle::Solid,
        }]
    } else {
        Vec::new()
    };

    GuideSpec {
        schema_version: SUPPORTED_GUIDE_SPEC_SCHEMA_VERSION.to_string(),
        id: format!("{}-blockout", spec.id),
        title: format!("{} Blockout", spec.title),
        variant: SpatialGuideVariant::Blockout,
        target_image_path: None,
        canvas_width: w,
        canvas_height: h,
        depth_bands,
        mannequins,
        action_vectors,
        overlay_regions: Vec::new(),
    }
}

impl GuideRenderer {
    /// Construct a new `GuideRenderer`.
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    /// Validate `spec` and render it deterministically into a `SpatialGuide` PNG.
    ///
    /// For `OverlayGuide`, `targetImagePath` is resolved relative to `workspace_root`
    /// with strict symlink-free containment checks (`verify_contained_path_no_symlinks`
    /// and `read_regular_file_nofollow`).
    pub fn render(
        &self,
        spec: &GuideSpec,
        workspace_root: &Path,
    ) -> Result<RenderedSpatialGuide, CoreError> {
        let mut normalized = spec.clone();
        normalized.validate_and_normalize()?;

        let w = normalized.canvas_width;
        let h = normalized.canvas_height;
        checked_pixel_count(w, h, "SpatialGuide canvas")
            .map_err(|err| CoreError::GuideRenderFailed(err.to_string()))?;
        let mut pixmap = Pixmap::new(w, h).ok_or_else(|| {
            CoreError::GuideRenderFailed(format!("failed to allocate {w}x{h} pixmap"))
        })?;

        match normalized.variant {
            SpatialGuideVariant::OverlayGuide => {
                let raw_target = normalized
                    .target_image_path
                    .as_deref()
                    .expect("validated OverlayGuide has targetImagePath");
                let rel_path =
                    validate_strict_relative_path(raw_target, "GuideSpec.targetImagePath")
                        .map_err(CoreError::GuideRenderFailed)?;
                let contained_path = verify_contained_path_no_symlinks(
                    workspace_root,
                    &rel_path,
                    false,
                    "OverlayGuide targetImagePath",
                )?;
                let png_bytes =
                    read_regular_file_nofollow(&contained_path, "OverlayGuide target image")?;
                let decoded = decode_png_checked(&contained_path, &png_bytes)?;
                let target_rgba = if decoded.width() == w && decoded.height() == h {
                    decoded.into_rgba8()
                } else {
                    let src_rgba = decoded.into_rgba8();
                    image::imageops::resize(&src_rgba, w, h, FilterType::Lanczos3)
                };
                copy_rgba_into_premul_pixmap(&target_rgba, &mut pixmap);
            }
            SpatialGuideVariant::Blockout => {
                pixmap.fill(Color::from_rgba8(
                    STUDIO_BG_RGB[0],
                    STUDIO_BG_RGB[1],
                    STUDIO_BG_RGB[2],
                    255,
                ));
                render_depth_bands(&mut pixmap, &normalized.depth_bands, w, h)?;
                render_mannequins_via_resvg(&mut pixmap, &normalized.mannequins, w, h)?;
            }
        }

        render_overlay_regions(&mut pixmap, &normalized.overlay_regions, w, h)?;
        render_action_vectors(&mut pixmap, &normalized.action_vectors, w, h)?;

        let rgba = pixmap_into_unpremul_rgba(pixmap);
        let mut out_bytes = Vec::new();
        PngEncoder::new(&mut out_bytes)
            .write_image(rgba.as_raw(), w, h, ExtendedColorType::Rgba8)
            .map_err(|err| {
                CoreError::GuideRenderFailed(format!("encoding SpatialGuide PNG: {err}"))
            })?;

        Ok(RenderedSpatialGuide {
            png_bytes: out_bytes,
            width: w,
            height: h,
            variant: normalized.variant,
        })
    }
}

fn render_depth_bands(
    pixmap: &mut Pixmap,
    bands: &[DepthBand],
    w: u32,
    h: u32,
) -> Result<(), CoreError> {
    let wf = w as f32;
    let hf = h as f32;
    let divider_paint = solid_paint([0x1A, 0x1D, 0x24], 180);
    let divider_stroke = Stroke {
        width: 2.0,
        ..Stroke::default()
    };

    for band in bands {
        let rgb = parse_hex_rgb(&band.fill_hex)?;
        let y0 = (band.y_start_ratio as f32) * hf;
        let y1 = (band.y_end_ratio as f32) * hf;
        let bh = (y1 - y0).max(1.0);
        if let Some(rect) = Rect::from_xywh(0.0, y0, wf, bh) {
            let paint = solid_paint(rgb, 255);
            pixmap.fill_rect(rect, &paint, Transform::identity(), None);
        }
        if y0 > 0.5 {
            let mut pb = PathBuilder::new();
            pb.move_to(0.0, y0);
            pb.line_to(wf, y0);
            if let Some(line) = pb.finish() {
                pixmap.stroke_path(
                    &line,
                    &divider_paint,
                    &divider_stroke,
                    Transform::identity(),
                    None,
                );
            }
        }
    }
    Ok(())
}

fn render_mannequins_via_resvg(
    pixmap: &mut Pixmap,
    mannequins: &[SubjectMannequin],
    w: u32,
    h: u32,
) -> Result<(), CoreError> {
    let wf = w as f32;
    let hf = h as f32;
    // Reference scale: a 1.75m subject at scaleFactor=1.0 occupies ~38.5% of canvas height.
    let pixels_per_meter = hf * 0.22;

    for m in mannequins {
        let rgb = parse_hex_rgb(&m.palette_hex)?;
        let max_h = (hf * 0.92).max(1.0);
        let min_h = 24.0_f32.min(max_h);
        let pixel_h =
            ((m.height_meters * m.scale_factor) as f32 * pixels_per_meter).clamp(min_h, max_h);
        let pixel_w = (pixel_h * 0.44).max(1.0);
        let anchor_x = (m.x_ratio as f32) * wf;
        let anchor_y = (m.y_ratio as f32) * hf;

        let svg = build_mannequin_svg(pixel_w, pixel_h, rgb, m.facing);
        let tree = usvg::Tree::from_str(&svg, &usvg::Options::default()).map_err(|err| {
            CoreError::GuideRenderFailed(format!(
                "parsing SVG silhouette for mannequin '{}': {err}",
                m.id
            ))
        })?;

        let tx = anchor_x - pixel_w * 0.5;
        let ty = anchor_y - pixel_h;
        resvg::render(
            &tree,
            Transform::from_translate(tx, ty),
            &mut pixmap.as_mut(),
        );
    }
    Ok(())
}

/// Generate a crisp, text-free articulated SVG silhouette tinted with `rgb` and directional `facing`.
fn build_mannequin_svg(width: f32, height: f32, rgb: [u8; 3], facing: MannequinFacing) -> String {
    let fill = format!("#{:02X}{:02X}{:02X}", rgb[0], rgb[1], rgb[2]);
    let shade = format!(
        "#{:02X}{:02X}{:02X}",
        rgb[0].saturating_sub(35),
        rgb[1].saturating_sub(35),
        rgb[2].saturating_sub(35)
    );

    // 100x220 normalized silhouette coordinate space scaled to width x height.
    let mut svg = String::with_capacity(1024);
    let _ = write!(
        svg,
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{width:.2}" height="{height:.2}" viewBox="0 0 100 220">"##
    );
    // Ground shadow oval
    svg.push_str(
        r##"<ellipse cx="50" cy="214" rx="26" ry="5" fill="#101218" fill-opacity="0.45"/>"##,
    );

    match facing {
        MannequinFacing::Left => {
            // Side profile facing left
            let _ = write!(
                svg,
                r##"<rect x="44" y="130" width="14" height="82" rx="6" fill="{shade}" stroke="#1A1D24" stroke-width="2.5"/>
                   <rect x="36" y="128" width="14" height="84" rx="6" fill="{fill}" stroke="#1A1D24" stroke-width="2.5"/>
                   <rect x="33" y="54" width="32" height="78" rx="10" fill="{fill}" stroke="#1A1D24" stroke-width="2.5"/>
                   <rect x="37" y="64" width="12" height="58" rx="5" fill="{shade}" stroke="#1A1D24" stroke-width="2.5"/>
                   <circle cx="48" cy="28" r="19" fill="{fill}" stroke="#1A1D24" stroke-width="2.5"/>
                   <polygon points="18,28 33,20 33,36" fill="#F6AD55" stroke="#1A1D24" stroke-width="2"/>"##
            );
        }
        MannequinFacing::Right => {
            // Side profile facing right
            let _ = write!(
                svg,
                r##"<rect x="42" y="130" width="14" height="82" rx="6" fill="{shade}" stroke="#1A1D24" stroke-width="2.5"/>
                   <rect x="50" y="128" width="14" height="84" rx="6" fill="{fill}" stroke="#1A1D24" stroke-width="2.5"/>
                   <rect x="35" y="54" width="32" height="78" rx="10" fill="{fill}" stroke="#1A1D24" stroke-width="2.5"/>
                   <rect x="51" y="64" width="12" height="58" rx="5" fill="{shade}" stroke="#1A1D24" stroke-width="2.5"/>
                   <circle cx="52" cy="28" r="19" fill="{fill}" stroke="#1A1D24" stroke-width="2.5"/>
                   <polygon points="82,28 67,20 67,36" fill="#F6AD55" stroke="#1A1D24" stroke-width="2"/>"##
            );
        }
        MannequinFacing::Front => {
            let _ = write!(
                svg,
                r##"<rect x="29" y="128" width="16" height="84" rx="7" fill="{fill}" stroke="#1A1D24" stroke-width="2.5"/>
                   <rect x="55" y="128" width="16" height="84" rx="7" fill="{fill}" stroke="#1A1D24" stroke-width="2.5"/>
                   <rect x="14" y="58" width="13" height="68" rx="6" fill="{fill}" stroke="#1A1D24" stroke-width="2.5"/>
                   <rect x="73" y="58" width="13" height="68" rx="6" fill="{fill}" stroke="#1A1D24" stroke-width="2.5"/>
                   <rect x="27" y="54" width="46" height="78" rx="11" fill="{fill}" stroke="#1A1D24" stroke-width="2.5"/>
                   <circle cx="50" cy="28" r="19" fill="{fill}" stroke="#1A1D24" stroke-width="2.5"/>
                   <polygon points="50,36 42,24 58,24" fill="#F6AD55" stroke="#1A1D24" stroke-width="2"/>"##
            );
        }
        MannequinFacing::Back => {
            let _ = write!(
                svg,
                r##"<rect x="29" y="128" width="16" height="84" rx="7" fill="{shade}" stroke="#1A1D24" stroke-width="2.5"/>
                   <rect x="55" y="128" width="16" height="84" rx="7" fill="{shade}" stroke="#1A1D24" stroke-width="2.5"/>
                   <rect x="14" y="58" width="13" height="68" rx="6" fill="{shade}" stroke="#1A1D24" stroke-width="2.5"/>
                   <rect x="73" y="58" width="13" height="68" rx="6" fill="{shade}" stroke="#1A1D24" stroke-width="2.5"/>
                   <rect x="27" y="54" width="46" height="78" rx="11" fill="{shade}" stroke="#1A1D24" stroke-width="2.5"/>
                   <line x1="50" y1="60" x2="50" y2="122" stroke="#1A1D24" stroke-width="3"/>
                   <circle cx="50" cy="28" r="19" fill="{shade}" stroke="#1A1D24" stroke-width="2.5"/>"##
            );
        }
    }
    svg.push_str("</svg>");
    svg
}

fn render_overlay_regions(
    pixmap: &mut Pixmap,
    regions: &[OverlayRegion],
    w: u32,
    h: u32,
) -> Result<(), CoreError> {
    let wf = w as f32;
    let hf = h as f32;

    for region in regions {
        let stroke_rgb = parse_hex_rgb(&region.stroke_hex)?;
        let halo_rgb = parse_hex_rgb(&region.halo_hex)?;
        let fill_rgb = parse_hex_rgb(&region.fill_hex)?;
        let fill_alpha = (region.opacity * 255.0).round().clamp(0.0, 255.0) as u8;

        let pts: Vec<(f32, f32)> = region
            .points
            .iter()
            .map(|p| ((p.x_ratio as f32) * wf, (p.y_ratio as f32) * hf))
            .collect();

        let (min_x, min_y, max_x, max_y) = pts.iter().fold(
            (
                f32::INFINITY,
                f32::INFINITY,
                f32::NEG_INFINITY,
                f32::NEG_INFINITY,
            ),
            |(ax0, ay0, ax1, ay1), &(px, py)| (ax0.min(px), ay0.min(py), ax1.max(px), ay1.max(py)),
        );
        let bw = (max_x - min_x).max(2.0);
        let bh = (max_y - min_y).max(2.0);

        let path_opt = match region.kind {
            OverlayRegionKind::Rect => {
                Rect::from_xywh(min_x, min_y, bw, bh).map(PathBuilder::from_rect)
            }
            OverlayRegionKind::Ellipse => {
                Rect::from_xywh(min_x, min_y, bw, bh).and_then(PathBuilder::from_oval)
            }
            OverlayRegionKind::Polyline => {
                let mut pb = PathBuilder::new();
                pb.move_to(pts[0].0, pts[0].1);
                for &(px, py) in &pts[1..] {
                    pb.line_to(px, py);
                }
                pb.finish()
            }
        };

        if let Some(path) = path_opt {
            if region.kind != OverlayRegionKind::Polyline && fill_alpha > 0 {
                let fill_paint = solid_paint(fill_rgb, fill_alpha);
                pixmap.fill_path(
                    &path,
                    &fill_paint,
                    FillRule::Winding,
                    Transform::identity(),
                    None,
                );
            }

            let halo_paint = solid_paint(halo_rgb, 240);
            let halo_stroke = Stroke {
                width: 7.0,
                line_cap: LineCap::Round,
                line_join: LineJoin::Round,
                ..Stroke::default()
            };
            pixmap.stroke_path(
                &path,
                &halo_paint,
                &halo_stroke,
                Transform::identity(),
                None,
            );

            let inner_paint = solid_paint(stroke_rgb, 255);
            let inner_stroke = Stroke {
                width: 3.0,
                line_cap: LineCap::Round,
                line_join: LineJoin::Round,
                ..Stroke::default()
            };
            pixmap.stroke_path(
                &path,
                &inner_paint,
                &inner_stroke,
                Transform::identity(),
                None,
            );
        }
    }
    Ok(())
}

fn render_action_vectors(
    pixmap: &mut Pixmap,
    vectors: &[ActionVector],
    w: u32,
    h: u32,
) -> Result<(), CoreError> {
    let wf = w as f32;
    let hf = h as f32;

    for v in vectors {
        let stroke_rgb = parse_hex_rgb(&v.stroke_hex)?;
        let halo_rgb = parse_hex_rgb(&v.halo_hex)?;
        let sw = (v.stroke_width_px as f32).max(1.0);

        let x0 = (v.from_x_ratio as f32) * wf;
        let y0 = (v.from_y_ratio as f32) * hf;
        let x1 = (v.to_x_ratio as f32) * wf;
        let y1 = (v.to_y_ratio as f32) * hf;

        let dx = x1 - x0;
        let dy = y1 - y0;
        let len = (dx * dx + dy * dy).sqrt();
        if len < 1.0 {
            continue;
        }
        let ux = dx / len;
        let uy = dy / len;
        let head_len = (sw * 3.8).max(10.0).min(len * 0.45);
        let head_half_w = head_len * 0.58;

        let shaft_end_x = x1 - ux * head_len * 0.7;
        let shaft_end_y = y1 - uy * head_len * 0.7;

        let mut shaft_pb = PathBuilder::new();
        shaft_pb.move_to(x0, y0);
        shaft_pb.line_to(shaft_end_x, shaft_end_y);
        if let Some(shaft_path) = shaft_pb.finish() {
            let dash = match v.style {
                VectorStrokeStyle::Solid => None,
                VectorStrokeStyle::Dashed => StrokeDash::new(vec![sw * 3.0, sw * 2.0], 0.0),
            };
            let halo_stroke = Stroke {
                width: sw + 4.0,
                line_cap: LineCap::Round,
                line_join: LineJoin::Round,
                dash: dash.clone(),
                ..Stroke::default()
            };
            let halo_paint = solid_paint(halo_rgb, 245);
            pixmap.stroke_path(
                &shaft_path,
                &halo_paint,
                &halo_stroke,
                Transform::identity(),
                None,
            );

            let inner_stroke = Stroke {
                width: sw,
                line_cap: LineCap::Round,
                line_join: LineJoin::Round,
                dash,
                ..Stroke::default()
            };
            let inner_paint = solid_paint(stroke_rgb, 255);
            pixmap.stroke_path(
                &shaft_path,
                &inner_paint,
                &inner_stroke,
                Transform::identity(),
                None,
            );
        }

        // Crisp haloed arrowhead at (x1, y1).
        let base_x = x1 - ux * head_len;
        let base_y = y1 - uy * head_len;
        let px = -uy * head_half_w;
        let py = ux * head_half_w;

        let mut head_pb = PathBuilder::new();
        head_pb.move_to(x1, y1);
        head_pb.line_to(base_x + px, base_y + py);
        head_pb.line_to(base_x - px, base_y - py);
        head_pb.close();
        if let Some(head_path) = head_pb.finish() {
            let halo_stroke = Stroke {
                width: 4.0,
                line_join: LineJoin::Round,
                ..Stroke::default()
            };
            let halo_paint = solid_paint(halo_rgb, 245);
            pixmap.stroke_path(
                &head_path,
                &halo_paint,
                &halo_stroke,
                Transform::identity(),
                None,
            );
            let fill_paint = solid_paint(stroke_rgb, 255);
            pixmap.fill_path(
                &head_path,
                &fill_paint,
                FillRule::Winding,
                Transform::identity(),
                None,
            );
        }
    }
    Ok(())
}

fn solid_paint(rgb: [u8; 3], alpha: u8) -> Paint<'static> {
    let mut paint = Paint::default();
    paint.set_color_rgba8(rgb[0], rgb[1], rgb[2], alpha);
    paint.anti_alias = true;
    paint
}

fn copy_rgba_into_premul_pixmap(src: &RgbaImage, dst: &mut Pixmap) {
    let data = dst.data_mut();
    for (i, px) in src.pixels().enumerate() {
        let a = u16::from(px.0[3]);
        let r = ((u16::from(px.0[0]) * a + 127) / 255) as u8;
        let g = ((u16::from(px.0[1]) * a + 127) / 255) as u8;
        let b = ((u16::from(px.0[2]) * a + 127) / 255) as u8;
        let offset = i * 4;
        data[offset] = r;
        data[offset + 1] = g;
        data[offset + 2] = b;
        data[offset + 3] = px.0[3];
    }
}

fn pixmap_into_unpremul_rgba(pixmap: Pixmap) -> RgbaImage {
    let w = pixmap.width();
    let h = pixmap.height();
    let mut raw = pixmap.take();
    let (pixels, remainder) = raw.as_chunks_mut::<4>();
    debug_assert!(remainder.is_empty());
    for px in pixels {
        let a = px[3];
        if a == 0 {
            px[0] = 0;
            px[1] = 0;
            px[2] = 0;
        } else if a < 255 {
            let af = f32::from(a) / 255.0;
            px[0] = (f32::from(px[0]) / af).round().clamp(0.0, 255.0) as u8;
            px[1] = (f32::from(px[1]) / af).round().clamp(0.0, 255.0) as u8;
            px[2] = (f32::from(px[2]) / af).round().clamp(0.0, 255.0) as u8;
        }
    }
    RgbaImage::from_raw(w, h, raw).expect("Pixmap raw buffer matches 4 * w * h")
}

fn require_non_empty_str(field: &str, value: &str) -> Result<(), CoreError> {
    if value.trim().is_empty() || value.chars().any(char::is_control) {
        return Err(CoreError::GuideRenderFailed(format!(
            "GuideSpec {field} must be a non-empty string without control characters"
        )));
    }
    Ok(())
}

fn require_unit_ratio(field: &str, val: f64) -> Result<(), CoreError> {
    if !val.is_finite() || !(0.0..=1.0).contains(&val) {
        return Err(CoreError::GuideRenderFailed(format!(
            "GuideSpec {field} ({val}) must be a finite number in [0, 1]"
        )));
    }
    Ok(())
}

fn require_pos_finite(field: &str, val: f64) -> Result<(), CoreError> {
    if !val.is_finite() || val <= 0.0 {
        return Err(CoreError::GuideRenderFailed(format!(
            "GuideSpec {field} ({val}) must be a positive finite number"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io::Cursor;
    #[cfg(unix)]
    use std::os::unix::fs::symlink;

    use image::{DynamicImage, ImageBuffer, ImageFormat, Rgba};
    use rstest::rstest;
    use tempfile::TempDir;

    use super::*;

    fn sample_blockout_json() -> &'static str {
        r##"{
  "schemaVersion": "1.0",
  "id": "guide-blockout-001",
  "title": "Default Blockout SpatialGuide",
  "variant": "Blockout",
  "targetImagePath": null,
  "canvasWidth": 512,
  "canvasHeight": 512,
  "depthBands": [
    {
      "id": "band-bg",
      "label": "Background Horizon",
      "plane": "background",
      "yStartRatio": 0,
      "yEndRatio": 0.36,
      "fillHex": "#2c3344"
    },
    {
      "id": "band-mg",
      "label": "Midground Stage",
      "plane": "midground",
      "yStartRatio": 0.36,
      "yEndRatio": 0.72,
      "fillHex": "#3b4254"
    },
    {
      "id": "band-fg",
      "label": "Foreground Anchor",
      "plane": "foreground",
      "yStartRatio": 0.72,
      "yEndRatio": 1,
      "fillHex": "#4a5164"
    }
  ],
  "mannequins": [
    {
      "id": "mannequin-01",
      "subjectSlug": "courier-ren",
      "label": "Courier Ren (1.75m)",
      "xRatio": 0.42,
      "yRatio": 0.78,
      "heightMeters": 1.75,
      "scaleFactor": 1.0,
      "facing": "right",
      "poseHint": "Three-quarter ready stance",
      "paletteHex": "#63b3ed"
    }
  ],
  "actionVectors": [
    {
      "id": "vector-01",
      "label": "Primary Motion Vector",
      "fromXRatio": 0.42,
      "fromYRatio": 0.56,
      "toXRatio": 0.72,
      "toYRatio": 0.44,
      "strokeHex": "#f6ad55",
      "haloHex": "#1a1d24",
      "strokeWidthPx": 4,
      "style": "dashed"
    }
  ],
  "overlayRegions": []
}"##
    }

    fn sample_overlay_json(target_path: &str) -> String {
        format!(
            r##"{{
  "schemaVersion": "1.0",
  "id": "guide-overlay-001",
  "title": "Default OverlayGuide Trajectory Cue",
  "variant": "OverlayGuide",
  "targetImagePath": "{target_path}",
  "canvasWidth": 256,
  "canvasHeight": 256,
  "depthBands": [],
  "mannequins": [],
  "actionVectors": [
    {{
      "id": "vector-01",
      "label": "Trajectory Cue",
      "fromXRatio": 0.2,
      "fromYRatio": 0.8,
      "toXRatio": 0.8,
      "toYRatio": 0.2,
      "strokeHex": "#F6AD55",
      "haloHex": "#1A1D24",
      "strokeWidthPx": 4,
      "style": "solid"
    }}
  ],
  "overlayRegions": [
    {{
      "id": "region-rect",
      "label": "Target Box",
      "kind": "rect",
      "points": [
        {{ "xRatio": 0.1, "yRatio": 0.1 }},
        {{ "xRatio": 0.4, "yRatio": 0.4 }}
      ],
      "strokeHex": "#68D391",
      "haloHex": "#1A1D24",
      "fillHex": "#68D391",
      "opacity": 0.2
    }},
    {{
      "id": "region-ellipse",
      "label": "Target Oval",
      "kind": "ellipse",
      "points": [
        {{ "xRatio": 0.55, "yRatio": 0.55 }},
        {{ "xRatio": 0.85, "yRatio": 0.85 }}
      ],
      "strokeHex": "#63B3ED",
      "haloHex": "#1A1D24",
      "fillHex": "#63B3ED",
      "opacity": 0.2
    }},
    {{
      "id": "region-poly",
      "label": "Arc Path",
      "kind": "polyline",
      "points": [
        {{ "xRatio": 0.15, "yRatio": 0.5 }},
        {{ "xRatio": 0.3, "yRatio": 0.45 }},
        {{ "xRatio": 0.45, "yRatio": 0.52 }}
      ],
      "strokeHex": "#F56565",
      "haloHex": "#1A1D24",
      "fillHex": "#F56565",
      "opacity": 0.0
    }}
  ]
}}"##
        )
    }

    #[test]
    fn parses_blockout_guidespec_matching_typescript_contract_and_renders_deterministically() {
        let spec = GuideSpec::from_json_str(sample_blockout_json()).expect("valid Blockout JSON");
        assert_eq!(spec.variant, SpatialGuideVariant::Blockout);
        assert_eq!(spec.target_image_path, None);
        assert_eq!(spec.depth_bands[0].fill_hex, "#2C3344");
        assert_eq!(spec.mannequins[0].palette_hex, "#63B3ED");

        let reserialized = spec.to_json_string().unwrap();
        assert!(reserialized.contains("\"targetImagePath\": null"));
        assert!(reserialized.contains("\"schemaVersion\": \"1.0\""));

        let renderer = GuideRenderer::new();
        let ws = TempDir::new().unwrap();
        let r1 = renderer.render(&spec, ws.path()).expect("render 1");
        let r2 = renderer.render(&spec, ws.path()).expect("render 2");
        assert_eq!(
            r1.png_bytes, r2.png_bytes,
            "rendering must be byte-deterministic"
        );

        let img = image::load_from_memory(&r1.png_bytes).unwrap().to_rgba8();
        assert_eq!((img.width(), img.height()), (512, 512));
        // Background depth band color in top-left corner (#2C3344)
        assert_eq!(img.get_pixel(8, 8).0, [0x2C, 0x33, 0x44, 255]);
        // Foreground depth band color in bottom-left corner (#4A5164)
        assert_eq!(img.get_pixel(8, 500).0, [0x4A, 0x51, 0x64, 255]);
        // Mannequin torso near (0.42*512=215, 0.78*512 - 120 = 280) should contain #63B3ED tint
        assert_ne!(img.get_pixel(215, 310).0, [0x3B, 0x42, 0x54, 255]);
    }

    #[test]
    fn renders_overlay_guide_onto_contained_target_image() {
        let ws = TempDir::new().unwrap();
        fs::create_dir_all(ws.path().join("runs")).unwrap();
        let target: RgbaImage = ImageBuffer::from_pixel(256, 256, Rgba([90, 100, 115, 255]));
        let mut target_buf = Vec::new();
        DynamicImage::ImageRgba8(target)
            .write_to(&mut Cursor::new(&mut target_buf), ImageFormat::Png)
            .unwrap();
        fs::write(ws.path().join("runs/v001.png"), target_buf).unwrap();

        let json = sample_overlay_json("runs/v001.png");
        let spec = GuideSpec::from_json_str(&json).expect("valid OverlayGuide JSON");
        let rendered = GuideRenderer::new()
            .render(&spec, ws.path())
            .expect("OverlayGuide renders");

        let img = image::load_from_memory(&rendered.png_bytes)
            .unwrap()
            .to_rgba8();
        assert_eq!((img.width(), img.height()), (256, 256));
        // Unannotated corner retains exact target pixel color
        assert_eq!(img.get_pixel(2, 2).0, [90, 100, 115, 255]);
        // Center of action vector (128, 128) is painted with vector stroke
        assert_ne!(img.get_pixel(128, 128).0, [90, 100, 115, 255]);
    }

    #[rstest]
    #[case::traversal_target_path("../outside.png")]
    #[case::absolute_target_path("/etc/passwd")]
    #[case::symlink_target_path("runs/link.png")]
    fn overlay_guide_rejects_unsafe_or_symlinked_target_image_path(#[case] target_path: &str) {
        let ws = TempDir::new().unwrap();
        fs::create_dir_all(ws.path().join("runs")).unwrap();
        let outside = ws.path().join("outside.png");
        let img: RgbaImage = ImageBuffer::from_pixel(64, 64, Rgba([50, 50, 50, 255]));
        let mut buf = Vec::new();
        DynamicImage::ImageRgba8(img)
            .write_to(&mut Cursor::new(&mut buf), ImageFormat::Png)
            .unwrap();
        fs::write(&outside, buf).unwrap();

        #[cfg(unix)]
        if target_path == "runs/link.png" {
            symlink(&outside, ws.path().join("runs/link.png")).unwrap();
        }

        let spec = GuideSpec::from_json_str(&sample_overlay_json(target_path)).unwrap();
        let err = GuideRenderer::new()
            .render(&spec, ws.path())
            .expect_err("unsafe targetImagePath must fail closed");
        assert!(matches!(
            err,
            CoreError::GuideRenderFailed(_) | CoreError::Io { .. }
        ));
    }

    #[rstest]
    #[case::wrong_schema_version("\"schemaVersion\": \"1.0\"", "\"schemaVersion\": \"2.0\"")]
    #[case::inverted_depth_band("\"yEndRatio\": 0.36", "\"yEndRatio\": 0.0")]
    #[case::blockout_with_non_null_target(
        "\"targetImagePath\": null",
        "\"targetImagePath\": \"runs/v001.png\""
    )]
    #[case::unknown_field(
        "\"title\": \"Default Blockout SpatialGuide\"",
        "\"title\": \"Default Blockout SpatialGuide\", \"extraProp\": 123"
    )]
    #[case::invalid_hex_color("\"fillHex\": \"#2c3344\"", "\"fillHex\": \"not-hex\"")]
    #[case::zero_height_meters("\"heightMeters\": 1.75", "\"heightMeters\": 0")]
    fn rejects_invalid_guidespec_json_fail_closed(#[case] needle: &str, #[case] replacement: &str) {
        let invalid = sample_blockout_json().replacen(needle, replacement, 1);
        let err = GuideSpec::from_json_str(&invalid).expect_err("invalid GuideSpec must fail");
        assert!(matches!(err, CoreError::GuideRenderFailed(_)));
    }

    #[rstest]
    #[case::short_vector_5px_on_512(512, 512, 5.0)]
    #[case::short_vector_15px_on_512(512, 512, 15.0)]
    #[case::small_blockout_20x20_with_15px_vector(20, 20, 15.0)]
    #[case::tiny_blockout_10x10_with_5px_vector(10, 10, 5.0)]
    fn renders_short_action_vectors_and_small_blockout_without_panic(
        #[case] canvas_w: u32,
        #[case] canvas_h: u32,
        #[case] vector_len_px: f64,
    ) {
        let mut spec = GuideSpec::from_json_str(sample_blockout_json()).unwrap();
        spec.canvas_width = canvas_w;
        spec.canvas_height = canvas_h;
        let dx_ratio = (vector_len_px / f64::from(canvas_w)).min(0.8);
        spec.action_vectors[0].from_x_ratio = 0.1;
        spec.action_vectors[0].from_y_ratio = 0.5;
        spec.action_vectors[0].to_x_ratio = 0.1 + dx_ratio;
        spec.action_vectors[0].to_y_ratio = 0.5;

        let ws = TempDir::new().unwrap();
        let rendered = GuideRenderer::new()
            .render(&spec, ws.path())
            .expect("small Blockout and short ActionVector must render without panic");
        assert_eq!((rendered.width, rendered.height), (canvas_w, canvas_h));
        let decoded = image::load_from_memory(&rendered.png_bytes).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (canvas_w, canvas_h));
    }

    #[rstest]
    #[case::excessive_stroke_width("stroke_width")]
    #[case::excessive_depth_bands("depth_bands")]
    #[case::excessive_mannequins("mannequins")]
    #[case::excessive_action_vectors("action_vectors")]
    #[case::excessive_overlay_regions("overlay_regions")]
    #[case::excessive_polyline_points("polyline_points")]
    fn validate_and_normalize_rejects_excessive_guidespec_bounds(#[case] case_kind: &str) {
        let mut spec = GuideSpec::from_json_str(sample_blockout_json()).unwrap();
        match case_kind {
            "stroke_width" => {
                spec.action_vectors[0].stroke_width_px = f64::from(MAX_GUIDE_CANVAS_DIM) + 1.0;
            }
            "depth_bands" => {
                let template = spec.depth_bands[0].clone();
                spec.depth_bands = vec![template; MAX_GUIDE_DEPTH_BANDS + 1];
            }
            "mannequins" => {
                let template = spec.mannequins[0].clone();
                spec.mannequins = vec![template; MAX_GUIDE_MANNEQUINS + 1];
            }
            "action_vectors" => {
                let template = spec.action_vectors[0].clone();
                spec.action_vectors = vec![template; MAX_GUIDE_ACTION_VECTORS + 1];
            }
            "overlay_regions" => {
                let region = OverlayRegion {
                    id: "r".to_string(),
                    label: "R".to_string(),
                    kind: OverlayRegionKind::Rect,
                    points: vec![
                        NormalizedPoint {
                            x_ratio: 0.1,
                            y_ratio: 0.1,
                        },
                        NormalizedPoint {
                            x_ratio: 0.2,
                            y_ratio: 0.2,
                        },
                    ],
                    stroke_hex: "#FFFFFF".to_string(),
                    halo_hex: "#000000".to_string(),
                    fill_hex: "#FFFFFF".to_string(),
                    opacity: 0.1,
                };
                spec.overlay_regions = vec![region; MAX_GUIDE_OVERLAY_REGIONS + 1];
            }
            "polyline_points" => {
                spec.overlay_regions.push(OverlayRegion {
                    id: "poly".to_string(),
                    label: "Poly".to_string(),
                    kind: OverlayRegionKind::Polyline,
                    points: vec![
                        NormalizedPoint {
                            x_ratio: 0.1,
                            y_ratio: 0.1,
                        };
                        MAX_GUIDE_REGION_POINTS + 1
                    ],
                    stroke_hex: "#FFFFFF".to_string(),
                    halo_hex: "#000000".to_string(),
                    fill_hex: "#FFFFFF".to_string(),
                    opacity: 0.0,
                });
            }
            other => panic!("unexpected case: {other}"),
        }
        let err = spec
            .validate_and_normalize()
            .expect_err("oversized GuideSpec bounds must fail closed");
        assert!(matches!(err, CoreError::GuideRenderFailed(_)));
    }
}
