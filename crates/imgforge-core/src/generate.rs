//! === UNIFIED SCENE GENERATION PIPELINE ===
//!
//! PURPOSE:
//! Unifies `0`-reference `explore` generation (using `Style`/`Atmosphere`/`SceneSpec`
//! without requiring a `ReferencePack`), `1`-reference generation, and multi-reference
//! generation (`<= 3` foreground focal subjects) with automatic dynamic `Atlas` packing,
//! `Style` exemplar attachment, optional `SpatialGuide` labeling, optional prose-to-`SceneSpec`
//! `SceneComposer` translation (ADR 0004), fail-closed `ColorQaEngine` verification,
//! and immutable versioned `Run` recording.
//!
//! THREAT MODEL / DESIGN RATIONALE ("WHY"):
//! 1. Applying CLI/caller `width`/`height` overrides onto a cloned `SceneSpec` BEFORE
//!    `PromptCompiler::compile` prevents contradictions between `[OUTPUT]` and
//!    `[RENDER_TARGET]` headers sent to `ImageBackend`.
//! 2. Enforcing `Visibility` compatibility across `SceneSpec`, resolved `Subject`s,
//!    `ReferencePack`s, `SpatialGuide`s, and explicit `Run` reference PNGs BEFORE
//!    invoking `ImageBackend` guarantees spoiler-sensitive assets never reach the model.
//! 3. All input artifacts (cards, `SceneSpec`, `Atlas` slices, `Style` exemplars,
//!    `SpatialGuide`, explicit `-i` references) are read with `read_regular_file_nofollow`
//!    and hashed into `RunMetadata.input_hashes` for full DAG provenance.
//!
//! EXECUTION LIFECYCLE / DATA FLOW:
//! 1. Resolve `SceneSpec` directly or via read-only `SceneComposer` (prose-only).
//! 2. Apply optional `width`/`height` overrides onto cloned `SceneSpec`.
//! 3. Resolve `Subject`s, `Style`, `Atmosphere`, `ReferencePack`s, and check `Visibility`.
//! 4. Compile deterministic positive-only prompt (`PromptCompiler`).
//! 5. Pack dynamic `Atlas` (if multi-ref / pack slices), attach `Style` exemplars and
//!    optional `SpatialGuide` with explicit `[REFERENCE_ROLES]`.
//! 6. Invoke `ImageBackend`, decode PNG safely, run fail-closed `ColorQaEngine`,
//!    and record versioned `Run` (`Lifecycle::Draft` or `Lifecycle::Candidate`).

use std::collections::{BTreeMap, HashSet};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tempfile::TempDir;

use crate::atlas::{
    AtlasPackRequest, AtlasPlacement, AtlasSliceSelection, DEFAULT_ATLAS_HEIGHT,
    DEFAULT_ATLAS_PADDING_PX, DEFAULT_ATLAS_WIDTH, pack_dynamic_atlas,
};
use crate::atmosphere::Atmosphere;
use crate::backend::{ImageBackend, ImageGenerationRequest};
use crate::color_qa::{
    ColorQaEngine, ColorQaReport, ExposureQaConfig, PaletteQaConfig, SparkleConfig, SubjectRoiSpec,
};
use crate::composer::SceneComposer;
use crate::edit::OwnedSubjectRoi;
use crate::error::CoreError;
use crate::lifecycle::{Lifecycle, sha256_hex, validate_slug};
use crate::prompt_compiler::{CompiledPrompt, PromptCompiler};
use crate::reference_pack::ReferencePackManifest;
use crate::registry::{CARD_FILENAME, LibraryRegistry, ResolvedSubject};
use crate::scene_spec::{SceneSpec, Visibility, check_visibility_compatibility};
use crate::slicer::{PixelRect, SlicesManifest};
use crate::store::{
    RecordRunParams, RecordedRun, RunBackendMetadata, RunQaReports, RunStore, decode_png_checked,
    read_regular_file_nofollow, verify_contained_path_no_symlinks, write_atomic,
};
use crate::style::Style;

/// Reference preparation strategy for unified `generate`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GenerationMode {
    /// Zero-reference exploration using `Style`/`Atmosphere`/`SceneSpec` without requiring a `ReferencePack`.
    Explore,
    /// Standard 1-reference or multi-reference (`<= 3` subjects) generation with `Atlas` + `Style` exemplars.
    #[default]
    Standard,
}

/// Source input for `execute_generate` (either structured `SceneSpec` or unstructured prose).
#[derive(Debug, Clone)]
pub enum GenerateSceneInput {
    /// Structured `SceneSpec` loaded from YAML/JSON or provided directly.
    Spec(Box<SceneSpec>),
    /// Unstructured prose translated via read-only `SceneComposer` (ADR 0004).
    Prose(String),
}

/// Subject ROI binding using a `Subject` slug resolved from `LibraryRegistry`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubjectRoiBinding {
    /// Resolved `Subject` slug whose `PaletteLock` will be verified.
    pub subject_slug: String,
    /// Pixel ROIs inside the generated image (empty evaluates the full canvas).
    #[serde(default)]
    pub rois: Vec<PixelRect>,
}

