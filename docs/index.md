---
layout: home
title: "imgforge — Visual Consistency Engine"
titleTemplate: false
description: "Standalone Rust visual consistency engine with deterministic PromptCompiler, style-scoped ReferencePacks, fail-closed OKLab Color QA, PatchEdit compositing, MCP server, and embedded Web Canvas UI."

hero:
  name: "imgforge"
  text: "Visual Consistency Engine"
  tagline: "Deterministic prompting, style-scoped ReferencePacks, fail-closed OKLab Color QA, and localized PatchEdit compositing in a single Rust binary."
  image:
    src: /logo-placeholder.svg
    alt: imgforge geometric mark
  actions:
    - theme: brand
      text: Getting Started
      link: /getting-started
    - theme: alt
      text: How It Works
      link: /architecture
    - theme: alt
      text: Philosophy
      link: /philosophy

features:
  - title: Decoupled Subject, Style & Atmosphere
    details: Keep Subject proportions, PaletteLock hex swatches, and ExclusiveSurfaceMap style-neutral while scoping ReferencePacks per (Subject, Style) pair and isolating Atmosphere lighting.
  - title: Deterministic PromptCompiler
    details: Compile structured SceneSpecs into positive-only prompts in sub-millisecond Rust—eliminating double-LLM drift while keeping read-only SceneComposer available for prose.
  - title: Calibration Strips & Fail-Closed QA
    details: Composite #252830 gray-wedge and PaletteLock strips on the host, then enforce CIE L*, OKLab k-means, DeltaE_2000, and DoG sparkle checks without silent re-grading.
  - title: SpatialGuides & PatchEdit Blending
    details: Author Blockout and OverlayGuide layouts in the embedded Web Canvas UI, render deterministically with tiny-skia and resvg, and blend 1:1 patches via collar Reinhard + Laplacian pyramids.
  - title: Isolated ChatGPT Plus OAuth
    details: Run codex exec inside ephemeral 0700 TempCodexHome sandboxes using tool-local auth.json credentials—zero OPENAI_API_KEY billing and zero browser scraping.
  - title: CLI, MCP Server & Localhost UI
    details: Ship one native Linux/macOS/Windows binary with clap CLI commands, an 8-tool / 4-resource rmcp server, and an embedded Preact + Konva SPA bound strictly to 127.0.0.1.
---

## Quick Install

Install a checksum-verified release binary or build inside the pinned Rust + Node builder container:

::: code-group

```bash [macOS / Linux]
curl -fsSL https://github.com/Hartorn-Inc/imgforge/releases/latest/download/install.sh | bash
```

```powershell [Windows (PowerShell)]
irm https://github.com/Hartorn-Inc/imgforge/releases/latest/download/install.ps1 | iex
```

```bash [Docker + Make]
make build
```

:::

Then provision tool-local credentials and inspect your workspace:

```bash
imgforge auth login
imgforge auth status
imgforge status
```

## Why `imgforge` Exists

Prompt-only image generation drifts across shots: armor trims change hue, secondary props swap hands, ink outlines bleed into painterly scenes, and silent post-processing ruins intentional dark atmospheres. `imgforge` replaces ad-hoc prompting with an inspectable engineering pipeline where structure, color math, `Lifecycle` locking, and `Visibility` isolation are enforced deterministically.

Read [Philosophy](/philosophy) for the design rationale or explore [How It Works](/architecture) with interactive architectural diagrams.
