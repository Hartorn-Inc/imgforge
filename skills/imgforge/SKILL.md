---
name: imgforge
description: Visual consistency workflow for imgforge CLI and rmcp MCP tools (compose, generate, pack, edit, guide, export, validate, promote) and imgforge://* resources. Use when generating, editing, slicing, packing, validating, promoting, or exporting consistent multi-asset artwork, managing ReferencePacks, Atlases, SpatialGuides, ColorQaEngine checks, Lifecycle states, or Visibility rules.
---

# imgforge Workflow

`imgforge` produces style-consistent raster artwork across recurring `Subject`s, `Style`s, and `Atmosphere`s using deterministic Rust compilation, host-calibrated `ReferencePack`s, fail-closed `ColorQaEngine` verification, and immutable `Lifecycle` promotion.

When choosing exact CLI flags, MCP parameters, or resource payloads, read [`CLI-AND-MCP-REFERENCE.md`](CLI-AND-MCP-REFERENCE.md). When authoring a `SceneSpec` directly without `SceneComposer`, use the `scene-composition` skill.

## 1. Workspace, Auth, and Precedence

1. **Workspace resolution** follows strict non-fallback precedence for explicit overrides:
   `--workspace <PATH>` > non-empty `IMGFORGE_WORKSPACE` > nearest `imgforge.toml` ascending from CWD.
   An invalid `--workspace` or `IMGFORGE_WORKSPACE` fails closed without falling back to CWD.
2. **Tool-local OAuth (`imgforge auth`)** stores credentials at `$XDG_DATA_HOME/imgforge/codex/auth.json` (only when `$XDG_DATA_HOME` is non-empty and absolute; else `$HOME/.local/share/imgforge/codex/auth.json`) with `0700` directory and `0600` file permissions.
   - Verify readiness via `imgforge auth status` (or `imgforge status` inside a workspace); provision via `imgforge auth login [--device-auth]`.
   - `CodexNativeBackend`, `SceneComposer`, and `VisionCritic` never read `~/.codex/auth.json` or `OPENAI_API_KEY`; each invocation materializes a private `TempCodexHome` and removes it on completion.
3. **Library precedence** resolves `Subject`, `Style`, and `Atmosphere` cards strictly in order:
   `project` (`<workspace>/library/...`) > `user` (`$XDG_CONFIG_HOME/imgforge/...` or `$HOME/.config/imgforge/...`) > `builtin` (`Style` and `Atmosphere` only; `Subject` has no built-ins).
   Malformed higher-tier cards fail closed instead of falling back to lower tiers.

## 2. Core Operating Workflow

Pick the entry path that matches the task:

- **Direct `SceneSpec` (preferred for agents):** Author `SceneSpec` (`schema_version: 1`) directly, validate with `validate` (`target: "scene_spec"`), and pass it to `generate`.
- **Prose intent:** Convert unstructured prose once with `compose` (or `generate --compose` / `compose_prose`), inspect the returned `SceneSpec`, then iterate deterministically on the structured `SceneSpec`.

Follow this progression for consistent multi-asset production:

1. **Inspect workspace state:** Read MCP resources `imgforge://subjects`, `imgforge://styles-atmospheres`, `imgforge://manifest`, and `imgforge://runs` (or run `imgforge library list`, `imgforge run list`, and `imgforge status`).
2. **Explore or bootstrap (optional):** Use `generate` in `explore` mode (`--explore` / `explore: true`) to iterate on a `Style`, `Atmosphere`, or `SceneSpec` with `0` references before committing to a `ReferencePack`. An approved `explore` `Run` PNG can serve as a `Seed` input.
3. **Build a style-scoped `ReferencePack`:** Run `pack` (`action: "generate"` / `imgforge pack generate --subject <S> --style <ST> [--seed <PNG>]`).
   - `character` and `creature` subjects execute a **2-step anchor chain**: Step 1 renders `turnaround.png` (`1536x1024`, `#252830` backdrop, optional `Seed`), and Step 2 anchors `poses_expressions.png` and `inventory_flatlay.png` on `turnaround.png` plus `Style` exemplars.
   - `prop` and `location` subjects render a single `sheet.png` (`1536x1024`).
   - `imgforge` validates `#252830` structure, composites the bottom `#252830` calibration strip (6-step `L*` gray wedge + pixel-exact `48x48` `PaletteLock` swatches) on the host, and slices components via foreground bounding boxes into `slices/*.png` and `slices.json`.
   - When `Subject`s already have packs, `generate` in `Standard` mode automatically packs needed slices into dynamic `#252830` `Atlas` page(s) (up to `3` focal subjects) alongside `Style` exemplars. Use `pack` `action: "atlas"` when explicitly testing or inspecting a subset of `slice_ids`.
