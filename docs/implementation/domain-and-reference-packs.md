# Domain Model, Library Precedence, Lifecycle, and ReferencePacks

Companion implementation reference for style-neutral `Subject`s, first-class `Style`s and `Atmosphere`s, `Visibility` isolation, `project > user > builtin` precedence, `Lifecycle` + `manifest.lock.json`, style-scoped `ReferencePack` pipelines, host calibration strips, bounding-box slicing, and dynamic `Atlas` packing ([ADR 0003](../adr/0003-style-scoped-reference-packs.md), [ADR 0005](../adr/0005-host-composited-calibration-and-fail-closed-color-qa.md)).

## Domain Schema, Precedence & Chain Comparison

| Concept | Storage Location | Precedence / Scope | Key Invariants | Source |
|---|---|---|---|---|
| `Subject` | `library/subjects/<slug>/card.yaml` | `project > user` (no built-ins) | Style-neutral; `height_m > 0` required for `character`/`creature`; `PaletteLock`; `ExclusiveSurfaceMap`; `Visibility` | `crates/imgforge-core/src/subject.rs` |
| `Style` | `library/styles/<slug>/card.yaml` + `exemplars/*.png` | `project > user > builtin` (`5` presets) | `1..=3` safe relative `.png` exemplars; positive `style_guardrails` | `crates/imgforge-core/src/style.rs`, `presets.rs` |
| `Atmosphere` | `library/atmospheres/<slug>/card.yaml` | `project > user > builtin` (`4` presets) | `color_temperature_k` in `1000..=20000`; `max_ambient_tint_blend` in `0.0..=1.0`; `ExposureFloor` | `crates/imgforge-core/src/atmosphere.rs`, `presets.rs` |
| `ReferencePack` (`character`/`creature`) | `library/subjects/<slug>/packs/<style_slug>/` | Scoped per `(subject, style)` | 2-step chain: `turnaround.png` -> `poses_expressions.png` + `inventory_flatlay.png` (`1536x1024`) | `crates/imgforge-core/src/reference_pack.rs` |
| `ReferencePack` (`prop`/`location`) | `library/subjects/<slug>/packs/<style_slug>/` | Scoped per `(subject, style)` | 1-sheet template: `sheet.png` (`1536x1024`) | `crates/imgforge-core/src/reference_pack.rs` |
| `Lifecycle` & `manifest.lock.json` | `vNNN.meta.json`, `pack.json`, `library/manifest.lock.json` | Strict forward state machine | `draft -> candidate -> validated -> canonical -> superseded`; `SeedInput` is strictly input-only | `crates/imgforge-core/src/lifecycle.rs`, `store.rs`, `validation.rs` |
| Dynamic `Atlas` | Ephemeral or `pack atlas` PNG (`1536x1024`) | `1..=3` focal `Subject`s | Packs selected `slice_ids` on `#252830` with `16 px` padding; zero clipping or overlap | `crates/imgforge-core/src/atlas.rs` |

---

## 1. Domain Schemas: `Subject`, `Style`, `Atmosphere`, and `Visibility`

