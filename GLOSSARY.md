# imgforge

`imgforge` is a standalone visual consistency engine for generating, editing, validating, and exporting multi-asset raster artwork via both agentic CLI/MCP workflows and a local Web Canvas UI.

## Language

### Core Domain Entities

**Subject**:
A recurring visual entity (`character`, `creature`, `prop`, or `location`) defined by a style-neutral identity specification (`card.yaml`) including proportions, physical height, material bindings, and `PaletteLock` swatches.
_Avoid_: CharacterCard, PropCard, Entity, AssetSpec

**Style**:
A first-class rendering definition (`card.yaml` plus `1–3` visual `exemplars`) specifying medium, linework, shading, texture, framing rules, and anti-drift guardrails independently of any `Subject` or `Atmosphere`.
_Avoid_: StyleCard, Theme, RenderPreset

**Atmosphere**:
A lighting and environmental mood specification (`card.yaml`) defining light direction, color temperature (Kelvin), `exposure_floor`, maximum ambient tint blend, and atmospheric particles.
_Avoid_: AmbianceCard, LightingCard, Mood

**ReferencePack**:
A style-scoped set of canonical reference sheets (`turnaround`, `poses_expressions`, and `inventory_flatlay` for `character`/`creature`, or single-sheet templates for `prop`/`location`) and their component-detected slices belonging to one `(Subject, Style)` pair.
_Avoid_: CharSheet, ModelSheet, GlobalRefSheet

**Seed**:
An initial input image (for example an existing parchment portrait or an approved `explore` `Run`) supplied as a bootstrap visual input to generate a `ReferencePack`.
_Avoid_: DraftState, AnchorState

**Atlas**:
A composite image dynamically packed from selected `ReferencePack` slices (such as specific character angles and weapons) to minimize visual attention dilution in multi-subject scenes.
_Avoid_: MegaSheet, MontageDump

### Color & Composition Contracts

**PaletteLock**:
A set of exact unshaded base albedo hex colors bound to a `Subject` for deterministic OKLab/`DeltaE_2000` color-drift verification.
_Avoid_: ColorTheme, SwatchHint

**ExclusiveSurfaceMap**:
An exhaustive assignment of material and base color to every visible surface or garment slot of a `Subject` so neighboring colors cannot bleed across boundaries.
_Avoid_: NegativeColorBan, SlotColors

**SceneSpec**:
A structured, machine-validated composition specification (camera shot, depth planes, left-to-right slots, metric heights, hand bindings, `Subject` refs, `Style`, and `Atmosphere`) consumed by the `PromptCompiler`.
_Avoid_: RawPrompt, CompositionNote

**SceneComposer**:
The optional LLM stage (`codex exec -s read-only`) that translates unstructured prose intent into a structured `SceneSpec`.
_Avoid_: PromptWriter

**PromptCompiler**:
The deterministic Rust stage that validates `SceneSpec` invariants and compiles the final positive-only prompt payload sent to an `ImageBackend`.
_Avoid_: PromptSanitizer, LLMCompiler

### Spatial Guidance

**SpatialGuide**:
A programmatic visual reference image rendered deterministically from a `GuideSpec` JSON definition by `GuideRenderer` to constrain layout, scale, or trajectories.
_Avoid_: Patron, LayoutHint

**OverlayGuide**:
A `SpatialGuide` variant that draws haloed vector lines, regions, or cues directly onto a copy of an existing target image for targeted trajectory or object fixes.
_Avoid_: MethodA, GuidedOverlay

**Blockout**:
A `SpatialGuide` variant rendered from scratch containing depth-band color zones, proportionally scaled `Subject` mannequins, and action vectors.
_Avoid_: MethodB, StickFigurePatron

**GuideSpec**:
The shared JSON schema edited in the Web Canvas UI and consumed by `GuideRenderer` (`tiny-skia`/`resvg`) to produce a `SpatialGuide`.
_Avoid_: PatronJson, CanvasDrawing

**GuideRenderer**:
The deterministic Rust vector-to-raster engine (`tiny-skia`/`resvg`) that renders a `GuideSpec` into a `SpatialGuide` PNG.
_Avoid_: PatronRenderer

### Editing, Lifecycle & Deliverables

**WholeImageEdit**:
An edit strategy that passes the entire target image and reference anchors to `image_gen` for global or multi-region adjustments.
_Avoid_: Mode5A, FullFix

**PatchEdit**:
A localized edit strategy that extracts a `1:1` square crop around an ROI or painted mask, optionally blanks/cues the target region, invokes `image_gen`, and seamlessly composites the patch via collar-ring OKLab Reinhard transfer and Laplacian pyramid blending.
_Avoid_: Mode5B, Mode5C, MaskInpaint

**Run**:
A single versioned execution record (`vNNN.png` + `vNNN.meta.json`) capturing exact input hashes, compiled prompt, backend metadata, and automated/VLM QA reports.
_Avoid_: Job, GenerationFile

**Lifecycle**:
The five-state progression of an asset (`draft` -> `candidate` -> `validated` -> `canonical` -> `superseded`), where `canonical` assets are immutable and frozen in `manifest.lock.json`.
_Avoid_: ApprovalFlag, SeedState

**VisionCritic**:
The structured read-only VLM inspection stage (`codex exec --output-schema`) that evaluates a `Run` against binary per-trait checklists (identity, hand props, weapon geometry, style adherence, zero leaked text).
_Avoid_: AutoJudge, CriticBot

**Export**:
A deterministic, reproducible post-processed deliverable (such as a circular VTT token, upscaled battlemap JPEG, or WebP web image) derived from a `Run` or `canonical` asset rather than tracked as an independent lifecycle version.
_Avoid_: DerivedAsset, OutputVersion

**Visibility**:
An access classification (`public`, `restricted`, or `internal`) attached to a `Subject`, `SceneSpec`, or `Export` profile to prevent spoiler-sensitive assets from leaking into public outputs.
_Avoid_: SpoilerFlag, AudienceMode
