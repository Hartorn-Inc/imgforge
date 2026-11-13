/** === IMGFORGE REST & WEBSOCKET API CLIENT ===
 *
 * PURPOSE:
 * Connects the Web Canvas SPA to all live REST and WebSocket endpoints exposed
 * by `crates/imgforge-cli/src/server.rs` on `127.0.0.1:<port>`.
 *
 * DESIGN RATIONALE ("WHY"):
 * `server.rs` enforces `#[serde(deny_unknown_fields)]` on request payloads and
 * mixes outer `camelCase` DTOs with nested `snake_case` core reports
 * (`ColorQaReport`, `CriticReport`, `ReinhardGainSummary`, `ManifestEntry`).
 * All requests construct exact wire fields without `any` escape hatches, and
 * all responses decode defensively into `ApiResult<T>` so malformed or error
 * responses never throw unhandled exceptions in the UI.
 *
 * EXECUTION LIFECYCLE / DATA FLOW:
 * 1. `requestJson()` performs `fetch` with `Accept: application/json`, parses
 *    structured `{ error, message }` errors, and runs a strict runtime decoder.
 * 2. `connectWorkspaceEvents()` opens `GET /api/ws`, parses `WorkspaceEvent`
 *    frames, and reconnects automatically with bounded exponential backoff.
 */

import type {
  AtmosphereSummary,
  ColorInspectionTelemetry,
  ColorQaReport,
  CriticReport,
  ExportArtifactDto,
  ExportProfileName,
  ExportResponse,
  ExposureFloor,
  ExposureMetrics,
  ExposureReport,
  ForbiddenHueIntrusion,
  GuideRenderResponse,
  GuideValidateResponse,
  HeatmapKindKey,
  InspectionHeatmaps,
  InspectRunResponse,
  LStarHistogramBin,
  LibrarySource,
  LifecycleState,
  ManifestEntry,
  ManifestEntryKind,
  ManifestLockSummary,
  OklabClusterSummary,
  OklabVectorscopePoint,
  OverrideMode,
  PaletteAdherenceReport,
  PaletteLockSwatch,
  PatchEditResponse,
  PatchPrepMode,
  PixelRect,
  PromotePackResponse,
  PromoteRunResponse,
  RectOverride,
  ReferencePackSummary,
  ReferenceSheetKind,
  ReinhardGainSummary,
  ReslicePackResponse,
  RunRecord,
  SheetRecordDto,
  SliceRecordDto,
  SliceSource,
  SparkleBlob,
  SparkleReport,
  StyleSummary,
  SubjectCategory,
  SubjectSummary,
  TraitCategory,
  TraitCheckInputDto,
  TraitCheckResult,
  UntouchedDriftReport,
  ValidatePackResponse,
  ValidateRunResponse,
  Visibility,
  WorkspaceEvent,
  WorkspaceSummary,
} from '../types/domain';
import {
  type GuideSpec,
  type GuideSpecValidationError,
  validateGuideSpec,
} from '../types/guideSpec';

/** Structured API error returned when an endpoint rejects or fails. */
export interface ApiErrorInfo {
  readonly code: string;
  readonly message: string;
  readonly httpStatus: number | null;
}

/** Discriminated result returned by all API client operations. */
export type ApiResult<T> =
  { readonly ok: true; readonly data: T } | { readonly ok: false; readonly error: ApiErrorInfo };

/** Request payload for `POST /api/styles`. */
export interface CreateStylePayload {
  readonly slug: string;
  readonly displayName: string;
  readonly medium?: string;
  readonly linework?: string;
  readonly shadingAndTexture?: string;
  readonly paletteProfile?: string;
  readonly framingAndBorder?: string;
  readonly styleGuardrails?: readonly string[];
  readonly cloneFromSlug?: string;
  readonly exemplarsBase64?: readonly string[];
}

/** Request payload for `PUT /api/styles/{slug}`. */
export interface UpdateStylePayload {
  readonly displayName?: string;
  readonly medium?: string;
  readonly linework?: string;
  readonly shadingAndTexture?: string;
  readonly paletteProfile?: string;
  readonly framingAndBorder?: string;
  readonly styleGuardrails?: readonly string[];
}

/** Request payload for `POST /api/styles/{slug}/exemplars`. */
export interface UploadExemplarsPayload {
  readonly exemplarsBase64: readonly string[];
  readonly replace: boolean;
}

/** Response returned by `POST /api/styles/{slug}/promote`. */
export interface PromoteStyleResponse {
  readonly style: StyleSummary;
  readonly manifestLock: ManifestLockSummary | null;
}

/** Request payload for `POST /api/atmospheres` matching `CreateAtmosphereRequest`. */
export interface CreateAtmospherePayload {
  readonly slug: string;
  readonly displayName: string;
  readonly keyLightDirection: string;
  readonly secondaryLightDirection: string;
  readonly colorTemperatureK: number;
  readonly maxAmbientTintBlend: number;
  readonly minMeanLStar: number;
  readonly minP10LStar: number;
  readonly weatherAndParticles: string;
}

/** Request payload for `PUT /api/atmospheres/{slug}` matching `UpdateAtmosphereRequest`. */
export interface UpdateAtmospherePayload {
  readonly displayName?: string;
  readonly keyLightDirection?: string;
  readonly secondaryLightDirection?: string;
  readonly colorTemperatureK?: number;
  readonly maxAmbientTintBlend?: number;
  readonly minMeanLStar?: number;
  readonly minP10LStar?: number;
  readonly weatherAndParticles?: string;
}

/** Request payload for `POST /api/subjects/{slug}/palette-lock`. */
export interface UpdatePaletteLockPayload {
  readonly name: string;
  readonly colorHex: string;
  readonly surfaceSlot?: string;
}

/** Request payload for `POST /api/packs/{subject}/{style}/reslice`. */
export interface ReslicePackPayload {
  readonly sheetRole: ReferenceSheetKind;
  readonly mode: OverrideMode;
  readonly overrides: readonly RectOverride[];
}

/** Request payload for `POST /api/packs/{subject}/{style}/validate`. */
export interface ValidatePackPayload {
  readonly traits?: readonly TraitCheckInputDto[];
  readonly runCritic?: boolean;
}

/** Request payload for `POST /api/edit/patch` (mutually exclusive target & mask fields enforced). */
export interface PatchEditApiPayload {
  readonly slug: string;
  readonly targetImagePath?: string;
  readonly runSlug?: string;
  readonly runVersion?: number;
  readonly roi?: PixelRect;
  readonly maskPngBase64?: string;
  readonly prepMode: PatchPrepMode;
  readonly collarWidthPx: number;
  readonly featherPx: number;
  readonly prompt: string;
  readonly visibility: Visibility;
  readonly referenceImages?: readonly string[];
}

/** Request payload for `POST /api/runs/{slug}/{version}/inspect`. */
export interface InspectRunPayload {
  readonly atmosphereSlug?: string;
  readonly subjectSlugs?: readonly string[];
  readonly beforeImagePath?: string;
  readonly beforeRunVersion?: number;
  readonly maxSamples?: number;
}

/** Request payload for `POST /api/runs/{slug}/{version}/validate`. */
export interface ValidateRunPayload {
  readonly atmosphereSlug?: string;
  readonly subjectSlugs?: readonly string[];
  readonly referenceImages?: readonly string[];
  readonly traits?: readonly TraitCheckInputDto[];
  readonly runCritic?: boolean;
}

