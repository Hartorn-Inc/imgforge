# Reference

Canonical schema, state-machine, and vocabulary reference for `imgforge`. Every term here matches [`GLOSSARY.md`](https://github.com/Hartorn-Inc/imgforge/blob/main/GLOSSARY.md) 1:1.

---

## 1. Domain Entities (`Subject`, `Style`, `Atmosphere`)

### `Subject` (`library/subjects/<slug>/card.yaml`)

A style-neutral identity specification resolved strictly along `project > user` (no built-in subjects):

| Field | Type / Invariant | Purpose |
|---|---|---|
| `kind` | `character` \| `creature` \| `prop` \| `location` | Selects `ReferencePack` chain (`2-step` vs `1-sheet`) and `height_m` rules |
| `slug` | kebab-case string matching directory name | Canonical identifier used in `SceneSpec`, `pack`, and `--slice` |
| `display_name` | non-empty string | Human-readable label |
| `height_m` | positive float (`> 0.0`, required for `character`/`creature`) | Drives proportional scaling in `PromptCompiler` and `Blockout` mannequins |
| `hand_bindings` | optional default `left_hand` / `right_hand` items | Prevents left/right prop swaps across scenes |
| `asymmetric_markers` | list of left/right distinguishing features | Guides `VisionCritic` and `PromptCompiler` |
| `exclusive_surface_map` | unique `slot -> { material, hex }` (`#RRGGBB`) | Prevents color bleed between adjacent garments or surfaces |
| `positive_weapon_geometry` | positive-only silhouette/grip descriptions | Avoids negative-token drift |
| `palette_lock` | named `#RRGGBB` base albedo swatches (normalized uppercase) | Anchor swatches for `#252830` strip and OKLab/`DeltaE_2000` QA |
| `visibility` | `public` \| `restricted` \| `internal` | Prevents restricted assets from leaking into lower-visibility outputs |
| `lifecycle` | `draft` \| `candidate` \| `validated` \| `canonical` \| `superseded` | Tracks card maturity |

### `Style` (`library/styles/<slug>/card.yaml`)

Resolved along `project > user > builtin`:

- **Fields:** `slug`, `display_name`, `medium`, `linework`, `shading_and_texture`, `palette_profile`, `framing_and_border`, positive `style_guardrails`, and `exemplars` (`1..=3` workspace-contained `.png` paths verified symlink-free with 8-byte PNG signatures).
- **Built-in Presets (5):** `franco-belgian-line`, `dark-fantasy-realistic`, `manga-ink`, `parchment-handout`, `topdown-battlemap`.

### `Atmosphere` (`library/atmospheres/<slug>/card.yaml`)

Resolved along `project > user > builtin`:

- **Fields:** `slug`, `display_name`, `key_light_direction`, `secondary_light_direction`, `color_temperature_k` (`1000..=20000`), `max_ambient_tint_blend` (`0.0..=1.0`), `exposure_floor` (`min_p10_l_star <= min_mean_l_star` in `0.0..=100.0`), and `weather_and_particles`.
- **Built-in Presets (4):** `neutral-studio`, `golden-dawn`, `overcast-dusk`, `torchlit-crypt`.

---

## 2. `Visibility` Classification

`Visibility` is a strictly ordered enum protecting spoiler-sensitive or internal assets:

| Level | Rank | Allowed Sources | Typical Use |
|---|---|---|---|
| `public` | `0` (strictest output) | `public` assets only | Published web images, public VTT packs |
| `restricted` | `1` | `public` + `restricted` assets | Campaign-gated or pre-release scenes |
| `internal` | `2` (most permissive) | `public` + `restricted` + `internal` | Authoring drafts, unreleased story elements |

`check_visibility_compatibility` fails closed (`CoreError::VisibilityViolation`) whenever any referenced `Subject`, `Seed`, `ReferencePack`, `Run`, `Atlas` slice, or PNG input exceeds the target `SceneSpec`, `Atlas`, `Edit`, `Validate`, or `Export` `Visibility`.

---

## 3. `Lifecycle` & `manifest.lock.json`

`Run` (`runs/<slug>/vNNN.png` + `vNNN.meta.json`) and `ReferencePack` (`pack.json`) progress strictly forward:

```text
draft ──▶ candidate ──▶ validated ──▶ canonical ──▶ superseded
```

| State | Meaning | Allowed Transitions |
|---|---|---|
| `draft` | Newly generated or QA-rejected artifact | `-> candidate` |
| `candidate` | Structural and initial QA checks passed; awaiting full validation | `-> validated` (or reworked as new `draft`) |
| `validated` | Verified SHA-256, `ColorQaEngine` gates, and `VisionCritic` report | `-> canonical` |
| `canonical` | Immutable and frozen by SHA-256 in `library/manifest.lock.json` | `-> superseded` (automatically when a newer entry promotes) |
| `superseded` | Terminal historical canonical record | None (fail-closed) |

> **Note:** `Seed` (`SeedInput`) is strictly a bootstrap visual input for `pack generate` and never enters `Lifecycle`.

---

## 4. `Export` Profiles & `--match-grade`

`imgforge export` produces deterministic deliverables from a `Run` (`--run slug:ver`) or PNG (`--input path`) without creating `Lifecycle` versions:

| Profile | Outputs | Key Options |
|---|---|---|
| `vtt-token` | `<slug>_token.png` (optional `<slug>_token.jpg`) | `1:1` center crop, `Lanczos3` to `--token-size` (`16..=4096`, default `512`), `4x4` SS AA circular mask, `--ring-color` (`#D4A843`), optional `--jpeg-backdrop` |
| `vtt-map` | `<slug>_<cols>x<rows>.jpg` | `Lanczos3` to `(cols * grid_px) x (rows * grid_px)` (default `24x18` @ `140 px`) or `--scale-factor`, flattened over `#252830` |
| `web-webp` | `<slug>.webp` | Deterministic WebP encoding |
| `grid-2x2` | `<slug>_tl.png`, `_tr.png`, `_bl.png`, `_br.png` | Exact quadrant split of a 2x2 contact sheet |

- **Opt-in `--match-grade`:** Runs *only* when `--match-grade` is passed, supporting damped CIE `L*` CDF matching (`--grade-ref`, `--grade-strength` in `0.25..=0.45`, default `0.35`, preserving `a*`/`b*`) and Adobe `.cube` 3D LUTs (`--lut`, `LUT_3D_SIZE` `2..=65`).

---

## 5. Canonical Glossary Index

Source of truth: [`GLOSSARY.md`](https://github.com/Hartorn-Inc/imgforge/blob/main/GLOSSARY.md) on `main`. Use exact glossary terms across all code, schemas, prompts, and docs.

| Term | Canonical Definition |
|---|---|
| **`Subject`** | Recurring visual entity (`character`, `creature`, `prop`, or `location`) defined by a style-neutral `card.yaml` with proportions, `height_m`, material bindings, and `PaletteLock` swatches. |
| **`Style`** | First-class rendering definition (`card.yaml` + `1..=3` `exemplars`) specifying medium, linework, shading, texture, framing rules, and positive guardrails independently of `Subject` or `Atmosphere`. |
| **`Atmosphere`** | Lighting and environmental specification (`card.yaml`) defining light direction, Kelvin temperature, `exposure_floor`, maximum ambient tint blend, and atmospheric particles. |
| **`ReferencePack`** | Style-scoped canonical reference sheets (`turnaround`, `poses_expressions`, `inventory_flatlay` for `character`/`creature`, or single `sheet` for `prop`/`location`) and component-detected slices per `(Subject, Style)` pair. |
| **`Seed`** | Bootstrap visual input image (such as an existing portrait or `explore` `Run`) supplied to generate a `ReferencePack`. |
| **`Atlas`** | Composite `1536x1024` image dynamically shelf-packed on `#252830` from selected `ReferencePack` slices (`1..=3` focal `Subject`s) to prevent attention dilution. |
| **`PaletteLock`** | Exact unshaded base albedo hex swatches bound to a `Subject` for deterministic OKLab/`DeltaE_2000` color-drift verification. |
| **`ExclusiveSurfaceMap`** | Exhaustive assignment of material and base hex color to every visible surface or garment slot of a `Subject` so neighboring colors cannot bleed across boundaries. |
| **`SceneSpec`** | Structured, machine-validated composition specification (camera shot, depth planes, left-to-right slots, metric heights, hand bindings, `Subject` refs, `Style`, `Atmosphere`) consumed by `PromptCompiler`. |
| **`SceneComposer`** | Optional read-only LLM stage (`codex exec -s read-only --output-schema`) translating unstructured prose into a structured `SceneSpec`. |
| **`PromptCompiler`** | Deterministic Rust stage validating `SceneSpec` invariants and compiling the positive-only prompt payload sent to an `ImageBackend`. |
| **`SpatialGuide`** | Programmatic visual reference image rendered deterministically from a `GuideSpec` JSON by `GuideRenderer` to constrain layout, scale, or trajectories. |
| **`OverlayGuide`** | `SpatialGuide` variant drawing haloed vector regions and action cues directly onto a copy of a workspace-contained target image. |
| **`Blockout`** | `SpatialGuide` variant rendered from scratch on `#252830` with depth-band zones, proportionally scaled `Subject` mannequins, and action vectors. |
| **`GuideSpec`** | Shared JSON schema (`schemaVersion: "1.0"`) authored in the Web Canvas UI and consumed by `GuideRenderer`. |
| **`GuideRenderer`** | Deterministic Rust vector-to-raster engine (`tiny-skia`/`resvg`) rendering a `GuideSpec` into a `SpatialGuide` PNG. |
| **`WholeImageEdit`** | Edit strategy passing the entire target image and reference anchors to `ImageBackend` with fail-closed `ColorQaEngine` gating and zero silent re-grading. |
| **`PatchEdit`** | Localized edit strategy extracting a `1:1` crop with a `16..=32 px` collar, running `ImageBackend`, and blending via collar OKLab Reinhard transfer and 5-level Laplacian pyramids. |
| **`Run`** | Versioned execution record (`vNNN.png` + `vNNN.meta.json`) capturing input hashes, compiled prompt, backend metadata, and QA reports. |
| **`Lifecycle`** | Strict five-state asset progression (`draft -> candidate -> validated -> canonical -> superseded`) where `canonical` entries are frozen in `manifest.lock.json`. |
| **`VisionCritic`** | Structured read-only VLM inspection stage (`codex exec -s read-only --output-schema`) checking binary per-trait criteria (`identity`, `hand_props`, `weapon_geometry`, `style_adherence`, `zero_leaked_text`). |
| **`Export`** | Deterministic post-processed deliverable (`vtt-token`, `vtt-map`, `web-webp`, `grid-2x2`) derived from a `Run` or PNG without creating a `Lifecycle` version. |
| **`Visibility`** | Ordered access classification (`public`, `restricted`, `internal`) preventing sensitive assets from leaking into lower-visibility outputs. |
