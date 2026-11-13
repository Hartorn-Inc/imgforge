# Architecture, Workspace, Auth, and Backend Boundaries

Companion implementation reference for workspace configuration, tool-local ChatGPT Plus OAuth isolation, `CodexNativeBackend`, `PromptCompiler`, `SceneComposer`, `VisionCritic`, and the CLI/MCP/Web Canvas surfaces ([ADR 0001](../adr/0001-rust-core-with-embedded-web-canvas.md), [ADR 0002](../adr/0002-chatgpt-plus-oauth-via-isolated-codex-exec.md), [ADR 0004](../adr/0004-deterministic-prompt-compiler-over-double-llm.md)).

## Mode & Boundary Comparison

| Surface / Stage | Invocation / Transport | Credentials & Sandbox | Output / Failure Mode | Source |
|---|---|---|---|---|
| Workspace Discovery | `--workspace` > `IMGFORGE_WORKSPACE` > CWD ancestors | Read-only TOML (`deny_unknown_fields`) | Fails closed on invalid explicit override; no CWD fallback | `crates/imgforge-core/src/config.rs` |
| `AuthManager` / `TempCodexHome` | `imgforge auth login\|status\|logout` | `$XDG_DATA_HOME/imgforge/codex/auth.json` (`0700`/`0600`), clears `OPENAI_API_KEY` | Per-invocation RAII `0700` temp `CODEX_HOME` deleted on `Drop` | `crates/imgforge-core/src/auth.rs` |
| `PromptCompiler` | Deterministic Rust (`SceneSpec` -> `CompiledPrompt`) | Zero subprocesses, zero network | Sub-ms validation; fails closed on negations or `Visibility` leaks | `crates/imgforge-core/src/prompt_compiler.rs` |
| `SceneComposer` | Optional (`compose` / `--compose`) | `codex exec -s read-only --ephemeral --output-schema` | Untrusted prose delimiters; validates output via `PromptCompiler` | `crates/imgforge-core/src/composer.rs` |
| `CodexNativeBackend` | `ImageBackend` async trait (`generate`, `pack`, `edit`) | `codex exec --ephemeral --json` in `TempCodexHome` | Enforces `<= 5%` aspect-ratio tolerance + `Lanczos3` resampling | `crates/imgforge-core/src/backend.rs` |
| `VisionCritic` | Read-only VLM inspection (`validate`, `critic`) | `codex exec -s read-only --ephemeral --output-schema` | Strict `CriticReport` 1:1 trait completeness; fail-closed | `crates/imgforge-core/src/critic.rs` |
| CLI (`imgforge`) | `clap` binary entrypoint | Optional workspace for `auth`/`ui`/`validate`/`status` | Delegates to `imgforge-core` and `mcp`/`server` | `crates/imgforge-cli/src/main.rs` |
| MCP (`ImgforgeMcpServer`) | `rmcp` stdio (default) or `--http 127.0.0.1:<port>` | Strict workspace containment, zero symlinks, Host/Origin checks | Exactly 8 tools and 4 `imgforge://*` resources | `crates/imgforge-cli/src/mcp.rs` |
| Embedded Web Canvas UI | `axum` on `127.0.0.1:<port>` (`rust-embed`) | Strict loopback Host allowlist + CSP (`default-src 'self'`) | Serves SPA + `GET /api/health`; JSON 404 on unknown `/api/*` | `crates/imgforge-cli/src/server.rs` |

---

## 1. Workspace Discovery (`crates/imgforge-core/src/config.rs`)

`resolve_workspace` locates `imgforge.toml` (`schema_version = 1`, `#[serde(deny_unknown_fields)]`) in strict order:

1. `--workspace <PATH>` (`WorkspaceResolutionSource::CliFlag`)
2. Non-empty `IMGFORGE_WORKSPACE` (`WorkspaceResolutionSource::EnvVar`)
3. Nearest `imgforge.toml` walking `cwd.ancestors()` (`WorkspaceResolutionSource::AncestorsFromCwd`)

