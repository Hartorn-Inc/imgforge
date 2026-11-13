# imgforge CLI and MCP Reference

All filesystem paths passed to MCP tools are validated strictly inside the resolved workspace root with zero symlinks or `..` traversal.

## 1. CLI Subcommands & Flags

Global flags: `--workspace <PATH>`, `--codex-bin <PATH>` (or `IMGFORGE_CODEX_BIN`).

| Command | Key Flags / Arguments | Purpose |
|---|---|---|
| `imgforge auth login` | `[--device-auth]` | Provision tool-local ChatGPT Plus OAuth `auth.json` (`0600`) |
| `imgforge auth status` | — | Verify local `auth.json` integrity and `codex login status` |
| `imgforge auth logout` | — | Remove tool-local `auth.json` |
| `imgforge compose` | `--prose <TEXT>` \| `--prose-file <PATH>`, `[--out <PATH>]`, `[--format yaml\|json]` | Translate prose to validated `SceneSpec` via `SceneComposer` |
| `imgforge validate` | `<SCENE_SPEC>` \| `--run <SLUG:VER>` \| `--pack <SUBJECT:STYLE>`, `[--atmosphere <SLUG>]`, `[--subject <SLUG>...]`, `[-i/--ref <PATH>...]`, `[--trait <ID:CAT:CRITERION>...]`, `[--critic]`, `[--json]` | Validate `SceneSpec`, or advance `Run`/`ReferencePack` to `validated` |
| `imgforge promote` | `--run <SLUG:VER>` \| `--pack <SUBJECT:STYLE>`, `[--json]` | Promote `validated` `Run` or `ReferencePack` to `canonical` in `manifest.lock.json` |
| `imgforge generate` | `--scene <PATH>` \| `--compose <PROSE>`, `[--slug <SLUG>]`, `[--explore]`, `[--width <PX>]`, `[--height <PX>]`, `[--slice <SUBJ:STYLE:ID1,ID2...>]`, `[-i/--ref <FILE>...]`, `[--spatial-guide <PATH>]`, `[--subject-roi <SUBJ[:X,Y,W,H]>...]`, `[--out <PATH>]`, `[--allow-qa-fail]`, `[--draft]`, `[--json]` | Run `0`-ref `explore`, `1`-ref, or multi-ref auto-`Atlas` generation |
| `imgforge pack generate` | `--subject <SLUG>`, `--style <SLUG>`, `[--seed <PATH>]`, `[--seed-label <TEXT>]`, `[--json]` | Generate style-scoped `ReferencePack` (2-step or 1-sheet), calibrate, and slice |
| `imgforge pack slice` | `--sheet <PATH>`, `--out-dir <PATH>`, `[--strip-height 72]`, `[--json]` | Slice foreground components against `#252830` |
| `imgforge pack atlas` | `--out <PATH>`, `--slice <SUBJ:STYLE:ID1,ID2...>`, `[--visibility public\|restricted\|internal]`, `[--width 1536]`, `[--height 1024]`, `[--json]` | Pack selected slices onto `#252830` `Atlas` page(s) |
| `imgforge edit whole` | `--target <PATH>`, `--slug <SLUG>`, `--prompt <TEXT>`, `[--visibility ...]`, `[--atmosphere <SLUG>]`, `[--subject <SLUG>...]`, `[-i/--ref <FILE>...]`, `[--allow-qa-fail]`, `[--json]` | `WholeImageEdit` with fail-closed `ColorQaEngine` gating |
| `imgforge edit patch` | `--target <PATH>`, `--slug <SLUG>`, `--prompt <TEXT>`, `--roi <X,Y,W,H>` \| `--mask <PATH>`, `[--visibility ...]`, `[--prep crop-only\|blank-inside-mask\|cue-outline]`, `[--collar-width 24]`, `[--feather 12]`, `[-i/--ref <FILE>...]`, `[--json]` | `PatchEdit` (`1:1` crop + collar Reinhard + 5-level Laplacian blend) |
| `imgforge guide validate` | `<SPEC>` | Validate `GuideSpec` JSON (`schemaVersion: "1.0"`) |
| `imgforge guide render` | `--spec <PATH>`, `--out <PATH>`, `[--json]` | Render `OverlayGuide` or `Blockout` `SpatialGuide` PNG |
| `imgforge export` | `--run <SLUG:VER>` \| `--input <PATH>`, `[--source-visibility ...]`, `--profile vtt-token\|vtt-map\|web-webp\|grid-2x2`, `[--visibility ...]`, `--out-dir <DIR>`, `[--slug <SLUG>]`, `[--token-size 512]`, `[--ring-color #D4A843]`, `[--jpeg-backdrop #HEX]`, `[--cols 24]`, `[--rows 18]`, `[--grid-px 140]`, `[--scale-factor <F>]`, `[--jpeg-quality 90]`, `[--match-grade]`, `[--grade-ref <PATH>]`, `[--grade-strength 0.35]`, `[--lut <PATH>]`, `[--json]` | Derive deterministic `Export` deliverables |
| `imgforge run` | `validate <SLUG:VER> ...` \| `promote <SLUG:VER> [--json]` \| `list [--slug <SLUG>] [--json]` | Inspect, validate, or promote `Run`s |
| `imgforge library` | `list [--kind subject\|style\|atmosphere] [--json]` \| `show <KIND> <SLUG> [--json]` \| `verify-lock` \| `promote --subject <S> --style <ST> [--json]` | Inspect library cards and verify `manifest.lock.json` |
| `imgforge critic` | `--image <PATH>`, `[-i/--ref <PATH>...]`, `--trait <ID:CAT:CRITERION>...` | Run standalone `VisionCritic` checklist |
| `imgforge ui` | `[--port <PORT>]` | Serve embedded Web Canvas SPA strictly on `127.0.0.1:<port>` |
| `imgforge mcp` | `[--http 127.0.0.1:<PORT>]` | Serve `rmcp` MCP server over stdio (default) or loopback HTTP `/mcp` |
| `imgforge status` | `[--json]` | Inspect workspace root, backend, and local auth provisioning |

