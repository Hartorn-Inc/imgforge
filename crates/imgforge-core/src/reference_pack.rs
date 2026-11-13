//! === STYLE-SCOPED REFERENCE PACK PIPELINE ===
//!
//! PURPOSE:
//! Generates and persists style-scoped `ReferencePack` sheets and sliced
//! components under `library/subjects/<slug>/packs/<style_slug>/` via the
//! `ImageBackend` seam (ADR 0003, ADR 0005).
//!
//! THREAT MODEL / DESIGN RATIONALE ("WHY"):
//! 1. Scoping packs by `(subject_slug, style_slug)` prevents outline and medium
//!    bleed across styles while keeping `Subject` identity style-neutral.
//! 2. `character` and `creature` subjects execute a strict 2-step anchor chain:
//!    Step 1 generates `turnaround.png` (`1536x1024`, `#252830` backdrop, optional
//!    `SeedInput`), and Step 2 anchors `poses_expressions.png` and
//!    `inventory_flatlay.png` onto `turnaround.png` + `Style` exemplars.
//!    `prop` and `location` subjects execute a single-sheet `sheet.png` template.
//! 3. Every generated sheet undergoes structural dimension and `#252830` backdrop
//!    validation, receives a host-composited `#252830` calibration strip (`L*`
//!    wedge + pixel-exact `PaletteLock` swatches), and is sliced by foreground
//!    bounding-box detection into `slices/` + `slices.json`.
//!
//! EXECUTION LIFECYCLE / DATA FLOW:
//! 1. Resolve `Subject` and `Style` via `LibraryRegistry`.
//! 2. Materialize `Style` exemplars (and optional `SeedInput`) for `-i` anchors.
//! 3. Execute 2-step or 1-sheet generation via `ImageBackend`.
//! 4. Validate `#252830` structure, composite host calibration strip, slice
//!    components, and persist `pack.json` + `slices.json` atomically.

use std::collections::{BTreeMap, HashSet};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tempfile::TempDir;

use crate::backend::{ImageBackend, ImageGenerationRequest};
use crate::calibration::{
    CalibrationStripLayout, SheetStructuralReport, StructuralValidationConfig,
    composite_calibration_strip_png, validate_reference_sheet_structure,
};
use crate::error::CoreError;
use crate::lifecycle::{Lifecycle, SeedInput, now_utc_iso, sha256_hex};
use crate::registry::{LibraryRegistry, ResolvedStyle, ResolvedSubject};
use crate::scene_spec::Visibility;
use crate::slicer::{RectOverride, SliceRecord, SlicerConfig, SlicesManifest, slice_sheet_image};
use crate::store::{
    read_regular_file_nofollow, verify_contained_path_no_symlinks,
    verify_existing_dir_prefix_no_symlinks, write_atomic,
};
use crate::style::Style;
use crate::subject::{Subject, SubjectKind};

/// Canonical `ReferencePack` sheet width in pixels.
pub const REFERENCE_SHEET_WIDTH: u32 = 1536;

/// Canonical `ReferencePack` sheet height in pixels.
pub const REFERENCE_SHEET_HEIGHT: u32 = 1024;

/// Supported `ReferencePackManifest` schema version.
pub const SUPPORTED_REFERENCE_PACK_SCHEMA_VERSION: u32 = 1;

fn default_pack_version() -> u32 {
    1
}

/// Role of an individual sheet inside a style-scoped `ReferencePack`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReferenceSheetRole {
    /// Step 1 anchor turnaround sheet (`turnaround.png`) for `character`/`creature`.
    Turnaround,
    /// Step 2 expression & dynamic pose sheet (`poses_expressions.png`) anchored on turnaround.
    PosesExpressions,
    /// Step 2 isolated props/equipment flatlay (`inventory_flatlay.png`) anchored on turnaround.
    InventoryFlatlay,
    /// Single-sheet template (`sheet.png`) for `prop`/`location`.
    SingleSheet,
}

impl ReferenceSheetRole {
    /// Canonical PNG filename inside `packs/<style_slug>/`.
    #[must_use]
    pub const fn filename(self) -> &'static str {
        match self {
            Self::Turnaround => "turnaround.png",
            Self::PosesExpressions => "poses_expressions.png",
            Self::InventoryFlatlay => "inventory_flatlay.png",
            Self::SingleSheet => "sheet.png",
        }
    }

    /// Short prefix used when naming sliced components for this sheet.
    #[must_use]
    pub const fn slice_prefix(self) -> &'static str {
        match self {
            Self::Turnaround => "turnaround",
            Self::PosesExpressions => "poses",
            Self::InventoryFlatlay => "inventory",
            Self::SingleSheet => "sheet",
        }
    }
}

