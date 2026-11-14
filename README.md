# imgforge

`imgforge` is a standalone, domain-agnostic visual consistency engine for generating, editing, validating, and exporting multi-asset raster artwork through both agentic CLI/MCP workflows and an embedded local Web Canvas UI.

See [`GLOSSARY.md`](GLOSSARY.md) for the canonical domain vocabulary (`Subject`, `Style`, `Atmosphere`, `ReferencePack`, `Seed`, `Atlas`, `PaletteLock`, `ExclusiveSurfaceMap`, `SceneSpec`, `SpatialGuide`, `WholeImageEdit`, `PatchEdit`, `Run`, `Lifecycle`, `VisionCritic`, `Export`, and `Visibility`).

## Architecture

- **Single Rust Binary Core**: Built with `axum`, `rmcp`, `image`, `palette`, `tiny-skia`, and `resvg`, exposing CLI commands and an MCP server while embedding the frontend via `rust-embed`. See [ADR 0001](docs/adr/0001-rust-core-with-embedded-web-canvas.md).
- **Embedded Web Canvas SPA**: A `TypeScript + Vite + Preact + Konva` interface served strictly on `127.0.0.1:<port>` for authoring `GuideSpec` layouts (`OverlayGuide` and `Blockout`), inspecting `ReferencePack` slices, painting `PatchEdit` masks, and reviewing `Run` `Lifecycle` states.
- **Deterministic Prompting & Color QA**: `PromptCompiler` deterministically compiles validated `SceneSpec` payloads into positive-only prompts, while optional prose input can be structured by `SceneComposer` (`codex exec -s read-only`). See [ADR 0004](docs/adr/0004-deterministic-prompt-compiler-over-double-llm.md) and [ADR 0005](docs/adr/0005-host-composited-calibration-and-fail-closed-color-qa.md).
- **Style-Scoped References**: `Subject` identity (`card.yaml`, `PaletteLock`, `ExclusiveSurfaceMap`) stays style-neutral while `ReferencePack` sheets are scoped per `(Subject, Style)` pair. See [ADR 0003](docs/adr/0003-style-scoped-reference-packs.md).
- **Isolated Codex Native Backend**: `CodexNativeBackend` invokes `codex exec` with built-in `image_gen` backed by ChatGPT Plus OAuth inside an isolated per-job `CODEX_HOME`, enforcing `<= 5%` aspect-ratio tolerance with host Lanczos resampling and `PatchEdit` blending. See [ADR 0002](docs/adr/0002-chatgpt-plus-oauth-via-isolated-codex-exec.md).

## Documentation & Skills

| Artifact | Purpose |
|---|---|
| [`GLOSSARY.md`](GLOSSARY.md) | Canonical domain vocabulary and forbidden aliases |
| [`docs/implementation/architecture-and-backend.md`](docs/implementation/architecture-and-backend.md) | Workspace discovery, `imgforge auth`, `CodexNativeBackend`, `PromptCompiler`, `SceneComposer`, `VisionCritic`, CLI, MCP, and Web Canvas server |
| [`docs/implementation/domain-and-reference-packs.md`](docs/implementation/domain-and-reference-packs.md) | `Subject`, `Style`, `Atmosphere`, `Visibility`, `Lifecycle`, `manifest.lock.json`, `ReferencePack`, calibration strip, slicer, and `Atlas` |
| [`docs/implementation/imaging-color-qa-and-export.md`](docs/implementation/imaging-color-qa-and-export.md) | `ColorQaEngine`, `WholeImageEdit`, `PatchEdit`, `GuideRenderer`, and `Export` profiles |
| [`skills/imgforge/SKILL.md`](skills/imgforge/SKILL.md) | Portable agent workflow skill for `imgforge` CLI & MCP tools |
| [`skills/scene-composition/SKILL.md`](skills/scene-composition/SKILL.md) | Portable skill for direct `SceneSpec` authoring without `SceneComposer` |

## Installation

Install prebuilt release binaries verified with `SHA256SUMS.txt`, or build from source:

- **macOS / Linux (one-line installer, installs to `~/.local/bin`):**

  ```bash
  curl -fsSL https://github.com/Hartorn-Inc/imgforge/releases/latest/download/install.sh | bash
  ```

  Optional environment overrides: `IMGFORGE_VERSION` (for example `0.1.0` or `v0.1.0`, default `latest`) and `IMGFORGE_INSTALL_DIR` (default `~/.local/bin`).

- **Windows (PowerShell, installs to `%LOCALAPPDATA%\imgforge\bin` and adds to User `PATH`):**

  ```powershell
  irm https://github.com/Hartorn-Inc/imgforge/releases/latest/download/install.ps1 | iex
  ```

- **Build from Source (Docker + Make):**

  ```bash
  make build
  ```

## Build & Verification (Docker + Make)

All build, format, lint, and test commands run inside the pinned `Dockerfile` (`Rust 1.100.0` + `Node 22.23.5 LTS` on Debian Bookworm) via `Makefile`. No Rust or Node toolchains need to be installed on the host.

```bash
make help          # List available workflow targets
make docker-build  # Build the pinned Rust + Node 22 LTS builder image
make fmt-check     # Verify formatting without modifying files
make lint          # Run frontend lint/typecheck and cargo clippy (-D warnings)
make test          # Run frontend and Rust workspace tests
make ui-build      # Build the embedded TypeScript/Vite/Preact/Konva SPA
make build         # Build the SPA and compile target/release/imgforge
make clean         # Remove target/ and frontend/dist/ build outputs
```

Cargo and npm caches are persisted on the host under `$XDG_CACHE_HOME/imgforge` (default `~/.cache/imgforge`) and mounted into the container under the caller's UID/GID.

## Native WSL2 Execution

`Dockerfile` compiles against Debian Bookworm (`glibc 2.36`), producing a native Linux binary (`target/release/imgforge`) that executes directly on Ubuntu 24.04 (`glibc 2.39`) WSL2 hosts without installing host toolchains.

Running the compiled binary natively on the WSL2 host enables:

- Direct `codex exec` subprocess spawning with isolated per-job `CODEX_HOME` directories.
- Binding the embedded Web Canvas UI on `127.0.0.1:<port>` for seamless access from a host Windows browser.

## Authentication (`imgforge auth`)

`imgforge` never requires `OPENAI_API_KEY` and never symlinks or mutates a user-global `~/.codex/auth.json`.

Provision tool-local ChatGPT Plus OAuth credentials before running generation commands:

```bash
./target/release/imgforge auth login           # Interactive browser OAuth (add --device-auth for headless flows)
./target/release/imgforge auth status          # Verify local auth.json integrity and login readiness
```

Credentials are stored exclusively at `$XDG_DATA_HOME/imgforge/codex/auth.json` (default `~/.local/share/imgforge/codex/auth.json`) with `0600` file permissions (`0700` on parent directories). During each `CodexNativeBackend` invocation, `imgforge` materializes a private `0600` copy inside a fresh temporary `CODEX_HOME` directory and cleans it up when the run completes.

## License

`imgforge` is licensed under the [MIT License](LICENSE).
