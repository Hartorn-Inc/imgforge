/** === DOMAIN CONTRACTS ===
 *
 * PURPOSE:
 * Defines canonical TypeScript interfaces and discriminated unions for the
 * `imgforge` Web Canvas SPA, matching `GLOSSARY.md` entity boundaries and
 * `crates/imgforge-cli/src/server.rs` REST + WebSocket DTO contracts.
 *
 * DESIGN RATIONALE ("WHY"):
 * Keeping all UI state models aligned 1:1 with Rust core concepts (`Subject`,
 * `Style`, `Atmosphere`, `ReferencePack`, `PaletteLock`, `PatchEdit`, `Run`,
 * `Lifecycle`, `Visibility`, `Export`) prevents terminology drift across the
 * HTTP/WebSocket boundary. Business logic, OKLab/`DeltaE_2000` math, and
 * `GuideRenderer` rasterization remain strictly in Rust core.
 *
 * NOTE ON WIRE CASING:
 * Outer HTTP/WebSocket DTOs from `server.rs` serialize in `camelCase`
 * (`WorkspaceSummaryResponse`, `StyleDto`, `ColorInspectionTelemetry`, etc.),
 * whereas embedded Rust core reports (`ColorQaReport`, `CriticReport`,
 * `ReinhardGainSummary`, `ManifestEntry`) serialize in `snake_case`.
 */

import type { GuideSpec, GuideSpecValidationError, SpatialGuideVariant } from './guideSpec';

/** Five-state asset progression; `canonical` assets are immutable in `manifest.lock.json`. */
export type LifecycleState = 'draft' | 'candidate' | 'validated' | 'canonical' | 'superseded';

/** Access classification preventing restricted assets from leaking into public `Export` outputs. */
export type Visibility = 'public' | 'restricted' | 'internal';

/** Allowed categories for a style-neutral `Subject`. */
export type SubjectCategory = 'character' | 'creature' | 'prop' | 'location';

/** Library precedence tier from `LibraryRegistry` (`project > user > builtin`). */
export type LibrarySource = 'project' | 'user' | 'builtin';

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
  readonly source: LibrarySource;
  readonly asymmetricMarkers: readonly string[];
  readonly positiveWeaponGeometry: readonly string[];
  readonly tags: readonly string[];
}

/** First-class `Style` rendering specification summary. */
export interface StyleSummary {
  readonly slug: string;
  readonly name: string;
  readonly medium: string;
  readonly exemplarCount: number;
  readonly lifecycle: LifecycleState;
  readonly source: LibrarySource;
  readonly linework: string;
  readonly shadingAndTexture: string;
  readonly paletteProfile: string;
  readonly framingAndBorder: string;
  readonly styleGuardrails: readonly string[];
  readonly exemplarUrls: readonly string[];
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
  readonly source: LibrarySource;
  readonly secondaryLightDirection: string;
  readonly minMeanLStar: number;
  readonly minP10LStar: number;
}

/** Style-scoped `ReferencePack` sheet kind. */
export type ReferenceSheetKind =
  'turnaround' | 'poses_expressions' | 'inventory_flatlay' | 'single_sheet';

/** Axis-aligned pixel rectangle (`PixelRect`). */
export interface PixelRect {
  readonly x: number;
  readonly y: number;
  readonly width: number;
  readonly height: number;
}

/** Source indicating how a `ReferencePack` slice bounding box was produced. */
export type SliceSource = 'auto' | 'manual_override';

/** Manual bounding-box override interaction mode (`OverrideMode`). */
export type OverrideMode = 'replace_all' | 'merge_by_id';

/** Manual bounding-box override submitted to `POST /api/packs/{subject}/{style}/reslice`. */
export interface RectOverride {
  readonly id: string;
  readonly rect: PixelRect;
  readonly label?: string | null;
}

/** Generated sheet metadata within a `ReferencePack`. */
export interface SheetRecordDto {
  readonly role: ReferenceSheetKind;
  readonly filename: string;
  readonly imageUrl: string;
  readonly sha256: string;
  readonly width: number;
  readonly height: number;
  readonly bgCoverage: number;
  readonly borderBgCoverage: number;
  readonly foregroundPixels: number;
  readonly stripY: number;
  readonly stripHeight: number;
}

/** Extracted foreground component slice inside a `ReferencePack`. */
export interface SliceRecordDto {
  readonly id: string;
  readonly file: string;
  readonly imageUrl: string;
  readonly rect: PixelRect;
  readonly source: SliceSource;
  readonly label: string | null;
  readonly sha256: string;
}

