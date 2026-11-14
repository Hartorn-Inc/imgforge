---
title: "Philosophy"
description: "Why prompt-only generation drifts and how imgforge decouples Subject, Style, and Atmosphere with deterministic PromptCompiler, fail-closed Color QA, and isolated OAuth."
---

# Philosophy

`imgforge` is built around a straightforward engineering premise: **generative models should paint pixels, while deterministic host code should own structure, color invariants, spatial geometry, and asset locking.**

When multi-asset visual production relies on prose prompts and manual retries alone, every generation introduces silent drift. `imgforge` separates creative intent from structural enforcement across five core architectural decisions.

---

## 1. Why Prompt-Only Generation Drifts

Unstructured prose forces a single text prompt to carry four incompatible responsibilities at once:

1. **Identity** — what a `Subject` looks like, how tall it stands, which hand holds which weapon, and which exact hex colors belong on each garment slot.
2. **Rendering Technique** — line weight, brushwork, shading curves, and framing rules (`Style`).
3. **Scene Lighting** — key light direction, Kelvin temperature, shadow exposure floor, and weather (`Atmosphere`).
4. **Spatial Layout** — camera shot, depth planes, left-to-right ordering, and motion trajectories (`SceneSpec` and `SpatialGuide`).

When those four axes live inside one paragraph of prose, editing a character's cloak color accidentally alters the lighting temperature, adding a second character shifts relative heights, and changing the camera angle mutates weapon geometry.

## 2. Decoupling `Subject`, `Style`, and `Atmosphere`

`imgforge` separates those axes into first-class YAML specifications validated on load:

- **`Subject` (`library/subjects/<slug>/card.yaml`):** Style-neutral physical identity—proportions, `height_m`, `PaletteLock` base albedo swatches, `ExclusiveSurfaceMap` slot-to-material bindings, `asymmetric_markers`, `positive_weapon_geometry`, and `Visibility`.
- **`Style` (`library/styles/<slug>/card.yaml` + `exemplars/*.png`):** Rendering medium, linework, shading, palette profile, positive `style_guardrails`, and `1..=3` visual exemplars.
- **`Atmosphere` (`library/atmospheres/<slug>/card.yaml`):** Key and secondary light directions, `color_temperature_k`, `max_ambient_tint_blend`, `ExposureFloor`, and atmospheric particles.

As documented in [ADR 0003](/adr/0003-style-scoped-reference-packs), storing a single global turnaround sheet per `Subject` fails in cross-style workflows: passing a Franco-Belgian inked sheet into a painterly dark-fantasy generation causes ink outlines to bleed into the output. Scoping each `ReferencePack` under `library/subjects/<subject_slug>/packs/<style_slug>/` keeps `Subject` identity shared while giving every `(Subject, Style)` pair clean visual anchors.

## 3. Deterministic `PromptCompiler` Over Double-LLM Rewriting

In many AI imaging pipelines, an agent constructs structured intent, hands it to an LLM "prompt writer" to expand into prose, and passes that prose to an image model. That middle LLM hop adds 5–15 seconds of latency per call and quietly rewrites hand bindings, left-to-right slot ordering, and weapon descriptions.

Per [ADR 0004](/adr/0004-deterministic-prompt-compiler-over-double-llm), `imgforge` makes `SceneSpec` the canonical boundary:

- **`PromptCompiler` (Rust, sub-millisecond):** Deterministically validates `SceneSpec` invariants—finite depth planes (`far_meters > near_meters`), `<= 3` strictly left-to-right `foreground_slots`, explicit `left_hand` / `right_hand` bindings, unique `ExclusiveSurfaceMap` slots, `Visibility` compatibility, and zero active negation tokens—and compiles fixed `[SCENE]`, `[CAMERA]`, `[DEPTH_BANDS]`, `[FOREGROUND_SLOTS_LEFT_TO_RIGHT]`, `[STYLE]`, `[ATMOSPHERE]`, and `[OUTPUT]` sections.
- **`SceneComposer` (`codex exec -s read-only`, optional):** Invoked *only* when a human supplies unstructured prose via `imgforge compose` or `imgforge generate --compose`. Untrusted prose is bounded (`32 KiB`), fenced inside `<<<UNTRUSTED_SCENE_PROSE>>>` delimiters, parsed with `#[serde(deny_unknown_fields)]`, and immediately validated through `PromptCompiler`.

