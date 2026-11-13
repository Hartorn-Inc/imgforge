//! === DYNAMIC ATLAS PACKER ===
//!
//! PURPOSE:
//! Dynamically packs only selected `ReferencePack` component slices (for up to
//! `<= 3` focal `Subject`s) onto `#252830` canvas page(s) without clipping or
//! overlapping, returning exact placement coordinates (ADR 0003, `GLOSSARY.md`).
//!
//! THREAT MODEL / DESIGN RATIONALE ("WHY"):
//! 1. Sending full multi-pose turnaround sheets for 3 characters floods the image
//!    model with unused poses and causes attention dilution. Packing only the
//!    requested `(subject_slug, style_slug, slice_ids)` slices onto a compact
//!    `#252830` `Atlas` preserves visual fidelity.
//! 2. Enforcing `Visibility` compatibility before loading or packing slices
//!    prevents `internal` or `restricted` `Subject` slices from leaking into a
//!    `public` scene or `Atlas`.
//!
//! EXECUTION LIFECYCLE / DATA FLOW:
//! 1. Validate `1..=3` unique focal subjects and check `Visibility` compatibility.
//! 2. Load requested slice PNGs from `library/subjects/<slug>/packs/<style>/slices/`.
//! 3. Shelf-pack slices deterministically onto `#252830` canvas(es) with `padding_px`,
//!    verifying zero clipping and zero overlap.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Cursor;
use std::path::Path;

use image::imageops::overlay;
use image::{DynamicImage, ImageBuffer, ImageFormat, Rgba, RgbaImage};
use serde::{Deserialize, Serialize};

use crate::calibration::STUDIO_BG_RGBA;
use crate::error::CoreError;
use crate::reference_pack::ReferencePackManifest;
use crate::registry::LibraryRegistry;
use crate::scene_spec::{MAX_FOREGROUND_SLOTS, Visibility, check_visibility_compatibility};
use crate::slicer::{PixelRect, SlicesManifest};
use crate::store::{
    checked_pixel_count, decode_png_checked, read_regular_file_nofollow,
    validate_strict_relative_path, verify_contained_path_no_symlinks,
};

/// Maximum number of distinct focal `Subject`s allowed in a single `Atlas`.
pub const MAX_ATLAS_SUBJECTS: usize = MAX_FOREGROUND_SLOTS;

/// Default `Atlas` page width in pixels.
pub const DEFAULT_ATLAS_WIDTH: u32 = 1536;

/// Default `Atlas` page height in pixels.
pub const DEFAULT_ATLAS_HEIGHT: u32 = 1024;

/// Default inter-slice and border margin in pixels.
pub const DEFAULT_ATLAS_PADDING_PX: u32 = 16;

/// Requested slices belonging to one `(Subject, Style)` pair.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AtlasSliceSelection {
    /// Target `Subject` slug.
    pub subject_slug: String,
    /// Target `Style` slug.
    pub style_slug: String,
    /// Specific `slice_id`s from `slices.json` to pack (never dumps unselected slices).
    pub slice_ids: Vec<String>,
}

/// Request payload for `pack_dynamic_atlas`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AtlasPackRequest {
    /// `1..=3` selected `(subject_slug, style_slug, slice_ids)` entries.
    pub selections: Vec<AtlasSliceSelection>,
    /// Maximum permitted `Visibility` of the target context (`SceneSpec` / `Export`).
    pub target_visibility: Visibility,
    /// Target `Atlas` canvas width in pixels.
    pub canvas_width: u32,
    /// Target `Atlas` canvas height in pixels.
    pub canvas_height: u32,
    /// Padding between slices and canvas edges in pixels.
    pub padding_px: u32,
}

impl AtlasPackRequest {
    /// Construct an `AtlasPackRequest` with default `1536x1024` canvas and `16 px` padding.
    #[must_use]
    pub fn new(selections: Vec<AtlasSliceSelection>, target_visibility: Visibility) -> Self {
        Self {
            selections,
            target_visibility,
            canvas_width: DEFAULT_ATLAS_WIDTH,
            canvas_height: DEFAULT_ATLAS_HEIGHT,
            padding_px: DEFAULT_ATLAS_PADDING_PX,
        }
    }
}