If an explicit tier (`--workspace` or `IMGFORGE_WORKSPACE`) points to a missing directory or malformed `imgforge.toml`, resolution fails closed with `CoreError::WorkspaceInvalid` or `CoreError::ConfigInvalid` without falling back to CWD. `WorkspacePathsConfig` (`library_dir`, `runs_dir`, `exports_dir`) rejects absolute paths and `..` traversal. `BackendConfig` intentionally omits any binary path key so untrusted workspace manifests cannot override `--codex-bin` / `IMGFORGE_CODEX_BIN`.

## 2. Tool-Local OAuth & `TempCodexHome` (`crates/imgforge-core/src/auth.rs`)

`resolve_codex_auth_dir` stores credentials exclusively under `$XDG_DATA_HOME/imgforge/codex/auth.json` (when `$XDG_DATA_HOME` is non-empty and absolute; otherwise `$HOME/.local/share/imgforge/codex/auth.json`).

- `AuthManager::login` creates the auth directory at `0700`, spawns `codex login [--device-auth]` with `CODEX_HOME` pointed at that directory and `OPENAI_API_KEY` removed, verifies `auth.json` is a non-symlink regular file, and enforces `0600` permissions.
- `TempCodexHome::create` validates `0700` dir and `0600` non-symlink `auth.json` (`O_NOFOLLOW`), allocates a private `0700` `TempDir`, writes private `0600` copies of `auth.json` and minimal `config.toml` (`O_CREAT | O_EXCL`), and removes the directory automatically on `Drop`.

## 3. `ImageBackend` & `CodexNativeBackend` (`crates/imgforge-core/src/backend.rs`)

`ImageBackend` is the async testing seam (`name()`, `generate(&ImageGenerationRequest) -> ImageGenerationOutput`) shared across `generate`, `pack`, and `edit`.

`CodexNativeBackend::generate` validates target pixel bounds (`checked_pixel_count`), verifies every `-i` reference PNG via `read_regular_file_nofollow` and `decode_png_checked`, materializes a `TempCodexHome`, builds `[RENDER_TARGET]` instructions (`build_generation_prompt`), and spawns `codex exec --ephemeral --json [-i <FILE>...] -- <PROMPT>` via argument arrays with `OPENAI_API_KEY` removed. It extracts a path-safe `thread_id` (`[A-Za-z0-9_-]+`) from JSONL stdout, locates the generated PNG under `$CODEX_HOME/generated_images/<thread_id>/` (or newest regular `.png` fallback, rejecting symlinks), verifies the 8-byte PNG signature, rejects aspect-ratio deviation exceeding `5%` (`MAX_ASPECT_RATIO_ERROR = 0.05`) fail-closed, and resamples via `FilterType::Lanczos3` to exact `width x height`.

## 4. `PromptCompiler` & `SceneComposer` (`prompt_compiler.rs`, `composer.rs`)

- **`PromptCompiler` (`crates/imgforge-core/src/prompt_compiler.rs`):** Validates `SceneSpec` (`schema_version == 1`, non-empty identifiers without control characters, finite non-negative `depth_bands` with `far_meters > near_meters`, `<= 3` `foreground_slots` strictly increasing left-to-right by `position_x_ratio`, positive `height_meters`, explicit `left_hand`/`right_hand` `HandBinding`s, non-empty unique `ExclusiveSurfaceMap` slots with `#RRGGBB` hex, `Visibility` compatibility against resolved `Subject` cards, and zero active negation tokens/phrases). Compiles deterministic `[SCENE]`, `[CAMERA]`, `[DEPTH_BANDS]`, `[FOREGROUND_SLOTS_LEFT_TO_RIGHT]`, `[STYLE]`, `[ATMOSPHERE]`, and `[OUTPUT]` sections, collapsing whitespace in prose fields to prevent section-header spoofing.
- **`SceneComposer` (`crates/imgforge-core/src/composer.rs`):** Invoked only for unstructured prose (`compose` or `--compose`). Bounds prose to `32 KiB` (`MAX_UNTRUSTED_PROSE_BYTES`), neutralizes `<<<`/`>>>` delimiter breakouts inside `<<<UNTRUSTED_SCENE_PROSE>>>` ... `<<<END_UNTRUSTED_SCENE_PROSE>>>`, runs `codex exec -s read-only --ephemeral --output-schema` with `SCENE_SPEC_OUTPUT_SCHEMA` (`additionalProperties: false`), deserializes with `#[serde(deny_unknown_fields)]`, and validates through `PromptCompiler::compile`.