/** Optional `--match-grade` configuration for `POST /api/export`. */
export interface ExportMatchGradePayload {
  readonly enabled: boolean;
  readonly gradeRefPath?: string;
  readonly gradeStrength?: number;
  readonly cubeLutPath?: string;
}

/** Request payload for `POST /api/export`. */
export interface ExportApiPayload {
  readonly runSlug: string;
  readonly runVersion: number;
  readonly profile: ExportProfileName;
  readonly visibility?: Visibility;
  readonly outputSlug?: string;
  readonly outDir?: string;
  readonly tokenSize?: number;
  readonly ringColorHex?: string;
  readonly jpegBackdropHex?: string;
  readonly cols?: number;
  readonly rows?: number;
  readonly gridPx?: number;
  readonly scaleFactor?: number;
  readonly jpegQuality?: number;
  readonly matchGrade?: ExportMatchGradePayload;
}

/** Options for request transport. */
export interface ApiRequestOptions {
  readonly signal?: AbortSignal;
  readonly fetchImpl?: typeof fetch;
}

// === 1. Defensive Runtime Decoders ===

function isRecord(val: unknown): val is Record<string, unknown> {
  return typeof val === 'object' && val !== null && !Array.isArray(val);
}

function asString(val: unknown): string | null {
  return typeof val === 'string' ? val : null;
}

function asNullableString(val: unknown): string | null | undefined {
  if (val === null) {
    return null;
  }
  return typeof val === 'string' ? val : undefined;
}

function asNumber(val: unknown): number | null {
  return typeof val === 'number' && Number.isFinite(val) ? val : null;
}

function asBoolean(val: unknown): boolean | null {
  return typeof val === 'boolean' ? val : null;
}

function asStringArray(val: unknown): readonly string[] | null {
  if (!Array.isArray(val)) {
    return null;
  }
  const out: string[] = [];
  for (const item of val) {
    if (typeof item !== 'string') {
      return null;
    }
    out.push(item);
  }
  return out;
}

function asLifecycle(val: unknown): LifecycleState | null {
  if (
    val === 'draft' ||
    val === 'candidate' ||
    val === 'validated' ||
    val === 'canonical' ||
    val === 'superseded'
  ) {
    return val;
  }
  return null;
}

function asVisibility(val: unknown): Visibility | null {
  if (val === 'public' || val === 'restricted' || val === 'internal') {
    return val;
  }
  return null;
}

function asLibrarySource(val: unknown): LibrarySource | null {
  if (val === 'project' || val === 'user' || val === 'builtin') {
    return val;
  }
  return null;
}

function asSubjectCategory(val: unknown): SubjectCategory | null {
  if (val === 'character' || val === 'creature' || val === 'prop' || val === 'location') {
    return val;
  }
  return null;
}

function asReferenceSheetKind(val: unknown): ReferenceSheetKind | null {
  if (
    val === 'turnaround' ||
    val === 'poses_expressions' ||
    val === 'inventory_flatlay' ||
    val === 'single_sheet'
  ) {
    return val;
  }
  return null;
}

function asSliceSource(val: unknown): SliceSource | null {
  if (val === 'auto' || val === 'manual_override') {
    return val;
  }
  return null;
}

function asManifestEntryKind(val: unknown): ManifestEntryKind | null {
  if (
    val === 'subject' ||
    val === 'style' ||
    val === 'atmosphere' ||
    val === 'reference_pack' ||
    val === 'run'
  ) {
    return val;
  }
  return null;
}

function asTraitCategory(val: unknown): TraitCategory | null {
  if (
    val === 'identity' ||
    val === 'hand_props' ||
    val === 'weapon_geometry' ||
    val === 'style_adherence' ||
    val === 'zero_leaked_text'
  ) {
    return val;
  }
  return null;
}

function decodePixelRect(val: unknown): PixelRect | null {
  if (!isRecord(val)) {
    return null;
  }
  const x = asNumber(val['x']);
  const y = asNumber(val['y']);
  const width = asNumber(val['width']);
  const height = asNumber(val['height']);
  if (x === null || y === null || width === null || height === null) {
    return null;
  }
  return { x, y, width, height };
}

function decodePaletteSwatch(val: unknown): PaletteLockSwatch | null {
  if (!isRecord(val)) {
    return null;
  }
  const label = asString(val['label']);
  const hex = asString(val['hex']);
  const surfaceSlot = asString(val['surfaceSlot']);
  if (label === null || hex === null || surfaceSlot === null) {
    return null;
  }
  return { label, hex, surfaceSlot };
}

export function decodeSubjectDto(val: unknown): SubjectSummary | null {
  if (!isRecord(val)) {
    return null;
  }
  const slug = asString(val['slug']);
  const name = asString(val['name']);
  const category = asSubjectCategory(val['category']);
  const heightMeters = asNumber(val['heightMeters']);
  const visibility = asVisibility(val['visibility']);
  const lifecycle = asLifecycle(val['lifecycle']);
  const source = asLibrarySource(val['source']);
  const asymmetricMarkers = asStringArray(val['asymmetricMarkers']);
  const positiveWeaponGeometry = asStringArray(val['positiveWeaponGeometry']);
  const tags = asStringArray(val['tags']);
  if (
    slug === null ||
    name === null ||
    category === null ||
    heightMeters === null ||
    visibility === null ||
    lifecycle === null ||
    source === null ||
    asymmetricMarkers === null ||
    positiveWeaponGeometry === null ||
    tags === null ||
    !Array.isArray(val['paletteLock'])
  ) {
    return null;
  }
  const paletteLock: PaletteLockSwatch[] = [];
  for (const rawSwatch of val['paletteLock']) {
    const sw = decodePaletteSwatch(rawSwatch);
    if (sw === null) {
      return null;
    }
    paletteLock.push(sw);
  }
  return {
    slug,
    name,
    category,
    heightMeters,
    paletteLock,
    visibility,
    lifecycle,
    source,
    asymmetricMarkers,
    positiveWeaponGeometry,
    tags,
  };
}

export function decodeStyleDto(val: unknown): StyleSummary | null {
  if (!isRecord(val)) {
    return null;
  }
  const slug = asString(val['slug']);
  const name = asString(val['name']);
  const medium = asString(val['medium']);
  const exemplarCount = asNumber(val['exemplarCount']);
  const lifecycle = asLifecycle(val['lifecycle']);
  const source = asLibrarySource(val['source']);
  const linework = asString(val['linework']);
  const shadingAndTexture = asString(val['shadingAndTexture']);
  const paletteProfile = asString(val['paletteProfile']);
  const framingAndBorder = asString(val['framingAndBorder']);
  const styleGuardrails = asStringArray(val['styleGuardrails']);
  const exemplarUrls = asStringArray(val['exemplarUrls']);
  if (
    slug === null ||
    name === null ||
    medium === null ||
    exemplarCount === null ||
    lifecycle === null ||
    source === null ||
    linework === null ||
    shadingAndTexture === null ||
    paletteProfile === null ||
    framingAndBorder === null ||
    styleGuardrails === null ||
    exemplarUrls === null
  ) {
    return null;
  }
  return {
    slug,
    name,
    medium,
    exemplarCount,
    lifecycle,
    source,
    linework,
    shadingAndTexture,
    paletteProfile,
    framingAndBorder,
    styleGuardrails,
    exemplarUrls,
  };
}

