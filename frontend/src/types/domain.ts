/** === DOMAIN CONTRACTS ===
 *
 * PURPOSE:
 * Defines canonical TypeScript interfaces and discriminated unions for the
 * `imgforge` Web Canvas SPA, matching `GLOSSARY.md` entity boundaries.
 *
 * DESIGN RATIONALE ("WHY"):
 * Keeping all UI state models aligned 1:1 with Rust core concepts (`Subject`,
 * `Style`, `Atmosphere`, `ReferencePack`, `PaletteLock`, `PatchEdit`, `Run`,
 * `Lifecycle`, `Visibility`, `Export`) prevents terminology drift across the
 * HTTP/WebSocket boundary. Business logic, OKLab/`DeltaE_2000` math, and
 * `GuideRenderer` rasterization remain strictly in Rust core.
 *
 * DATA FLOW:
 * 1. Backend payloads (`GET /api/health`, future workspace endpoints) decode into
 *    typed domain summaries.
 * 2. Every asynchronous view wraps data in `AsyncViewState<T>` so loading,
 *    empty, error, disabled, and ready UI states are always explicit.
 */

/** Five-state asset progression; `canonical` assets are immutable in `manifest.lock.json`. */
export type LifecycleState = 'draft' | 'candidate' | 'validated' | 'canonical' | 'superseded';

/** Access classification preventing restricted assets from leaking into public `Export` outputs. */
export type Visibility = 'public' | 'restricted' | 'internal';

/** Allowed categories for a style-neutral `Subject`. */
export type SubjectCategory = 'character' | 'creature' | 'prop' | 'location';

/** Single exact unshaded base albedo color entry inside a `PaletteLock`. */
export interface PaletteLockSwatch {
  readonly label: string;
  readonly hex: string;
  readonly surfaceSlot: string;
}

/** Style-neutral `Subject` summary bound to a `PaletteLock`. */
export interface SubjectSummary {
  readonly slug: string;
  readonly name: string;
  readonly category: SubjectCategory;
  readonly heightMeters: number;
  readonly paletteLock: readonly PaletteLockSwatch[];
  readonly visibility: Visibility;
  readonly lifecycle: LifecycleState;
}

/** First-class `Style` rendering specification summary. */
export interface StyleSummary {
  readonly slug: string;
  readonly name: string;
  readonly medium: string;
  readonly exemplarCount: number;
  readonly lifecycle: LifecycleState;
}

/** Lighting and environmental `Atmosphere` specification summary. */
export interface AtmosphereSummary {
  readonly slug: string;
  readonly name: string;
  readonly lightDirection: string;
  readonly colorTemperatureKelvin: number;
  readonly exposureFloor: number;
  readonly maxAmbientTintBlend: number;
  readonly particles: string;
}

/** Style-scoped `ReferencePack` sheet kind. */
export type ReferenceSheetKind =
  'turnaround' | 'poses_expressions' | 'inventory_flatlay' | 'single_sheet';

/** Style-scoped `ReferencePack` metadata for a `(Subject, Style)` pair. */
export interface ReferencePackSummary {
  readonly subjectSlug: string;
  readonly styleSlug: string;
  readonly sheetKind: ReferenceSheetKind;
  readonly sliceCount: number;
  readonly lifecycle: LifecycleState;
}

/** Localized `PatchEdit` extraction strategy (`roi` crop or painted `mask`). */
export type PatchRegionMode = 'roi' | 'mask';

/** Draft configuration for a `1:1` square `PatchEdit` operation. */
export interface PatchEditDraft {
  readonly targetRunId: string | null;
  readonly mode: PatchRegionMode;
  readonly cropSizePx: number;
  readonly originXPx: number;
  readonly originYPx: number;
  readonly blankTargetRegion: boolean;
  readonly collarWidthPx: number;
}

/** Versioned `Run` (`vNNN`) metadata summary for the `Run Inspector` view. */
export interface RunRecord {
  readonly runId: string;
  readonly versionLabel: string;
  readonly lifecycle: LifecycleState;
  readonly visibility: Visibility;
  readonly widthPx: number;
  readonly heightPx: number;
  readonly visionCriticPassed: boolean | null;
  readonly colorQaMaxDeltaE2000: number | null;
  readonly createdAtIso: string;
}

/** Explicit top-level navigation tabs matching the Web Canvas SPA workspace layout. */
export type WorkspaceTab = 'style_atmosphere' | 'spatial_guide' | 'patch_edit' | 'run_inspector';

/** Explicit UI data-view state ensuring loading, empty, error, and disabled are always handled. */
export type AsyncViewState<T> =
  | { readonly status: 'loading'; readonly message: string }
  | { readonly status: 'empty'; readonly title: string; readonly detail: string }
  | { readonly status: 'error'; readonly message: string; readonly retryable: boolean }
  | { readonly status: 'disabled'; readonly reason: string }
  | { readonly status: 'ready'; readonly data: T };

/** Health payload returned by `GET /api/health` on the local `127.0.0.1:<port>` server. */
export interface HealthStatus {
  readonly status: 'ok' | 'degraded';
  readonly version: string;
  readonly backend: string;
  readonly authProvisioned: boolean;
}