/// Request payload for `execute_generate`.
#[derive(Debug, Clone)]
pub struct GenerateRequest {
    /// Structured `SceneSpec` or prose translated by `SceneComposer`.
    pub scene_input: GenerateSceneInput,
    /// Optional target `Run` stream slug (defaults to `SceneSpec.id`).
    pub slug: Option<String>,
    /// Generation strategy (`Explore` vs `Standard`).
    pub mode: GenerationMode,
    /// Optional width override applied to cloned `SceneSpec` before `PromptCompiler::compile`.
    pub width_override: Option<u32>,
    /// Optional height override applied to cloned `SceneSpec` before `PromptCompiler::compile`.
    pub height_override: Option<u32>,
    /// Explicit `(subject_slug, style_slug, slice_ids)` selections for dynamic `Atlas` packing.
    pub atlas_selections: Vec<AtlasSliceSelection>,
    /// Explicit reference PNG paths passed to `ImageBackend`.
    pub reference_images: Vec<PathBuf>,
    /// Optional `SpatialGuide` PNG path attached with explicit role labeling.
    pub spatial_guide_path: Option<PathBuf>,
    /// Optional `Subject` ROI bindings for `PaletteLock` / forbidden-hue QA.
    pub subject_rois: Vec<SubjectRoiBinding>,
    /// Optional inline `OwnedSubjectRoi` overrides (for programmatic callers).
    pub explicit_subject_rois: Vec<OwnedSubjectRoi>,
    /// Optional exposure QA config.
    pub exposure_config: ExposureQaConfig,
    /// Optional `PaletteLock` QA config (supports forbidden-hue bleed sectors).
    pub palette_config: PaletteQaConfig,
    /// Optional allowed-mask for yellow-sparkle DoG detector.
    pub allowed_sparkle_mask: Option<Vec<f32>>,
    /// Sparkle detector configuration.
    pub sparkle_config: SparkleConfig,
    /// Fail closed (`Err(CoreError::ColorQaFailed)`) when Color QA fails.
    pub fail_closed_on_qa: bool,
    /// Target initial `Lifecycle` when QA passes (`Lifecycle::Draft` or `Lifecycle::Candidate`).
    pub initial_lifecycle: Lifecycle,
    /// Optional extra copy path for generated PNG (for CLI `--out`).
    pub extra_out_path: Option<PathBuf>,
}

impl GenerateRequest {
    /// Construct a default `Standard` generation request from a `SceneSpec`.
    #[must_use]
    pub fn from_spec(spec: SceneSpec) -> Self {
        Self {
            scene_input: GenerateSceneInput::Spec(Box::new(spec)),
            slug: None,
            mode: GenerationMode::Standard,
            width_override: None,
            height_override: None,
            atlas_selections: Vec::new(),
            reference_images: Vec::new(),
            spatial_guide_path: None,
            subject_rois: Vec::new(),
            explicit_subject_rois: Vec::new(),
            exposure_config: ExposureQaConfig::default(),
            palette_config: PaletteQaConfig::default(),
            allowed_sparkle_mask: None,
            sparkle_config: SparkleConfig::default(),
            fail_closed_on_qa: true,
            initial_lifecycle: Lifecycle::Candidate,
            extra_out_path: None,
        }
    }
}

/// Structured output returned by `execute_generate`.
#[derive(Debug, Clone, Serialize)]
pub struct GenerateResult {
    /// Recorded `Run` metadata and filesystem paths.
    pub run_id: String,
    /// Monotonic `Run` version number (`1`, `2`, ...).
    pub version: u32,
    /// Recorded `Lifecycle` (`draft` or `candidate`).
    pub lifecycle: Lifecycle,
    /// Path to `runs/<slug>/vNNN.png`.
    pub image_path: PathBuf,
    /// Path to `runs/<slug>/vNNN.meta.json`.
    pub meta_path: PathBuf,
    /// Effective `SceneSpec` after optional `SceneComposer` and dimension overrides.
    pub effective_scene_spec: SceneSpec,
    /// Exact positive-only compiled prompt passed to `ImageBackend`.
    pub compiled_prompt: String,
    /// Placements from dynamically packed `Atlas` page(s), if any.
    pub atlas_placements: Vec<AtlasPlacement>,
    /// Number of reference images attached to `ImageBackend`.
    pub reference_count: usize,
    /// Fail-closed `ColorQaReport`.
    pub qa_report: ColorQaReport,
    /// Recorded `RecordedRun` (skipped from JSON serialization to avoid duplicate nesting).
    #[serde(skip_serializing)]
    pub recorded_run: RecordedRun,
}

#[derive(Debug, Clone)]
struct ReferenceAttachment {
    path: PathBuf,
    role_description: String,
}