## 4. Calibration Strips & Fail-Closed Color QA Over Silent Re-Grading

Asking an image model to paint its own reference color swatches yields distorted RGB values that poison downstream checks. Conversely, silently applying global color grading to every generated scene washes out intentional twilight or torchlit `Atmosphere`s.

Per [ADR 0005](/adr/0005-host-composited-calibration-and-fail-closed-color-qa), `imgforge` draws a crisp boundary between **inspection**, **local blending**, and **explicit export grading**:

- **Host-Composited Calibration Strip:** `imgforge` deterministically paints the <span class="forge-swatch-chip"></span>`#252830` neutral studio calibration strip—a 6-step CIE `L*` gray wedge (`15, 30, 45, 60, 75, 90`) plus pixel-exact `48x48` `PaletteLock` swatches—onto `ReferencePack` sheets on the host.
- **Fail-Closed `ColorQaEngine`:** Evaluates CIE `L*` exposure floors and shadow crush, OKLab k-means (`K = 6`) cluster adherence against `PaletteLock` (`DeltaE_2000`), forbidden hue sectors, untouched-region `1 - M` drift, and Difference-of-Gaussians (`DoG`) yellow-dot sparkle artifacts. Whole-image runs fail closed (`CoreError::ColorQaFailed`) and emit diagnostic RGBA heatmaps rather than silently mutating pixels.
- **Automatic Local Harmonization Only in `PatchEdit`:** When replacing a localized ROI or painted mask, `PatchEdit` crops a `1:1` square with a `16..=32 px` context collar, runs `ImageBackend`, applies 10%-trimmed OKLab Reinhard transfer (`[0.75, 1.35]` clamped gains) from the collar ring, and blends via a cosine distance-transform feather and 5-level Laplacian pyramid—guaranteeing `0.0` `DeltaE_2000` drift outside the composite footprint.
- **Opt-In Grading Only in `Export`:** Global damped `L*` CDF matching (`0.25..=0.45`) and Adobe `.cube` 3D LUTs run exclusively when requested via `imgforge export --match-grade`.

## 5. Isolated ChatGPT Plus OAuth Over Browser Scraping or API Keys

Per [ADR 0002](/adr/0002-chatgpt-plus-oauth-via-isolated-codex-exec), `imgforge` ships `CodexNativeBackend` as its sole built-in `ImageBackend`. Instead of charging per-image `OPENAI_API_KEY` API calls or automating fragile browser DOM / Turnstile flows, `imgforge` wraps `codex exec` with built-in `image_gen` backed by ChatGPT Plus OAuth:

- **Tool-Local Credentials:** `imgforge auth login` stores OAuth credentials strictly under `$XDG_DATA_HOME/imgforge/codex/auth.json` (`0700` directory, `0600` regular non-symlink file) without symlinking or mutating a user's global `~/.codex/auth.json`.
- **Per-Invocation `TempCodexHome`:** Every `CodexNativeBackend`, `SceneComposer`, and `VisionCritic` call allocates a private `0700` temporary `CODEX_HOME`, strips `OPENAI_API_KEY`, writes `0600` copies of `auth.json` and minimal `config.toml`, and deletes the directory on `Drop`.
- **Host Aspect-Ratio Guard & Resampling:** Output PNGs are checked for the 8-byte PNG signature, rejected fail-closed if aspect-ratio error exceeds `5%`, and resampled with `FilterType::Lanczos3` to exact target dimensions.
