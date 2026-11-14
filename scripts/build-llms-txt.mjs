#!/usr/bin/env node

import { existsSync, mkdirSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);
const REPO_ROOT = path.resolve(__dirname, "..");

const SITE_URL = "https://hartorn-inc.github.io/imgforge/";
const REPO_URL = "https://github.com/Hartorn-Inc/imgforge";
const LICENSE_URL = "https://opensource.org/licenses/MIT";

const PUBLIC_DIR = path.join(REPO_ROOT, "docs", "public");
const LLMS_TXT_PATH = path.join(PUBLIC_DIR, "llms.txt");
const LLMS_FULL_TXT_PATH = path.join(PUBLIC_DIR, "llms-full.txt");

const TOP_LEVEL_ORDER = [
  "index.md",
  "philosophy.md",
  "architecture.md",
  "getting-started.md",
  "reference.md",
];

function normalizeLf(text) {
  return text.replace(/\r\n/g, "\n").replace(/\r/g, "\n");
}

function stripFrontmatter(markdown) {
  const normalized = normalizeLf(markdown);
  const match = normalized.match(/^---\n[\s\S]*?\n---\n*/);
  const body = match ? normalized.slice(match[0].length) : normalized;
  return body.trim();
}

function listMarkdownFiles(dirPath, preferredOrder = []) {
  if (!existsSync(dirPath)) {
    return [];
  }
  const entries = readdirSync(dirPath, { withFileTypes: true })
    .filter((entry) => entry.isFile() && entry.name.endsWith(".md"))
    .map((entry) => entry.name);

  const rank = new Map(preferredOrder.map((name, idx) => [name, idx]));
  entries.sort((a, b) => {
    const ra = rank.has(a) ? rank.get(a) : Number.MAX_SAFE_INTEGER;
    const rb = rank.has(b) ? rank.get(b) : Number.MAX_SAFE_INTEGER;
    if (ra !== rb) {
      return ra - rb;
    }
    return a.localeCompare(b, "en");
  });

  return entries.map((name) => path.join(dirPath, name));
}

function toDocUrl(relPath) {
  const posixRel = relPath.split(path.sep).join("/");
  if (posixRel === "GLOSSARY.md") {
    return `${REPO_URL}/blob/main/GLOSSARY.md`;
  }
  if (!posixRel.startsWith("docs/")) {
    return `${REPO_URL}/blob/main/${posixRel}`;
  }
  const docRel = posixRel
    .slice("docs/".length)
    .replace(/(?:^|\/)index\.md$/, (m) => (m.startsWith("/") ? "/" : ""))
    .replace(/\.md$/, "");
  return `${SITE_URL}${docRel}`;
}