export function decodeAtmosphereDto(val: unknown): AtmosphereSummary | null {
  if (!isRecord(val)) {
    return null;
  }
  const slug = asString(val['slug']);
  const name = asString(val['name']);
  const lightDirection = asString(val['lightDirection']);
  const colorTemperatureKelvin = asNumber(val['colorTemperatureKelvin']);
  const exposureFloor = asNumber(val['exposureFloor']);
  const maxAmbientTintBlend = asNumber(val['maxAmbientTintBlend']);
  const particles = asString(val['particles']);
  const source = asLibrarySource(val['source']);
  const secondaryLightDirection = asString(val['secondaryLightDirection']);
  const minMeanLStar = asNumber(val['minMeanLStar']);
  const minP10LStar = asNumber(val['minP10LStar']);
  if (
    slug === null ||
    name === null ||
    lightDirection === null ||
    colorTemperatureKelvin === null ||
    exposureFloor === null ||
    maxAmbientTintBlend === null ||
    particles === null ||
    source === null ||
    secondaryLightDirection === null ||
    minMeanLStar === null ||
    minP10LStar === null
  ) {
    return null;
  }
  return {
    slug,
    name,
    lightDirection,
    colorTemperatureKelvin,
    exposureFloor,
    maxAmbientTintBlend,
    particles,
    source,
    secondaryLightDirection,
    minMeanLStar,
    minP10LStar,
  };
}

function decodeSheetDto(val: unknown): SheetRecordDto | null {
  if (!isRecord(val)) {
    return null;
  }
  const role = asReferenceSheetKind(val['role']);
  const filename = asString(val['filename']);
  const imageUrl = asString(val['imageUrl']);
  const sha256 = asString(val['sha256']);
  const width = asNumber(val['width']);
  const height = asNumber(val['height']);
  const bgCoverage = asNumber(val['bgCoverage']);
  const borderBgCoverage = asNumber(val['borderBgCoverage']);
  const foregroundPixels = asNumber(val['foregroundPixels']);
  const stripY = asNumber(val['stripY']);
  const stripHeight = asNumber(val['stripHeight']);
  if (
    role === null ||
    filename === null ||
    imageUrl === null ||
    sha256 === null ||
    width === null ||
    height === null ||
    bgCoverage === null ||
    borderBgCoverage === null ||
    foregroundPixels === null ||
    stripY === null ||
    stripHeight === null
  ) {
    return null;
  }
  return {
    role,
    filename,
    imageUrl,
    sha256,
    width,
    height,
    bgCoverage,
    borderBgCoverage,
    foregroundPixels,
    stripY,
    stripHeight,
  };
}

function decodeSliceDto(val: unknown): SliceRecordDto | null {
  if (!isRecord(val)) {
    return null;
  }
  const id = asString(val['id']);
  const file = asString(val['file']);
  const imageUrl = asString(val['imageUrl']);
  const rect = decodePixelRect(val['rect']);
  const source = asSliceSource(val['source']);
  const label = asNullableString(val['label']);
  const sha256 = asString(val['sha256']);
  if (
    id === null ||
    file === null ||
    imageUrl === null ||
    rect === null ||
    source === null ||
    label === undefined ||
    sha256 === null
  ) {
    return null;
  }
  return { id, file, imageUrl, rect, source, label, sha256 };
}

export function decodeReferencePackDto(val: unknown): ReferencePackSummary | null {
  if (!isRecord(val)) {
    return null;
  }
  const subjectSlug = asString(val['subjectSlug']);
  const styleSlug = asString(val['styleSlug']);
  const sheetKind = asReferenceSheetKind(val['sheetKind']);
  const sliceCount = asNumber(val['sliceCount']);
  const lifecycle = asLifecycle(val['lifecycle']);
  const version = asNumber(val['version']);
  const visibility = asVisibility(val['visibility']);
  const createdAtIso = asString(val['createdAtIso']);
  if (
    subjectSlug === null ||
    styleSlug === null ||
    sheetKind === null ||
    sliceCount === null ||
    lifecycle === null ||
    version === null ||
    visibility === null ||
    createdAtIso === null ||
    !Array.isArray(val['sheets']) ||
    !Array.isArray(val['slices'])
  ) {
    return null;
  }
  const sheets: SheetRecordDto[] = [];
  for (const item of val['sheets']) {
    const sheet = decodeSheetDto(item);
    if (sheet === null) {
      return null;
    }
    sheets.push(sheet);
  }
  const slices: SliceRecordDto[] = [];
  for (const item of val['slices']) {
    const slice = decodeSliceDto(item);
    if (slice === null) {
      return null;
    }
    slices.push(slice);
  }
  return {
    subjectSlug,
    styleSlug,
    sheetKind,
    sliceCount,
    lifecycle,
    version,
    visibility,
    sheets,
    slices,
    createdAtIso,
  };
}

export function decodeRunDto(val: unknown): RunRecord | null {
  if (!isRecord(val)) {
    return null;
  }
  const runId = asString(val['runId']);
  const slug = asString(val['slug']);
  const version = asNumber(val['version']);
  const versionLabel = asString(val['versionLabel']);
  const lifecycle = asLifecycle(val['lifecycle']);
  const visibility = asVisibility(val['visibility']);
  const widthPx = asNumber(val['widthPx']);
  const heightPx = asNumber(val['heightPx']);
  const rawCritic = val['visionCriticPassed'];
  const visionCriticPassed =
    rawCritic === null ? null : typeof rawCritic === 'boolean' ? rawCritic : undefined;
  const rawDelta = val['colorQaMaxDeltaE2000'];
  const colorQaMaxDeltaE2000 = rawDelta === null ? null : asNumber(rawDelta);
  const createdAtIso = asString(val['createdAtIso']);
  const imageUrl = asString(val['imageUrl']);
  const compiledPrompt = asString(val['compiledPrompt']);
  const backendName = asString(val['backendName']);
  const structuralChecks = asStringArray(val['structuralChecks']);
  if (
    runId === null ||
    slug === null ||
    version === null ||
    versionLabel === null ||
    lifecycle === null ||
    visibility === null ||
    widthPx === null ||
    heightPx === null ||
    visionCriticPassed === undefined ||
    (rawDelta !== null && colorQaMaxDeltaE2000 === null) ||
    createdAtIso === null ||
    imageUrl === null ||
    compiledPrompt === null ||
    backendName === null ||
    structuralChecks === null
  ) {
    return null;
  }
  return {
    runId,
    slug,
    version,
    versionLabel,
    lifecycle,
    visibility,
    widthPx,
    heightPx,
    visionCriticPassed,
    colorQaMaxDeltaE2000,
    createdAtIso,
    imageUrl,
    compiledPrompt,
    backendName,
    structuralChecks,
  };
}

function decodeManifestEntry(val: unknown): ManifestEntry | null {
  if (!isRecord(val)) {
    return null;
  }
  const entryKind = asManifestEntryKind(val['entry_kind']);
  const key = asString(val['key']);
  const version = asNumber(val['version']);
  const versionLabel = asString(val['version_label']);
  const lifecycle = asLifecycle(val['lifecycle']);
  const visibility = asVisibility(val['visibility']);
  const promotedAtIso = asString(val['promoted_at_iso']);
  const rawHashes = val['artifact_hashes'];
  if (
    entryKind === null ||
    key === null ||
    version === null ||
    versionLabel === null ||
    lifecycle === null ||
    visibility === null ||
    promotedAtIso === null ||
    !isRecord(rawHashes)
  ) {
    return null;
  }
  const artifactHashes: Record<string, string> = {};
  for (const [k, v] of Object.entries(rawHashes)) {
    if (typeof v !== 'string') {
      return null;
    }
    artifactHashes[k] = v;
  }
  return {
    entry_kind: entryKind,
    key,
    version,
    version_label: versionLabel,
    lifecycle,
    visibility,
    artifact_hashes: artifactHashes,
    promoted_at_iso: promotedAtIso,
  };
}

