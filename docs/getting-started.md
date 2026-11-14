---
title: "Getting Started"
description: "Install imgforge, provision tool-local ChatGPT Plus OAuth, configure imgforge.toml, and run ReferencePack, SceneSpec, PatchEdit, Export, MCP, and Web Canvas UI workflows."
---

# Getting Started

Get `imgforge` installed, authenticate with tool-local ChatGPT Plus OAuth, configure an `imgforge.toml` workspace, and run your first end-to-end generation, `PatchEdit`, and `Export` workflow.

---

## 1. Install `imgforge`

Prebuilt release archives are verified against `SHA256SUMS.txt` on every install, or you can compile natively from source inside the repository's pinned Docker builder.

::: code-group

```bash [macOS / Linux]
curl -fsSL https://github.com/Hartorn-Inc/imgforge/releases/latest/download/install.sh | bash
```

```powershell [Windows (PowerShell)]
irm https://github.com/Hartorn-Inc/imgforge/releases/latest/download/install.ps1 | iex
```

```bash [Docker + Make (Source)]
git clone https://github.com/Hartorn-Inc/imgforge.git
cd imgforge
make build
# Compiled binary: ./target/release/imgforge
```

:::

- **macOS / Linux overrides:** `IMGFORGE_VERSION` (for example `0.1.0` or `v0.1.0`, default `latest`) and `IMGFORGE_INSTALL_DIR` (default `~/.local/bin`).
- **Windows overrides:** `-Version` / `$env:IMGFORGE_VERSION` and `-InstallDir` / `$env:IMGFORGE_INSTALL_DIR` (default `%LOCALAPPDATA%\imgforge\bin`, added to User `PATH`).
- **WSL2 note:** `Dockerfile` targets Debian Bookworm (`glibc 2.36`) so `target/release/imgforge` runs directly on Ubuntu 24.04 (`glibc 2.39`) WSL2 hosts with native access to `codex exec` and `127.0.0.1` ports.

---

## 2. Provision ChatGPT Plus OAuth (`imgforge auth`)

`imgforge` never requires `OPENAI_API_KEY` and never modifies `~/.codex/auth.json`. Credentials are stored strictly at `$XDG_DATA_HOME/imgforge/codex/auth.json` (default `~/.local/share/imgforge/codex/auth.json`) with `0700` directory and `0600` file permissions:

```bash
# Interactive browser OAuth (add --device-auth for headless/remote terminals)
imgforge auth login

# Verify local auth.json integrity and Codex login readiness
imgforge auth status

# Remove tool-local credentials when needed
imgforge auth logout
```

---

## 3. Workspace Discovery & `imgforge.toml`

`imgforge` discovers your project workspace in strict non-fallback precedence:

1. `--workspace <PATH>` CLI flag
2. Non-empty `IMGFORGE_WORKSPACE` environment variable
3. Nearest `imgforge.toml` walking upward from the current working directory

If an explicit `--workspace` or `IMGFORGE_WORKSPACE` path lacks a valid `imgforge.toml`, `imgforge` fails closed rather than falling back to CWD.

Create a minimal `imgforge.toml` (`schema_version = 1`, `deny_unknown_fields`) at your project root:

```toml
schema_version = 1

[backend]
kind = "codex-native"

[paths]
library_dir = "library"
runs_dir = "runs"
exports_dir = "exports"

[server]
port = 8080
```

Verify workspace and auth readiness:

```bash
imgforge status
```

---

## 4. CLI Quickstart

### Inspect Built-in `Style` and `Atmosphere` Presets

```bash
imgforge library list
imgforge library show style dark-fantasy-realistic
imgforge library show atmosphere torchlit-crypt
```

### Generate, Validate, and Promote a `ReferencePack`

Given a `Subject` card at `library/subjects/warden-kael/card.yaml`:

```bash
# 1. Generate a style-scoped ReferencePack (turnaround + poses_expressions + inventory_flatlay)
imgforge pack generate \
  --subject warden-kael \
  --style dark-fantasy-realistic \
  --seed assets/seeds/warden-portrait.png

# 2. Validate SHA-256 digests, calibration swatch DeltaE_2000 <= 0.5, and VisionCritic checks
imgforge validate --pack warden-kael:dark-fantasy-realistic --critic

# 3. Lock the validated ReferencePack in library/manifest.lock.json
imgforge promote --pack warden-kael:dark-fantasy-realistic
```

### Generate a Scene `Run`

Author a `SceneSpec` YAML/JSON file (or use `--compose` once for prose-to-`SceneSpec` translation via read-only `SceneComposer`):

```bash
# Deterministic SceneSpec -> PromptCompiler -> CodexNativeBackend -> ColorQaEngine -> RunStore
imgforge generate \
  --scene scenes/harbor-ambush.yaml \
  --slice warden-kael:dark-fantasy-realistic:turnaround_front,inventory_blade \
  --subject-roi warden-kael

# Validate the recorded Run (v001) and promote it to canonical
imgforge validate --run harbor-ambush:1 --atmosphere torchlit-crypt --subject warden-kael --critic
imgforge promote --run harbor-ambush:1
```

### Apply a Localized `PatchEdit`

Fix a localized region without drifting the rest of the frame:

```bash
imgforge edit patch \
  --target runs/harbor-ambush/v001.png \
  --slug harbor-ambush \
  --roi 640,310,220,220 \
  --prep cue-outline \
  --collar-width 24 \
  --feather 12 \
  --prompt "Replace the damaged lantern glass with intact brass framing and warm amber glow."
```

`PatchEdit` extracts a `1:1` square crop with the context collar, applies 10%-trimmed OKLab Reinhard transfer (`[0.75, 1.35]`), composites with a 5-level Laplacian pyramid, and verifies `0.0` `DeltaE_2000` drift outside the composite footprint.

### Export Deliverables

Derive deterministic deliverables without creating new `Lifecycle` versions:

```bash
# Circular anti-aliased VTT token (512x512 PNG)
imgforge export \
  --run harbor-ambush:v001 \
  --profile vtt-token \
  --visibility public \
  --out-dir exports/tokens

# WebP delivery with explicit opt-in color grading
imgforge export \
  --run harbor-ambush:v001 \
  --profile web-webp \
  --visibility public \
  --match-grade \
  --grade-strength 0.35 \
  --out-dir exports/web
```

---

## 5. MCP Server (`imgforge mcp`)

Run `imgforge` as a Model Context Protocol server over stdio (default) or loopback Streamable HTTP:

```bash
# Stdio transport (default for coding agents)
imgforge mcp

# Loopback Streamable HTTP with Host/Origin checks
imgforge mcp --http 127.0.0.1:3001
```

`ImgforgeMcpServer` exposes **8 workspace-contained tools** (`compose`, `generate`, `pack`, `edit`, `guide`, `export`, `validate`, `promote`) and **4 read-only resources** (`imgforge://manifest`, `imgforge://subjects`, `imgforge://styles-atmospheres`, `imgforge://runs`). Every path argument rejects `..` traversal, NUL bytes, and symlinks outside the resolved workspace root.

---

## 6. Embedded Web Canvas UI (`imgforge ui`)

Launch the embedded `TypeScript + Vite + Preact + Konva` Web Canvas SPA strictly on `127.0.0.1`:

```bash
imgforge ui
# Override port from imgforge.toml:
imgforge ui --port 8090
```

Open `http://127.0.0.1:8080` to author `GuideSpec` (`Blockout` and `OverlayGuide`) layouts, inspect `ReferencePack` slices, paint `PatchEdit` masks, review `ColorQaEngine` heatmaps, and promote `Run` `Lifecycle` states.