function buildLlmsTxt() {
  const lines = [
    "# imgforge",
    "",
    "> Standalone, pure-English, domain-agnostic visual consistency engine built as a single Rust binary (`axum`, `rmcp`, `image`, `palette`, `tiny-skia`, `resvg`) with an embedded `TypeScript + Vite + Preact + Konva` Web Canvas SPA (`rust-embed`).",
    "",
    `- Documentation Site: ${SITE_URL}`,
    `- Full Documentation Corpus: ${SITE_URL}llms-full.txt`,
    `- Source Repository: ${REPO_URL}`,
    `- License: MIT (${LICENSE_URL})`,
    "",
    "## Core Architecture & Pipeline",
    "",
    "- **Single Native Binary (`imgforge-core` + `imgforge-cli`):** Built inside the pinned Rust + Node 22 `Dockerfile` via `Makefile` and executed natively on the host for `codex exec` and loopback `127.0.0.1:<port>` Web Canvas UI serving.",
    "- **Decoupled Domain Specifications:** `Subject` (style-neutral physical identity, `height_m`, `PaletteLock`, `ExclusiveSurfaceMap`, `Visibility`), `Style` (`1..=3` exemplars, 5 built-in presets), and `Atmosphere` (light directions, Kelvin temperature, `ExposureFloor`, 4 built-in presets), resolved along `project > user > builtin` (`Subject` has no built-ins) with symlink-free `O_NOFOLLOW` containment.",
    "- **Deterministic `PromptCompiler` & Optional `SceneComposer`:** `PromptCompiler` validates `SceneSpec` invariants in sub-millisecond Rust (depth planes, `<= 3` left-to-right foreground slots, explicit hand bindings, unique `ExclusiveSurfaceMap` slots, `Visibility` compatibility, zero active negation tokens); `SceneComposer` (`codex exec -s read-only --output-schema`) runs only on explicit `compose` / `--compose` prose inputs.",
    "- **Style-Scoped `ReferencePack` & Dynamic `Atlas`:** Scoped per `(Subject, Style)` under `library/subjects/<subject_slug>/packs/<style_slug>/` (`turnaround` -> `poses_expressions` + `inventory_flatlay` for `character`/`creature`, or single `sheet` for `prop`/`location`), bootstrapped optionally from a `Seed`, sliced by foreground bounding-box detection, and dynamically shelf-packed into `1536x1024` `Atlas` pages (`1..=3` focal `Subject`s) on `#252830`.",
    "- **Isolated `CodexNativeBackend`:** Sole built-in `ImageBackend` using tool-local ChatGPT Plus OAuth at `$XDG_DATA_HOME/imgforge/codex/auth.json` (`0700`/`0600`) copied into ephemeral `0700` `TempCodexHome` sandboxes with `OPENAI_API_KEY` stripped, `<= 5%` aspect-ratio error guard, and `FilterType::Lanczos3` resampling.",
    "- **Structured `VisionCritic` & `Lifecycle`:** Read-only VLM inspection (`identity`, `hand_props`, `weapon_geometry`, `style_adherence`, `zero_leaked_text`), strict forward `Lifecycle` (`draft -> candidate -> validated -> canonical -> superseded`) frozen in `library/manifest.lock.json`, and ordered `Visibility` isolation (`public < restricted < internal`).",
    "- **Spatial Guidance, Editing & `Export`:** `GuideRenderer` (`tiny-skia` + `resvg`) deterministically renders `GuideSpec` JSON (`Blockout` and `OverlayGuide`); `WholeImageEdit` and localized `1:1` `PatchEdit` record versioned `Run`s (`vNNN.png` + `vNNN.meta.json`); `Export` derives `vtt-token`, `vtt-map`, `web-webp`, and `grid-2x2` deliverables without creating `Lifecycle` versions.",
    "",
    "## Color QA Invariants (Fail-Closed)",
    "",
    "- **Host-Composited Calibration Strip:** `imgforge` deterministically composites the `#252830` neutral calibration strip (6-step CIE `L*` gray wedge `15, 30, 45, 60, 75, 90` plus `48x48` `PaletteLock` swatches) onto `ReferencePack` sheets on the host and verifies swatch accuracy at `DeltaE_2000 <= 0.5`.",
    "- **Read-Only `ColorQaEngine`:** Evaluates CIE `L*` exposure floors and shadow crush (`L* < 5.0`), OKLab k-means (`K = 6`) `PaletteLock` adherence (`DeltaE_2000`), forbidden hue sectors, untouched-region `1 - M` drift, and Difference-of-Gaussians (`DoG`) yellow-dot sparkle artifacts (`4..=64 px^2`), failing closed (`CoreError::ColorQaFailed`) with diagnostic RGBA heatmaps and zero silent whole-image re-grading.",
    "- **Zero Out-of-ROI Drift in `PatchEdit`:** Extracts a `1:1` square crop with a `16..=32 px` collar, applies 10%-trimmed OKLab Reinhard transfer (`[0.75, 1.35]` clamped gains) from the collar ring, and blends via cosine distance-transform feather and 5-level Laplacian pyramids—enforcing `0.0` `DeltaE_2000` drift outside the composite footprint.",
    "- **Opt-In Grading Only in `Export`:** Global damped CIE `L*` CDF matching (`0.25..=0.45`) and Adobe `.cube` 3D LUTs execute exclusively when requested via `imgforge export --match-grade`.",
    "",
    "## CLI Commands (`imgforge`)",
    "",
    "- `auth`: Provision, inspect, and clear tool-local ChatGPT Plus OAuth credentials (`login`, `status`, `logout`).",
    "- `compose`: Translate bounded prose (`<= 32 KiB`) into a `PromptCompiler`-validated `SceneSpec` via read-only `SceneComposer`.",
    "- `validate`: Deterministically validate a `SceneSpec`, `Run`, or `ReferencePack` (`draft -> candidate -> validated`).",
    "- `promote`: Promote a `validated` `Run` or `ReferencePack` to immutable `canonical` in `library/manifest.lock.json`.",
    "- `generate`: Compile a `SceneSpec` (or prose via `--compose`) and generate a QA-checked `Run`.",
    "- `run`: Inspect, validate, or promote recorded `Run`s (`runs/<slug>/vNNN.png`).",
    "- `edit`: Execute `WholeImageEdit` (`whole`) or `PatchEdit` (`patch`) with OKLab/`DeltaE_2000` Color QA.",
    "- `guide`: Validate or deterministically render a `SpatialGuide` (`Blockout` or `OverlayGuide`) from `GuideSpec` JSON.",
    "- `export`: Derive `vtt-token`, `vtt-map`, `web-webp`, or `grid-2x2` deliverables with `Visibility` gating and optional `--match-grade`.",
    "- `critic`: Run structured read-only `VisionCritic` binary checklist inspection.",
    "- `library`: Inspect `Subject`, `Style`, `Atmosphere`, and `manifest.lock.json` entries.",
    "- `pack`: Generate, slice, and pack style-scoped `ReferencePack` sheets and dynamic `Atlas` pages.",
    "- `eval`: Run reproducible, resumable strategy evaluation across frozen benchmark cases.",
    "- `ui`: Launch the embedded `TypeScript + Vite + Preact + Konva` Web Canvas SPA on `127.0.0.1:<port>`.",
    "- `mcp`: Start the `rmcp` MCP server over stdio (default) or `--http 127.0.0.1:<port>`.",
    "- `status`: Inspect resolved `imgforge.toml` workspace configuration and tool-local auth readiness.",
    "",
    "## MCP Server (`8` Tools, `4` Resources)",
    "",
    "- **MCP Tools:** `compose`, `generate`, `pack`, `edit`, `guide`, `export`, `validate`, `promote` (all path inputs enforce traversal/NUL rejection, symlink rejection, and workspace containment).",
    "- **MCP Resources:** `imgforge://manifest`, `imgforge://subjects`, `imgforge://styles-atmospheres`, `imgforge://runs`.",
    "",
    "## Documentation Pages",
    "",
    `- [Home](${SITE_URL}): Overview, install options, and core capabilities.`,
    `- [Philosophy](${SITE_URL}philosophy): Design rationale for decoupling Subject, Style, and Atmosphere, deterministic PromptCompiler, fail-closed Color QA, and isolated OAuth.`,
    `- [How It Works](${SITE_URL}architecture): End-to-end architecture diagrams, Lifecycle state machine, and subsystem mapping.`,
    `- [Getting Started](${SITE_URL}getting-started): Installation, imgforge auth, imgforge.toml setup, CLI quickstart, MCP server, and Web Canvas UI.`,
    `- [Reference](${SITE_URL}reference): Canonical schemas, Visibility ranks, Lifecycle transitions, Export profiles, and Glossary index.`,
    "",
    "## Implementation Specs",
    "",
    `- [Architecture, Workspace, Auth, and Backend Boundaries](${SITE_URL}implementation/architecture-and-backend): Workspace resolution, TempCodexHome, CodexNativeBackend, PromptCompiler, SceneComposer, VisionCritic, CLI, MCP, and axum server.`,
    `- [Domain Model, Library Precedence, Lifecycle, and ReferencePacks](${SITE_URL}implementation/domain-and-reference-packs): Subject/Style/Atmosphere schemas, LibraryRegistry precedence, Lifecycle, #252830 calibration, bbox slicing, and dynamic Atlas packing.`,
    `- [Imaging, Color QA, SpatialGuides, Editing, and Export](${SITE_URL}implementation/imaging-color-qa-and-export): ColorQaEngine detectors, WholeImageEdit vs. PatchEdit Laplacian blending, GuideRenderer, and Export profiles.`,
    "",
    "## Architecture Decision Records (ADRs)",
    "",
    `- [ADR 0001: Rust Core Binary with Embedded Localhost Web Canvas UI](${SITE_URL}adr/0001-rust-core-with-embedded-web-canvas)`,
    `- [ADR 0002: ChatGPT Plus OAuth via Isolated Codex Exec](${SITE_URL}adr/0002-chatgpt-plus-oauth-via-isolated-codex-exec)`,
    `- [ADR 0003: Style-Scoped Reference Packs Under Style-Neutral Subject Identity](${SITE_URL}adr/0003-style-scoped-reference-packs)`,
    `- [ADR 0004: Deterministic Rust PromptCompiler with Optional Prose-Only SceneComposer](${SITE_URL}adr/0004-deterministic-prompt-compiler-over-double-llm)`,
    `- [ADR 0005: Host-Composited Calibration Strips and Fail-Closed Whole-Image Color QA](${SITE_URL}adr/0005-host-composited-calibration-and-fail-closed-color-qa)`,
    "",
    "## Full Corpus",
    "",
    `- [llms-full.txt](${SITE_URL}llms-full.txt): Complete concatenated Markdown documentation corpus (docs, implementation specs, ADRs, and GLOSSARY.md) with frontmatter stripped.`,
    "",
  ];

  return `${lines.join("\n")}`;
}

