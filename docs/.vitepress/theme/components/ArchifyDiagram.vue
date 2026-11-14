<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { useData, withBase } from "vitepress";

interface Props {
  slug: string;
  title?: string;
  caption?: string;
  height?: string;
}

const props = withDefaults(defineProps<Props>(), {
  title: "",
  caption: "",
  height: "560px",
});

const { isDark } = useData();

const mode = ref<"embedded" | "interactive">("embedded");
const isLoading = ref(true);
const hasError = ref(false);
const errorDetail = ref("");
const isFullscreen = ref(false);
const reloadSeed = ref(0);
const containerRef = ref<HTMLElement | null>(null);
const iframeRef = ref<HTMLIFrameElement | null>(null);

const resolvedTheme = computed<"dark" | "light">(() =>
  isDark.value ? "dark" : "light"
);

const standalonePath = computed(() =>
  withBase(`/diagrams/${props.slug}.html`)
);

const standaloneUrl = computed(
  () => `${standalonePath.value}?theme=${resolvedTheme.value}`
);

// Keep initial URL reactive to mode and reloadSeed; theme switches sync in-place when loaded
// so pan/zoom state in Interactive mode is preserved.
const committedTheme = ref<"dark" | "light">(resolvedTheme.value);

const iframeSrc = computed(() => {
  const params = new URLSearchParams();
  if (mode.value === "embedded") {
    params.set("embed", "1");
  }
  params.set("theme", committedTheme.value);
  if (reloadSeed.value > 0) {
    params.set("_r", String(reloadSeed.value));
  }
  return `${standalonePath.value}?${params.toString()}`;
});

const frameStyle = computed(() => ({
  height: isFullscreen.value ? "calc(100vh - 54px)" : props.height,
}));

function syncIframeThemeInPlace(nextTheme: "dark" | "light"): boolean {
  try {
    const doc = iframeRef.value?.contentDocument;
    if (!doc?.documentElement) {
      return false;
    }
    doc.documentElement.setAttribute("data-theme", nextTheme);
    return true;
  } catch {
    return false;
  }
}

watch(resolvedTheme, (nextTheme) => {
  if (!isLoading.value && !hasError.value && syncIframeThemeInPlace(nextTheme)) {
    return;
  }
  committedTheme.value = nextTheme;
  isLoading.value = true;
  hasError.value = false;
});

watch(mode, () => {
  committedTheme.value = resolvedTheme.value;
  isLoading.value = true;
  hasError.value = false;
  errorDetail.value = "";
});

function setMode(nextMode: "embedded" | "interactive"): void {
  if (mode.value !== nextMode) {
    mode.value = nextMode;
  }
}

async function verifyAssetAvailable(): Promise<boolean> {
  if (typeof window === "undefined" || typeof fetch === "undefined") {
    return true;
  }
  try {
    const response = await fetch(standalonePath.value, { method: "HEAD" });
    if (!response.ok) {
      errorDetail.value = `HTTP ${response.status} while loading ${props.slug}.html`;
      return false;
    }
    return true;
  } catch {
    return true;
  }
}

async function handleFrameLoad(): Promise<void> {
  try {
    const doc = iframeRef.value?.contentDocument;
    if (doc) {
      const hasSvg = Boolean(doc.querySelector("svg"));
      const text = doc.body?.innerText ?? "";
      if (!hasSvg && /404|not found/i.test(text)) {
        hasError.value = true;
        errorDetail.value = `Diagram artifact "${props.slug}" could not be found.`;
        isLoading.value = false;
        return;
      }
      doc.documentElement.setAttribute("data-theme", resolvedTheme.value);
    } else {
      const ok = await verifyAssetAvailable();
      if (!ok) {
        hasError.value = true;
        isLoading.value = false;
        return;
      }
    }
  } catch {
    // Cross-origin guards or detached frames fall back cleanly.
  }
  hasError.value = false;
  isLoading.value = false;
}

function handleFrameError(): void {
  isLoading.value = false;
  hasError.value = true;
  errorDetail.value = `Failed to load diagram "${props.slug}".`;
}

function retryLoad(): void {
  committedTheme.value = resolvedTheme.value;
  isLoading.value = true;
  hasError.value = false;
  errorDetail.value = "";
  reloadSeed.value += 1;
}