/// Persisted metadata for a single generated sheet inside `pack.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeneratedSheetRecord {
    /// Role of this sheet within the `ReferencePack`.
    pub role: ReferenceSheetRole,
    /// Filename relative to the pack directory.
    pub filename: String,
    /// Lowercase SHA-256 hex digest of the final host-calibrated sheet PNG.
    pub sha256: String,
    /// Structural `#252830` and dimension validation report.
    pub structural_report: SheetStructuralReport,
    /// Host-composited calibration strip pixel coordinates.
    pub calibration_strip: CalibrationStripLayout,
    /// Optional backend thread ID.
    #[serde(default)]
    pub thread_id: Option<String>,
}

/// Persisted `pack.json` manifest stored under `library/subjects/<slug>/packs/<style_slug>/pack.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReferencePackManifest {
    /// Schema version; must equal `1`.
    pub schema_version: u32,
    /// Monotonic version number for this style-scoped `ReferencePack`.
    #[serde(default = "default_pack_version")]
    pub version: u32,
    /// `Subject` slug.
    pub subject_slug: String,
    /// `Style` slug.
    pub style_slug: String,
    /// `SubjectKind` determining 2-step vs 1-sheet chain.
    pub subject_kind: SubjectKind,
    /// Inherited `Visibility` of the `Subject`.
    pub visibility: Visibility,
    /// `Lifecycle` state (starts in `draft`).
    pub lifecycle: Lifecycle,
    /// Optional bootstrap `SeedInput`.
    #[serde(default)]
    pub seed: Option<SeedInput>,
    /// Generated and calibrated sheets.
    pub sheets: Vec<GeneratedSheetRecord>,
    /// All sliced component records across sheets (referencing `slices/<id>.png`).
    pub slices: Vec<SliceRecord>,
    /// Sorted map of relative pack filenames to SHA-256 hex digests.
    pub artifact_hashes: BTreeMap<String, String>,
    /// RFC 3339 UTC timestamp when generated.
    pub created_at_iso: String,
}

/// Request parameters for generating a style-scoped `ReferencePack`.
#[derive(Debug, Clone)]
pub struct ReferencePackRequest {
    /// Target `Subject` slug.
    pub subject_slug: String,
    /// Target `Style` slug.
    pub style_slug: String,
    /// Optional bootstrap `SeedInput` PNG path.
    pub seed_path: Option<PathBuf>,
    /// Optional descriptive label for the `SeedInput`.
    pub seed_label: Option<String>,
    /// Structural validation thresholds.
    pub structural_config: StructuralValidationConfig,
    /// Base slicer options (`exclude_bottom_rows` and `id_prefix` are populated automatically per sheet).
    pub slicer_config: SlicerConfig,
    /// Optional per-sheet `RectOverride` lists keyed by `ReferenceSheetRole`.
    pub sheet_overrides: BTreeMap<ReferenceSheetRole, Vec<RectOverride>>,
}

impl ReferencePackRequest {
    /// Construct a default `ReferencePackRequest` for `(subject_slug, style_slug)`.
    #[must_use]
    pub fn new(subject_slug: impl Into<String>, style_slug: impl Into<String>) -> Self {
        Self {
            subject_slug: subject_slug.into(),
            style_slug: style_slug.into(),
            seed_path: None,
            seed_label: None,
            structural_config: StructuralValidationConfig::default(),
            slicer_config: SlicerConfig::default(),
            sheet_overrides: BTreeMap::new(),
        }
    }
}

/// Result returned by `ReferencePackPipeline::generate`.
#[derive(Debug, Clone, PartialEq)]
pub struct GeneratedReferencePack {
    /// Target pack directory (`library/subjects/<subject_slug>/packs/<style_slug>`).
    pub pack_dir: PathBuf,
    /// Path to `pack.json`.
    pub pack_manifest_path: PathBuf,
    /// Path to `slices.json`.
    pub slices_manifest_path: PathBuf,
    /// Parsed `ReferencePackManifest`.
    pub manifest: ReferencePackManifest,
}

/// Style-scoped `ReferencePack` orchestration pipeline executing through `ImageBackend`.
pub struct ReferencePackPipeline<'a> {
    backend: &'a dyn ImageBackend,
    registry: &'a LibraryRegistry,
    library_dir: PathBuf,
}

impl<'a> ReferencePackPipeline<'a> {
    /// Construct a `ReferencePackPipeline` bound to a backend, library registry, and project `library_dir`.
    #[must_use]
    pub fn new(
        backend: &'a dyn ImageBackend,
        registry: &'a LibraryRegistry,
        library_dir: PathBuf,
    ) -> Self {
        Self {
            backend,
            registry,
            library_dir,
        }
    }

