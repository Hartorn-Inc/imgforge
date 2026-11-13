# Imaging, Color QA, SpatialGuides, Editing, and Export

Companion implementation reference for fail-closed OKLab/`DeltaE_2000` `ColorQaEngine`, `WholeImageEdit` and `PatchEdit` Laplacian compositing, `GuideSpec` / `GuideRenderer` (`tiny-skia` + `resvg`), and deterministic `Export` profiles with explicit `--match-grade` ([ADR 0002](../adr/0002-chatgpt-plus-oauth-via-isolated-codex-exec.md), [ADR 0005](../adr/0005-host-composited-calibration-and-fail-closed-color-qa.md)).

## Pipeline & Mode Comparison

| Pipeline / Stage | Mutation / Grading Behavior | Spatial / Color Strategy | QA / Failure Policy | Source |
|---|---|---|---|---|
| `ColorQaEngine` | Read-only QA; never mutates or re-grades whole images | CIE `L*` exposure floor, OKLab k-means (`K = 6`) + `DeltaE_2000`, `1 - M` drift, DoG sparkle (`4..=64 px^2`) | Fail-closed (`CoreError::ColorQaFailed`); emits RGBA diagnostic heatmaps | `crates/imgforge-core/src/color_qa.rs` |
| `WholeImageEdit` | Never re-grades backend output | Passes target PNG + reference anchors to `ImageBackend` | Runs `ColorQaEngine`; records `Draft` and fails closed unless `allow_qa_fail` | `crates/imgforge-core/src/edit.rs` |
| `PatchEdit` | Local collar harmonization ONLY (`[0.75, 1.35]`) | `1:1` square crop + `16..=32 px` collar + `PatchPrepMode` + 5-level Laplacian + cosine DT feather | Guarantees zero `DeltaE_2000` drift outside footprint (`1 - M`) | `crates/imgforge-core/src/edit.rs` |
| `GuideRenderer` (`OverlayGuide`) | Annotates copy of workspace-contained `targetImagePath` | `tiny-skia` haloed `OverlayRegion`s + `ActionVector`s | Symlink-free containment + `GuideSpec` `schemaVersion: "1.0"` validation | `crates/imgforge-core/src/guide_renderer.rs` |
| `GuideRenderer` (`Blockout`) | Renders from scratch on `#252830` | `DepthBand` zones + `resvg` text-free `SubjectMannequin`s + `ActionVector`s | `targetImagePath` must be `null`; byte-deterministic PNG output | `crates/imgforge-core/src/guide_renderer.rs` |
| `Export` | Opt-in `--match-grade` ONLY (`L*` CDF `0.25..=0.45` + `.cube` 3D LUT) | `vtt_token` (`4x` SS AA), `vtt_map` (`Lanczos3`), `web_webp`, `grid_2x2` | Fail-closed `Visibility` gate; creates no `Lifecycle` version | `crates/imgforge-core/src/export.rs` |

---

## 1. Fail-Closed `ColorQaEngine` (`crates/imgforge-core/src/color_qa.rs`)

`ColorQaEngine` operates strictly read-only on `&RgbaImage` inputs (`palette::Oklab`, `Lab<D65>`, `Ciede2000`):