function decodeManifestLock(val: unknown): ManifestLockSummary | null {
  if (!isRecord(val)) {
    return null;
  }
  const schemaVersion = asNumber(val['schemaVersion']);
  const canonicalCount = asNumber(val['canonicalCount']);
  const supersededCount = asNumber(val['supersededCount']);
  const rawCanonical = val['canonical'];
  const rawSuperseded = val['superseded'];
  if (
    schemaVersion === null ||
    canonicalCount === null ||
    supersededCount === null ||
    !isRecord(rawCanonical) ||
    !Array.isArray(rawSuperseded)
  ) {
    return null;
  }
  const canonical: Record<string, ManifestEntry> = {};
  for (const [k, v] of Object.entries(rawCanonical)) {
    const entry = decodeManifestEntry(v);
    if (entry === null) {
      return null;
    }
    canonical[k] = entry;
  }
  const superseded: ManifestEntry[] = [];
  for (const item of rawSuperseded) {
    const entry = decodeManifestEntry(item);
    if (entry === null) {
      return null;
    }
    superseded.push(entry);
  }
  return {
    schemaVersion,
    canonicalCount,
    supersededCount,
    canonical,
    superseded,
  };
}

export function decodeWorkspaceSummary(val: unknown): WorkspaceSummary | null {
  if (!isRecord(val)) {
    return null;
  }
  const hasWorkspace = asBoolean(val['hasWorkspace']);
  const workspaceRoot = asNullableString(val['workspaceRoot']);
  const workspaceName = asNullableString(val['workspaceName']);
  const authProvisioned = asBoolean(val['authProvisioned']);
  const manifestLock = decodeManifestLock(val['manifestLock']);
  if (
    hasWorkspace === null ||
    workspaceRoot === undefined ||
    workspaceName === undefined ||
    authProvisioned === null ||
    manifestLock === null ||
    !Array.isArray(val['subjects']) ||
    !Array.isArray(val['styles']) ||
    !Array.isArray(val['atmospheres']) ||
    !Array.isArray(val['referencePacks']) ||
    !Array.isArray(val['runs'])
  ) {
    return null;
  }
  const subjects: SubjectSummary[] = [];
  for (const item of val['subjects']) {
    const decoded = decodeSubjectDto(item);
    if (decoded === null) {
      return null;
    }
    subjects.push(decoded);
  }
  const styles: StyleSummary[] = [];
  for (const item of val['styles']) {
    const decoded = decodeStyleDto(item);
    if (decoded === null) {
      return null;
    }
    styles.push(decoded);
  }
  const atmospheres: AtmosphereSummary[] = [];
  for (const item of val['atmospheres']) {
    const decoded = decodeAtmosphereDto(item);
    if (decoded === null) {
      return null;
    }
    atmospheres.push(decoded);
  }
  const referencePacks: ReferencePackSummary[] = [];
  for (const item of val['referencePacks']) {
    const decoded = decodeReferencePackDto(item);
    if (decoded === null) {
      return null;
    }
    referencePacks.push(decoded);
  }
  const runs: RunRecord[] = [];
  for (const item of val['runs']) {
    const decoded = decodeRunDto(item);
    if (decoded === null) {
      return null;
    }
    runs.push(decoded);
  }
  return {
    hasWorkspace,
    workspaceRoot,
    workspaceName,
    authProvisioned,
    subjects,
    styles,
    atmospheres,
    referencePacks,
    runs,
    manifestLock,
  };
}

function decodeExposureReport(val: unknown): ExposureReport | null {
  if (!isRecord(val)) {
    return null;
  }
  const passed = asBoolean(val['passed']);
  const reasons = asStringArray(val['reasons']);
  const rawMetrics = val['metrics'];
  const rawFloor = val['floor'];
  if (passed === null || reasons === null || !isRecord(rawMetrics) || !isRecord(rawFloor)) {
    return null;
  }
  const meanLStar = asNumber(rawMetrics['mean_l_star']);
  const p10LStar = asNumber(rawMetrics['p10_l_star']);
  const p90LStar = asNumber(rawMetrics['p90_l_star']);
  const clippedBlackRatio = asNumber(rawMetrics['clipped_black_ratio']);
  const pixelCount = asNumber(rawMetrics['pixel_count']);
  const minMeanLStar = asNumber(rawFloor['min_mean_l_star']);
  const minP10LStar = asNumber(rawFloor['min_p10_l_star']);
  if (
    meanLStar === null ||
    p10LStar === null ||
    p90LStar === null ||
    clippedBlackRatio === null ||
    pixelCount === null ||
    minMeanLStar === null ||
    minP10LStar === null
  ) {
    return null;
  }
  const metrics: ExposureMetrics = {
    mean_l_star: meanLStar,
    p10_l_star: p10LStar,
    p90_l_star: p90LStar,
    clipped_black_ratio: clippedBlackRatio,
    pixel_count: pixelCount,
  };
  const floor: ExposureFloor = {
    min_mean_l_star: minMeanLStar,
    min_p10_l_star: minP10LStar,
  };
  return { passed, metrics, floor, reasons };
}

function decodeOklabCluster(val: unknown): OklabClusterSummary | null {
  if (!isRecord(val)) {
    return null;
  }
  const rawCentroid = val['centroid_oklab'];
  const centroidHex = asString(val['centroid_hex']);
  const weight = asNumber(val['weight']);
  const pixelCount = asNumber(val['pixel_count']);
  const closestSwatchName = asString(val['closest_swatch_name']);
  const closestSwatchHex = asString(val['closest_swatch_hex']);
  const deltaE2000 = asNumber(val['delta_e2000']);
  const lightnessShift = asNumber(val['lightness_shift']);
  const chromaShift = asNumber(val['chroma_shift']);
  if (
    !Array.isArray(rawCentroid) ||
    rawCentroid.length !== 3 ||
    centroidHex === null ||
    weight === null ||
    pixelCount === null ||
    closestSwatchName === null ||
    closestSwatchHex === null ||
    deltaE2000 === null ||
    lightnessShift === null ||
    chromaShift === null
  ) {
    return null;
  }
  const l = asNumber(rawCentroid[0]);
  const a = asNumber(rawCentroid[1]);
  const b = asNumber(rawCentroid[2]);
  if (l === null || a === null || b === null) {
    return null;
  }
  return {
    centroid_oklab: [l, a, b],
    centroid_hex: centroidHex,
    weight,
    pixel_count: pixelCount,
    closest_swatch_name: closestSwatchName,
    closest_swatch_hex: closestSwatchHex,
    delta_e2000: deltaE2000,
    lightness_shift: lightnessShift,
    chroma_shift: chromaShift,
  };
}

function decodeForbiddenHueIntrusion(val: unknown): ForbiddenHueIntrusion | null {
  if (!isRecord(val)) {
    return null;
  }
  const sectorName = asString(val['sector_name']);
  const fraction = asNumber(val['fraction']);
  const pixelCount = asNumber(val['pixel_count']);
  if (sectorName === null || fraction === null || pixelCount === null) {
    return null;
  }
  return { sector_name: sectorName, fraction, pixel_count: pixelCount };
}