## 5. `VisionCritic` (`crates/imgforge-core/src/critic.rs`)

`VisionCritic::evaluate` inspects a target PNG (`-i <target>`) against optional reference anchors (`-i <ref>...`) with `codex exec -s read-only --ephemeral --output-schema` (`CRITIC_REPORT_OUTPUT_SCHEMA`). Checklist entries (`TraitCheckInput`) cover five canonical `TraitCategory` values (`identity`, `hand_props`, `weapon_geometry`, `style_adherence`, `zero_leaked_text`) and are wrapped in `<<<TRAIT_CHECKLIST>>>` delimiters with control characters stripped. `validate_critic_report_for_traits` verifies 1:1 trait ID and category completeness and requires `report.passed == report.checks.iter().all(|c| c.passed)`.

## 6. CLI, MCP, and Embedded Web Canvas Server (`main.rs`, `mcp.rs`, `server.rs`)

- **CLI (`crates/imgforge-cli/src/main.rs`):** Exposes `auth`, `compose`, `validate`, `promote`, `generate`, `run`, `edit`, `guide`, `export`, `critic`, `library`, `pack`, `ui`, `mcp`, and `status`. Commands that do not mutate a workspace (`auth`, `ui`, `compose`, `guide`, `critic`, `validate <SCENE_SPEC>`, `library list/show`) operate cleanly even when no `imgforge.toml` is present.
- **MCP (`crates/imgforge-cli/src/mcp.rs`):** `ImgforgeMcpServer` uses `rmcp` (`ToolRouter` + `ServerHandler`) over stdio (default) or `--http 127.0.0.1:<port>` (`/mcp` with `enforce_origin_validation()` and loopback-only binding). Exposes 8 tools (`compose`, `generate`, `pack`, `edit`, `guide`, `export`, `validate`, `promote`) and 4 resources (`imgforge://manifest`, `imgforge://subjects`, `imgforge://styles-atmospheres`, `imgforge://runs`). Every MCP path argument passes `validate_no_traversal_or_nul`, `symlink_metadata`, and canonical prefix containment inside the resolved workspace root, while downstream `imgforge-core` file reads enforce `O_NOFOLLOW`.
- **Web Canvas Server (`crates/imgforge-cli/src/server.rs`):** `serve_ui` binds strictly to `127.0.0.1:<port>`, embeds `frontend/dist` via `rust-embed`, validates `Host` (`127.0.0.1`, `localhost`, or `[::1]`), attaches strict `Content-Security-Policy` and `X-Content-Type-Options: nosniff` headers, serves `GET /api/health` (`status`, `version`, `backend`, `authProvisioned`) alongside the Phase 5 REST/WS endpoints (`/api/workspace`, `/api/assets/*`, `/api/styles`, `/api/atmospheres`, `/api/subjects/*/palette-lock`, `/api/guide/*`, `/api/packs/*`, `/api/edit/patch`, `/api/runs/*`, `/api/export`, `/api/ws`), and returns structured `404` JSON for unknown `/api/*` routes. (Phase 6 adds the `imgforge eval` CLI subcommand in `crates/imgforge-cli/src/main.rs`.)

---

## Data Flow & Extension Checklist

```text
  Prose Input ──(optional)──> SceneComposer (codex exec -s read-only --output-schema)
                                    │
                                    ▼
  Structured SceneSpec ─────> PromptCompiler (Rust: negations, geometry, Visibility)
                                    │
                                    ▼
  CompiledPrompt + Refs ────> CodexNativeBackend (TempCodexHome -> codex exec --json)
                                    │
                                    ▼
  8-byte PNG + <=5% AR ─────> Lanczos3 WxH ──> ColorQaEngine ──> RunStore (vNNN.png/.meta.json)
```

**Adding a new `ImageBackend` or MCP capability:**

1. Implement `ImageBackend` in `crates/imgforge-core/src/backend.rs` and keep `CodexNativeBackend` isolated behind `TempCodexHome`.
2. Wire any new `BackendKind` variant in `crates/imgforge-core/src/config.rs` with `#[serde(deny_unknown_fields)]`.
3. Enforce `validate_no_traversal_or_nul` and workspace containment helpers in `crates/imgforge-cli/src/mcp.rs` for any new path parameter before calling `imgforge-core`.
