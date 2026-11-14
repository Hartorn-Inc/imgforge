# How It Works

`imgforge` compiles into a single Rust binary (`imgforge-cli` + `imgforge-core`) with an embedded `TypeScript + Vite + Preact + Konva` Web Canvas SPA (`rust-embed`). All generation, validation, slicing, editing, and export logic lives in `imgforge-core`, while `imgforge-cli` exposes CLI commands, an `rmcp` MCP server, and a strictly loopback `axum` Web UI server.

See the [Implementation Docs](/implementation/architecture-and-backend) and [ADRs](/adr/0001-rust-core-with-embedded-web-canvas) for module-level invariants.

---

## 1. Architecture & Generation Pipeline

Every request—whether initiated from the CLI, an MCP coding agent, or the local Web Canvas UI—converges on the same deterministic core pipeline:

<ArchifyDiagram
  slug="imgforge-pipeline"
  title="imgforge Architecture & Generation Pipeline"
  caption="End-to-end flow across CLI / MCP / Web Canvas UI, SceneComposer, PromptCompiler, LibraryRegistry, ReferencePack & Atlas, CodexNativeBackend, ColorQaEngine, VisionCritic, and RunStore."
  height="600px"
/>

### Subsystem Mapping

| Subsystem | Role | Crate / Source | Spec |
|---|---|---|---|
| **CLI & MCP Server** | `clap` subcommands + `rmcp` (`8` tools, `4` resources, workspace-contained paths) | `crates/imgforge-cli/src/main.rs`, `mcp.rs` | [Architecture & Backend](/implementation/architecture-and-backend) |
| **Web Canvas UI** | Loopback `127.0.0.1:<port>` `axum` server + embedded `Preact`/`Konva` SPA with CSP | `crates/imgforge-cli/src/server.rs`, `frontend/` | [ADR 0001](/adr/0001-rust-core-with-embedded-web-canvas) |
| **Workspace & Auth** | `imgforge.toml` (`deny_unknown_fields`) discovery + `0700`/`0600` `TempCodexHome` | `crates/imgforge-core/src/config.rs`, `auth.rs` | [ADR 0002](/adr/0002-chatgpt-plus-oauth-via-isolated-codex-exec) |
| **SceneComposer** | Optional `codex exec -s read-only --output-schema` prose-to-`SceneSpec` stage | `crates/imgforge-core/src/composer.rs` | [ADR 0004](/adr/0004-deterministic-prompt-compiler-over-double-llm) |
| **PromptCompiler** | Sub-ms Rust `SceneSpec` validation (negations, geometry, `Visibility`) | `crates/imgforge-core/src/prompt_compiler.rs` | [ADR 0004](/adr/0004-deterministic-prompt-compiler-over-double-llm) |
| **LibraryRegistry** | `project > user > builtin` resolution with symlink-free `O_NOFOLLOW` checks | `crates/imgforge-core/src/registry.rs` | [Domain & ReferencePacks](/implementation/domain-and-reference-packs) |
| **ReferencePack & Atlas** | Style-scoped `1536x1024` sheet chains, `#252830` calibration, bbox slicing, dynamic `Atlas` | `reference_pack.rs`, `calibration.rs`, `slicer.rs`, `atlas.rs` | [ADR 0003](/adr/0003-style-scoped-reference-packs) |
| **CodexNativeBackend** | `ImageBackend` trait via `codex exec --json` (`<= 5%` AR guard + `Lanczos3`) | `crates/imgforge-core/src/backend.rs` | [ADR 0002](/adr/0002-chatgpt-plus-oauth-via-isolated-codex-exec) |
| **ColorQaEngine** | Read-only CIE `L*`, OKLab k-means (`K=6`), `DeltaE_2000`, `1 - M` drift, DoG sparkle | `crates/imgforge-core/src/color_qa.rs` | [ADR 0005](/adr/0005-host-composited-calibration-and-fail-closed-color-qa) |
| **VisionCritic** | Read-only VLM binary checklist (`identity`, `hand_props`, `weapon_geometry`, `style_adherence`, `zero_leaked_text`) | `crates/imgforge-core/src/critic.rs` | [Architecture & Backend](/implementation/architecture-and-backend) |

---

## 2. `Run` & `ReferencePack` `Lifecycle` and `Visibility` Gate