- **`Subject` (`crates/imgforge-core/src/subject.rs`):** Defines `kind` (`character`, `creature`, `prop`, `location`), kebab-case `slug`, `display_name`, `height_m` (mandatory positive float for `character`/`creature`), optional default `hand_bindings`, `asymmetric_markers`, `exclusive_surface_map`, `positive_weapon_geometry`, `palette_lock`, `visibility`, `tags`, and `lifecycle`. All `#RRGGBB` hex colors are normalized uppercase on load.
- **`Style` (`crates/imgforge-core/src/style.rs`, `presets.rs`):** Captures `medium`, `linework`, `shading_and_texture`, `palette_profile`, `framing_and_border`, `style_guardrails`, and `1..=3` relative `.png` paths in `exemplars`. Five built-in presets ship compiled into the binary (`franco-belgian-line`, `dark-fantasy-realistic`, `manga-ink`, `parchment-handout`, `topdown-battlemap`) with embedded `64x64` PNG exemplars.
- **`Atmosphere` (`crates/imgforge-core/src/atmosphere.rs`, `presets.rs`):** Captures `key_light_direction`, `secondary_light_direction`, `color_temperature_k` (`1000..=20000`), `max_ambient_tint_blend` (`0.0..=1.0`), `exposure_floor` (`min_p10_l_star <= min_mean_l_star` in `0.0..=100.0`), and `weather_and_particles`. Four built-in presets ship compiled in (`neutral-studio`, `golden-dawn`, `overcast-dusk`, `torchlit-crypt`).
- **`Visibility` (`crates/imgforge-core/src/scene_spec.rs`, `registry.rs`, `store.rs`):** Ordered enum `Public < Restricted < Internal`. `check_visibility_compatibility` fails closed (`CoreError::VisibilityViolation`) whenever an asset's visibility exceeds the target `SceneSpec`, `Atlas`, `Edit`, `Validate`, or `Export` context. `inspect_asset_image_visibility` inspects both `RunStore` (`vNNN.meta.json`) and `LibraryRegistry` (`card.yaml` and `pack.json`, taking the strictest classification).

## 2. `LibraryRegistry` Precedence (`crates/imgforge-core/src/registry.rs`)

`LibraryRegistry` resolves cards in strict order: `project` (`<workspace>/library`) > `user` (`$XDG_CONFIG_HOME/imgforge` if non-empty and absolute, else `$HOME/.config/imgforge`) > `builtin`.

Every container and card directory is verified with `verify_optional_contained_dir_no_symlinks`, `read_regular_file_nofollow`, and directory-to-`slug` equality. `load_and_verify_style_exemplars` walks every path segment for symlinks, enforces canonical containment inside the style directory, checks the 8-byte PNG signature, and decodes the image. Malformed higher-tier cards fail closed rather than falling back to a lower tier.

## 3. `Lifecycle`, `SeedInput`, and `manifest.lock.json` (`lifecycle.rs`, `store.rs`, `validation.rs`)

- **Five-State Progression (`crates/imgforge-core/src/lifecycle.rs`):** `Lifecycle` advances strictly along `draft -> candidate -> validated -> canonical -> superseded`. Backward, skipped, or post-`superseded` transitions fail closed (`CoreError::LifecycleTransitionInvalid`). `SeedInput` (path + SHA-256 + optional label) is strictly a bootstrap input role and never appears in `Lifecycle`.
- **Immutable Storage (`crates/imgforge-core/src/store.rs`):** `RunStore` writes `runs/<slug>/vNNN.png` and `vNNN.meta.json` atomically (`NamedTempFile` + `sync_all` + `persist`) with full `input_hashes`. `ManifestLockStore` maintains `library/manifest.lock.json`, refusing non-increasing versions or unsafe relative artifact paths before mutating disk, promoting `validated -> canonical`, transitioning prior `canonical` entries to `superseded`, and verifying all checksums via `verify_canonical_checksums`.
- **Validation Service (`crates/imgforge-core/src/validation.rs`):** `validate_run` and `validate_reference_pack` verify symlink-free containment, SHA-256 digests, dimensions, `ColorQaEngine` gates (plus pixel-exact `DeltaE_2000 <= 0.5` calibration swatches on packs), and optional/required `VisionCritic` reports before transitioning `draft -> candidate -> validated`.

## 4. Style-Scoped `ReferencePack` & `#252830` Calibration (`reference_pack.rs`, `calibration.rs`)

`ReferencePackPipeline::generate` writes under `library/subjects/<subject_slug>/packs/<style_slug>/` (refusing overwrite if the existing pack is `canonical` or `superseded`):

1. **Visibility & Seed Check:** If a `seed_path` is provided, `inspect_asset_image_visibility` enforces `seed_vis <= subject.visibility` before invoking `ImageBackend`.
2. **Kind-Specific Chain (`1536x1024`):**
   - `character` / `creature`: Step 1 generates `turnaround.png` using `[Seed?, Style exemplars...]`. Step 2 generates `poses_expressions.png` and `inventory_flatlay.png` anchored on `[turnaround.png, Style exemplars...]`.
   - `prop` / `location`: Generates single-sheet `sheet.png` using `[Seed?, Style exemplars...]`.
