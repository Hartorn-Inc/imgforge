# 0002. ChatGPT Plus OAuth via Isolated Codex Exec as Sole Built-in Image Backend

- **Status**: accepted

`imgforge` defines an extensible `ImageBackend` trait, but ships `CodexNativeBackend` (spawning `codex exec` with an isolated per-job `CODEX_HOME` and built-in `image_gen` backed by ChatGPT Plus OAuth) as its sole active backend; `OPENAI_API_KEY` API calls and `chatgpt.com` Chrome CDP automation (`ChatGptCdpBackend`) are excluded from core.

Credentials are stored exclusively in a tool-local location managed by `imgforge auth` at `$XDG_DATA_HOME/imgforge/codex/auth.json` (falling back to `~/.local/share/imgforge/codex/auth.json` when `XDG_DATA_HOME` is unset) with strict `0600` file permissions (`0700` on parent directories), rather than symlinking or reading a global `~/.codex/auth.json`. For each `codex exec` invocation, `CodexNativeBackend` materializes a private copy of that tool-local `auth.json` (`0600`) inside a fresh temporary `CODEX_HOME` directory and removes it once the run finishes, isolating `imgforge` from user-global Codex CLI state and concurrent sessions.

Because Codex `image_gen` provides neither native alpha-mask inpainting nor strict exact-pixel sizing guarantees (occasionally returning scaled square dimensions such as `1254x1254` for `1024x1024`), `imgforge` enforces a `<= 5%` aspect-ratio tolerance (fail-closed otherwise) with host-side Lanczos resampling, and unifies ROI-crop and painted-mask edits into a single host-blended `PatchEdit` pipeline. This eliminates per-image API billing and browser-DOM/Turnstile fragility at the cost of coupling to `codex-cli` output conventions.