Assets follow a strict forward state machine (`draft` -> `candidate` -> `validated` -> `canonical` -> `superseded`) paired with ordered `Visibility` isolation (`public < restricted < internal`):

<ArchifyDiagram
  slug="imgforge-lifecycle"
  title="Run & ReferencePack Lifecycle and Visibility Gate"
  caption="Strict forward Lifecycle transitions, fail-closed ColorQaEngine / VisionCritic rejection, manifest.lock.json SHA-256 freezing, and Visibility gating for Atlas and Export."
  height="560px"
/>

### Key Invariants

- **`Seed` Is Input-Only (`crates/imgforge-core/src/lifecycle.rs`):** A `Seed` (`SeedInput` path + SHA-256 + optional label) bootstraps a `ReferencePack` and never enters `Lifecycle`.
- **Atomic `RunStore` & `ManifestLockStore` (`crates/imgforge-core/src/store.rs`):** Every `Run` writes `runs/<slug>/vNNN.png` and `vNNN.meta.json` atomically (`NamedTempFile` + `sync_all` + `persist`) with full `input_hashes`. Promoting a `validated` `Run` or `ReferencePack` to `canonical` freezes its SHA-256 in `library/manifest.lock.json` and marks any prior `canonical` entry `superseded`.
- **Fail-Closed Validation (`crates/imgforge-core/src/validation.rs`):** `validate_run` and `validate_reference_pack` verify symlink-free containment, SHA-256 digests, calibration swatch `DeltaE_2000 <= 0.5`, sparkle/exposure gates, and `VisionCritic` checklists before allowing `validated` status.
- **`Visibility` Containment (`crates/imgforge-core/src/scene_spec.rs`, `registry.rs`):** `check_visibility_compatibility` blocks `restricted` or `internal` assets from entering lower-classification `SceneSpec`, `Atlas`, `Edit`, `Validate`, or `Export` contexts.

---

## 3. `SpatialGuide`, `PatchEdit`, `WholeImageEdit`, and `Export`

When a generated `Run` needs layout control, localized repair, or final delivery packaging, `imgforge` provides deterministic vector guidance, calibrated compositing, and reproducible export profiles:

<ArchifyDiagram
  slug="imgforge-editing-and-guides"
  title="SpatialGuide, PatchEdit, WholeImageEdit, and Export"
  caption="GuideRenderer (OverlayGuide & Blockout), WholeImageEdit vs. 1:1 PatchEdit Laplacian compositing with zero out-of-ROI drift, and Visibility-gated Export profiles."
  height="580px"
/>

### Imaging & Export Mapping

| Stage | Mechanism | Source | Spec |
|---|---|---|---|
| **`GuideRenderer`** | Renders `GuideSpec` (`schemaVersion: "1.0"`) into deterministic `Blockout` (`#252830` depth bands + `resvg` mannequins) or `OverlayGuide` (`tiny-skia` haloed vectors) PNGs | `crates/imgforge-core/src/guide_renderer.rs` | [Imaging, Color QA & Export](/implementation/imaging-color-qa-and-export) |
| **`WholeImageEdit`** | Forwards `[target, refs...]` to `ImageBackend` without mutating output colors; fails closed via `ColorQaEngine` unless `--allow-qa-fail` is set | `crates/imgforge-core/src/edit.rs` | [Imaging, Color QA & Export](/implementation/imaging-color-qa-and-export) |
| **`PatchEdit`** | Centers `1:1` crop with `16..=32 px` collar, applies `PatchPrepMode`, transfers 10%-trimmed OKLab Reinhard (`[0.75, 1.35]`), blends via cosine DT + 5-level Laplacian pyramid, verifies `0.0` outside `DeltaE_2000` | `crates/imgforge-core/src/edit.rs`, `color_qa.rs` | [ADR 0005](/adr/0005-host-composited-calibration-and-fail-closed-color-qa) |
| **`Export`** | Enforces `Visibility` and outputs `vtt-token` (`4x` SS AA), `vtt-map`, `web-webp`, or `grid-2x2`; runs `L*` CDF (`0.25..=0.45`) or `.cube` 3D LUT only with explicit `--match-grade` | `crates/imgforge-core/src/export.rs` | [Imaging, Color QA & Export](/implementation/imaging-color-qa-and-export) |