function decodePaletteAdherence(val: unknown): PaletteAdherenceReport | null {
  if (!isRecord(val)) {
    return null;
  }
  const passed = asBoolean(val['passed']);
  const subjectSlug = asString(val['subject_slug']);
  const maxSwatch = asNumber(val['max_swatch_delta_e2000']);
  const weightedMean = asNumber(val['weighted_mean_delta_e2000']);
  const reasons = asStringArray(val['reasons']);
  if (
    passed === null ||
    subjectSlug === null ||
    maxSwatch === null ||
    weightedMean === null ||
    reasons === null ||
    !Array.isArray(val['clusters']) ||
    !Array.isArray(val['forbidden_hue_intrusions'])
  ) {
    return null;
  }
  const clusters: OklabClusterSummary[] = [];
  for (const item of val['clusters']) {
    const cluster = decodeOklabCluster(item);
    if (cluster === null) {
      return null;
    }
    clusters.push(cluster);
  }
  const intrusions: ForbiddenHueIntrusion[] = [];
  for (const item of val['forbidden_hue_intrusions']) {
    const intr = decodeForbiddenHueIntrusion(item);
    if (intr === null) {
      return null;
    }
    intrusions.push(intr);
  }
  return {
    passed,
    subject_slug: subjectSlug,
    clusters,
    max_swatch_delta_e2000: maxSwatch,
    weighted_mean_delta_e2000: weightedMean,
    forbidden_hue_intrusions: intrusions,
    reasons,
  };
}

function decodeUntouchedDrift(val: unknown): UntouchedDriftReport | null {
  if (!isRecord(val)) {
    return null;
  }
  const passed = asBoolean(val['passed']);
  const untouchedPixelCount = asNumber(val['untouched_pixel_count']);
  const mean = asNumber(val['mean_delta_e2000']);
  const p95 = asNumber(val['p95_delta_e2000']);
  const max = asNumber(val['max_delta_e2000']);
  const fraction = asNumber(val['fraction_exceeding_threshold']);
  const threshold = asNumber(val['threshold_delta_e2000']);
  const reasons = asStringArray(val['reasons']);
  if (
    passed === null ||
    untouchedPixelCount === null ||
    mean === null ||
    p95 === null ||
    max === null ||
    fraction === null ||
    threshold === null ||
    reasons === null
  ) {
    return null;
  }
  return {
    passed,
    untouched_pixel_count: untouchedPixelCount,
    mean_delta_e2000: mean,
    p95_delta_e2000: p95,
    max_delta_e2000: max,
    fraction_exceeding_threshold: fraction,
    threshold_delta_e2000: threshold,
    reasons,
  };
}

function decodeSparkleReport(val: unknown): SparkleReport | null {
  if (!isRecord(val)) {
    return null;
  }
  const passed = asBoolean(val['passed']);
  const totalFlaggedPixels = asNumber(val['total_flagged_pixels']);
  const reasons = asStringArray(val['reasons']);
  if (
    passed === null ||
    totalFlaggedPixels === null ||
    reasons === null ||
    !Array.isArray(val['flagged_blobs'])
  ) {
    return null;
  }
  const blobs: SparkleBlob[] = [];
  for (const item of val['flagged_blobs']) {
    if (!isRecord(item)) {
      return null;
    }
    const bounds = decodePixelRect(item['bounds']);
    const areaPx = asNumber(item['area_px']);
    const peakResidual = asNumber(item['peak_residual']);
    if (bounds === null || areaPx === null || peakResidual === null) {
      return null;
    }
    blobs.push({ bounds, area_px: areaPx, peak_residual: peakResidual });
  }
  return {
    passed,
    flagged_blobs: blobs,
    total_flagged_pixels: totalFlaggedPixels,
    reasons,
  };
}

export function decodeColorQaReport(val: unknown): ColorQaReport | null {
  if (!isRecord(val)) {
    return null;
  }
  const passed = asBoolean(val['passed']);
  const maxDeltaE2000 = asNumber(val['max_delta_e2000']);
  const notes = asStringArray(val['notes']);
  if (
    passed === null ||
    maxDeltaE2000 === null ||
    notes === null ||
    !Array.isArray(val['palette_adherence'])
  ) {
    return null;
  }
  const rawExposure = val['exposure'];
  const exposure = rawExposure === null ? null : decodeExposureReport(rawExposure);
  if (rawExposure !== null && exposure === null) {
    return null;
  }
  const paletteAdherence: PaletteAdherenceReport[] = [];
  for (const item of val['palette_adherence']) {
    const parsed = decodePaletteAdherence(item);
    if (parsed === null) {
      return null;
    }
    paletteAdherence.push(parsed);
  }
  const rawDrift = val['untouched_drift'];
  const untouchedDrift = rawDrift === null ? null : decodeUntouchedDrift(rawDrift);
  if (rawDrift !== null && untouchedDrift === null) {
    return null;
  }
  const rawSparkle = val['sparkle'];
  const sparkle = rawSparkle === null ? null : decodeSparkleReport(rawSparkle);
  if (rawSparkle !== null && sparkle === null) {
    return null;
  }
  return {
    passed,
    exposure,
    palette_adherence: paletteAdherence,
    untouched_drift: untouchedDrift,
    sparkle,
    max_delta_e2000: maxDeltaE2000,
    notes,
  };
}

export function decodeCriticReport(val: unknown): CriticReport | null {
  if (!isRecord(val)) {
    return null;
  }
  const schemaVersion = asNumber(val['schema_version']);
  const passed = asBoolean(val['passed']);
  const summary = asString(val['summary']);
  if (
    schemaVersion === null ||
    passed === null ||
    summary === null ||
    !Array.isArray(val['checks'])
  ) {
    return null;
  }
  const checks: TraitCheckResult[] = [];
  for (const item of val['checks']) {
    if (!isRecord(item)) {
      return null;
    }
    const traitId = asString(item['trait_id']);
    const category = asTraitCategory(item['category']);
    const itemPassed = asBoolean(item['passed']);
    const observation = asString(item['observation']);
    if (traitId === null || category === null || itemPassed === null || observation === null) {
      return null;
    }
    checks.push({
      trait_id: traitId,
      category,
      passed: itemPassed,
      observation,
    });
  }
  return {
    schema_version: schemaVersion,
    passed,
    checks,
    summary,
  };
}

function decodeTelemetry(val: unknown): ColorInspectionTelemetry | null {
  if (!isRecord(val)) {
    return null;
  }
  const width = asNumber(val['width']);
  const height = asNumber(val['height']);
  const opaquePixelCount = asNumber(val['opaquePixelCount']);
  if (
    width === null ||
    height === null ||
    opaquePixelCount === null ||
    !Array.isArray(val['lStarBins']) ||
    !Array.isArray(val['vectorscopePoints'])
  ) {
    return null;
  }
  const lStarBins: LStarHistogramBin[] = [];
  for (const item of val['lStarBins']) {
    if (!isRecord(item)) {
      return null;
    }
    const binIndex = asNumber(item['binIndex']);
    const minLStar = asNumber(item['minLStar']);
    const maxLStar = asNumber(item['maxLStar']);
    const count = asNumber(item['count']);
    const fraction = asNumber(item['fraction']);
    if (
      binIndex === null ||
      minLStar === null ||
      maxLStar === null ||
      count === null ||
      fraction === null
    ) {
      return null;
    }
    lStarBins.push({ binIndex, minLStar, maxLStar, count, fraction });
  }
  const vectorscopePoints: OklabVectorscopePoint[] = [];
  for (const item of val['vectorscopePoints']) {
    if (!isRecord(item)) {
      return null;
    }
    const oklabL = asNumber(item['oklabL']);
    const a = asNumber(item['a']);
    const b = asNumber(item['b']);
    const lStar = asNumber(item['lStar']);
    const hex = asString(item['hex']);
    if (oklabL === null || a === null || b === null || lStar === null || hex === null) {
      return null;
    }
    vectorscopePoints.push({ oklabL, a, b, lStar, hex });
  }
  return {
    width,
    height,
    opaquePixelCount,
    lStarBins,
    vectorscopePoints,
  };
}