/// Execute unified `generate` (`0`-ref `explore`, `1`-ref, or multi-ref auto-`Atlas`)
/// through `ImageBackend` with fail-closed `Visibility` and `ColorQaEngine` gates.
#[allow(clippy::too_many_arguments)]
pub async fn execute_generate(
    backend: &dyn ImageBackend,
    composer: Option<&SceneComposer>,
    registry: &LibraryRegistry,
    run_store: &RunStore,
    request: &GenerateRequest,
) -> Result<GenerateResult, CoreError> {
    if !matches!(
        request.initial_lifecycle,
        Lifecycle::Draft | Lifecycle::Candidate
    ) {
        return Err(CoreError::LifecycleTransitionInvalid {
            from: Lifecycle::Draft.as_str().to_string(),
            to: request.initial_lifecycle.as_str().to_string(),
            reason: "generate may only record Runs as 'draft' or 'candidate'".to_string(),
        });
    }

    let mut input_hashes = BTreeMap::new();
    let mut effective_spec = match &request.scene_input {
        GenerateSceneInput::Spec(spec) => {
            let spec_json = serde_json::to_vec(spec.as_ref())
                .map_err(|err| CoreError::SceneSpecInvalid(err.to_string()))?;
            input_hashes.insert("scene_spec".to_string(), sha256_hex(&spec_json));
            (**spec).clone()
        }
        GenerateSceneInput::Prose(prose) => {
            let comp = composer.ok_or_else(|| {
                CoreError::InvalidGenerationRequest(
                    "SceneComposer is required when generating from unstructured prose".to_string(),
                )
            })?;
            input_hashes.insert("compose_prose".to_string(), sha256_hex(prose.as_bytes()));
            let composed = comp.compose(prose).await?;
            let spec_json = serde_json::to_vec(&composed.spec)
                .map_err(|err| CoreError::SceneSpecInvalid(err.to_string()))?;
            input_hashes.insert("composed_scene_spec".to_string(), sha256_hex(&spec_json));
            composed.spec
        }
    };

    // Apply width/height overrides onto cloned SceneSpec BEFORE PromptCompiler::compile.
    if let Some(w) = request.width_override {
        if w == 0 {
            return Err(CoreError::InvalidGenerationRequest(
                "width_override must be greater than 0".to_string(),
            ));
        }
        effective_spec.width = Some(w);
    }
    if let Some(h) = request.height_override {
        if h == 0 {
            return Err(CoreError::InvalidGenerationRequest(
                "height_override must be greater than 0".to_string(),
            ));
        }
        effective_spec.height = Some(h);
    }

    let run_slug = request
        .slug
        .clone()
        .unwrap_or_else(|| effective_spec.id.clone());
    validate_slug(&run_slug, "Run").map_err(CoreError::InvalidGenerationRequest)?;

    // Resolve Subject cards from LibraryRegistry and gather Visibility overrides before compilation.
    let mut resolved_subjects: BTreeMap<String, ResolvedSubject> = BTreeMap::new();
    let mut subject_visibility_overrides: BTreeMap<String, Visibility> = BTreeMap::new();

    for slot in &effective_spec.foreground_slots {
        let subj_slug = slot.subject_ref.trim();
        if !resolved_subjects.contains_key(subj_slug) {
            match registry.resolve_subject(subj_slug) {
                Ok(resolved) => {
                    subject_visibility_overrides
                        .insert(subj_slug.to_string(), resolved.subject.visibility);
                    let card_bytes = read_regular_file_nofollow(
                        &resolved.dir.join(CARD_FILENAME),
                        "Subject card",
                    )?;
                    input_hashes.insert(format!("subject:{subj_slug}"), sha256_hex(&card_bytes));
                    resolved_subjects.insert(subj_slug.to_string(), resolved);
                }
                Err(CoreError::RegistryResolutionFailed { .. })
                    if request.atlas_selections.is_empty() => {}
                Err(err) => return Err(err),
            }
        }
    }

    for sel in &request.atlas_selections {
        let subj_slug = sel.subject_slug.trim();
        if !resolved_subjects.contains_key(subj_slug) {
            let resolved = registry.resolve_subject(subj_slug)?;
            subject_visibility_overrides.insert(subj_slug.to_string(), resolved.subject.visibility);
            let card_bytes =
                read_regular_file_nofollow(&resolved.dir.join(CARD_FILENAME), "Subject card")?;
            input_hashes.insert(format!("subject:{subj_slug}"), sha256_hex(&card_bytes));
            resolved_subjects.insert(subj_slug.to_string(), resolved);
        }
    }

    for binding in &request.subject_rois {
        let subj_slug = binding.subject_slug.trim();
        if !resolved_subjects.contains_key(subj_slug) {
            let resolved = registry.resolve_subject(subj_slug)?;
            subject_visibility_overrides.insert(subj_slug.to_string(), resolved.subject.visibility);
            let card_bytes =
                read_regular_file_nofollow(&resolved.dir.join(CARD_FILENAME), "Subject card")?;
            input_hashes.insert(format!("subject:{subj_slug}"), sha256_hex(&card_bytes));
            resolved_subjects.insert(subj_slug.to_string(), resolved);
        }
    }

    // Enforce Subject Visibility compatibility before PromptCompiler / ImageBackend.
    check_visibility_compatibility(
        effective_spec.visibility,
        "Generate",
        resolved_subjects
            .iter()
            .map(|(slug, res)| (slug.as_str(), res.subject.visibility)),
    )?;
    check_visibility_compatibility(
        effective_spec.visibility,
        "Generate",
        request
            .explicit_subject_rois
            .iter()
            .map(|s| (s.subject_slug.as_str(), s.visibility)),
    )?;

    // Compile deterministic positive-only prompt with updated dimensions & subject visibilities.
    let compiled: CompiledPrompt = PromptCompiler::compile_with_subject_visibility(
        &effective_spec,
        &subject_visibility_overrides,
    )?;

    // Optionally resolve Style & Atmosphere cards to enrich prompt and enforce ExposureFloor QA.
    let resolved_style = match registry.resolve_style(&effective_spec.style_ref) {
        Ok(style) => {
            if let Some(ref dir) = style.dir {
                let card_bytes =
                    read_regular_file_nofollow(&dir.join(CARD_FILENAME), "Style card")?;
                input_hashes.insert(
                    format!("style:{}", style.style.slug),
                    sha256_hex(&card_bytes),
                );
            } else {
                let yaml = style.style.to_yaml_string()?;
                input_hashes.insert(
                    format!("style:{}", style.style.slug),
                    sha256_hex(yaml.as_bytes()),
                );
            }
            Some(style)
        }
        Err(CoreError::RegistryResolutionFailed { .. }) => None,
        Err(err) => return Err(err),
    };

    let resolved_atmosphere = match registry.resolve_atmosphere(&effective_spec.atmosphere_ref) {
        Ok(atmo) => {
            if let Some(ref dir) = atmo.dir {
                let card_bytes =
                    read_regular_file_nofollow(&dir.join(CARD_FILENAME), "Atmosphere card")?;
                input_hashes.insert(
                    format!("atmosphere:{}", atmo.atmosphere.slug),
                    sha256_hex(&card_bytes),
                );
            } else {
                let yaml = atmo.atmosphere.to_yaml_string()?;
                input_hashes.insert(
                    format!("atmosphere:{}", atmo.atmosphere.slug),
                    sha256_hex(yaml.as_bytes()),
                );
            }
            Some(atmo)
        }
        Err(CoreError::RegistryResolutionFailed { .. }) => None,
        Err(err) => return Err(err),
    };

    // Prepare temporary staging directory for materialized exemplars and packed Atlas PNGs.
    let staging_dir = TempDir::new().map_err(|err| CoreError::Io {
        context: "creating Generate temporary reference staging directory".to_string(),
        source: err,
    })?;
    let mut attachments: Vec<ReferenceAttachment> = Vec::new();
    let mut atlas_placements: Vec<AtlasPlacement> = Vec::new();

    if request.mode == GenerationMode::Standard {
        // 1. Dynamic Atlas packing from explicit selections or discovered ReferencePacks.
        let effective_selections = if !request.atlas_selections.is_empty() {
            request.atlas_selections.clone()
        } else if !effective_spec.foreground_slots.is_empty()
            && let Some(library_dir) = registry.project_library_dir()
        {
            discover_foreground_pack_selections(
                library_dir,
                &effective_spec,
                &resolved_subjects,
                !request.reference_images.is_empty(),
            )?
        } else {
            Vec::new()
        };

        if !effective_selections.is_empty() {
            let library_dir = registry.project_library_dir().ok_or_else(|| {
                CoreError::AtlasPackFailed(
                    "dynamic Atlas packing requires a project library directory".to_string(),
                )
            })?;

            // Enforce ReferencePack Visibility and record pack.json SHA-256 before packing.
            for sel in &effective_selections {
                let rel_pack = Path::new("subjects")
                    .join(&sel.subject_slug)
                    .join("packs")
                    .join(&sel.style_slug);
                if let Ok(pack_dir) = verify_contained_path_no_symlinks(
                    library_dir,
                    &rel_pack,
                    true,
                    "ReferencePack directory",
                ) {
                    let pack_json_path = pack_dir.join("pack.json");
                    if pack_json_path.exists() {
                        let verified_pack_json = verify_contained_path_no_symlinks(
                            &pack_dir,
                            Path::new("pack.json"),
                            false,
                            "pack.json",
                        )?;
                        let pack_bytes =
                            read_regular_file_nofollow(&verified_pack_json, "pack.json")?;
                        let pack_manifest: ReferencePackManifest =
                            serde_json::from_slice(&pack_bytes).map_err(|err| {
                                CoreError::ReferencePackFailed(format!(
                                    "parsing {}: {err}",
                                    verified_pack_json.display()
                                ))
                            })?;
                        check_visibility_compatibility(
                            effective_spec.visibility,
                            "Generate ReferencePack",
                            [(
                                format!("{}/{}", sel.subject_slug, sel.style_slug).as_str(),
                                pack_manifest.visibility,
                            )],
                        )?;
                        input_hashes.insert(
                            format!("pack:{}/{}", sel.subject_slug, sel.style_slug),
                            sha256_hex(&pack_bytes),
                        );
                    }
                }
            }

            let atlas_req = AtlasPackRequest {
                selections: effective_selections,
                target_visibility: effective_spec.visibility,
                canvas_width: DEFAULT_ATLAS_WIDTH,
                canvas_height: DEFAULT_ATLAS_HEIGHT,
                padding_px: DEFAULT_ATLAS_PADDING_PX,
            };
            let atlas = pack_dynamic_atlas(registry, library_dir, &atlas_req)?;
            atlas_placements = atlas.placements;

            let atlas_dir = staging_dir.path().join("atlas");
            fs::create_dir_all(&atlas_dir).map_err(|err| CoreError::Io {
                context: format!("creating Atlas staging directory {}", atlas_dir.display()),
                source: err,
            })?;
            for page in &atlas.pages {
                let page_path = atlas_dir.join(format!("atlas_p{}.png", page.page_index + 1));
                write_atomic(&page_path, &page.png_bytes, "Atlas page PNG")?;
                input_hashes.insert(
                    format!("atlas_page_{}", page.page_index + 1),
                    sha256_hex(&page.png_bytes),
                );
                let slice_names: Vec<&str> = page
                    .placements
                    .iter()
                    .map(|p| p.slice_id.as_str())
                    .collect();
                attachments.push(ReferenceAttachment {
                    path: page_path,
                    role_description: format!(
                        "Dynamic Atlas page {} containing packed ReferencePack slices [{}]",
                        page.page_index + 1,
                        slice_names.join(", ")
                    ),
                });
            }
        }

        // 2. Attach Style exemplars when resolved.
        if let Some(ref style) = resolved_style {
            let exemplar_cache = staging_dir.path().join("exemplars");
            let exemplar_paths = style.materialize_exemplars(&exemplar_cache)?;
            for (idx, ex_path) in exemplar_paths.into_iter().enumerate() {
                let ex_bytes = read_regular_file_nofollow(&ex_path, "Style exemplar PNG")?;
                input_hashes.insert(format!("style_exemplar_{}", idx + 1), sha256_hex(&ex_bytes));
                attachments.push(ReferenceAttachment {
                    path: ex_path,
                    role_description: format!(
                        "Style exemplar {} for '{}' rendering medium and linework",
                        idx + 1,
                        style.style.slug
                    ),
                });
            }
        }
    }

    // 3. Validate & attach explicit reference images (checked against RunStore Visibility).
    for (idx, ref_path) in request.reference_images.iter().enumerate() {
        let bytes = read_regular_file_nofollow(ref_path, "reference PNG")?;
        let _ = decode_png_checked(ref_path, &bytes)?;
        if let Some(run_vis) = run_store.inspect_run_image_visibility(ref_path)? {
            check_visibility_compatibility(
                effective_spec.visibility,
                "Generate Reference",
                [(ref_path.display().to_string().as_str(), run_vis)],
            )?;
        }
        input_hashes.insert(format!("ref_{idx}"), sha256_hex(&bytes));
        attachments.push(ReferenceAttachment {
            path: ref_path.clone(),
            role_description: format!("Explicit reference anchor {}", idx + 1),
        });
    }

    // 4. Validate & attach optional SpatialGuide with explicit role labeling.
    if let Some(ref guide_path) = request.spatial_guide_path {
        let guide_bytes = read_regular_file_nofollow(guide_path, "SpatialGuide PNG")?;
        let _ = decode_png_checked(guide_path, &guide_bytes)?;
        if let Some(run_vis) = run_store.inspect_run_image_visibility(guide_path)? {
            check_visibility_compatibility(
                effective_spec.visibility,
                "Generate SpatialGuide",
                [(guide_path.display().to_string().as_str(), run_vis)],
            )?;
        }
        input_hashes.insert("spatial_guide".to_string(), sha256_hex(&guide_bytes));
        attachments.push(ReferenceAttachment {
            path: guide_path.clone(),
            role_description: "SpatialGuide layout reference (match depth zones, subject scale ratios, and action trajectories)".to_string(),
        });
    }

    // Build final backend prompt including resolved Style/Atmosphere details and reference roles.
    let final_prompt = build_enriched_generate_prompt(
        &compiled.prompt,
        resolved_style.as_ref().map(|s| &s.style),
        resolved_atmosphere.as_ref().map(|a| &a.atmosphere),
        request.mode,
        &attachments,
    );

    let reference_paths: Vec<PathBuf> = attachments.iter().map(|a| a.path.clone()).collect();
    let reference_count = reference_paths.len();

    let output = backend
        .generate(&ImageGenerationRequest {
            prompt: final_prompt.clone(),
            width: compiled.target_width,
            height: compiled.target_height,
            reference_images: reference_paths,
        })
        .await?;

    let generated_img =
        decode_png_checked(Path::new("generated_output.png"), &output.png_bytes)?.to_rgba8();

    // Run fail-closed ColorQaEngine checks (Atmosphere.exposure_floor, PaletteLock, sparkles).
    let mut owned_rois = request.explicit_subject_rois.clone();
    for binding in &request.subject_rois {
        let subj_slug = binding.subject_slug.trim();
        let resolved = resolved_subjects.get(subj_slug).ok_or_else(|| {
            CoreError::RegistryResolutionFailed {
                kind: "Subject",
                slug: subj_slug.to_string(),
                reason: "Subject not resolved for PaletteLock QA".to_string(),
            }
        })?;
        owned_rois.push(OwnedSubjectRoi {
            subject_slug: resolved.subject.slug.clone(),
            visibility: resolved.subject.visibility,
            palette_lock: resolved.subject.palette_lock.clone(),
            rois: binding.rois.clone(),
        });
    }

    let subject_specs: Vec<SubjectRoiSpec<'_>> = owned_rois
        .iter()
        .map(|s| SubjectRoiSpec {
            subject_slug: &s.subject_slug,
            palette_lock: &s.palette_lock,
            rois: s.rois.clone(),
        })
        .collect();

    let exposure_arg = resolved_atmosphere
        .as_ref()
        .map(|a| (a.atmosphere.exposure_floor, &request.exposure_config));

    let qa_engine = ColorQaEngine::new();
    let qa_report = qa_engine.evaluate_whole_image(
        &generated_img,
        exposure_arg,
        &subject_specs,
        &request.palette_config,
        None,
        Some((
            request.allowed_sparkle_mask.as_deref(),
            &request.sparkle_config,
        )),
    )?;

    let mut recorded_run = run_store.record_run(RecordRunParams {
        slug: run_slug.clone(),
        visibility: effective_spec.visibility,
        png_bytes: output.png_bytes.clone(),
        input_hashes,
        compiled_prompt: final_prompt.clone(),
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

    if qa_report.passed && request.initial_lifecycle == Lifecycle::Candidate {
        let updated_meta = run_store.transition_run_lifecycle(
            &run_slug,
            recorded_run.metadata.version,
            Lifecycle::Candidate,
        )?;
        recorded_run.metadata = updated_meta;
    }

    if request.fail_closed_on_qa && !qa_report.passed {
        return Err(CoreError::ColorQaFailed(format!(
            "generate '{}' failed Color QA (recorded as Draft '{}'): {}",
            run_slug,
            recorded_run.metadata.run_id,
            qa_report.notes.join("; ")
        )));
    }

    if let Some(ref extra_out) = request.extra_out_path {
        write_atomic(extra_out, &output.png_bytes, "generated PNG")?;
    }

    Ok(GenerateResult {
        run_id: recorded_run.metadata.run_id.clone(),
        version: recorded_run.metadata.version,
        lifecycle: recorded_run.metadata.lifecycle,
        image_path: recorded_run.image_path.clone(),
        meta_path: recorded_run.meta_path.clone(),
        effective_scene_spec: effective_spec,
        compiled_prompt: final_prompt,
        atlas_placements,
        reference_count,
        qa_report,
        recorded_run,
    })
}

fn discover_foreground_pack_selections(
    library_dir: &Path,
    spec: &SceneSpec,
    resolved_subjects: &BTreeMap<String, ResolvedSubject>,
    has_explicit_refs: bool,
) -> Result<Vec<AtlasSliceSelection>, CoreError> {
    let mut selections = Vec::new();
    let mut seen = HashSet::new();

    for slot in &spec.foreground_slots {
        let subj_slug = slot.subject_ref.trim();
        if !seen.insert(subj_slug.to_string()) {
            continue;
        }
        let rel_pack = Path::new("subjects")
            .join(subj_slug)
            .join("packs")
            .join(spec.style_ref.trim());
        let slices_rel = rel_pack.join("slices.json");

        let pack_exists = match verify_contained_path_no_symlinks(
            library_dir,
            &slices_rel,
            false,
            "ReferencePack slices.json",
        ) {
            Ok(slices_path) => {
                let bytes = read_regular_file_nofollow(&slices_path, "ReferencePack slices.json")?;
                let manifest: SlicesManifest = serde_json::from_slice(&bytes).map_err(|err| {
                    CoreError::AtlasPackFailed(format!(
                        "invalid slices.json at {}: {err}",
                        slices_path.display()
                    ))
                })?;
                let slice_ids: Vec<String> = manifest.slices.iter().map(|s| s.id.clone()).collect();
                if slice_ids.is_empty() {
                    return Err(CoreError::AtlasPackFailed(format!(
                        "ReferencePack ({}, {}) slices.json contains 0 slices",
                        subj_slug, spec.style_ref
                    )));
                }
                selections.push(AtlasSliceSelection {
                    subject_slug: subj_slug.to_string(),
                    style_slug: spec.style_ref.trim().to_string(),
                    slice_ids,
                });
                true
            }
            Err(CoreError::Io { source, .. }) if source.kind() == std::io::ErrorKind::NotFound => {
                false
            }
            Err(err) => return Err(err),
        };

        if !pack_exists && !has_explicit_refs && resolved_subjects.contains_key(subj_slug) {
            return Err(CoreError::ReferencePackFailed(format!(
                "missing style-scoped ReferencePack for ({}, {}); run 'imgforge pack generate --subject {} --style {}' or use '--explore'",
                subj_slug,
                spec.style_ref.trim(),
                subj_slug,
                spec.style_ref.trim()
            )));
        }
    }

    Ok(selections)
}

fn build_enriched_generate_prompt(
    compiled_prompt: &str,
    style: Option<&Style>,
    atmosphere: Option<&Atmosphere>,
    mode: GenerationMode,
    attachments: &[ReferenceAttachment],
) -> String {
    let mut out = String::with_capacity(compiled_prompt.len() + 512);
    out.push_str(compiled_prompt.trim_end());

    let mode_label = match mode {
        GenerationMode::Explore => "explore_0_ref",
        GenerationMode::Standard if attachments.len() <= 1 => "single_ref",
        GenerationMode::Standard => "multi_ref_atlas",
    };
    let _ = write!(out, "\n\n[GENERATION_MODE]\nmode: {mode_label}");

    if let Some(st) = style {
        let _ = write!(
            out,
            "\n\n[STYLE_DETAILS]\nmedium: {}\nlinework: {}\nshading_and_texture: {}\npalette_profile: {}\nframing_and_border: {}\nguardrails: {}",
            st.medium,
            st.linework,
            st.shading_and_texture,
            st.palette_profile,
            st.framing_and_border,
            st.style_guardrails.join("; ")
        );
    }

    if let Some(atmo) = atmosphere {
        let _ = write!(
            out,
            "\n\n[ATMOSPHERE_DETAILS]\nkey_light_direction: {}\nsecondary_light_direction: {}\ncolor_temperature_k: {}K\nmax_ambient_tint_blend: {:.2}\nweather_and_particles: {}",
            atmo.key_light_direction,
            atmo.secondary_light_direction,
            atmo.color_temperature_k,
            atmo.max_ambient_tint_blend,
            atmo.weather_and_particles
        );
    }

    if !attachments.is_empty() {
        out.push_str("\n\n[REFERENCE_ROLES]");
        for (idx, att) in attachments.iter().enumerate() {
            let name = att
                .path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("reference.png");
            let _ = write!(
                out,
                "\n- ref_{} ({name}): {}",
                idx + 1,
                att.role_description
            );
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use image::{DynamicImage, ImageBuffer, ImageFormat, Rgba, RgbaImage};
    use rstest::rstest;
    use tempfile::TempDir;

    use super::*;
    use crate::backend::ImageGenerationOutput;
    use crate::calibration::STUDIO_BG_RGBA;
    use crate::reference_pack::{ReferencePackPipeline, ReferencePackRequest};
    use crate::scene_spec::{
        CameraSpec, DepthBandSpec, DepthPlane, ExclusiveSurfaceMap, ForegroundSlot, HandBinding,
        SurfaceBinding,
    };

    #[derive(Clone)]
    struct StubGenerateBackend {
        fill: [u8; 4],
        calls: Arc<Mutex<Vec<ImageGenerationRequest>>>,
    }

    impl StubGenerateBackend {
        fn new(fill: [u8; 4]) -> Self {
            Self {
                fill,
                calls: Arc::new(Mutex::new(Vec::new())),
            }
        }
    }

    #[async_trait]
    impl ImageBackend for StubGenerateBackend {
        fn name(&self) -> &'static str {
            "stub-generate-backend"
        }

        async fn generate(
            &self,
            request: &ImageGenerationRequest,
        ) -> Result<ImageGenerationOutput, CoreError> {
            self.calls.lock().unwrap().push(request.clone());
            let mut img: RgbaImage = ImageBuffer::from_pixel(
                request.width,
                request.height,
                Rgba([STUDIO_BG_RGBA[0], STUDIO_BG_RGBA[1], STUDIO_BG_RGBA[2], 255]),
            );
            // If requested as studio sheet (1536x1024), paint foreground boxes for pack tests;
            // otherwise fill solid color for scene generation QA tests.
            if request.width == 1536
                && request.height == 1024
                && request.prompt.contains("[REFERENCE_PACK_SHEET]")
            {
                for (start_x, color) in [(180u32, [210u8, 160, 120]), (820u32, [130u8, 170, 210])] {
                    for y in 120..760 {
                        for x in start_x..(start_x + 260) {
                            img.put_pixel(x, y, Rgba([color[0], color[1], color[2], 255]));
                        }
                    }
                }
            } else {
                img = ImageBuffer::from_pixel(request.width, request.height, Rgba(self.fill));
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
                thread_id: Some("thr_gen_stub".to_string()),
            })
        }
    }

    fn write_subject(library_dir: &Path, slug: &str, visibility: &str, hex: &str) {
        let dir = library_dir.join("subjects").join(slug);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("card.yaml"),
            format!(
                r##"schema_version: 1
kind: character
slug: {slug}
display_name: Subject {slug}
height_m: 1.75
exclusive_surface_map:
  bindings:
    - slot: coat
      material: wool
      color_hex: "{hex}"
palette_lock:
  swatches:
    - name: coat
      color_hex: "{hex}"
      surface_slot: coat
visibility: {visibility}
"##
            ),
        )
        .unwrap();
    }

    fn sample_scene(id: &str, visibility: Visibility, slots: Vec<ForegroundSlot>) -> SceneSpec {
        SceneSpec {
            schema_version: 1,
            id: id.to_string(),
            title: "Harbor Arrival".to_string(),
            visibility,
            camera: CameraSpec {
                shot: "medium-wide 35mm".to_string(),
                angle: Some("eye-level".to_string()),
                framing: None,
            },
            depth_bands: vec![DepthBandSpec {
                plane: DepthPlane::Foreground,
                near_meters: 1.0,
                far_meters: 5.0,
                description: "wet stone pier".to_string(),
            }],
            foreground_slots: slots,
            style_ref: "franco-belgian-line".to_string(),
            atmosphere_ref: "golden-dawn".to_string(),
            action_summary: "Travelers meet on a misty stone pier".to_string(),
            width: Some(512),
            height: Some(512),
        }
    }

    fn sample_slot(x: f64, subject_ref: &str, visibility: Visibility) -> ForegroundSlot {
        ForegroundSlot {
            position_x_ratio: x,
            subject_ref: subject_ref.to_string(),
            visibility,
            height_meters: 1.75,
            pose: "standing facing three-quarters right".to_string(),
            left_hand: HandBinding {
                item: Some("brass lantern".to_string()),
                action: "holding".to_string(),
            },
            right_hand: HandBinding {
                item: None,
                action: "resting at side".to_string(),
            },
            exclusive_surface_map: ExclusiveSurfaceMap {
                bindings: vec![SurfaceBinding {
                    slot: "coat".to_string(),
                    material: "wool".to_string(),
                    color_hex: "#969BA0".to_string(),
                    finish: None,
                }],
            },
        }
    }

    #[tokio::test]
    async fn generate_explore_0_ref_applies_dimension_override_before_compile_and_records_candidate()
     {
        let root = TempDir::new().unwrap();
        let library_dir = root.path().join("library");
        fs::create_dir_all(&library_dir).unwrap();
        let registry = LibraryRegistry::new(Some(library_dir), None);
        let run_store = RunStore::new(root.path().join("runs"));
        let backend = StubGenerateBackend::new([150, 155, 160, 255]);

        let spec = sample_scene("explore-harbor", Visibility::Public, Vec::new());
        let mut req = GenerateRequest::from_spec(spec);
        req.mode = GenerationMode::Explore;
        req.width_override = Some(640);
        req.height_override = Some(480);

        let res = execute_generate(&backend, None, &registry, &run_store, &req)
            .await
            .expect("0-ref explore generation succeeds");

        assert_eq!(res.lifecycle, Lifecycle::Candidate);
        assert_eq!(res.reference_count, 0);
        assert!(res.compiled_prompt.contains("dimensions: 640x480"));
        assert!(res.compiled_prompt.contains("aspect_ratio: 4:3"));
        assert!(res.compiled_prompt.contains("mode: explore_0_ref"));
        assert!(res.compiled_prompt.contains("[STYLE_DETAILS]"));
        assert!(res.compiled_prompt.contains("[ATMOSPHERE_DETAILS]"));

        let calls = backend.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!((calls[0].width, calls[0].height), (640, 480));
        assert!(calls[0].reference_images.is_empty());
        assert!(
            res.recorded_run
                .metadata
                .input_hashes
                .contains_key("scene_spec")
        );
        assert!(
            res.recorded_run
                .metadata
                .input_hashes
                .contains_key("style:franco-belgian-line")
        );
        assert!(
            res.recorded_run
                .metadata
                .input_hashes
                .contains_key("atmosphere:golden-dawn")
        );
    }

    #[tokio::test]
    async fn generate_multi_ref_packs_dynamic_atlas_and_labels_spatial_guide() {
        let root = TempDir::new().unwrap();
        let library_dir = root.path().join("library");
        write_subject(&library_dir, "courier-ren", "public", "#969BA0");
        write_subject(&library_dir, "archivist-mira", "public", "#969BA0");

        let registry = LibraryRegistry::new(Some(library_dir.clone()), None);
        let backend = StubGenerateBackend::new([150, 155, 160, 255]);
        let pack_pipeline = ReferencePackPipeline::new(&backend, &registry, library_dir.clone());
        pack_pipeline
            .generate(&ReferencePackRequest::new(
                "courier-ren",
                "franco-belgian-line",
            ))
            .await
            .unwrap();
        pack_pipeline
            .generate(&ReferencePackRequest::new(
                "archivist-mira",
                "franco-belgian-line",
            ))
            .await
            .unwrap();

        let guide_path = root.path().join("blockout.png");
        let guide_img: RgbaImage = ImageBuffer::from_pixel(128, 128, Rgba([60, 80, 100, 255]));
        DynamicImage::ImageRgba8(guide_img)
            .save_with_format(&guide_path, ImageFormat::Png)
            .unwrap();

        let run_store = RunStore::new(root.path().join("runs"));
        let scene = sample_scene(
            "harbor-duo",
            Visibility::Public,
            vec![
                sample_slot(0.3, "courier-ren", Visibility::Public),
                sample_slot(0.7, "archivist-mira", Visibility::Public),
            ],
        );
        let mut req = GenerateRequest::from_spec(scene);
        req.spatial_guide_path = Some(guide_path.clone());
        req.subject_rois = vec![SubjectRoiBinding {
            subject_slug: "courier-ren".to_string(),
            rois: vec![PixelRect {
                x: 8,
                y: 8,
                width: 64,
                height: 64,
            }],
        }];

        backend.calls.lock().unwrap().clear();
        let res = execute_generate(&backend, None, &registry, &run_store, &req)
            .await
            .expect("multi-ref auto-Atlas + SpatialGuide succeeds");

        assert_eq!(res.atlas_placements.len(), 12);
        // 3 Atlas pages (12 260x640 slices across 1536x1024 pages) + 1 Style exemplar + 1 SpatialGuide = 5 references
        assert_eq!(res.reference_count, 5);
        assert!(res.compiled_prompt.contains("[REFERENCE_ROLES]"));
        assert!(res.compiled_prompt.contains("Dynamic Atlas page 1"));
        assert!(res.compiled_prompt.contains("Style exemplar 1"));
        assert!(
            res.compiled_prompt
                .contains("SpatialGuide layout reference")
        );
        assert!(
            res.recorded_run
                .metadata
                .input_hashes
                .contains_key("spatial_guide")
        );
        assert!(
            res.recorded_run
                .metadata
                .input_hashes
                .contains_key("atlas_page_1")
        );
    }

    #[rstest]
    #[case::public_scene_blocks_restricted_subject("restricted", Visibility::Public)]
    #[case::public_scene_blocks_internal_subject("internal", Visibility::Public)]
    #[case::restricted_scene_blocks_internal_subject("internal", Visibility::Restricted)]
    #[tokio::test]
    async fn generate_enforces_visibility_before_invoking_backend(
        #[case] subj_vis: &str,
        #[case] scene_vis: Visibility,
    ) {
        let root = TempDir::new().unwrap();
        let library_dir = root.path().join("library");
        write_subject(&library_dir, "secret-npc", subj_vis, "#969BA0");
        let registry = LibraryRegistry::new(Some(library_dir), None);
        let run_store = RunStore::new(root.path().join("runs"));
        let backend = StubGenerateBackend::new([150, 155, 160, 255]);

        let scene = sample_scene(
            "leak-check",
            scene_vis,
            vec![sample_slot(0.5, "secret-npc", Visibility::Public)],
        );
        let mut req = GenerateRequest::from_spec(scene);
        req.mode = GenerationMode::Explore;

        let err = execute_generate(&backend, None, &registry, &run_store, &req)
            .await
            .expect_err("visibility violation must fail closed");
        assert!(matches!(
            err,
            CoreError::VisibilityViolation { .. } | CoreError::PromptCompilationFailed(_)
        ));
        assert!(backend.calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn generate_fails_closed_on_shadow_crush_exposure_floor_while_recording_draft() {
        let root = TempDir::new().unwrap();
        let library_dir = root.path().join("library");
        fs::create_dir_all(&library_dir).unwrap();
        let registry = LibraryRegistry::new(Some(library_dir), None);
        let run_store = RunStore::new(root.path().join("runs"));
        let backend = StubGenerateBackend::new([4, 4, 5, 255]);

        let scene = sample_scene("dark-crush", Visibility::Public, Vec::new());
        let mut req = GenerateRequest::from_spec(scene);
        req.mode = GenerationMode::Explore;

        let err = execute_generate(&backend, None, &registry, &run_store, &req)
            .await
            .expect_err("crushed black output must fail ColorQaEngine");
        assert!(matches!(err, CoreError::ColorQaFailed(_)));

        let runs = run_store.list_runs("dark-crush").unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].lifecycle, Lifecycle::Draft);
    }

    #[tokio::test]
    async fn generate_supports_compose_prose_via_scene_composer() {
        let root = TempDir::new().unwrap();
        let auth_dir = root.path().join("auth");
        fs::create_dir_all(&auth_dir).unwrap();
        fs::set_permissions(&auth_dir, fs::Permissions::from_mode(0o700)).unwrap();
        let auth_file = auth_dir.join("auth.json");
        fs::write(&auth_file, r#"{"token":"ok"}"#).unwrap();
        fs::set_permissions(&auth_file, fs::Permissions::from_mode(0o600)).unwrap();

        let stub_codex = root.path().join("codex-compose");
        fs::write(
            &stub_codex,
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
{"schema_version":1,"id":"composed-pier","title":"Composed Pier","visibility":"public","camera":{"shot":"medium 35mm","angle":null,"framing":null},"depth_bands":[{"plane":"foreground","near_meters":1.0,"far_meters":4.0,"description":"stone pier"}],"foreground_slots":[],"style_ref":"franco-belgian-line","atmosphere_ref":"golden-dawn","action_summary":"Empty sunlit pier at dawn","width":512,"height":512}
JSON
"##,
        )
        .unwrap();
        fs::set_permissions(&stub_codex, fs::Permissions::from_mode(0o755)).unwrap();

        let composer =
            SceneComposer::with_codex_bin(auth_dir, stub_codex.to_string_lossy().into_owned());
        let library_dir = root.path().join("library");
        fs::create_dir_all(&library_dir).unwrap();
        let registry = LibraryRegistry::new(Some(library_dir), None);
        let run_store = RunStore::new(root.path().join("runs"));
        let backend = StubGenerateBackend::new([150, 155, 160, 255]);

        let req = GenerateRequest {
            scene_input: GenerateSceneInput::Prose("Empty sunlit pier at dawn".to_string()),
            slug: None,
            mode: GenerationMode::Explore,
            width_override: Some(384),
            height_override: Some(384),
            atlas_selections: Vec::new(),
            reference_images: Vec::new(),
            spatial_guide_path: None,
            subject_rois: Vec::new(),
            explicit_subject_rois: Vec::new(),
            exposure_config: ExposureQaConfig::default(),
            palette_config: PaletteQaConfig::default(),
            allowed_sparkle_mask: None,
            sparkle_config: SparkleConfig::default(),
            fail_closed_on_qa: true,
            initial_lifecycle: Lifecycle::Candidate,
            extra_out_path: None,
        };

        let res = execute_generate(&backend, Some(&composer), &registry, &run_store, &req)
            .await
            .expect("compose + generate succeeds");
        assert_eq!(res.effective_scene_spec.id, "composed-pier");
        assert_eq!(
            (
                res.effective_scene_spec.width,
                res.effective_scene_spec.height
            ),
            (Some(384), Some(384))
        );
        assert!(res.compiled_prompt.contains("dimensions: 384x384"));
    }
}
