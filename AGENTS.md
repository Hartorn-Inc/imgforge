# imgforge

`imgforge` is a standalone, pure-English, domain-agnostic visual consistency engine built as a single Rust binary (`axum`, `rmcp`, `image`, `palette`, `tiny-skia`, `resvg`) with an embedded `TypeScript + Vite + Preact + Konva` Web Canvas SPA (`rust-embed`).

## Domain & Architecture References

- **Canonical Glossary:** `GLOSSARY.md` (use exact glossary terms; never use `_Avoid_` aliases in code, APIs, schemas, or docs).
- **ADRs:** `docs/adr/0001-rust-core-with-embedded-web-canvas.md` through `0005-host-composited-calibration-and-fail-closed-color-qa.md`.
- **Implementation Docs:** `docs/implementation/` (one concise companion doc per major subsystem).
- **Language & Purity:** All code, comments, docstrings, schemas, prompts, tests, and docs must be 100% pure English and domain-agnostic.

## Workflow & Verification

All build, lint, format, and test commands run inside the repo's `Dockerfile` (`rust` + `node`) container via `Makefile` (never probe or install host Rust/Node toolchains):

- Format check: `make fmt-check`
- Lint / typecheck: `make lint`
- Test suite: `make test`
- Full build (SPA + Rust release binary): `make build`

Compiled Linux binaries are executed natively on the WSL2 host for `codex exec` and `127.0.0.1:<port>` Web UI serving.

## Agents

The main agent acts strictly as orchestrator: decompose work, dispatch builders in isolated git worktrees, run parallel reviews, and verify gates without writing application code directly.

### Builder Lanes (Disjoint Ownership)

| Agent | Owns | Verification |
|---|---|---|
| `core-builder` | `Cargo.toml`, `Cargo.lock`, `crates/**`, `src/**`, `tests/**`, `eval/**`, `imgforge.toml` | `make fmt-check && make lint && make test && make build` |
| `frontend-builder` | `frontend/**` | `make fmt-check && make lint && make test && make build` |
| `infra-builder` | `Dockerfile`, `Makefile`, `.github/**`, `docs/**`, `skills/**`, `README.md` | `make fmt-check && make lint && make test && make build` |

### Reviewer & Critic Routing

- **Module boundaries, ADRs, `GLOSSARY.md` changes:** `architecture-reviewer`
- **Rust domain logic, schemas, `Lifecycle`, `ReferencePack`, `PatchEdit`, OKLab/`DeltaE_2000`, `Export`, `eval`:** `domain-reviewer`
- **Embedded SPA (`frontend/**`):** `frontend-reviewer`
- **Build, `Dockerfile`, `Makefile`, CI, dependency manifests:** `infra-reviewer`
- **Auth (`imgforge auth`), isolated `CODEX_HOME`, subprocesses, `Visibility`, `127.0.0.1` server, CI permissions:** `security-reviewer`
- **Prompts, `--output-schema` JSONs, `PromptCompiler`, `SceneComposer`, `VisionCritic`, `skills/**`:** `prompt-critic`
- **Repo-wide standards, pure English, parametrized tests:** `standards-reviewer`
- **Acceptance criteria & spec parity:** `spec-reviewer`

### Plumbing & Escalation (User-Level `~/.cursor/agents/`)

- **Test execution:** `test-runner`
- **Git/worktree/PR/rebase plumbing:** `stack-operator`
- **Hard design or conflicting review escalation:** `senior-architect`

### Orchestration Pacing (Gemini Main Agent)

- Arm a background wake-up loop before multi-slice build/review waves.
- Cap concurrency to at most **1 active builder** and **2 parallel reviewers** per wave.
- Builders work in dedicated git worktrees and commit after each completed slice; never leave uncommitted work on `main`.
