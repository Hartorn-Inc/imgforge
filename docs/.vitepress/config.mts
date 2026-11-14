import { defineConfig } from "vitepress";

const rawBase = process.env.VITEPRESS_BASE ?? "/imgforge/";
const base = rawBase.endsWith("/") ? rawBase : `${rawBase}/`;
const siteUrl = "https://hartorn-inc.github.io/imgforge/";

const structuredData = {
  "@context": "https://schema.org",
  "@type": "SoftwareSourceCode",
  name: "imgforge",
  description:
    "Standalone visual consistency engine built in Rust with an embedded TypeScript + Preact + Konva Web Canvas UI, style-scoped ReferencePacks, deterministic PromptCompiler, and fail-closed OKLab Color QA.",
  url: siteUrl,
  codeRepository: "https://github.com/Hartorn-Inc/imgforge",
  license: "https://opensource.org/licenses/MIT",
  programmingLanguage: ["Rust", "TypeScript"],
  author: {
    "@type": "Organization",
    name: "Hartorn-Inc",
    url: "https://github.com/Hartorn-Inc",
  },
};

export default defineConfig({
  lang: "en-US",
  title: "imgforge",
  description: "Standalone Visual Consistency Engine",
  base,
  cleanUrls: true,
  lastUpdated: true,
  sitemap: {
    hostname: siteUrl,
  },
  head: [
    ["link", { rel: "icon", type: "image/svg+xml", href: `${base}favicon.svg` }],
    [
      "link",
      {
        rel: "icon",
        type: "image/png",
        sizes: "32x32",
        href: `${base}favicon-32x32.png`,
      },
    ],
    [
      "link",
      {
        rel: "icon",
        type: "image/png",
        sizes: "16x16",
        href: `${base}favicon-16x16.png`,
      },
    ],
    [
      "link",
      {
        rel: "apple-touch-icon",
        sizes: "180x180",
        href: `${base}apple-touch-icon.png`,
      },
    ],
    [
      "link",
      {
        rel: "alternate",
        type: "text/plain",
        href: `${siteUrl}llms.txt`,
        title: "LLM Documentation Index (llms.txt)",
      },
    ],
    [
      "link",
      {
        rel: "alternate",
        type: "text/plain",
        href: `${siteUrl}llms-full.txt`,
        title: "Complete LLM Documentation Corpus (llms-full.txt)",
      },
    ],
    ["meta", { property: "og:site_name", content: "imgforge" }],
    ["meta", { property: "og:type", content: "website" }],
    ["meta", { property: "og:image", content: `${siteUrl}og-image.png` }],
    ["meta", { property: "og:image:width", content: "1200" }],
    ["meta", { property: "og:image:height", content: "630" }],
    [
      "meta",
      {
        property: "og:image:alt",
        content: "imgforge — Standalone Visual Consistency Engine",
      },
    ],
    ["meta", { name: "twitter:card", content: "summary_large_image" }],
    ["meta", { name: "twitter:image", content: `${siteUrl}og-image.png` }],
    [
      "meta",
      {
        name: "keywords",
        content:
          "imgforge, visual consistency engine, ReferencePack, PromptCompiler, SceneSpec, SpatialGuide, PatchEdit, OKLab, DeltaE_2000, Rust, MCP server, Web Canvas UI",
      },
    ],
    [
      "meta",
      { name: "robots", content: "index, follow, max-image-preview:large" },
    ],
    ["meta", { name: "theme-color", content: "#0f172a" }],
    [
      "script",
      { type: "application/ld+json" },
      JSON.stringify(structuredData),
    ],
  ],
  transformHead({ pageData }) {
    const cleanPath = pageData.relativePath
      .replace(/(?:^|\/)index\.md$/, (match) =>
        match.startsWith("/") ? "/" : ""
      )
      .replace(/\.md$/, "");
    const canonicalUrl = `${siteUrl}${cleanPath}`;
    const rawTitle =
      (typeof pageData.frontmatter.title === "string" &&
        pageData.frontmatter.title.trim()) ||
      pageData.title ||
      "imgforge";
    const pageTitle =
      rawTitle === "imgforge" || rawTitle.includes("imgforge")
        ? rawTitle
        : `${rawTitle} | imgforge`;
    const pageDescription =
      (typeof pageData.frontmatter.description === "string" &&
        pageData.frontmatter.description.trim()) ||
      pageData.description ||
      "Standalone Visual Consistency Engine";

    return [
      ["link", { rel: "canonical", href: canonicalUrl }],
      ["meta", { property: "og:title", content: pageTitle }],
      ["meta", { property: "og:description", content: pageDescription }],
      ["meta", { property: "og:url", content: canonicalUrl }],
      ["meta", { property: "og:locale", content: "en_US" }],
      ["meta", { name: "twitter:title", content: pageTitle }],
      ["meta", { name: "twitter:description", content: pageDescription }],
    ];
  },
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
