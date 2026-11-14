import { defineConfig } from "vitepress";

const rawBase = process.env.VITEPRESS_BASE ?? "/imgforge/";
const base = rawBase.endsWith("/") ? rawBase : `${rawBase}/`;

export default defineConfig({
  title: "imgforge",
  description: "Standalone Visual Consistency Engine",
  base,
  cleanUrls: true,
  lastUpdated: true,
  sitemap: {
    hostname: "https://hartorn-inc.github.io/imgforge/",
  },
  head: [
    ["link", { rel: "icon", type: "image/svg+xml", href: `${base}favicon.svg` }],
  ],
  themeConfig: {
    logo: "/logo-placeholder.svg",
    siteTitle: "imgforge",
    nav: [
      { text: "Home", link: "/" },
      { text: "Philosophy", link: "/philosophy" },
      { text: "How It Works", link: "/architecture" },
      { text: "Getting Started", link: "/getting-started" },
      { text: "Reference", link: "/reference" },
      {
        text: "Specs",
        items: [
          {
            text: "Architecture & Backend",
            link: "/implementation/architecture-and-backend",
          },
          {
            text: "Domain & ReferencePacks",
            link: "/implementation/domain-and-reference-packs",
          },
          {
            text: "Imaging, Color QA & Export",
            link: "/implementation/imaging-color-qa-and-export",
          },
          {
            text: "ADR 0001: Rust Core & Web Canvas",
            link: "/adr/0001-rust-core-with-embedded-web-canvas",
          },
          {
            text: "ADR 0002: Isolated Codex Exec OAuth",
            link: "/adr/0002-chatgpt-plus-oauth-via-isolated-codex-exec",
          },
          {
            text: "ADR 0003: Style-Scoped ReferencePacks",
            link: "/adr/0003-style-scoped-reference-packs",
          },
          {
            text: "ADR 0004: Deterministic PromptCompiler",
            link: "/adr/0004-deterministic-prompt-compiler-over-double-llm",
          },
          {
            text: "ADR 0005: Calibration & Color QA",
            link: "/adr/0005-host-composited-calibration-and-fail-closed-color-qa",
          },
        ],
      },
    ],
    sidebar: [
      {
        text: "Documentation",
        items: [
          { text: "Home", link: "/" },
          { text: "Philosophy", link: "/philosophy" },
          { text: "How It Works", link: "/architecture" },
          { text: "Getting Started", link: "/getting-started" },
          { text: "Reference", link: "/reference" },
        ],
      },
      {
        text: "Implementation Docs",
        items: [
          {
            text: "Architecture & Backend",
            link: "/implementation/architecture-and-backend",
          },
          {
            text: "Domain & ReferencePacks",
            link: "/implementation/domain-and-reference-packs",
          },
          {
            text: "Imaging, Color QA & Export",
            link: "/implementation/imaging-color-qa-and-export",
          },
        ],
      },
      {
        text: "Architecture Decisions (ADRs)",
        items: [
          {
            text: "0001 · Rust Core & Web Canvas",
            link: "/adr/0001-rust-core-with-embedded-web-canvas",
          },
          {
            text: "0002 · Isolated Codex Exec OAuth",
            link: "/adr/0002-chatgpt-plus-oauth-via-isolated-codex-exec",
          },
          {
            text: "0003 · Style-Scoped ReferencePacks",
            link: "/adr/0003-style-scoped-reference-packs",
          },
          {
            text: "0004 · Deterministic PromptCompiler",
            link: "/adr/0004-deterministic-prompt-compiler-over-double-llm",
          },
          {
            text: "0005 · Host Calibration & Color QA",
            link: "/adr/0005-host-composited-calibration-and-fail-closed-color-qa",
          },
        ],
      },
    ],
    socialLinks: [
      { icon: "github", link: "https://github.com/Hartorn-Inc/imgforge" },
    ],
    search: {
      provider: "local",
    },
    editLink: {
      pattern: "https://github.com/Hartorn-Inc/imgforge/edit/main/docs/:path",
      text: "Edit this page on GitHub",
    },
    footer: {
      message: "Released under the MIT License.",
      copyright: "Copyright © 2026 Hartorn-Inc",
    },
  },
});