4. **Constrain complex staging with `SpatialGuide`:** Author a `GuideSpec` (`schemaVersion: "1.0"`) for `Blockout` (depth-band zones, scaled `resvg` `Subject` mannequins, `ActionVector`s) or `OverlayGuide` (haloed vector cues over an existing workspace PNG), render via `guide` (`action: "render"`), and pass the PNG to `generate` (`--spatial-guide` / `spatial_guide_path`).
5. **Generate and inspect `Run`s:** Run `generate` to record `runs/<slug>/vNNN.png` and `vNNN.meta.json`.
   - `ColorQaEngine` runs fail-closed checks on `Atmosphere.exposure_floor` (`mean L*`, `P10 L*`, clipped black `L* < 5.0`), `PaletteLock` OKLab k-means (`K = 6`) + forbidden-hue bleed, and high-pass DoG yellow-sparkle blobs (`1..=64 px^2`).
   - Whole-image QA never silently re-grades generated images; passing runs advance to `candidate` (unless `--draft` / `draft: true` is passed), while failing runs are recorded as `draft` and fail closed unless `--allow-qa-fail` / `allow_qa_fail: true` is set.
6. **Refine targeted defects (`WholeImageEdit` vs `PatchEdit`):**
   - Use **`PatchEdit`** (`edit` `mode: "patch"`) for localized fixes (`roi` or `mask_path`). It extracts a `1:1` square crop with a `16..=32 px` collar, optionally applies `crop_only`, `blank_inside_mask` (`#252830`), or `cue_outline`, invokes `ImageBackend` at `WxW`, harmonizes tone/color via collar OKLab Reinhard transfer clamped to `[0.75, 1.35]`, blends with cosine distance-transform feathering and a 5-level Laplacian pyramid, and verifies zero `DeltaE_2000` drift outside the footprint.
   - Use **`WholeImageEdit`** (`edit` `mode: "whole"`) only for global adjustments; it enforces `ColorQaEngine` fail-closed without mutating untouched pixels or re-grading.
7. **Validate and promote along `Lifecycle`:**
   - Advance `draft -> candidate -> validated` using `validate` (`target: "run"` or `target: "reference_pack"`) with binary `VisionCritic` traits (`identity`, `hand_props`, `weapon_geometry`, `style_adherence`, `zero_leaked_text`) and Color QA.
   - Advance `validated -> canonical` using `promote` (`action: "run"` or `action: "reference_pack"`), which locks SHA-256 checksums in `library/manifest.lock.json` and marks any prior `canonical` version `superseded`. Verify lockfile integrity anytime with `imgforge library verify-lock`.
   - `Seed` is strictly an input role (`SeedInput`), never a `Lifecycle` state.
8. **Derive deliverables with `Export`:** Run `export` (`vtt_token`, `vtt_map`, `web_webp`, `grid_2x2`).
   - `Visibility` (`public` < `restricted` < `internal`) is enforced fail-closed across `SceneSpec`, `ReferencePack`, `Atlas`, `Edit`, `Validate`, and `Export` so spoiler-sensitive assets never leak into `public` outputs.
   - Explicit `--match-grade` (damped `L*` CDF matching in `0.25..=0.45` and `.cube` 3D LUT trilinear grading) is supported exclusively on `export`.