3. **Structural Gate & Host Calibration (`crates/imgforge-core/src/calibration.rs`):** `validate_reference_sheet_structure` enforces exact `1536x1024`, minimum `#252830` (`STUDIO_BG_RGB`) overall coverage (`>= 15%`) and 1 px perimeter coverage (`>= 50%`), and non-blank foreground (`>= 256 px`). `composite_calibration_strip_png` paints the bottom `#252830` strip (`72 px` tall when `48x48` cells fit) with a 6-step CIE `L*` gray wedge (`15, 30, 45, 60, 75, 90`) and pixel-exact `PaletteLock` swatches.

## 5. Bounding-Box Slicer & Dynamic `Atlas` (`slicer.rs`, `atlas.rs`)

- **Foreground Slicer (`crates/imgforge-core/src/slicer.rs`):** Excludes bottom calibration strip rows (`exclude_bottom_rows`), builds a foreground mask where `studio_bg_distance > 28.0`, bridges thin gaps via separable 1D-sliding-window dilation (`merge_gap_px`, default `8 px`), extracts 8-connected components using un-dilated bounds, optionally merges centroids by `grid_priors: (cols, rows)`, applies manual `RectOverride`s (`ReplaceAll` or `MergeById`), and atomically writes `slices/<id>.png` and `slices.json`. Multi-sheet `character`/`creature` packs require `sheet_overrides` keyed by `ReferenceSheetRole` to avoid slice ID collisions.
- **Dynamic `Atlas` (`crates/imgforge-core/src/atlas.rs`):** `pack_dynamic_atlas` validates `1..=3` focal subjects and `Visibility` across both `Subject` and `pack.json`, loads only requested `slice_ids` from `slices.json`, sorts height-descending, and shelf-packs onto `#252830` `1536x1024` page(s) with `16 px` padding, verifying zero clipping and zero overlap (`verify_page_placements`).

---

## ReferencePack Flow & Extension Checklist

```text
  Subject card.yaml + Style card.yaml (+ optional Seed PNG)
         │
         ▼  [visibility check + symlink/O_NOFOLLOW check]
  ReferencePackPipeline::generate
    ├── character/creature: Step 1 turnaround.png ──> Step 2 poses_expressions.png + inventory_flatlay.png
    └── prop/location:      1-sheet sheet.png
         │
         ▼  validate_reference_sheet_structure (#252830) -> host calibration strip -> bbox slicer
  pack.json (Lifecycle::Draft) + slices.json + slices/*.png
         │
         ▼  validate_reference_pack (SHA-256 + swatch DeltaE<=0.5 + sparkle QA + VisionCritic)
  Lifecycle::Validated ──> promote_reference_pack ──> Lifecycle::Canonical (manifest.lock.json)
```

**Adding a built-in `Style`/`Atmosphere` preset or a `ReferenceSheetRole`:**

1. Register any new built-in preset slug in `BUILTIN_STYLE_SLUGS` or `BUILTIN_ATMOSPHERE_SLUGS`, define its positive-only card in `builtin_style`/`builtin_atmosphere`, and add a deterministic PNG entry to `exemplar_cache` in `crates/imgforge-core/src/presets.rs`.
2. Add any new sheet variant to `ReferenceSheetRole` (`filename`, `slice_prefix`) and wire its generation step inside `ReferencePackPipeline::generate` in `crates/imgforge-core/src/reference_pack.rs`.
3. Update `validate_reference_pack` in `crates/imgforge-core/src/validation.rs` and `slicer.rs` `sheet_overrides` handling if the new role changes sheet counts or slicing rules.
4. Add parametrized unit tests covering preset validation, `LibraryRegistry` precedence, and sheet slicing/validation in `crates/imgforge-core`.
