# 0004. Deterministic Rust PromptCompiler with Optional Prose-Only SceneComposer

- **Status**: accepted

`imgforge` splits prompt preparation into two distinct stages: a deterministic, sub-millisecond Rust `PromptCompiler` (`SceneSpec` -> validated positive-only prompt) and an optional `SceneComposer` (`codex exec -s read-only`) invoked only when the caller supplies unstructured prose rather than a `SceneSpec`.

Running an LLM compiler on every request—even when an MCP coding agent or the Web Canvas UI has already produced a structured `SceneSpec`—adds 5–15 seconds of latency per call and introduces double-LLM drift on weapon geometry and slot ordering. Making `SceneSpec` the canonical boundary keeps agentic runs deterministic and fast while still allowing human prose prompts via `imgforge compose` or `--compose`.