function decodeReinhardGains(val: unknown): ReinhardGainSummary | null {
  if (!isRecord(val)) {
    return null;
  }
  const gainL = asNumber(val['gain_l']);
  const gainA = asNumber(val['gain_a']);
  const gainB = asNumber(val['gain_b']);
  const collarPixelCount = asNumber(val['collar_pixel_count']);
  if (gainL === null || gainA === null || gainB === null || collarPixelCount === null) {
    return null;
  }
  return {
    gain_l: gainL,
    gain_a: gainA,
    gain_b: gainB,
    collar_pixel_count: collarPixelCount,
  };
}

function decodeGuideErrors(val: unknown): readonly GuideSpecValidationError[] | null {
  if (!Array.isArray(val)) {
    return null;
  }
  const out: GuideSpecValidationError[] = [];
  for (const item of val) {
    if (!isRecord(item)) {
      return null;
    }
    const path = asString(item['path']);
    const message = asString(item['message']);
    if (path === null || message === null) {
      return null;
    }
    out.push({ path, message });
  }
  return out;
}

export function decodeWorkspaceEvent(val: unknown): WorkspaceEvent | null {
  if (!isRecord(val)) {
    return null;
  }
  const eventType = asString(val['eventType']);
  const resourceKind = asString(val['resourceKind']);
  const id = asString(val['id']);
  const rawLifecycle = val['lifecycle'];
  const lifecycle = rawLifecycle === null ? null : asLifecycle(rawLifecycle);
  const detail = asString(val['detail']);
  const timestampIso = asString(val['timestampIso']);
  if (
    eventType === null ||
    resourceKind === null ||
    id === null ||
    (rawLifecycle !== null && lifecycle === null) ||
    detail === null ||
    timestampIso === null
  ) {
    return null;
  }
  return { eventType, resourceKind, id, lifecycle, detail, timestampIso };
}

// === 2. HTTP Transport Helper ===

async function requestJson<T>(
  method: string,
  path: string,
  body: unknown,
  decoder: (raw: unknown) => T | null,
  options: ApiRequestOptions = {},
): Promise<ApiResult<T>> {
  const fetchImpl = options.fetchImpl ?? fetch;
  try {
    const headers: Record<string, string> = {
      Accept: 'application/json',
    };
    const init: RequestInit = {
      method,
      headers,
    };
    if (options.signal !== undefined) {
      init.signal = options.signal;
    }
    if (body !== undefined) {
      headers['Content-Type'] = 'application/json';
      init.body = JSON.stringify(body);
    }

    const response = await fetchImpl(path, init);
    let parsed: unknown = null;
    try {
      parsed = (await response.json()) as unknown;
    } catch (err) {
      if (options.signal?.aborted) {
        throw err;
      }
      parsed = null;
    }

    if (!response.ok) {
      let code = `http_${String(response.status)}`;
      let message = `Request ${method} ${path} failed with HTTP ${String(response.status)}`;
      if (isRecord(parsed)) {
        const errCode = asString(parsed['error']);
        const errMsg = asString(parsed['message']);
        if (errCode !== null && errCode.length > 0) {
          code = errCode;
        }
        if (errMsg !== null && errMsg.length > 0) {
          message = errMsg;
        } else if (errCode !== null && errCode.length > 0) {
          message = `${errCode} (HTTP ${String(response.status)})`;
        }
      }
      return {
        ok: false,
        error: { code, message, httpStatus: response.status },
      };
    }

    const decoded = decoder(parsed);
    if (decoded === null) {
      return {
        ok: false,
        error: {
          code: 'malformed_response',
          message: `Malformed response payload from ${method} ${path}`,
          httpStatus: response.status,
        },
      };
    }
    return { ok: true, data: decoded };
  } catch (err) {
    const message = err instanceof Error ? err.message : 'Network request failed';
    return {
      ok: false,
      error: {
        code: 'network_error',
        message,
        httpStatus: null,
      },
    };
  }
}

// === 3. Endpoint Functions ===

export function fetchWorkspace(options?: ApiRequestOptions): Promise<ApiResult<WorkspaceSummary>> {
  return requestJson('GET', '/api/workspace', undefined, decodeWorkspaceSummary, options);
}

export function createStyle(
  payload: CreateStylePayload,
  options?: ApiRequestOptions,
): Promise<ApiResult<StyleSummary>> {
  return requestJson('POST', '/api/styles', payload, decodeStyleDto, options);
}

export function updateStyle(
  slug: string,
  payload: UpdateStylePayload,
  options?: ApiRequestOptions,
): Promise<ApiResult<StyleSummary>> {
  return requestJson(
    'PUT',
    `/api/styles/${encodeURIComponent(slug)}`,
    payload,
    decodeStyleDto,
    options,
  );
}

export function uploadStyleExemplars(
  slug: string,
  payload: UploadExemplarsPayload,
  options?: ApiRequestOptions,
): Promise<ApiResult<StyleSummary>> {
  return requestJson(
    'POST',
    `/api/styles/${encodeURIComponent(slug)}/exemplars`,
    payload,
    decodeStyleDto,
    options,
  );
}

export function promoteStyle(
  slug: string,
  targetLifecycle?: LifecycleState,
  options?: ApiRequestOptions,
): Promise<ApiResult<PromoteStyleResponse>> {
  const body = targetLifecycle !== undefined ? { targetLifecycle } : undefined;
  return requestJson(
    'POST',
    `/api/styles/${encodeURIComponent(slug)}/promote`,
    body,
    (raw) => {
      if (!isRecord(raw)) {
        return null;
      }
      const style = decodeStyleDto(raw['style']);
      const rawLock = raw['manifestLock'];
      const manifestLock = rawLock === null ? null : decodeManifestLock(rawLock);
      if (style === null || (rawLock !== null && manifestLock === null)) {
        return null;
      }
      return { style, manifestLock };
    },
    options,
  );
}

export function createAtmosphere(
  payload: CreateAtmospherePayload,
  options?: ApiRequestOptions,
): Promise<ApiResult<AtmosphereSummary>> {
  return requestJson('POST', '/api/atmospheres', payload, decodeAtmosphereDto, options);
}

export function updateAtmosphere(
  slug: string,
  payload: UpdateAtmospherePayload,
  options?: ApiRequestOptions,
): Promise<ApiResult<AtmosphereSummary>> {
  return requestJson(
    'PUT',
    `/api/atmospheres/${encodeURIComponent(slug)}`,
    payload,
    decodeAtmosphereDto,
    options,
  );
}

export function updateSubjectPaletteLock(
  slug: string,
  payload: UpdatePaletteLockPayload,
  options?: ApiRequestOptions,
): Promise<ApiResult<SubjectSummary>> {
  return requestJson(
    'POST',
    `/api/subjects/${encodeURIComponent(slug)}/palette-lock`,
    payload,
    decodeSubjectDto,
    options,
  );
}