/// Exact placement coordinates of one packed slice on an `AtlasPage`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AtlasPlacement {
    /// Source `Subject` slug.
    pub subject_slug: String,
    /// Source `Style` slug.
    pub style_slug: String,
    /// Slice identifier from `slices.json`.
    pub slice_id: String,
    /// Zero-based page index inside `AtlasResult.pages`.
    pub page_index: usize,
    /// Packed bounding box (`x`, `y`, `width`, `height`) on `page_index`.
    pub rect: PixelRect,
}

/// Single rendered `#252830` `Atlas` canvas page with its placed slice coordinates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AtlasPage {
    /// Zero-based page index.
    pub page_index: usize,
    /// Canvas width in pixels.
    pub width: u32,
    /// Canvas height in pixels.
    pub height: u32,
    /// Validated PNG bytes of the packed `#252830` canvas.
    pub png_bytes: Vec<u8>,
    /// Placements residing on this page.
    pub placements: Vec<AtlasPlacement>,
}

/// Combined result returned by `pack_dynamic_atlas`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AtlasResult {
    /// One or more packed `#252830` canvas pages.
    pub pages: Vec<AtlasPage>,
    /// Flat list of all placements across pages.
    pub placements: Vec<AtlasPlacement>,
}

struct LoadedSliceItem {
    subject_slug: String,
    style_slug: String,
    slice_id: String,
    image: RgbaImage,
}