## 2. MCP Tools (`8`) & Resources (`4`)

### Tools

1. **`compose`**: `prose` (required), `recommend_blockout` (bool), `out_path`, `format` (`"yaml" | "json"`). Returns `sceneSpec`, `compiledPrompt`, `writtenPath`, `blockoutRecommendation`.
2. **`generate`**: mutually exclusive `scene_path` | `scene_spec` | `compose_prose`; optional `slug`, `explore`, `width`, `height`, `atlas_selections` (`[{subject_slug, style_slug, slice_ids}]`), `reference_images`, `spatial_guide_path`, `subject_rois` (`[{subject_slug, rois: [{x, y, width, height}]}]`), `out_path`, `allow_qa_fail`, `draft`.
3. **`pack`**: `action` (`"generate" | "slice" | "atlas"`).
   - `"generate"`: `subject_slug`, `style_slug`, optional `seed_path`, `seed_label`.
   - `"slice"`: `sheet_path`, `out_dir`, optional `strip_height` (`72`).
   - `"atlas"`: `selections`, `out_path`, optional `visibility`, `width` (`1536`), `height` (`1024`).
4. **`edit`**: `mode` (`"whole" | "patch"`), `target_image_path`, `slug`, `prompt`, optional `visibility`, `reference_images`.
   - `"whole"`: optional `atmosphere_slug`, `subject_slugs`, `allow_qa_fail`.
   - `"patch"`: mutually exclusive `roi` (`{x, y, width, height}`) | `mask_path`, optional `prep_mode` (`"crop_only" | "blank_inside_mask" | "cue_outline"`), `collar_width_px` (`16..=32`), `feather_px` (`2..=collar_width_px`).
5. **`guide`**: `action` (`"validate" | "render"`), mutually exclusive `spec_path` | `guide_spec`, `out_path` (required for `"render"`).
6. **`export`**: mutually exclusive (`run_slug`, `run_version`) | `input_path` (with optional `source_visibility`), `profile` (`"vtt_token" | "vtt_map" | "web_webp" | "grid_2x2"`), `out_dir`, optional `visibility`, `output_slug`, `token_size`, `ring_color_hex`, `jpeg_backdrop_hex`, `cols`, `rows`, `grid_px`, `scale_factor`, `jpeg_quality`, `match_grade`, `grade_ref_path`, `grade_strength` (`0.25..=0.45`), `cube_lut_path`.
7. **`validate`**: `target` (`"scene_spec" | "run" | "reference_pack"`).
   - `"scene_spec"`: `scene_path` | `scene_spec`.
   - `"run"`: `slug`, `version`, optional `atmosphere_slug`, `subject_slugs`, `reference_images`, `traits` (`[{trait_id, category, criterion}]`), `run_critic`.
   - `"reference_pack"`: `subject_slug`, `style_slug`, optional `traits`, `run_critic`.
   - Trait `category` values: `"identity"`, `"hand_props"`, `"weapon_geometry"`, `"style_adherence"`, `"zero_leaked_text"`.
8. **`promote`**: `action` (`"run" | "reference_pack" | "status"`).
   - `"run"`: `slug`, `version`.
   - `"reference_pack"`: `subject_slug`, `style_slug`.
   - `"status"`: optional `slug` filter.

### Resources

| URI | MIME Type | Contents |
|---|---|---|
| `imgforge://manifest` | `application/json` | `library/manifest.lock.json` (`canonical` and `superseded` entries) |
| `imgforge://subjects` | `application/json` | Resolved style-neutral `Subject` cards across `project > user` tiers |
| `imgforge://styles-atmospheres` | `application/json` | Resolved `Style` and `Atmosphere` cards across `project > user > builtin` tiers |
| `imgforge://runs` | `application/json` | Recorded `Run` stream index, versions, and latest QA reports under `runs/` |