export function validateGuideRemote(
  spec: unknown,
  options?: ApiRequestOptions,
): Promise<ApiResult<GuideValidateResponse>> {
  return requestJson(
    'POST',
    '/api/guide/validate',
    { guideSpec: spec },
    (raw) => {
      if (!isRecord(raw)) {
        return null;
      }
      const valid = asBoolean(raw['valid']);
      const errors = decodeGuideErrors(raw['errors']);
      if (valid === null || errors === null) {
        return null;
      }
      let decodedSpec: GuideSpec | null = null;
      if (raw['spec'] !== null && raw['spec'] !== undefined) {
        const res = validateGuideSpec(raw['spec']);
        if (res.ok) {
          decodedSpec = res.value;
        }
      }
      return { valid, spec: decodedSpec, errors };
    },
    options,
  );
}

export function renderGuideRemote(
  spec: GuideSpec,
  options?: ApiRequestOptions,
): Promise<ApiResult<GuideRenderResponse>> {
  return requestJson(
    'POST',
    '/api/guide/render',
    { guideSpec: spec },
    (raw) => {
      if (!isRecord(raw)) {
        return null;
      }
      const id = asString(raw['id']);
      const variantRaw = raw['variant'];
      const variant =
        variantRaw === 'OverlayGuide' || variantRaw === 'Blockout' ? variantRaw : null;
      const width = asNumber(raw['width']);
      const height = asNumber(raw['height']);
      const pngBase64 = asString(raw['pngBase64']);
      const errors = decodeGuideErrors(raw['errors']);
      if (
        id === null ||
        variant === null ||
        width === null ||
        height === null ||
        pngBase64 === null ||
        errors === null
      ) {
        return null;
      }
      return { id, variant, width, height, pngBase64, errors };
    },
    options,
  );
}

export function resliceReferencePack(
  subjectSlug: string,
  styleSlug: string,
  payload: ReslicePackPayload,
  options?: ApiRequestOptions,
): Promise<ApiResult<ReslicePackResponse>> {
  return requestJson(
    'POST',
    `/api/packs/${encodeURIComponent(subjectSlug)}/${encodeURIComponent(styleSlug)}/reslice`,
    payload,
    (raw) => {
      if (!isRecord(raw)) {
        return null;
      }
      const schemaVersion = asNumber(raw['schemaVersion']);
      const sheetFilename = asString(raw['sheetFilename']);
      const sheetWidth = asNumber(raw['sheetWidth']);
      const sheetHeight = asNumber(raw['sheetHeight']);
      const calibrationStripHeight = asNumber(raw['calibrationStripHeight']);
      if (
        schemaVersion === null ||
        sheetFilename === null ||
        sheetWidth === null ||
        sheetHeight === null ||
        calibrationStripHeight === null ||
        !Array.isArray(raw['slices'])
      ) {
        return null;
      }
      const slices: SliceRecordDto[] = [];
      for (const item of raw['slices']) {
        const decoded = decodeSliceDto(item);
        if (decoded === null) {
          return null;
        }
        slices.push(decoded);
      }
      return {
        schemaVersion,
        sheetFilename,
        sheetWidth,
        sheetHeight,
        calibrationStripHeight,
        slices,
      };
    },
    options,
  );
}

export function validateReferencePack(
  subjectSlug: string,
  styleSlug: string,
  payload: ValidatePackPayload = {},
  options?: ApiRequestOptions,
): Promise<ApiResult<ValidatePackResponse>> {
  return requestJson(
    'POST',
    `/api/packs/${encodeURIComponent(subjectSlug)}/${encodeURIComponent(styleSlug)}/validate`,
    payload,
    (raw) => {
      if (!isRecord(raw)) {
        return null;
      }
      const referencePack = decodeReferencePackDto(raw['referencePack']);
      const checks = asStringArray(raw['checks']);
      const rawCritic = raw['criticReport'];
      const criticReport = rawCritic === null ? null : decodeCriticReport(rawCritic);
      if (
        referencePack === null ||
        checks === null ||
        (rawCritic !== null && criticReport === null)
      ) {
        return null;
      }
      return { referencePack, checks, criticReport };
    },
    options,
  );
}

export function promoteReferencePack(
  subjectSlug: string,
  styleSlug: string,
  options?: ApiRequestOptions,
): Promise<ApiResult<PromotePackResponse>> {
  return requestJson(
    'POST',
    `/api/packs/${encodeURIComponent(subjectSlug)}/${encodeURIComponent(styleSlug)}/promote`,
    undefined,
    (raw) => {
      if (!isRecord(raw)) {
        return null;
      }
      const referencePack = decodeReferencePackDto(raw['referencePack']);
      const canonicalCount = asNumber(raw['canonicalCount']);
      const supersededCount = asNumber(raw['supersededCount']);
      if (referencePack === null || canonicalCount === null || supersededCount === null) {
        return null;
      }
      return { referencePack, canonicalCount, supersededCount };
    },
    options,
  );
}

export function executePatchEdit(
  payload: PatchEditApiPayload,
  options?: ApiRequestOptions,
): Promise<ApiResult<PatchEditResponse>> {
  return requestJson(
    'POST',
    '/api/edit/patch',
    payload,
    (raw) => {
      if (!isRecord(raw)) {
        return null;
      }
      const run = decodeRunDto(raw['run']);
      const beforeImageUrl = asString(raw['beforeImageUrl']);
      const afterImageUrl = asString(raw['afterImageUrl']);
      const cropRect = decodePixelRect(raw['cropRect']);
      const roiRect = decodePixelRect(raw['roiRect']);
      const reinhardGains = decodeReinhardGains(raw['reinhardGains']);
      const colorQa = decodeColorQaReport(raw['colorQa']);
      if (
        run === null ||
        beforeImageUrl === null ||
        afterImageUrl === null ||
        cropRect === null ||
        roiRect === null ||
        reinhardGains === null ||
        colorQa === null
      ) {
        return null;
      }
      return {
        run,
        beforeImageUrl,
        afterImageUrl,
        cropRect,
        roiRect,
        reinhardGains,
        colorQa,
      };
    },
    options,
  );
}

function decodeHeatmaps(val: unknown): InspectionHeatmaps | null {
  if (!isRecord(val)) {
    return null;
  }
  const keys: readonly HeatmapKindKey[] = ['drift', 'bleed', 'sparkle', 'outOfRoi'];
  const decoded: Record<HeatmapKindKey, string | null> = {
    drift: null,
    bleed: null,
    sparkle: null,
    outOfRoi: null,
  };
  for (const key of keys) {
    const raw = val[key];
    if (raw === null || raw === undefined) {
      decoded[key] = null;
    } else if (typeof raw === 'string') {
      decoded[key] = raw;
    } else {
      return null;
    }
  }
  return decoded;
}

export function inspectRun(
  slug: string,
  version: number,
  payload: InspectRunPayload = {},
  options?: ApiRequestOptions,
): Promise<ApiResult<InspectRunResponse>> {
  return requestJson(
    'POST',
    `/api/runs/${encodeURIComponent(slug)}/${String(version)}/inspect`,
    payload,
    (raw) => {
      if (!isRecord(raw)) {
        return null;
      }
      const run = decodeRunDto(raw['run']);
      const telemetry = decodeTelemetry(raw['telemetry']);
      const colorQa = decodeColorQaReport(raw['colorQa']);
      const heatmaps = decodeHeatmaps(raw['heatmaps']);
      if (run === null || telemetry === null || colorQa === null || heatmaps === null) {
        return null;
      }
      return { run, telemetry, colorQa, heatmaps };
    },
    options,
  );
}