1. **Exposure & Shadow-Crush (`evaluate_exposure`):** Samples interior opaque pixels (inset `2 px` when dimensions allow), computes `mean_l_star`, `p10_l_star`, `p90_l_star`, and `clipped_black_ratio` (`L* < 5.0`), and checks against `Atmosphere.exposure_floor`, `max_clipped_black_ratio` (`0.15`), and optional anchor tolerances (`12.0` mean, `8.0` P10).
2. **`PaletteLock` Adherence & Bleed (`evaluate_palette_adherence`):** Runs deterministic farthest-first OKLab k-means (`K = 6`, `25` iterations) inside `SubjectRoiSpec` bounding boxes (or full image when empty). Clusters meeting `min_cluster_weight` (`0.03`) are compared against `PaletteLock` swatches (`max_swatch_delta_e2000 = 22.0`, `max_lightness_shift = 28.0`, `max_chroma_shift = 28.0`), and every ROI pixel is checked against configured `ForbiddenHueSector` ranges (supporting wrap-around hues).
3. **Untouched-Region Drift (`evaluate_untouched_drift`):** Evaluates `srgb_delta_e_2000` between `before` and `after` across all pixels where `edit_mask[i] <= 1e-4` (`1 - M`) against mean, P95, per-pixel max, and exceeding fraction bounds.
4. **Sparkle / Yellow-Dot Creep (`detect_sparkles`):** Computes normalized yellow-excess `((R + G) / 2 - B).max(0)`, applies separable 2D Gaussian blurs (`sigma_small = 0.8`, `sigma_large = 2.4`), thresholds positive DoG residual (`min_dog_residual = 0.20`) combined with bright CIELAB yellow sector (CIE `L* >= 78.0`, `b* >= 55.0`, `C* >= 60.0`, hue `72..=115 deg`) outside `allowed_mask`, suppresses high-pass edge candidates belonging to large yellow regions (`> 64 px^2`), and flags 8-connected blobs sized `4..=64 px^2`, failing when `> 0.20%` (`0.002`) of non-allowed pixels or `> 256` blobs are flagged.
5. **Diagnostic Heatmaps (`render_heatmap_rgba` / `render_heatmap_png`):** Renders transparent RGBA overlays for `HeatmapKind::Drift`, `Bleed`, `Sparkle`, and `OutOfRoi`.

## 2. `WholeImageEdit` & `PatchEdit` (`crates/imgforge-core/src/edit.rs`)

Both edit paths verify `Visibility` across target image, optional mask PNG, reference anchors, and `Subject` cards before invoking `ImageBackend`.

- **`WholeImageEdit` (`execute_whole_image_edit`):** Forwards `[target_image, refs...]` to `ImageBackend`, runs `ColorQaEngine::evaluate_whole_image` without altering pixels, records a `Run`, and fails closed on QA failure unless `fail_closed_on_qa = false`.
- **`PatchEdit` (`execute_patch_edit`, `composite_calibrated_patch`):**
  1. `resolve_unified_mask` normalizes `PixelRect` ROI, `SquareRoi`, `MaskImagePath` (varied alpha channel or luminance), or `MaskValues` into `[0.0, 1.0]` weights and computes the tight `roi_rect`.
  2. `compute_square_crop_rect` centers a `1:1` square crop sized `max(roi.width, roi.height) + 2 * collar_width_px` (`collar_width_px` in `16..=32`, default `24`), clamped to canvas bounds.
  3. `prepare_patch_target` prepares the `WxW` crop using `PatchPrepMode`: `CropOnly`, `BlankInsideMask` (`#252830`), or `CueOutline` (`#F6AD55` inner boundary + `#1A1D24` halo).
  4. `ImageBackend::generate` runs at exact `WxW`.
  5. `euclidean_distance_transform` computes exact `O(W * H)` distance outside `mask > 0.5`.
  6. `apply_collar_reinhard_oklab` computes 10%-trimmed mean/std in `Oklab` over the outer collar ring (`1..=collar_width_px` where `crop_mask <= 1e-3`) and transfers tone/color onto the generated patch with per-channel gains clamped strictly to `[0.75, 1.35]`.
  7. `build_cosine_feathered_alpha` applies raised-cosine falloff over `feather_px` (`2..=collar_width_px`, default `12`) and tapers interior crop boundaries to `0.0`.
  8. `laplacian_pyramid_blend` builds 5-level (`DEFAULT_PYRAMID_LEVELS = 5`) linear-light sRGB Laplacian pyramids and Gaussian alpha pyramids (`5x5` separable kernel), blends per band, reconstructs, and writes strictly where `feathered_alpha > 0.0`.
  9. `evaluate_untouched_drift` verifies zero `DeltaE_2000` mutation outside the composite footprint (`max_delta_e2000 == 0.0`).

## 3. `GuideSpec` & `GuideRenderer` (`crates/imgforge-core/src/guide_renderer.rs`)

`GuideSpec` matches `frontend/src/types/guideSpec.ts` 1:1 (`camelCase` JSON, `schemaVersion: "1.0"`, `#[serde(deny_unknown_fields)]`) with hard caps (`MAX_GUIDE_CANVAS_DIM = 16384`, `depthBands <= 64`, `mannequins <= 128`, `actionVectors <= 256`, `overlayRegions <= 256`, `points <= 1024`).