/** Style-scoped `ReferencePack` metadata for a `(Subject, Style)` pair. */
export interface ReferencePackSummary {
  readonly subjectSlug: string;
  readonly styleSlug: string;
  readonly sheetKind: ReferenceSheetKind;
  readonly sliceCount: number;
  readonly lifecycle: LifecycleState;
  readonly version: number;
  readonly visibility: Visibility;
  readonly sheets: readonly SheetRecordDto[];
  readonly slices: readonly SliceRecordDto[];
  readonly createdAtIso: string;
}

/** Versioned `Run` (`vNNN`) metadata summary for the `Run Inspector` view. */
export interface RunRecord {
  readonly runId: string;
  readonly slug: string;
  readonly version: number;
  readonly versionLabel: string;
  readonly lifecycle: LifecycleState;
  readonly visibility: Visibility;
  readonly widthPx: number;
  readonly heightPx: number;
  readonly visionCriticPassed: boolean | null;
  readonly colorQaMaxDeltaE2000: number | null;
  readonly createdAtIso: string;
  readonly imageUrl: string;
  readonly compiledPrompt: string;
  readonly backendName: string;
  readonly structuralChecks: readonly string[];
}

/** Canonical lock entry kind (`ManifestEntryKind`, `snake_case`). */
export type ManifestEntryKind = 'subject' | 'style' | 'atmosphere' | 'reference_pack' | 'run';

/** Single entry inside `library/manifest.lock.json` (`snake_case` wire schema). */
export interface ManifestEntry {
  readonly entry_kind: ManifestEntryKind;
  readonly key: string;
  readonly version: number;
  readonly version_label: string;
  readonly lifecycle: LifecycleState;
  readonly visibility: Visibility;
  readonly artifact_hashes: Readonly<Record<string, string>>;
  readonly promoted_at_iso: string;
}

/** `ManifestLockDto` returned inside `WorkspaceSummaryResponse`. */
export interface ManifestLockSummary {
  readonly schemaVersion: number;
  readonly canonicalCount: number;
  readonly supersededCount: number;
  readonly canonical: Readonly<Record<string, ManifestEntry>>;
  readonly superseded: readonly ManifestEntry[];
}

/** Full workspace catalog returned by `GET /api/workspace`. */
export interface WorkspaceSummary {
  readonly hasWorkspace: boolean;
  readonly workspaceRoot: string | null;
  readonly workspaceName: string | null;
  readonly authProvisioned: boolean;
  readonly subjects: readonly SubjectSummary[];
  readonly styles: readonly StyleSummary[];
  readonly atmospheres: readonly AtmosphereSummary[];
  readonly referencePacks: readonly ReferencePackSummary[];
  readonly runs: readonly RunRecord[];
  readonly manifestLock: ManifestLockSummary;
}

/** Live workspace mutation/lifecycle event broadcast over `GET /api/ws`. */
export interface WorkspaceEvent {
  readonly eventType: string;
  readonly resourceKind: string;
  readonly id: string;
  readonly lifecycle: LifecycleState | null;
  readonly detail: string;
  readonly timestampIso: string;
}

/** Localized `PatchEdit` extraction strategy (`roi` crop or painted `mask`). */
export type PatchRegionMode = 'roi' | 'mask';

/** Pre-edit preparation mode applied to the `1:1` crop before `ImageBackend` (`snake_case`). */
export type PatchPrepMode = 'crop_only' | 'blank_inside_mask' | 'cue_outline';

/** Draft configuration for a `1:1` square `PatchEdit` operation. */
export interface PatchEditDraft {
  readonly slug: string;
  readonly targetRunKey: string | null;
  readonly targetImagePath: string;
  readonly mode: PatchRegionMode;
  readonly cropSizePx: number;
  readonly originXPx: number;
  readonly originYPx: number;
  readonly prepMode: PatchPrepMode;
  readonly collarWidthPx: number;
  readonly featherPx: number;
  readonly prompt: string;
  readonly visibility: Visibility;
  readonly maskPngBase64: string | null;
  readonly canvasWidthPx: number;
  readonly canvasHeightPx: number;
}

/** Recorded per-channel OKLab Reinhard gain factors applied during `PatchEdit` (`snake_case`). */
export interface ReinhardGainSummary {
  readonly gain_l: number;
  readonly gain_a: number;
  readonly gain_b: number;
  readonly collar_pixel_count: number;
}