    /// Generate a style-scoped `ReferencePack`, calibrate sheets, slice components, and persist outputs.
    pub async fn generate(
        &self,
        request: &ReferencePackRequest,
    ) -> Result<GeneratedReferencePack, CoreError> {
        let resolved_subject: ResolvedSubject =
            self.registry.resolve_subject(&request.subject_slug)?;
        let resolved_style: ResolvedStyle = self.registry.resolve_style(&request.style_slug)?;

        validate_request_overrides(resolved_subject.subject.kind, request)?;

        let seed = match request.seed_path {
            Some(ref path) => Some(SeedInput::from_png_path(path, request.seed_label.clone())?),
            None => None,
        };

        let temp_exemplars = TempDir::new().map_err(|err| CoreError::Io {
            context: "creating temporary directory for Style exemplars".to_string(),
            source: err,
        })?;
        let exemplar_paths = resolved_style.materialize_exemplars(temp_exemplars.path())?;

        let rel_pack_dir = Path::new("subjects")
            .join(&resolved_subject.subject.slug)
            .join("packs")
            .join(&resolved_style.style.slug);
        let rel_slices_dir = rel_pack_dir.join("slices");
        verify_existing_dir_prefix_no_symlinks(
            &self.library_dir,
            &rel_slices_dir,
            "ReferencePack slices directory",
        )?;

        let pack_dir = self.library_dir.join(&rel_pack_dir);
        let pack_manifest_path = pack_dir.join("pack.json");
        let mut next_version = 1u32;
        match fs::symlink_metadata(&pack_manifest_path) {
            Ok(meta) => {
                if meta.file_type().is_symlink() || !meta.is_file() {
                    return Err(CoreError::ReferencePackFailed(format!(
                        "existing pack.json at {} must be a regular non-symlink file",
                        pack_manifest_path.display()
                    )));
                }
                let existing_bytes = read_regular_file_nofollow(&pack_manifest_path, "pack.json")?;
                let existing: ReferencePackManifest = serde_json::from_slice(&existing_bytes)
                    .map_err(|err| CoreError::ReferencePackFailed(err.to_string()))?;
                existing.lifecycle.ensure_mutable(&format!(
                    "pack:{}/{}",
                    resolved_subject.subject.slug, resolved_style.style.slug
                ))?;
                next_version = existing.version.saturating_add(1);
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => {
                return Err(CoreError::Io {
                    context: format!("inspecting pack.json at {}", pack_manifest_path.display()),
                    source: err,
                });
            }
        }

        fs::create_dir_all(pack_dir.join("slices")).map_err(|err| CoreError::Io {
            context: format!("creating ReferencePack directory {}", pack_dir.display()),
            source: err,
        })?;
        verify_contained_path_no_symlinks(
            &self.library_dir,
            &rel_slices_dir,
            true,
            "ReferencePack slices directory",
        )?;

        let mut sheet_records = Vec::new();
        let mut all_slices = Vec::new();
        let mut artifact_hashes = BTreeMap::new();

        match resolved_subject.subject.kind {
            SubjectKind::Character | SubjectKind::Creature => {
                // Step 1: Generate turnaround.png (1536x1024) with optional SeedInput + Style exemplars.
                let mut step1_refs = Vec::new();
                if let Some(ref s) = seed {
                    step1_refs.push(s.path.clone());
                }
                step1_refs.extend(exemplar_paths.iter().cloned());

                let turnaround_prompt = compile_pack_sheet_prompt(
                    &resolved_subject.subject,
                    &resolved_style.style,
                    ReferenceSheetRole::Turnaround,
                );
                let (turnaround_record, turnaround_slices, turnaround_path) = self
                    .generate_calibrate_and_slice_sheet(
                        &pack_dir,
                        ReferenceSheetRole::Turnaround,
                        turnaround_prompt,
                        step1_refs,
                        &resolved_subject.subject,
                        request,
                        &mut artifact_hashes,
                    )
                    .await?;
                sheet_records.push(turnaround_record);
                all_slices.extend(turnaround_slices);

                // Step 2: Anchor poses_expressions.png and inventory_flatlay.png onto turnaround.png + Style exemplars.
                let mut step2_refs = vec![turnaround_path];
                step2_refs.extend(exemplar_paths.iter().cloned());

                for role in [
                    ReferenceSheetRole::PosesExpressions,
                    ReferenceSheetRole::InventoryFlatlay,
                ] {
                    let prompt = compile_pack_sheet_prompt(
                        &resolved_subject.subject,
                        &resolved_style.style,
                        role,
                    );
                    let (rec, slices, _) = self
                        .generate_calibrate_and_slice_sheet(
                            &pack_dir,
                            role,
                            prompt,
                            step2_refs.clone(),
                            &resolved_subject.subject,
                            request,
                            &mut artifact_hashes,
                        )
                        .await?;
                    sheet_records.push(rec);
                    all_slices.extend(slices);
                }
            }
            SubjectKind::Prop | SubjectKind::Location => {
                // Single-sheet template (sheet.png).
                let mut refs = Vec::new();
                if let Some(ref s) = seed {
                    refs.push(s.path.clone());
                }
                refs.extend(exemplar_paths);

                let prompt = compile_pack_sheet_prompt(
                    &resolved_subject.subject,
                    &resolved_style.style,
                    ReferenceSheetRole::SingleSheet,
                );
                let (rec, slices, _) = self
                    .generate_calibrate_and_slice_sheet(
                        &pack_dir,
                        ReferenceSheetRole::SingleSheet,
                        prompt,
                        refs,
                        &resolved_subject.subject,
                        request,
                        &mut artifact_hashes,
                    )
                    .await?;
                sheet_records.push(rec);
                all_slices.extend(slices);
            }
        }

        // Ensure no slice ID collided across sheets before writing slices.json.
        let mut seen_slice_ids = HashSet::with_capacity(all_slices.len());
        for slice in &all_slices {
            if !seen_slice_ids.insert(slice.id.as_str()) {
                return Err(CoreError::ReferencePackFailed(format!(
                    "duplicate slice_id '{}' produced across ReferencePack sheets",
                    slice.id
                )));
            }
        }

        // Write combined slices.json at the pack root.
        let primary_sheet = sheet_records
            .first()
            .map(|s| s.filename.clone())
            .unwrap_or_else(|| "sheet.png".to_string());
        let strip_h = sheet_records
            .first()
            .map(|s| s.calibration_strip.strip_height)
            .unwrap_or(0);
        let slices_manifest = SlicesManifest {
            schema_version: 1,
            sheet_filename: primary_sheet,
            sheet_width: REFERENCE_SHEET_WIDTH,
            sheet_height: REFERENCE_SHEET_HEIGHT,
            calibration_strip_height: strip_h,
            slices: all_slices.clone(),
        };
        let slices_json_bytes = serde_json::to_vec_pretty(&slices_manifest)
            .map_err(|err| CoreError::ReferencePackFailed(err.to_string()))?;
        let slices_manifest_path = pack_dir.join("slices.json");
        write_atomic(&slices_manifest_path, &slices_json_bytes, "slices.json")?;
        artifact_hashes.insert("slices.json".to_string(), sha256_hex(&slices_json_bytes));

        let manifest = ReferencePackManifest {
            schema_version: SUPPORTED_REFERENCE_PACK_SCHEMA_VERSION,
            version: next_version,
            subject_slug: resolved_subject.subject.slug,
            style_slug: resolved_style.style.slug,
            subject_kind: resolved_subject.subject.kind,
            visibility: resolved_subject.subject.visibility,
            lifecycle: Lifecycle::Draft,
            seed,
            sheets: sheet_records,
            slices: all_slices,
            artifact_hashes,
            created_at_iso: now_utc_iso(),
        };

        let pack_bytes = serde_json::to_vec_pretty(&manifest)
            .map_err(|err| CoreError::ReferencePackFailed(err.to_string()))?;
        write_atomic(&pack_manifest_path, &pack_bytes, "pack.json")?;

        Ok(GeneratedReferencePack {
            pack_dir,
            pack_manifest_path,
            slices_manifest_path,
            manifest,
        })
    }

    #[allow(clippy::too_many_arguments)]
    async fn generate_calibrate_and_slice_sheet(
        &self,
        pack_dir: &Path,
        role: ReferenceSheetRole,
        prompt: String,
        reference_images: Vec<PathBuf>,
        subject: &Subject,
        request: &ReferencePackRequest,
        artifact_hashes: &mut BTreeMap<String, String>,
    ) -> Result<(GeneratedSheetRecord, Vec<SliceRecord>, PathBuf), CoreError> {
        let output = self
            .backend
            .generate(&ImageGenerationRequest {
                prompt,
                width: REFERENCE_SHEET_WIDTH,
                height: REFERENCE_SHEET_HEIGHT,
                reference_images,
            })
            .await?;

        let structural_report = validate_reference_sheet_structure(
            &output.png_bytes,
            REFERENCE_SHEET_WIDTH,
            REFERENCE_SHEET_HEIGHT,
            &request.structural_config,
        )?;

        let (calibrated_png, calibration_strip) =
            composite_calibration_strip_png(&output.png_bytes, &subject.palette_lock)?;

        let filename = role.filename().to_string();
        let sheet_path = pack_dir.join(&filename);
        write_atomic(&sheet_path, &calibrated_png, &filename)?;
        let sheet_sha256 = sha256_hex(&calibrated_png);
        artifact_hashes.insert(filename.clone(), sheet_sha256.clone());

        let decoded = image::load_from_memory_with_format(&calibrated_png, image::ImageFormat::Png)
            .map_err(|err| CoreError::InvalidPng {
                path: sheet_path.clone(),
                reason: format!("re-decoding calibrated sheet failed: {err}"),
            })?
            .to_rgba8();

        let mut slice_cfg = request.slicer_config.clone();
        slice_cfg.exclude_bottom_rows = calibration_strip.strip_height;
        slice_cfg.id_prefix = Some(role.slice_prefix().to_string());
        if let Some(role_overrides) = request.sheet_overrides.get(&role) {
            slice_cfg.overrides = role_overrides.clone();
        }

        let extracted = slice_sheet_image(&decoded, &slice_cfg)?;
        let mut slice_records = Vec::with_capacity(extracted.len());
        for slice in extracted {
            let slice_dest = pack_dir.join(&slice.record.file);
            write_atomic(&slice_dest, &slice.png_bytes, &slice.record.id)?;
            artifact_hashes.insert(slice.record.file.clone(), slice.record.sha256.clone());
            slice_records.push(slice.record);
        }

        Ok((
            GeneratedSheetRecord {
                role,
                filename,
                sha256: sheet_sha256,
                structural_report,
                calibration_strip,
                thread_id: output.thread_id,
            },
            slice_records,
            sheet_path,
        ))
    }
}

fn validate_request_overrides(
    subject_kind: SubjectKind,
    request: &ReferencePackRequest,
) -> Result<(), CoreError> {
    match subject_kind {
        SubjectKind::Character | SubjectKind::Creature => {
            if !request.slicer_config.overrides.is_empty() {
                return Err(CoreError::ReferencePackFailed(
                    "multi-sheet Character/Creature ReferencePack cannot use unkeyed slicer_config.overrides; key overrides by ReferenceSheetRole via sheet_overrides".to_string(),
                ));
            }
            if request
                .sheet_overrides
                .contains_key(&ReferenceSheetRole::SingleSheet)
            {
                return Err(CoreError::ReferencePackFailed(
                    "Character/Creature ReferencePack cannot use SingleSheet overrides".to_string(),
                ));
            }
        }
        SubjectKind::Prop | SubjectKind::Location => {
            if request
                .sheet_overrides
                .keys()
                .any(|role| *role != ReferenceSheetRole::SingleSheet)
            {
                return Err(CoreError::ReferencePackFailed(
                    "Prop/Location ReferencePack only supports SingleSheet overrides".to_string(),
                ));
            }
            if !request.slicer_config.overrides.is_empty()
                && request
                    .sheet_overrides
                    .contains_key(&ReferenceSheetRole::SingleSheet)
            {
                return Err(CoreError::ReferencePackFailed(
                    "specify overrides in either slicer_config.overrides or sheet_overrides[SingleSheet], not both".to_string(),
                ));
            }
        }
    }

    let mut seen_override_ids = HashSet::new();
    for (role, overrides) in &request.sheet_overrides {
        for ov in overrides {
            let trimmed = ov.id.trim();
            if !seen_override_ids.insert(trimmed.to_string()) {
                return Err(CoreError::ReferencePackFailed(format!(
                    "sheet_overrides for {role:?} reuses slice override id '{trimmed}' from another sheet"
                )));
            }
        }
    }
    Ok(())
}

/// Build a positive-only, domain-agnostic prompt for a specific `ReferencePack` sheet role.
#[must_use]
pub fn compile_pack_sheet_prompt(
    subject: &Subject,
    style: &Style,
    role: ReferenceSheetRole,
) -> String {
    let mut out = String::with_capacity(512);
    let _ = writeln!(out, "[REFERENCE_PACK_SHEET]");
    let _ = writeln!(out, "sheet_role: {}", role.filename());
    let _ = writeln!(
        out,
        "studio_backdrop: solid uniform neutral #252830 studio background with clean 72px bottom margin"
    );
    let _ = writeln!(
        out,
        "layout_instruction: {}",
        match role {
            ReferenceSheetRole::Turnaround => {
                "full-body orthogonal turnaround showing front view, three-quarter view, side profile, and back view arranged side by side with consistent proportions"
            }
            ReferenceSheetRole::PosesExpressions => {
                "anchored character study matching the turnaround reference with three distinct dynamic action poses and four clear facial expression portraits spaced evenly on #252830"
            }
            ReferenceSheetRole::InventoryFlatlay => {
                "anchored equipment flatlay matching the turnaround reference displaying isolated held props, tools, garments, and gear arranged cleanly with generous #252830 spacing"
            }
            ReferenceSheetRole::SingleSheet => {
                "multi-angle canonical reference sheet displaying primary three-quarter view, side elevation, and close surface detail cleanly separated on #252830"
            }
        }
    );

    let _ = writeln!(out, "\n[SUBJECT]");
    let _ = writeln!(out, "slug: {}", subject.slug);
    let _ = writeln!(out, "kind: {}", subject.kind.as_str());
    let _ = writeln!(out, "display_name: {}", subject.display_name);
    if let Some(h) = subject.height_m {
        let _ = writeln!(out, "height_m: {h:.2}m");
    }
    if !subject.asymmetric_markers.is_empty() {
        let _ = writeln!(
            out,
            "asymmetric_markers: {}",
            subject.asymmetric_markers.join("; ")
        );
    }
    if !subject.positive_weapon_geometry.is_empty() {
        let _ = writeln!(
            out,
            "positive_weapon_geometry: {}",
            subject.positive_weapon_geometry.join("; ")
        );
    }
    let surfaces: Vec<String> = subject
        .exclusive_surface_map
        .bindings
        .iter()
        .map(|b| format!("{}={} {}", b.slot, b.material, b.color_hex))
        .collect();
    let _ = writeln!(out, "exclusive_surface_map: {}", surfaces.join("; "));

    let _ = writeln!(out, "\n[STYLE]");
    let _ = writeln!(out, "slug: {}", style.slug);
    let _ = writeln!(out, "medium: {}", style.medium);
    let _ = writeln!(out, "linework: {}", style.linework);
    let _ = writeln!(out, "shading_and_texture: {}", style.shading_and_texture);
    let _ = writeln!(out, "palette_profile: {}", style.palette_profile);
    let _ = write!(out, "guardrails: {}", style.style_guardrails.join("; "));
    out
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;
    #[cfg(unix)]
    use std::os::unix::fs::symlink;
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use image::{DynamicImage, ImageBuffer, ImageFormat, Rgba, RgbaImage};
    use rstest::rstest;
    use tempfile::TempDir;

    use super::*;
    use crate::backend::ImageGenerationOutput;
    use crate::calibration::{STUDIO_BG_RGB, parse_hex_rgb};
    use crate::slicer::PixelRect;

    #[derive(Clone)]
    struct MockPackBackend {
        calls: Arc<Mutex<Vec<ImageGenerationRequest>>>,
    }

    impl MockPackBackend {
        fn new() -> Self {
            Self {
                calls: Arc::new(Mutex::new(Vec::new())),
            }
        }
    }

    #[async_trait]
    impl ImageBackend for MockPackBackend {
        fn name(&self) -> &'static str {
            "mock-pack-backend"
        }

        async fn generate(
            &self,
            request: &ImageGenerationRequest,
        ) -> Result<ImageGenerationOutput, CoreError> {
            self.calls.lock().unwrap().push(request.clone());
            let mut img: RgbaImage = ImageBuffer::from_pixel(
                request.width,
                request.height,
                Rgba([STUDIO_BG_RGB[0], STUDIO_BG_RGB[1], STUDIO_BG_RGB[2], 255]),
            );
            // Draw two distinct mock foreground poses above the 72px bottom strip
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

    fn write_subject_card(library_dir: &Path, slug: &str, kind: &str) {
        let dir = library_dir.join("subjects").join(slug);
        fs::create_dir_all(&dir).unwrap();
        let height_line = if kind == "character" || kind == "creature" {
            "height_m: 1.75\n"
        } else {
            ""
        };
        let yaml = format!(
            r##"
schema_version: 1
kind: {kind}
slug: {slug}
display_name: Test Subject {slug}
{height_line}exclusive_surface_map:
  bindings:
    - slot: body
      material: painted wood
      color_hex: "#3b5998"
palette_lock:
  swatches:
    - name: primary_blue
      color_hex: "#3b5998"
      surface_slot: body
    - name: brass_accent
      color_hex: "#d4a843"
visibility: public
"##
        );
        fs::write(dir.join("card.yaml"), yaml).unwrap();
    }

    #[rstest]
    #[case::character_runs_2_step_anchor_chain("courier-ren", "character", 3, true)]
    #[case::creature_runs_2_step_anchor_chain("harbor-drake", "creature", 3, true)]
    #[case::prop_runs_single_sheet("brass-lantern", "prop", 1, false)]
    #[case::location_runs_single_sheet("stone-pier", "location", 1, false)]
    #[tokio::test]
    async fn reference_pack_pipeline_executes_kind_specific_chain_and_calibrates_and_slices(
        #[case] slug: &str,
        #[case] kind: &str,
        #[case] expected_sheets: usize,
        #[case] expect_step2_turnaround_anchor: bool,
    ) {
        let root = TempDir::new().expect("tempdir");
        let library_dir = root.path().join("library");
        write_subject_card(&library_dir, slug, kind);

        let seed_path = root.path().join("seed.png");
        let seed_img: RgbaImage = ImageBuffer::from_pixel(64, 64, Rgba([120, 100, 80, 255]));
        DynamicImage::ImageRgba8(seed_img)
            .save_with_format(&seed_path, ImageFormat::Png)
            .unwrap();

        let registry = LibraryRegistry::new(Some(library_dir.clone()), None);
        let backend = MockPackBackend::new();
        let pipeline = ReferencePackPipeline::new(&backend, &registry, library_dir.clone());

        let mut req = ReferencePackRequest::new(slug, "franco-belgian-line");
        req.seed_path = Some(seed_path.clone());
        req.seed_label = Some("initial-sketch".to_string());

        let generated = pipeline
            .generate(&req)
            .await
            .expect("pack generation succeeds");
        assert_eq!(generated.manifest.sheets.len(), expected_sheets);
        assert_eq!(generated.manifest.lifecycle, Lifecycle::Draft);
        assert!(generated.manifest.seed.is_some());
        assert!(generated.pack_manifest_path.is_file());
        assert!(generated.slices_manifest_path.is_file());

        let calls = backend.calls.lock().unwrap();
        assert_eq!(calls.len(), expected_sheets);
        assert!(calls[0].reference_images.contains(&seed_path));
        if expect_step2_turnaround_anchor {
            let expected_turnaround = generated.pack_dir.join("turnaround.png");
            assert!(calls[1].reference_images.contains(&expected_turnaround));
            assert!(calls[2].reference_images.contains(&expected_turnaround));
        }

        // Verify host-composited 48x48 PaletteLock swatches are pixel-exact on disk.
        let first_sheet_rec = &generated.manifest.sheets[0];
        let sheet_img = image::open(generated.pack_dir.join(&first_sheet_rec.filename))
            .unwrap()
            .to_rgba8();
        for swatch in &first_sheet_rec.calibration_strip.palette_swatch_cells {
            let expected = parse_hex_rgb(&swatch.color_hex).unwrap();
            let px = sheet_img
                .get_pixel(swatch.rect.x + 24, swatch.rect.y + 24)
                .0;
            assert_eq!([px[0], px[1], px[2]], expected);
        }

        // Each sheet produced 2 sliced components cleanly inside slices/.
        assert_eq!(generated.manifest.slices.len(), expected_sheets * 2);
        for slice in &generated.manifest.slices {
            assert!(generated.pack_dir.join(&slice.file).is_file());
        }
    }

    #[tokio::test]
    async fn canonical_reference_pack_locks_sha256_and_blocks_in_place_overwrite() {
        let root = TempDir::new().expect("tempdir");
        let ws = root.path();
        let library_dir = ws.join("library");
        write_subject_card(&library_dir, "brass-lantern", "prop");

        let registry = LibraryRegistry::new(Some(library_dir.clone()), None);
        let backend = MockPackBackend::new();
        let pipeline = ReferencePackPipeline::new(&backend, &registry, library_dir.clone());
        let req = ReferencePackRequest::new("brass-lantern", "franco-belgian-line");
        let generated = pipeline.generate(&req).await.unwrap();

        // Advance pack.json lifecycle: draft -> candidate -> validated
        let mut manifest: ReferencePackManifest =
            serde_json::from_slice(&fs::read(&generated.pack_manifest_path).unwrap()).unwrap();
        manifest.lifecycle = manifest
            .lifecycle
            .transition_to(Lifecycle::Candidate)
            .unwrap()
            .transition_to(Lifecycle::Validated)
            .unwrap();
        write_atomic(
            &generated.pack_manifest_path,
            &serde_json::to_vec_pretty(&manifest).unwrap(),
            "pack.json",
        )
        .unwrap();

        let lock_store = crate::store::ManifestLockStore::new(&library_dir);
        let (promoted, lock) = lock_store
            .promote_reference_pack_canonical(&generated.pack_dir, ws)
            .expect("promote ReferencePack to canonical");
        assert_eq!(promoted.lifecycle, Lifecycle::Canonical);
        assert!(
            lock.canonical
                .contains_key("pack:brass-lantern/franco-belgian-line")
        );
        lock_store.verify_canonical_checksums(ws).unwrap();

        let overwrite_err = pipeline
            .generate(&req)
            .await
            .expect_err("overwriting a canonical ReferencePack must fail closed");
        assert!(matches!(
            overwrite_err,
            CoreError::CanonicalImmutable { .. }
        ));
    }

    #[rstest]
    #[case::character_unkeyed_overrides_rejected("character", true)]
    #[case::creature_unkeyed_overrides_rejected("creature", true)]
    #[case::prop_unkeyed_overrides_allowed("prop", false)]
    #[tokio::test]
    async fn rejects_unkeyed_slicer_overrides_for_multi_sheet_packs(
        #[case] kind: &str,
        #[case] expect_err: bool,
    ) {
        let root = TempDir::new().expect("tempdir");
        let library_dir = root.path().join("library");
        write_subject_card(&library_dir, "test-subject", kind);

        let registry = LibraryRegistry::new(Some(library_dir.clone()), None);
        let backend = MockPackBackend::new();
        let pipeline = ReferencePackPipeline::new(&backend, &registry, library_dir);

        let mut req = ReferencePackRequest::new("test-subject", "franco-belgian-line");
        req.slicer_config.overrides = vec![RectOverride {
            id: "front".to_string(),
            rect: PixelRect {
                x: 180,
                y: 120,
                width: 260,
                height: 640,
            },
            label: None,
        }];

        let res = pipeline.generate(&req).await;
        assert_eq!(res.is_err(), expect_err);
        if expect_err {
            assert!(matches!(
                res.unwrap_err(),
                CoreError::ReferencePackFailed(_)
            ));
            assert!(
                backend.calls.lock().unwrap().is_empty(),
                "invalid override configuration must fail before invoking backend"
            );
        }
    }

    #[tokio::test]
    async fn character_pack_supports_role_keyed_sheet_overrides_without_collisions() {
        let root = TempDir::new().expect("tempdir");
        let library_dir = root.path().join("library");
        write_subject_card(&library_dir, "courier-ren", "character");

        let registry = LibraryRegistry::new(Some(library_dir.clone()), None);
        let backend = MockPackBackend::new();
        let pipeline = ReferencePackPipeline::new(&backend, &registry, library_dir);

        let mut req = ReferencePackRequest::new("courier-ren", "franco-belgian-line");
        let sample_rect = PixelRect {
            x: 180,
            y: 120,
            width: 260,
            height: 640,
        };
        req.sheet_overrides.insert(
            ReferenceSheetRole::Turnaround,
            vec![RectOverride {
                id: "turn_front".to_string(),
                rect: sample_rect,
                label: Some("front".to_string()),
            }],
        );
        req.sheet_overrides.insert(
            ReferenceSheetRole::PosesExpressions,
            vec![RectOverride {
                id: "pose_action".to_string(),
                rect: sample_rect,
                label: Some("action".to_string()),
            }],
        );

        let generated = pipeline.generate(&req).await.expect("keyed overrides pass");
        let ids: HashSet<&str> = generated
            .manifest
            .slices
            .iter()
            .map(|s| s.id.as_str())
            .collect();
        assert!(ids.contains("turn_front"));
        assert!(ids.contains("pose_action"));
        assert_eq!(ids.len(), generated.manifest.slices.len());
    }

    #[cfg(unix)]
    #[rstest]
    #[case::symlinked_packs_dir("packs")]
    #[case::symlinked_style_pack_dir("style_pack")]
    #[case::symlinked_slices_dir("slices")]
    #[tokio::test]
    async fn rejects_symlinked_pack_or_slices_directories_fail_closed(#[case] target: &str) {
        let root = TempDir::new().expect("tempdir");
        let library_dir = root.path().join("library");
        write_subject_card(&library_dir, "brass-lantern", "prop");

        let outside = root.path().join("outside_escape");
        fs::create_dir_all(&outside).unwrap();

        let subject_dir = library_dir.join("subjects/brass-lantern");
        let packs_dir = subject_dir.join("packs");
        let style_pack_dir = packs_dir.join("franco-belgian-line");
        let slices_dir = style_pack_dir.join("slices");

        match target {
            "packs" => {
                symlink(&outside, &packs_dir).unwrap();
            }
            "style_pack" => {
                fs::create_dir_all(&packs_dir).unwrap();
                symlink(&outside, &style_pack_dir).unwrap();
            }
            "slices" => {
                fs::create_dir_all(&style_pack_dir).unwrap();
                symlink(&outside, &slices_dir).unwrap();
            }
            _ => unreachable!(),
        }

        let registry = LibraryRegistry::new(Some(library_dir.clone()), None);
        let backend = MockPackBackend::new();
        let pipeline = ReferencePackPipeline::new(&backend, &registry, library_dir);
        let req = ReferencePackRequest::new("brass-lantern", "franco-belgian-line");

        let err = pipeline
            .generate(&req)
            .await
            .expect_err("symlinked pack directory must fail closed");
        assert!(matches!(err, CoreError::Io { .. }));
        assert!(
            backend.calls.lock().unwrap().is_empty(),
            "symlink check must run before invoking backend"
        );
    }
}