- **`OverlayGuide`:** Requires non-empty workspace-relative `targetImagePath`, verified symlink-free inside `workspace_root` (`validate_strict_relative_path`, `verify_contained_path_no_symlinks`, `read_regular_file_nofollow`), Lanczos3-resized if needed to `canvasWidth x canvasHeight`.
- **`Blockout`:** Requires `targetImagePath: null`. Fills `#252830`, paints `depthBands` with `#1A1D24` dividers, and renders proportionally scaled `SubjectMannequin` silhouettes (`pixels_per_meter = canvasHeight * 0.22`) via `resvg` from pure text-free SVG geometry (`Left`, `Right`, `Front`, `Back`).
- Both variants finish by drawing `overlayRegions` (`Rect`, `Ellipse`, `Polyline`) and `actionVectors` (`Solid`, `Dashed`, triangular arrowheads) with high-contrast outer halos using `tiny-skia`. `build_recommended_blockout(&SceneSpec)` provides a shared scaffold for CLI, MCP, and UI.

## 4. `Export` Profiles & `--match-grade` (`crates/imgforge-core/src/export.rs`)

`execute_export` enforces `Visibility` (`source_visibility <= profile_visibility`, auto-raising `PngPath` visibility via `inspect_asset_image_visibility`) before reading or writing outputs, and writes atomically without creating `Lifecycle` versions:

- **Optional `--match-grade`:** Executed only when explicitly enabled. Supports `apply_damped_l_star_cdf_match` (`cdf_damp_strength` strictly in `0.25..=0.45`, default `0.35`, preserving `a*, b*`) and `CubeLut::parse` / `apply_to_image` (Adobe 3D `.cube` `LUT_3D_SIZE` `2..=65` with trilinear RGB interpolation).
- **`vtt_token`:** Center-crops `1:1`, Lanczos3-resizes to `size_px` (`16..=4096`, default `512`), composites a `4x4` supersampled anti-aliased circular mask with dark rims and role-colored ring (`#D4A843` default) to `<slug>_token.png`, plus optional flat-backdrop `<slug>_token.jpg`.
- **`vtt_map`:** Lanczos3-resizes to `(cols * grid_px) x (rows * grid_px)` (default `24x18` at `140 px`) or explicit `scale_factor`, flattens alpha over `#252830`, and encodes `<slug>_<cols>x<rows>.jpg`.
- **`web_webp`:** Encodes deterministic WebP `<slug>.webp`.
- **`grid_2x2`:** Splits into four quadrant PNGs (`_tl.png`, `_tr.png`, `_bl.png`, `_br.png`).

---

## `PatchEdit` & `Export` Decision Flow & Extension Checklist

```text
  Edit Needed?
    ├── Localized defect (ROI or mask) ──> PatchEdit: 1:1 crop + collar (16..=32px)
    │                                      ──> PatchPrepMode -> Backend WxW
    │                                      ──> Collar OKLab Reinhard [0.75, 1.35]
    │                                      ──> Cosine DT + 5-level Laplacian -> 0.0 drift outside
    └── Global lighting/composition    ──> WholeImageEdit -> ColorQaEngine (no silent re-grade)

  Deliverable Needed?
    └── Export (run_slug:ver or input) ──> Visibility gate -> optional --match-grade -> Profile PNG/JPG/WebP
```

**Adding a `ColorQaEngine` detector/heatmap, `GuideRenderer` cue, or `ExportProfile`:**

1. **Color QA (`crates/imgforge-core/src/color_qa.rs`):** Add new read-only detector metrics or `HeatmapKind` variants without mutating whole-image pixels, and surface failures through `ColorQaReport`.
2. **Spatial Guidance (`crates/imgforge-core/src/guide_renderer.rs`):** Update `GuideSpec` (`#[serde(deny_unknown_fields)]`) and `GuideRenderer::render` in lockstep with `frontend/src/types/guideSpec.ts` and deterministic `tiny-skia`/`resvg` vector drawing.
3. **Export (`crates/imgforge-core/src/export.rs`):** Add the `ExportProfile` variant and deterministic encoder in `execute_export` behind `check_visibility_compatibility`, keeping grading strictly gated behind explicit `--match-grade`.
4. **CLI/MCP & Tests:** Expose any new profile or flag in `crates/imgforge-cli/src/main.rs` and `mcp.rs`, then add parametrized tests verifying fail-closed invariants and byte-deterministic output.
