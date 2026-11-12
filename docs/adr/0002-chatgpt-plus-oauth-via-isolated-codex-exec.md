# 0002. ChatGPT Plus OAuth via Isolated Codex Exec as Sole Built-in Image Backend

- **Status**: accepted

`imgforge` defines an extensible `ImageBackend` trait, but ships `CodexNativeBackend` (spawning `codex exec` with an isolated per-job `CODEX_HOME` and built-in `image_gen` backed by ChatGPT Plus OAuth) as its sole active backend; `OPENAI_API_KEY` API calls and `chatgpt.com` Chrome CDP automation (`ChatGptCdpBackend`) are excluded from core.

Because Codex `image_gen` provides neither native alpha-mask inpainting nor strict exact-pixel sizing guarantees (occasionally returning scaled square dimensions such as `1254x1254` for `1024x1024`), `imgforge` enforces a `<= 5%` aspect-ratio tolerance (fail-closed otherwise) with host-side Lanczos resampling, and unifies ROI-crop and painted-mask edits into a single host-blended `PatchEdit` pipeline. This eliminates per-image API billing and browser-DOM/Turnstile fragility at the cost of coupling to `codex-cli` output conventions.