function buildLlmsFullTxt() {
  const docsTop = listMarkdownFiles(path.join(REPO_ROOT, "docs"), TOP_LEVEL_ORDER);
  const docsImpl = listMarkdownFiles(path.join(REPO_ROOT, "docs", "implementation"));
  const docsAdr = listMarkdownFiles(path.join(REPO_ROOT, "docs", "adr"));
  const glossaryPath = path.join(REPO_ROOT, "GLOSSARY.md");

  const sources = [...docsTop, ...docsImpl, ...docsAdr];
  if (existsSync(glossaryPath)) {
    sources.push(glossaryPath);
  }

  const sections = [];
  for (const absPath of sources) {
    const relPath = path.relative(REPO_ROOT, absPath).split(path.sep).join("/");
    const raw = readFileSync(absPath, "utf8");
    const stripped = stripFrontmatter(raw);
    const url = toDocUrl(relPath);
    sections.push(
      [
        "================================================================================",
        `FILE: ${relPath}`,
        `URL: ${url}`,
        "================================================================================",
        "",
        stripped,
      ].join("\n")
    );
  }

  return `${sections.join("\n\n")}\n`;
}

function syncOrCheck(targetPath, expectedContent, checkOnly) {
  const relPath = path.relative(REPO_ROOT, targetPath).split(path.sep).join("/");
  if (checkOnly) {
    if (!existsSync(targetPath)) {
      throw new Error(`Missing generated file: ${relPath}. Run: node scripts/build-llms-txt.mjs`);
    }
    const actualContent = normalizeLf(readFileSync(targetPath, "utf8"));
    if (actualContent !== expectedContent) {
      throw new Error(`Stale generated file: ${relPath}. Run: node scripts/build-llms-txt.mjs`);
    }
    console.log(`[llms-txt] checked ${relPath} (${Buffer.byteLength(expectedContent, "utf8")} bytes)`);
    return;
  }

  mkdirSync(path.dirname(targetPath), { recursive: true });
  writeFileSync(targetPath, expectedContent, "utf8");
  console.log(`[llms-txt] wrote ${relPath} (${Buffer.byteLength(expectedContent, "utf8")} bytes)`);
}

function main() {
  const args = new Set(process.argv.slice(2));
  const checkOnly = args.has("--check");

  const llmsTxt = buildLlmsTxt();
  const llmsFullTxt = buildLlmsFullTxt();

  syncOrCheck(LLMS_TXT_PATH, llmsTxt, checkOnly);
  syncOrCheck(LLMS_FULL_TXT_PATH, llmsFullTxt, checkOnly);
}

try {
  main();
} catch (err) {
  console.error(`[llms-txt] ERROR: ${err.message}`);
  process.exit(1);
}