/** Nested CIE `L*` exposure metrics (`snake_case`). */
export interface ExposureMetrics {
  readonly mean_l_star: number;
  readonly p10_l_star: number;
  readonly p90_l_star: number;
  readonly clipped_black_ratio: number;
  readonly pixel_count: number;
}

/** Nested `Atmosphere.exposure_floor` thresholds (`snake_case`). */
export interface ExposureFloor {
  readonly min_mean_l_star: number;
  readonly min_p10_l_star: number;
}

/** Exposure evaluation report (`snake_case`). */
export interface ExposureReport {
  readonly passed: boolean;
  readonly metrics: ExposureMetrics;
  readonly floor: ExposureFloor;
  readonly reasons: readonly string[];
}

/** Single OKLab k-means cluster summary (`snake_case`). */
export interface OklabClusterSummary {
  readonly centroid_oklab: readonly [number, number, number];
  readonly centroid_hex: string;
  readonly weight: number;
  readonly pixel_count: number;
  readonly closest_swatch_name: string;
  readonly closest_swatch_hex: string;
  readonly delta_e2000: number;
  readonly lightness_shift: number;
  readonly chroma_shift: number;
}

/** Forbidden hue sector intrusion (`snake_case`). */
export interface ForbiddenHueIntrusion {
  readonly sector_name: string;
  readonly fraction: number;
  readonly pixel_count: number;
}

/** `PaletteLock` adherence report (`snake_case`). */
export interface PaletteAdherenceReport {
  readonly passed: boolean;
  readonly subject_slug: string;
  readonly clusters: readonly OklabClusterSummary[];
  readonly max_swatch_delta_e2000: number;
  readonly weighted_mean_delta_e2000: number;
  readonly forbidden_hue_intrusions: readonly ForbiddenHueIntrusion[];
  readonly reasons: readonly string[];
}

/** Untouched-region (`1 - M`) drift report (`snake_case`). */
export interface UntouchedDriftReport {
  readonly passed: boolean;
  readonly untouched_pixel_count: number;
  readonly mean_delta_e2000: number;
  readonly p95_delta_e2000: number;
  readonly max_delta_e2000: number;
  readonly fraction_exceeding_threshold: number;
  readonly threshold_delta_e2000: number;
  readonly reasons: readonly string[];
}

/** Detected yellow-sparkle blob (`snake_case`). */
export interface SparkleBlob {
  readonly bounds: PixelRect;
  readonly area_px: number;
  readonly peak_residual: number;
}

/** Sparkle detector report (`snake_case`). */
export interface SparkleReport {
  readonly passed: boolean;
  readonly flagged_blobs: readonly SparkleBlob[];
  readonly total_flagged_pixels: number;
  readonly reasons: readonly string[];
}

/** Combined fail-closed `ColorQaReport` (`snake_case` matching `color_qa.rs`). */
export interface ColorQaReport {
  readonly passed: boolean;
  readonly exposure: ExposureReport | null;
  readonly palette_adherence: readonly PaletteAdherenceReport[];
  readonly untouched_drift: UntouchedDriftReport | null;
  readonly sparkle: SparkleReport | null;
  readonly max_delta_e2000: number;
  readonly notes: readonly string[];
}

/** `VisionCritic` trait category (`snake_case`). */
export type TraitCategory =
  'identity' | 'hand_props' | 'weapon_geometry' | 'style_adherence' | 'zero_leaked_text';

/** Single binary trait check input forwarded to validate endpoints (`camelCase`). */
export interface TraitCheckInputDto {
  readonly traitId: string;
  readonly category: TraitCategory;
  readonly criterion: string;
}

/** Single binary trait verdict returned by `VisionCritic` (`snake_case`). */
export interface TraitCheckResult {
  readonly trait_id: string;
  readonly category: TraitCategory;
  readonly passed: boolean;
  readonly observation: string;
}

/** Structured `VisionCritic` report (`snake_case` matching `critic.rs`). */
export interface CriticReport {
  readonly schema_version: number;
  readonly passed: boolean;
  readonly checks: readonly TraitCheckResult[];
  readonly summary: string;
}

/** Single CIE `L*` histogram bin from Rust core (`camelCase`). */
export interface LStarHistogramBin {
  readonly binIndex: number;
  readonly minLStar: number;
  readonly maxLStar: number;
  readonly count: number;
  readonly fraction: number;
}