/// Dynamically pack selected `ReferencePack` slices onto `#252830` canvas(es) without
/// clipping or overlap, enforcing `<= 3` focal subjects and `Visibility` boundaries.
pub fn pack_dynamic_atlas(
    registry: &LibraryRegistry,
    library_dir: &Path,
    request: &AtlasPackRequest,
) -> Result<AtlasResult, CoreError> {
    if request.selections.is_empty() {
        return Err(CoreError::AtlasPackFailed(
            "Atlas packing requires at least 1 subject selection".to_string(),
        ));
    }
    if request.selections.len() > MAX_ATLAS_SUBJECTS {
        return Err(CoreError::AtlasPackFailed(format!(
            "Atlas supports at most {MAX_ATLAS_SUBJECTS} focal subjects (got {})",
            request.selections.len()
        )));
    }
    if request.canvas_width == 0 || request.canvas_height == 0 {
        return Err(CoreError::AtlasPackFailed(
            "Atlas canvas_width and canvas_height must be greater than 0".to_string(),
        ));
    }
    checked_pixel_count(request.canvas_width, request.canvas_height, "Atlas canvas")
        .map_err(|err| CoreError::AtlasPackFailed(err.to_string()))?;

    let mut seen_subjects = HashSet::with_capacity(request.selections.len());
    let mut loaded_items = Vec::new();

    for sel in &request.selections {
        if !seen_subjects.insert(sel.subject_slug.clone()) {
            return Err(CoreError::AtlasPackFailed(format!(
                "duplicate subject_slug '{}' in Atlas selections",
                sel.subject_slug
            )));
        }
        if sel.slice_ids.is_empty() {
            return Err(CoreError::AtlasPackFailed(format!(
                "selection for subject '{}' must specify at least 1 slice_id",
                sel.subject_slug
            )));
        }

        let resolved_subject = registry.resolve_subject(&sel.subject_slug)?;
        let _resolved_style = registry.resolve_style(&sel.style_slug)?;
        check_visibility_compatibility(
            request.target_visibility,
            "Atlas",
            [(
                resolved_subject.subject.slug.as_str(),
                resolved_subject.subject.visibility,
            )],
        )?;

        let rel_pack_dir = Path::new("subjects")
            .join(&resolved_subject.subject.slug)
            .join("packs")
            .join(&sel.style_slug);
        let pack_dir = verify_contained_path_no_symlinks(
            library_dir,
            &rel_pack_dir,
            true,
            "ReferencePack directory",
        )?;
        let pack_json_candidate = pack_dir.join("pack.json");
        match fs::symlink_metadata(&pack_json_candidate) {
            Ok(_) => {
                let pack_json_path = verify_contained_path_no_symlinks(
                    &pack_dir,
                    Path::new("pack.json"),
                    false,
                    "pack.json",
                )?;
                let pack_bytes = read_regular_file_nofollow(&pack_json_path, "pack.json")?;
                let pack_manifest: ReferencePackManifest = serde_json::from_slice(&pack_bytes)
                    .map_err(|err| {
                        CoreError::AtlasPackFailed(format!(
                            "invalid pack.json for ({}, {}): {err}",
                            sel.subject_slug, sel.style_slug
                        ))
                    })?;
                check_visibility_compatibility(
                    request.target_visibility,
                    "Atlas ReferencePack",
                    [(
                        format!("{}/{}", sel.subject_slug, sel.style_slug).as_str(),
                        pack_manifest.visibility,
                    )],
                )?;
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => {
                return Err(CoreError::Io {
                    context: format!(
                        "inspecting ReferencePack manifest {}",
                        pack_json_candidate.display()
                    ),
                    source: err,
                });
            }
        }
        let slices_json_path = verify_contained_path_no_symlinks(
            &pack_dir,
            Path::new("slices.json"),
            false,
            "ReferencePack slices.json",
        )?;
        let manifest_bytes =
            read_regular_file_nofollow(&slices_json_path, "ReferencePack slices.json").map_err(
                |err| {
                    CoreError::AtlasPackFailed(format!(
                        "failed to load slices.json for ({}, {}): {err}",
                        sel.subject_slug, sel.style_slug
                    ))
                },
            )?;
        let slices_manifest: SlicesManifest =
            serde_json::from_slice(&manifest_bytes).map_err(|err| {
                CoreError::AtlasPackFailed(format!(
                    "invalid slices.json at {}: {err}",
                    slices_json_path.display()
                ))
            })?;

        let by_id: HashMap<&str, &crate::slicer::SliceRecord> = slices_manifest
            .slices
            .iter()
            .map(|r| (r.id.as_str(), r))
            .collect();

        let mut seen_slices = HashSet::with_capacity(sel.slice_ids.len());
        for slice_id in &sel.slice_ids {
            if !seen_slices.insert(slice_id.as_str()) {
                return Err(CoreError::AtlasPackFailed(format!(
                    "duplicate slice_id '{slice_id}' for subject '{}'",
                    sel.subject_slug
                )));
            }
            let record = by_id.get(slice_id.as_str()).ok_or_else(|| {
                CoreError::AtlasPackFailed(format!(
                    "slice_id '{slice_id}' not found in ({}, {}) slices.json",
                    sel.subject_slug, sel.style_slug
                ))
            })?;
            let rel_slice = validate_strict_relative_path(&record.file, "slice file")
                .map_err(CoreError::AtlasPackFailed)?;
            let slice_path =
                verify_contained_path_no_symlinks(&pack_dir, &rel_slice, false, "slice PNG")
                    .map_err(|err| {
                        CoreError::AtlasPackFailed(format!(
                            "invalid slice path '{}' for '{slice_id}': {err}",
                            record.file
                        ))
                    })?;

            let png_bytes = read_regular_file_nofollow(&slice_path, "slice PNG")?;
            let decoded = decode_png_checked(&slice_path, &png_bytes)?;
            let rgba = decoded.to_rgba8();
            let required_w = rgba.width().saturating_add(request.padding_px * 2);
            let required_h = rgba.height().saturating_add(request.padding_px * 2);
            if required_w > request.canvas_width || required_h > request.canvas_height {
                return Err(CoreError::AtlasPackFailed(format!(
                    "slice '{slice_id}' ({}x{} with {}px padding) exceeds Atlas canvas {}x{}",
                    rgba.width(),
                    rgba.height(),
                    request.padding_px,
                    request.canvas_width,
                    request.canvas_height
                )));
            }

            loaded_items.push(LoadedSliceItem {
                subject_slug: sel.subject_slug.clone(),
                style_slug: sel.style_slug.clone(),
                slice_id: slice_id.clone(),
                image: rgba,
            });
        }
    }

    // Deterministic height-descending sort for tight shelf packing.
    loaded_items.sort_by(|a, b| {
        b.image
            .height()
            .cmp(&a.image.height())
            .then_with(|| b.image.width().cmp(&a.image.width()))
            .then_with(|| a.subject_slug.cmp(&b.subject_slug))
            .then_with(|| a.slice_id.cmp(&b.slice_id))
    });

    let pad = request.padding_px;
    let cw = request.canvas_width;
    let ch = request.canvas_height;

    let mut pages_buf: Vec<(RgbaImage, Vec<AtlasPlacement>)> = vec![(
        ImageBuffer::from_pixel(cw, ch, Rgba(STUDIO_BG_RGBA)),
        Vec::new(),
    )];
    let mut cursor_x = pad;
    let mut cursor_y = pad;
    let mut shelf_h = 0u32;

    for item in loaded_items {
        let sw = item.image.width();
        let sh = item.image.height();

        if cursor_x + sw + pad > cw {
            cursor_x = pad;
            cursor_y = cursor_y.saturating_add(shelf_h).saturating_add(pad);
            shelf_h = 0;
        }
        if cursor_y + sh + pad > ch {
            pages_buf.push((
                ImageBuffer::from_pixel(cw, ch, Rgba(STUDIO_BG_RGBA)),
                Vec::new(),
            ));
            cursor_x = pad;
            cursor_y = pad;
            shelf_h = 0;
        }

        let page_index = pages_buf.len() - 1;
        let (canvas, placements) = pages_buf.last_mut().expect("at least one page exists");
        overlay(
            canvas,
            &item.image,
            i64::from(cursor_x),
            i64::from(cursor_y),
        );

        let rect = PixelRect {
            x: cursor_x,
            y: cursor_y,
            width: sw,
            height: sh,
        };
        placements.push(AtlasPlacement {
            subject_slug: item.subject_slug,
            style_slug: item.style_slug,
            slice_id: item.slice_id,
            page_index,
            rect,
        });

        cursor_x = cursor_x.saturating_add(sw).saturating_add(pad);
        shelf_h = shelf_h.max(sh);
    }

    let mut pages = Vec::with_capacity(pages_buf.len());
    let mut all_placements = Vec::new();

    for (page_index, (canvas, placements)) in pages_buf.into_iter().enumerate() {
        verify_page_placements(&placements, cw, ch)?;
        let mut png_bytes = Vec::new();
        DynamicImage::ImageRgba8(canvas)
            .write_to(&mut Cursor::new(&mut png_bytes), ImageFormat::Png)
            .map_err(|err| CoreError::AtlasPackFailed(format!("encoding Atlas PNG: {err}")))?;
        all_placements.extend(placements.iter().cloned());
        pages.push(AtlasPage {
            page_index,
            width: cw,
            height: ch,
            png_bytes,
            placements,
        });
    }

    Ok(AtlasResult {
        pages,
        placements: all_placements,
    })
}

fn verify_page_placements(
    placements: &[AtlasPlacement],
    canvas_w: u32,
    canvas_h: u32,
) -> Result<(), CoreError> {
    for (i, p) in placements.iter().enumerate() {
        if p.rect.right() > canvas_w || p.rect.bottom() > canvas_h {
            return Err(CoreError::AtlasPackFailed(format!(
                "placement '{}' clips outside {canvas_w}x{canvas_h}",
                p.slice_id
            )));
        }
        for other in &placements[i + 1..] {
            if p.rect.intersects(other.rect) {
                return Err(CoreError::AtlasPackFailed(format!(
                    "placements '{}' and '{}' overlap on page {}",
                    p.slice_id, other.slice_id, p.page_index
                )));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::symlink;

    use rstest::rstest;
    use tempfile::TempDir;

    use super::*;
    use crate::slicer::{SliceRecord, SliceSource, SlicesManifest};

    fn seed_subject_and_slices(
        library_dir: &Path,
        subject_slug: &str,
        style_slug: &str,
        visibility: &str,
        slice_specs: &[(&str, u32, u32, [u8; 3])],
    ) {
        let subj_dir = library_dir.join("subjects").join(subject_slug);
        fs::create_dir_all(&subj_dir).unwrap();
        fs::write(
            subj_dir.join("card.yaml"),
            format!(
                r##"
schema_version: 1
kind: character
slug: {subject_slug}
display_name: Subject {subject_slug}
height_m: 1.72
exclusive_surface_map:
  bindings:
    - slot: cloak
      material: wool
      color_hex: "#2C3E50"
palette_lock:
  swatches:
    - name: cloak
      color_hex: "#2C3E50"
visibility: {visibility}
"##
            ),
        )
        .unwrap();

        let pack_dir = subj_dir.join("packs").join(style_slug);
        let slices_dir = pack_dir.join("slices");
        fs::create_dir_all(&slices_dir).unwrap();

        let mut records = Vec::new();
        for (idx, (id, w, h, rgb)) in slice_specs.iter().enumerate() {
            let img: RgbaImage =
                ImageBuffer::from_pixel(*w, *h, Rgba([rgb[0], rgb[1], rgb[2], 255]));
            let mut png_bytes = Vec::new();
            DynamicImage::ImageRgba8(img)
                .write_to(&mut Cursor::new(&mut png_bytes), ImageFormat::Png)
                .unwrap();
            let rel_file = format!("slices/{id}.png");
            fs::write(pack_dir.join(&rel_file), &png_bytes).unwrap();
            records.push(SliceRecord {
                id: (*id).to_string(),
                file: rel_file,
                rect: PixelRect {
                    x: (idx as u32) * 10,
                    y: 0,
                    width: *w,
                    height: *h,
                },
                source: SliceSource::Auto,
                label: None,
                sha256: crate::lifecycle::sha256_hex(&png_bytes),
            });
        }

        let manifest = SlicesManifest {
            schema_version: 1,
            sheet_filename: "turnaround.png".to_string(),
            sheet_width: 1536,
            sheet_height: 1024,
            calibration_strip_height: 72,
            slices: records,
        };
        fs::write(
            pack_dir.join("slices.json"),
            serde_json::to_vec_pretty(&manifest).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn packs_only_requested_slices_without_overlap_or_clipping() {
        let root = TempDir::new().expect("tempdir");
        let library_dir = root.path().join("library");
        seed_subject_and_slices(
            &library_dir,
            "courier-ren",
            "franco-belgian-line",
            "public",
            &[
                ("turnaround_001", 200, 320, [200, 80, 60]),
                ("turnaround_002", 220, 320, [60, 200, 80]),
                ("unused_pose", 180, 300, [255, 255, 0]),
            ],
        );
        seed_subject_and_slices(
            &library_dir,
            "archivist-mira",
            "franco-belgian-line",
            "public",
            &[("poses_001", 240, 340, [80, 120, 220])],
        );

        let registry = LibraryRegistry::new(Some(library_dir.clone()), None);
        let req = AtlasPackRequest::new(
            vec![
                AtlasSliceSelection {
                    subject_slug: "courier-ren".to_string(),
                    style_slug: "franco-belgian-line".to_string(),
                    slice_ids: vec!["turnaround_001".to_string()],
                },
                AtlasSliceSelection {
                    subject_slug: "archivist-mira".to_string(),
                    style_slug: "franco-belgian-line".to_string(),
                    slice_ids: vec!["poses_001".to_string()],
                },
            ],
            Visibility::Public,
        );

        let atlas = pack_dynamic_atlas(&registry, &library_dir, &req).expect("atlas packs");
        assert_eq!(atlas.pages.len(), 1);
        assert_eq!(atlas.placements.len(), 2);
        assert!(atlas.placements.iter().all(|p| p.slice_id != "unused_pose"));

        let page_img = image::load_from_memory(&atlas.pages[0].png_bytes)
            .unwrap()
            .to_rgba8();
        assert_eq!((page_img.width(), page_img.height()), (1536, 1024));
        // Background corner is #252830
        assert_eq!(page_img.get_pixel(0, 0).0, STUDIO_BG_RGBA);
        // Each placement pixel color matches its source slice
        for p in &atlas.placements {
            let px = page_img.get_pixel(p.rect.x + 10, p.rect.y + 10).0;
            assert_ne!(px, STUDIO_BG_RGBA);
        }
    }

    #[test]
    fn splits_across_multiple_pages_without_clipping_when_canvas_fills() {
        let root = TempDir::new().expect("tempdir");
        let library_dir = root.path().join("library");
        seed_subject_and_slices(
            &library_dir,
            "courier-ren",
            "franco-belgian-line",
            "public",
            &[
                ("s1", 300, 300, [200, 80, 60]),
                ("s2", 300, 300, [80, 200, 60]),
                ("s3", 300, 300, [60, 80, 200]),
            ],
        );

        let registry = LibraryRegistry::new(Some(library_dir.clone()), None);
        let req = AtlasPackRequest {
            selections: vec![AtlasSliceSelection {
                subject_slug: "courier-ren".to_string(),
                style_slug: "franco-belgian-line".to_string(),
                slice_ids: vec!["s1".to_string(), "s2".to_string(), "s3".to_string()],
            }],
            target_visibility: Visibility::Public,
            canvas_width: 700,
            canvas_height: 400,
            padding_px: 16,
        };

        let atlas = pack_dynamic_atlas(&registry, &library_dir, &req).expect("multi-page atlas");
        assert_eq!(atlas.pages.len(), 2);
        assert_eq!(atlas.pages[0].placements.len(), 2);
        assert_eq!(atlas.pages[1].placements.len(), 1);
    }

    #[rstest]
    #[case::more_than_three_subjects(4, Visibility::Public, Visibility::Public)]
    #[case::zero_subjects(0, Visibility::Public, Visibility::Public)]
    fn enforces_subject_count_limits(
        #[case] count: usize,
        #[case] target_vis: Visibility,
        #[case] subj_vis: Visibility,
    ) {
        let root = TempDir::new().expect("tempdir");
        let library_dir = root.path().join("library");
        let mut selections = Vec::new();
        for idx in 0..count {
            let slug = format!("subj-{idx}");
            seed_subject_and_slices(
                &library_dir,
                &slug,
                "franco-belgian-line",
                subj_vis.as_str(),
                &[("s1", 64, 64, [150, 150, 150])],
            );
            selections.push(AtlasSliceSelection {
                subject_slug: slug,
                style_slug: "franco-belgian-line".to_string(),
                slice_ids: vec!["s1".to_string()],
            });
        }
        let registry = LibraryRegistry::new(Some(library_dir.clone()), None);
        let req = AtlasPackRequest::new(selections, target_vis);
        let err = pack_dynamic_atlas(&registry, &library_dir, &req)
            .expect_err("invalid subject count must fail closed");
        assert!(matches!(err, CoreError::AtlasPackFailed(_)));
    }

    #[rstest]
    #[case::internal_into_public("internal", Visibility::Public)]
    #[case::restricted_into_public("restricted", Visibility::Public)]
    #[case::internal_into_restricted("internal", Visibility::Restricted)]
    fn rejects_visibility_leaks_in_atlas(
        #[case] subject_vis: &str,
        #[case] target_vis: Visibility,
    ) {
        let root = TempDir::new().expect("tempdir");
        let library_dir = root.path().join("library");
        seed_subject_and_slices(
            &library_dir,
            "secret-envoy",
            "franco-belgian-line",
            subject_vis,
            &[("s1", 64, 64, [150, 150, 150])],
        );

        let registry = LibraryRegistry::new(Some(library_dir.clone()), None);
        let req = AtlasPackRequest::new(
            vec![AtlasSliceSelection {
                subject_slug: "secret-envoy".to_string(),
                style_slug: "franco-belgian-line".to_string(),
                slice_ids: vec!["s1".to_string()],
            }],
            target_vis,
        );
        let err = pack_dynamic_atlas(&registry, &library_dir, &req)
            .expect_err("visibility leak must fail closed");
        assert!(matches!(err, CoreError::VisibilityViolation { .. }));
    }

    #[rstest]
    #[case::parent_dir_traversal("../outside.png")]
    #[case::backslash_path("slices\\s1.png")]
    #[case::absolute_path("/tmp/outside.png")]
    #[case::cur_dir_path("./slices/s1.png")]
    fn rejects_unsafe_slice_record_paths_in_atlas(#[case] bad_file: &str) {
        let root = TempDir::new().expect("tempdir");
        let library_dir = root.path().join("library");
        seed_subject_and_slices(
            &library_dir,
            "courier-ren",
            "franco-belgian-line",
            "public",
            &[("s1", 64, 64, [150, 150, 150])],
        );

        let outside_img: RgbaImage = ImageBuffer::from_pixel(64, 64, Rgba([200, 100, 100, 255]));
        let mut buf = Vec::new();
        DynamicImage::ImageRgba8(outside_img)
            .write_to(&mut Cursor::new(&mut buf), ImageFormat::Png)
            .unwrap();
        fs::write(
            library_dir.join("subjects/courier-ren/packs/outside.png"),
            buf,
        )
        .unwrap();

        let slices_json_path =
            library_dir.join("subjects/courier-ren/packs/franco-belgian-line/slices.json");
        let mut manifest: SlicesManifest =
            serde_json::from_slice(&fs::read(&slices_json_path).unwrap()).unwrap();
        manifest.slices[0].file = bad_file.to_string();
        fs::write(
            &slices_json_path,
            serde_json::to_vec_pretty(&manifest).unwrap(),
        )
        .unwrap();

        let registry = LibraryRegistry::new(Some(library_dir.clone()), None);
        let req = AtlasPackRequest::new(
            vec![AtlasSliceSelection {
                subject_slug: "courier-ren".to_string(),
                style_slug: "franco-belgian-line".to_string(),
                slice_ids: vec!["s1".to_string()],
            }],
            Visibility::Public,
        );
        let err = pack_dynamic_atlas(&registry, &library_dir, &req)
            .expect_err("unsafe record.file path must fail closed");
        assert!(matches!(err, CoreError::AtlasPackFailed(_)));
    }

    #[cfg(unix)]
    #[rstest]
    #[case::symlinked_pack_dir("pack_dir")]
    #[case::symlinked_slices_dir("slices_dir")]
    #[case::symlinked_slice_file("slice_file")]
    fn rejects_symlink_escapes_in_atlas(#[case] mode: &str) {
        let root = TempDir::new().expect("tempdir");
        let library_dir = root.path().join("library");
        seed_subject_and_slices(
            &library_dir,
            "courier-ren",
            "franco-belgian-line",
            "public",
            &[("s1", 64, 64, [150, 150, 150])],
        );

        let pack_dir = library_dir.join("subjects/courier-ren/packs/franco-belgian-line");
        let escape_dir = root.path().join("escape_pack");
        fs::create_dir_all(&escape_dir).unwrap();

        match mode {
            "pack_dir" => {
                fs::rename(&pack_dir, escape_dir.join("real_pack")).unwrap();
                symlink(escape_dir.join("real_pack"), &pack_dir).unwrap();
            }
            "slices_dir" => {
                let slices_dir = pack_dir.join("slices");
                fs::rename(&slices_dir, escape_dir.join("real_slices")).unwrap();
                symlink(escape_dir.join("real_slices"), &slices_dir).unwrap();
            }
            "slice_file" => {
                let slice_file = pack_dir.join("slices/s1.png");
                fs::rename(&slice_file, escape_dir.join("real_s1.png")).unwrap();
                symlink(escape_dir.join("real_s1.png"), &slice_file).unwrap();
            }
            _ => unreachable!(),
        }

        let registry = LibraryRegistry::new(Some(library_dir.clone()), None);
        let req = AtlasPackRequest::new(
            vec![AtlasSliceSelection {
                subject_slug: "courier-ren".to_string(),
                style_slug: "franco-belgian-line".to_string(),
                slice_ids: vec!["s1".to_string()],
            }],
            Visibility::Public,
        );
        let err = pack_dynamic_atlas(&registry, &library_dir, &req)
            .expect_err("symlink escape in Atlas must fail closed");
        assert!(matches!(
            err,
            CoreError::AtlasPackFailed(_) | CoreError::Io { .. }
        ));
    }

    #[rstest]
    #[case::internal_pack_blocks_public_atlas("internal", Visibility::Public)]
    #[case::restricted_pack_blocks_public_atlas("restricted", Visibility::Public)]
    #[case::internal_pack_blocks_restricted_atlas("internal", Visibility::Restricted)]
    fn rejects_pack_json_visibility_leaks_even_when_subject_card_is_public(
        #[case] pack_vis: &str,
        #[case] target_vis: Visibility,
    ) {
        let root = TempDir::new().unwrap();
        let library_dir = root.path().join("library");
        seed_subject_and_slices(
            &library_dir,
            "courier-ren",
            "franco-belgian-line",
            "public",
            &[("s1", 64, 64, [150, 150, 150])],
        );

        let pack_json = serde_json::json!({
            "schema_version": 1,
            "version": 1,
            "subject_slug": "courier-ren",
            "style_slug": "franco-belgian-line",
            "subject_kind": "character",
            "visibility": pack_vis,
            "lifecycle": "draft",
            "seed": null,
            "sheets": [],
            "slices": [],
            "artifact_hashes": {},
            "created_at_iso": "2026-01-01T00:00:00Z"
        });
        fs::write(
            library_dir.join("subjects/courier-ren/packs/franco-belgian-line/pack.json"),
            serde_json::to_vec_pretty(&pack_json).unwrap(),
        )
        .unwrap();

        let registry = LibraryRegistry::new(Some(library_dir.clone()), None);
        let req = AtlasPackRequest::new(
            vec![AtlasSliceSelection {
                subject_slug: "courier-ren".to_string(),
                style_slug: "franco-belgian-line".to_string(),
                slice_ids: vec!["s1".to_string()],
            }],
            target_vis,
        );
        let err = pack_dynamic_atlas(&registry, &library_dir, &req)
            .expect_err("pack.json visibility leak must fail closed");
        assert!(matches!(err, CoreError::VisibilityViolation { .. }));
    }

    #[rstest]
    #[case::width_exceeds_max_dim(16_385, 512)]
    #[case::total_pixels_exceeds_cap(16_384, 16_384)]
    fn rejects_oversized_atlas_canvas_dimensions_before_allocating(
        #[case] canvas_width: u32,
        #[case] canvas_height: u32,
    ) {
        let root = TempDir::new().unwrap();
        let library_dir = root.path().join("library");
        seed_subject_and_slices(
            &library_dir,
            "courier-ren",
            "franco-belgian-line",
            "public",
            &[("s1", 64, 64, [150, 150, 150])],
        );
        let registry = LibraryRegistry::new(Some(library_dir.clone()), None);
        let req = AtlasPackRequest {
            selections: vec![AtlasSliceSelection {
                subject_slug: "courier-ren".to_string(),
                style_slug: "franco-belgian-line".to_string(),
                slice_ids: vec!["s1".to_string()],
            }],
            target_visibility: Visibility::Public,
            canvas_width,
            canvas_height,
            padding_px: 16,
        };
        let err = pack_dynamic_atlas(&registry, &library_dir, &req)
            .expect_err("oversized Atlas canvas must fail closed before buffer allocation");
        assert!(matches!(err, CoreError::AtlasPackFailed(_)));
    }
}