async function toggleFullscreen(): Promise<void> {
  const el = containerRef.value;
  if (!el) return;

  if (typeof document !== "undefined" && document.fullscreenElement) {
    try {
      await document.exitFullscreen();
      isFullscreen.value = false;
      return;
    } catch {
      isFullscreen.value = false;
      return;
    }
  }

  if (typeof el.requestFullscreen === "function") {
    try {
      await el.requestFullscreen();
      isFullscreen.value = true;
      return;
    } catch {
      // Fallback to CSS overlay fullscreen if browser blocks native Fullscreen API.
    }
  }
  isFullscreen.value = !isFullscreen.value;
}

function onFullscreenChange(): void {
  if (typeof document === "undefined") return;
  isFullscreen.value = document.fullscreenElement === containerRef.value;
}

function onKeyDown(event: KeyboardEvent): void {
  if (event.key === "Escape" && isFullscreen.value) {
    isFullscreen.value = false;
  }
}

onMounted(() => {
  if (typeof document !== "undefined") {
    document.addEventListener("fullscreenchange", onFullscreenChange);
    document.addEventListener("keydown", onKeyDown);
  }
});

onBeforeUnmount(() => {
  if (typeof document !== "undefined") {
    document.removeEventListener("fullscreenchange", onFullscreenChange);
    document.removeEventListener("keydown", onKeyDown);
  }
});
</script>

<template>
  <figure
    ref="containerRef"
    class="archify-card"
    :class="{ 'is-fullscreen': isFullscreen }"
  >
    <div class="archify-toolbar">
      <div class="archify-title-group">
        <span class="archify-badge" aria-hidden="true">Archify</span>
        <span v-if="title" class="archify-title">{{ title }}</span>
      </div>
      <div class="archify-actions">
        <div
          class="archify-mode-toggle"
          role="group"
          aria-label="Diagram display mode"
        >
          <button
            type="button"
            class="archify-btn"
            :class="{ 'is-active': mode === 'embedded' }"
            :aria-pressed="mode === 'embedded'"
            @click="setMode('embedded')"
          >
            Embedded
          </button>
          <button
            type="button"
            class="archify-btn"
            :class="{ 'is-active': mode === 'interactive' }"
            :aria-pressed="mode === 'interactive'"
            @click="setMode('interactive')"
          >
            Interactive
          </button>
        </div>
        <a
          class="archify-btn archify-link"
          :href="standaloneUrl"
          target="_blank"
          rel="noopener noreferrer"
        >
          Open Standalone
        </a>
        <button
          type="button"
          class="archify-btn"
          :aria-pressed="isFullscreen"
          @click="toggleFullscreen"
        >
          {{ isFullscreen ? "Exit Fullscreen" : "Fullscreen" }}
        </button>
      </div>
    </div>

    <div class="archify-stage" :style="frameStyle">
      <div v-if="isLoading && !hasError" class="archify-overlay" role="status">
        <div class="archify-spinner" aria-hidden="true"></div>
        <span>Loading {{ title || slug }} diagram…</span>
      </div>

      <div v-if="hasError" class="archify-overlay archify-error" role="alert">
        <p class="archify-error-title">Unable to render diagram</p>
        <p class="archify-error-detail">
          {{ errorDetail || `Could not load ${slug}.html.` }}
        </p>
        <div class="archify-error-actions">
          <button type="button" class="archify-btn is-active" @click="retryLoad">
            Retry
          </button>
          <a
            class="archify-btn archify-link"
            :href="standaloneUrl"
            target="_blank"
            rel="noopener noreferrer"
          >
            Open Standalone
          </a>
        </div>
      </div>

      <iframe
        v-show="!hasError"
        ref="iframeRef"
        class="archify-iframe"
        :src="iframeSrc"
        :title="title || `${slug} diagram`"
        loading="lazy"
        @load="handleFrameLoad"
        @error="handleFrameError"
      />
    </div>

    <figcaption v-if="caption" class="archify-caption">
      {{ caption }}
    </figcaption>
  </figure>
</template>