/** Single downsampled OKLab vectorscope point from Rust core (`camelCase`). */
export interface OklabVectorscopePoint {
  readonly oklabL: number;
  readonly a: number;
  readonly b: number;
  readonly lStar: number;
  readonly hex: string;
}

/** Combined `L*` histogram and OKLab vectorscope telemetry from Rust core (`camelCase`). */
export interface ColorInspectionTelemetry {
  readonly width: number;
  readonly height: number;
  readonly opaquePixelCount: number;
  readonly lStarBins: readonly LStarHistogramBin[];
  readonly vectorscopePoints: readonly OklabVectorscopePoint[];
}

/** Diagnostic RGBA heatmap overlay kind. */
export type HeatmapKindKey = 'drift' | 'bleed' | 'sparkle' | 'outOfRoi';

/** Base64 PNG diagnostic heatmap overlays from `/api/runs/{slug}/{version}/inspect`. */
export interface InspectionHeatmaps {
  readonly drift: string | null;
  readonly bleed: string | null;
  readonly sparkle: string | null;
  readonly outOfRoi: string | null;
}

/** Response returned by `POST /api/runs/{slug}/{version}/inspect`. */
export interface InspectRunResponse {
  readonly run: RunRecord;
  readonly telemetry: ColorInspectionTelemetry;
  readonly colorQa: ColorQaReport;
  readonly heatmaps: InspectionHeatmaps;
}

/** Response returned by `POST /api/edit/patch`. */
export interface PatchEditResponse {
  readonly run: RunRecord;
  readonly beforeImageUrl: string;
  readonly afterImageUrl: string;
  readonly cropRect: PixelRect;
  readonly roiRect: PixelRect;
  readonly reinhardGains: ReinhardGainSummary;
  readonly colorQa: ColorQaReport;
}

/** Response returned by `POST /api/guide/validate`. */
export interface GuideValidateResponse {
  readonly valid: boolean;
  readonly spec: GuideSpec | null;
  readonly errors: readonly GuideSpecValidationError[];
}

/** Response returned by `POST /api/guide/render`. */
export interface GuideRenderResponse {
  readonly id: string;
  readonly variant: SpatialGuideVariant;
  readonly width: number;
  readonly height: number;
  readonly pngBase64: string;
  readonly errors: readonly GuideSpecValidationError[];
}

/** Response returned by `POST /api/packs/{subject}/{style}/reslice`. */
export interface ReslicePackResponse {
  readonly schemaVersion: number;
  readonly sheetFilename: string;
  readonly sheetWidth: number;
  readonly sheetHeight: number;
  readonly calibrationStripHeight: number;
  readonly slices: readonly SliceRecordDto[];
}

/** Response returned by `POST /api/packs/{subject}/{style}/validate`. */
export interface ValidatePackResponse {
  readonly referencePack: ReferencePackSummary;
  readonly checks: readonly string[];
  readonly criticReport: CriticReport | null;
}

/** Response returned by `POST /api/packs/{subject}/{style}/promote`. */
export interface PromotePackResponse {
  readonly referencePack: ReferencePackSummary;
  readonly canonicalCount: number;
  readonly supersededCount: number;
}

/** Response returned by `POST /api/runs/{slug}/{version}/validate`. */
export interface ValidateRunResponse {
  readonly run: RunRecord;
  readonly colorQa: ColorQaReport;
  readonly criticReport: CriticReport | null;
}

/** Response returned by `POST /api/runs/{slug}/{version}/promote`. */
export interface PromoteRunResponse {
  readonly run: RunRecord;
  readonly canonicalCount: number;
  readonly supersededCount: number;
}

/** Supported `Export` deliverable profiles. */
export type ExportProfileName = 'vtt_token' | 'vtt_map' | 'web_webp' | 'grid_2x2';

/** Single exported deliverable artifact record. */
export interface ExportArtifactDto {
  readonly role: string;
  readonly path: string;
  readonly assetUrl: string | null;
  readonly width: number;
  readonly height: number;
  readonly sha256: string;
}

/** Response returned by `POST /api/export`. */
export interface ExportResponse {
  readonly sourceId: string;
  readonly sourceVisibility: Visibility;
  readonly profileVisibility: Visibility;
  readonly matchGradeApplied: boolean;
  readonly artifacts: readonly ExportArtifactDto[];
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