export function validateRun(
  slug: string,
  version: number,
  payload: ValidateRunPayload = {},
  options?: ApiRequestOptions,
): Promise<ApiResult<ValidateRunResponse>> {
  return requestJson(
    'POST',
    `/api/runs/${encodeURIComponent(slug)}/${String(version)}/validate`,
    payload,
    (raw) => {
      if (!isRecord(raw)) {
        return null;
      }
      const run = decodeRunDto(raw['run']);
      const colorQa = decodeColorQaReport(raw['colorQa']);
      const rawCritic = raw['criticReport'];
      const criticReport = rawCritic === null ? null : decodeCriticReport(rawCritic);
      if (run === null || colorQa === null || (rawCritic !== null && criticReport === null)) {
        return null;
      }
      return { run, colorQa, criticReport };
    },
    options,
  );
}

export function promoteRun(
  slug: string,
  version: number,
  options?: ApiRequestOptions,
): Promise<ApiResult<PromoteRunResponse>> {
  return requestJson(
    'POST',
    `/api/runs/${encodeURIComponent(slug)}/${String(version)}/promote`,
    undefined,
    (raw) => {
      if (!isRecord(raw)) {
        return null;
      }
      const run = decodeRunDto(raw['run']);
      const canonicalCount = asNumber(raw['canonicalCount']);
      const supersededCount = asNumber(raw['supersededCount']);
      if (run === null || canonicalCount === null || supersededCount === null) {
        return null;
      }
      return { run, canonicalCount, supersededCount };
    },
    options,
  );
}

export function exportRun(
  payload: ExportApiPayload,
  options?: ApiRequestOptions,
): Promise<ApiResult<ExportResponse>> {
  return requestJson(
    'POST',
    '/api/export',
    payload,
    (raw) => {
      if (!isRecord(raw)) {
        return null;
      }
      const sourceId = asString(raw['sourceId']);
      const sourceVisibility = asVisibility(raw['sourceVisibility']);
      const profileVisibility = asVisibility(raw['profileVisibility']);
      const matchGradeApplied = asBoolean(raw['matchGradeApplied']);
      if (
        sourceId === null ||
        sourceVisibility === null ||
        profileVisibility === null ||
        matchGradeApplied === null ||
        !Array.isArray(raw['artifacts'])
      ) {
        return null;
      }
      const artifacts: ExportArtifactDto[] = [];
      for (const item of raw['artifacts']) {
        if (!isRecord(item)) {
          return null;
        }
        const role = asString(item['role']);
        const path = asString(item['path']);
        const assetUrl = asNullableString(item['assetUrl']);
        const width = asNumber(item['width']);
        const height = asNumber(item['height']);
        const sha256 = asString(item['sha256']);
        if (
          role === null ||
          path === null ||
          assetUrl === undefined ||
          width === null ||
          height === null ||
          sha256 === null
        ) {
          return null;
        }
        artifacts.push({ role, path, assetUrl, width, height, sha256 });
      }
      return {
        sourceId,
        sourceVisibility,
        profileVisibility,
        matchGradeApplied,
        artifacts,
      };
    },
    options,
  );
}

// === 4. WebSocket Live Workspace Events ===

export type WsConnectionStatus = 'connecting' | 'connected' | 'reconnecting' | 'closed';

export interface WorkspaceWsCallbacks {
  readonly onEvent: (event: WorkspaceEvent) => void;
  readonly onStatusChange?: (status: WsConnectionStatus) => void;
  readonly buildUrl?: () => string;
  readonly webSocketFactory?: (url: string) => WebSocket;
}

/** Build the default same-origin WebSocket URL for `/api/ws`. */
export function buildDefaultWorkspaceWsUrl(): string {
  const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
  return `${protocol}//${window.location.host}/api/ws`;
}

/** Subscribe to `/api/ws` with graceful bounded exponential-backoff reconnect. */
export function connectWorkspaceEvents(callbacks: WorkspaceWsCallbacks): () => void {
  let closed = false;
  let attempt = 0;
  let socket: WebSocket | null = null;
  let retryTimer: ReturnType<typeof setTimeout> | null = null;

  const scheduleReconnect = (): void => {
    if (closed) {
      return;
    }
    callbacks.onStatusChange?.('reconnecting');
    const delayMs = Math.min(10_000, 750 * Math.pow(2, attempt));
    attempt = Math.min(attempt + 1, 5);
    retryTimer = setTimeout(() => {
      retryTimer = null;
      openSocket();
    }, delayMs);
  };

  const openSocket = (): void => {
    if (closed) {
      return;
    }
    const url = callbacks.buildUrl ? callbacks.buildUrl() : buildDefaultWorkspaceWsUrl();
    callbacks.onStatusChange?.(attempt === 0 ? 'connecting' : 'reconnecting');
    try {
      const factory =
        callbacks.webSocketFactory ?? ((targetUrl: string) => new WebSocket(targetUrl));
      const ws = factory(url);
      socket = ws;

      ws.addEventListener('open', () => {
        attempt = 0;
        callbacks.onStatusChange?.('connected');
      });

      ws.addEventListener('message', (msgEvent: MessageEvent<unknown>) => {
        if (typeof msgEvent.data !== 'string') {
          return;
        }
        try {
          const parsed = JSON.parse(msgEvent.data) as unknown;
          const ev = decodeWorkspaceEvent(parsed);
          if (ev !== null) {
            callbacks.onEvent(ev);
          }
        } catch {
          // Ignore malformed non-WorkspaceEvent frames.
        }
      });

      ws.addEventListener('close', () => {
        if (socket === ws) {
          socket = null;
        }
        if (!closed) {
          scheduleReconnect();
        }
      });

      ws.addEventListener('error', () => {
        ws.close();
      });
    } catch {
      scheduleReconnect();
    }
  };

  openSocket();

  return () => {
    closed = true;
    if (retryTimer !== null) {
      clearTimeout(retryTimer);
      retryTimer = null;
    }
    if (socket !== null) {
      socket.close();
      socket = null;
    }
    callbacks.onStatusChange?.('closed');
  };
}

// === 5. PNG File & Clipboard Base64 Helpers ===

/** Strip any `data:image/...;base64,` prefix and read raw base64 from a Blob/File. */
export async function blobToRawBase64(blob: Blob): Promise<string> {
  const buffer = await blob.arrayBuffer();
  const bytes = new Uint8Array(buffer);
  let binary = '';
  const chunkSize = 8192;
  for (let i = 0; i < bytes.length; i += chunkSize) {
    const chunk = bytes.subarray(i, i + chunkSize);
    binary += String.fromCharCode(...chunk);
  }
  return btoa(binary);
}

/** Extract all PNG `File` items from a `DataTransfer` (drag-drop or `Ctrl+V` clipboard paste). */
export function extractPngFilesFromDataTransfer(
  dataTransfer: DataTransfer | null,
): readonly File[] {
  if (dataTransfer === null) {
    return [];
  }
  const files: File[] = [];
  for (let i = 0; i < dataTransfer.files.length; i += 1) {
    const f = dataTransfer.files.item(i);
    if (f !== null && (f.type === 'image/png' || f.name.toLowerCase().endsWith('.png'))) {
      files.push(f);
    }
  }
  if (files.length === 0) {
    for (let i = 0; i < dataTransfer.items.length; i += 1) {
      const item = dataTransfer.items[i];
      if (item && item.kind === 'file' && item.type === 'image/png') {
        const f = item.getAsFile();
        if (f !== null) {
          files.push(f);
        }
      }
    }
  }
  return files;
}