<style scoped>
.archify-card {
  margin: 1.5rem 0 2rem;
  border: 1px solid var(--vp-c-divider);
  border-radius: 12px;
  overflow: hidden;
  background: var(--vp-c-bg-soft);
  box-shadow: 0 8px 24px -16px rgba(15, 23, 42, 0.35);
}

.archify-card.is-fullscreen {
  position: fixed;
  inset: 0;
  z-index: 100;
  margin: 0;
  border-radius: 0;
  border: none;
  display: flex;
  flex-direction: column;
  background: var(--vp-c-bg);
}

.archify-toolbar {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  justify-content: space-between;
  gap: 0.75rem;
  padding: 0.65rem 0.9rem;
  border-bottom: 1px solid var(--vp-c-divider);
  background: var(--vp-c-bg-alt);
}

.archify-title-group {
  display: flex;
  align-items: center;
  gap: 0.55rem;
  min-width: 0;
}

.archify-badge {
  display: inline-flex;
  align-items: center;
  padding: 0.12rem 0.5rem;
  border-radius: 999px;
  font-size: 0.72rem;
  font-weight: 600;
  letter-spacing: 0.04em;
  text-transform: uppercase;
  color: var(--vp-c-brand-1);
  background: var(--vp-c-brand-soft);
  border: 1px solid color-mix(in srgb, var(--vp-c-brand-2) 35%, transparent);
}

.archify-title {
  font-size: 0.9rem;
  font-weight: 600;
  color: var(--vp-c-text-1);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.archify-actions {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 0.45rem;
}

.archify-mode-toggle {
  display: inline-flex;
  border: 1px solid var(--vp-c-divider);
  border-radius: 8px;
  padding: 2px;
  background: var(--vp-c-bg);
}

.archify-btn {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  padding: 0.28rem 0.65rem;
  border-radius: 6px;
  border: 1px solid var(--vp-c-divider);
  background: var(--vp-c-bg);
  color: var(--vp-c-text-2);
  font-size: 0.78rem;
  font-weight: 500;
  line-height: 1.3;
  cursor: pointer;
  text-decoration: none;
  transition:
    color 0.15s ease,
    border-color 0.15s ease,
    background-color 0.15s ease;
}

.archify-mode-toggle .archify-btn {
  border: none;
  background: transparent;
}

.archify-btn:hover {
  color: var(--vp-c-text-1);
  border-color: var(--vp-c-brand-2);
}

.archify-btn.is-active {
  color: var(--vp-c-brand-1);
  background: var(--vp-c-brand-soft);
  border-color: var(--vp-c-brand-2);
}

.archify-stage {
  position: relative;
  width: 100%;
  background: var(--vp-c-bg);
}

.archify-card.is-fullscreen .archify-stage {
  flex: 1 1 auto;
}

.archify-iframe {
  display: block;
  width: 100%;
  height: 100%;
  border: 0;
}

.archify-overlay {
  position: absolute;
  inset: 0;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 0.65rem;
  padding: 1.5rem;
  text-align: center;
  color: var(--vp-c-text-2);
  background: var(--vp-c-bg-soft);
  z-index: 2;
}

.archify-spinner {
  width: 26px;
  height: 26px;
  border-radius: 50%;
  border: 2.5px solid var(--vp-c-divider);
  border-top-color: var(--vp-c-brand-1);
  animation: archify-spin 0.8s linear infinite;
}

.archify-error-title {
  margin: 0;
  font-weight: 600;
  color: var(--vp-c-text-1);
}

.archify-error-detail {
  margin: 0;
  font-size: 0.86rem;
  color: var(--vp-c-text-2);
}

.archify-error-actions {
  display: flex;
  gap: 0.5rem;
  margin-top: 0.35rem;
}

.archify-caption {
  margin: 0;
  padding: 0.6rem 0.9rem;
  border-top: 1px solid var(--vp-c-divider);
  font-size: 0.84rem;
  color: var(--vp-c-text-2);
  background: var(--vp-c-bg-alt);
}

@keyframes archify-spin {
  to {
    transform: rotate(360deg);
  }
}

@media (prefers-reduced-motion: reduce) {
  .archify-btn {
    transition: none;
  }

  .archify-spinner {
    animation: none;
    border-top-color: var(--vp-c-brand-2);
  }
}
</style>
