/** === GUIDESPEC SCHEMA & DEFENSIVE SERIALIZATION ===
 *
 * PURPOSE:
 * Defines strict TypeScript types for `GuideSpec` (`SpatialGuide` variants:
 * `OverlayGuide` and `Blockout`) plus fail-closed JSON deserialization and
 * serialization helpers shared between the Web Canvas SPA and Rust `GuideRenderer`.
 *
 * DESIGN RATIONALE ("WHY"):
 * Hand-edited or externally fetched `GuideSpec` JSON can contain out-of-range
 * coordinates, inverted depth bands, invalid hex colors, or missing discriminants.
 * Silent coercion would cause `GuideRenderer` (`tiny-skia`/`resvg`) to draw
 * misaligned `SpatialGuide` cues or fail mid-pipeline. All parsing therefore
 * validates every structural and geometric invariant upfront and returns
 * structured errors.
 *
 * EXECUTION LIFECYCLE / DATA FLOW:
 * 1. `parseGuideSpecJson(raw)` parses JSON, checks top-level fields, validates
 *    variant invariants (`OverlayGuide` vs `Blockout`), depth bands, `Subject`
 *    mannequins, action vectors, and overlay regions.
 * 2. `serializeGuideSpecJson(spec)` validates `spec` before emitting JSON so
 *    invalid in-memory edits never leave the client.
 */

/** Discriminator selecting the `SpatialGuide` rendering mode in `GuideRenderer`. */
export type SpatialGuideVariant = 'OverlayGuide' | 'Blockout';

/** Depth plane assignment for `Blockout` depth-band zones. */
export type DepthPlane = 'background' | 'midground' | 'foreground';

/** Canonical facing orientation for a scaled `Subject` mannequin. */
export type MannequinFacing = 'left' | 'right' | 'front' | 'back';

/** Stroke pattern for an action vector inside a `SpatialGuide`. */
export type VectorStrokeStyle = 'solid' | 'dashed';

/** Geometric primitive kind for an `OverlayGuide` region cue. */
export type OverlayRegionKind = 'rect' | 'ellipse' | 'polyline';

/** Normalized 2D coordinate in `[0, 1]` relative to canvas width/height. */
export interface NormalizedPoint {
  readonly xRatio: number;
  readonly yRatio: number;
}

/** Horizontal depth-band zone inside a `Blockout` `GuideSpec`. */
export interface DepthBand {
  readonly id: string;
  readonly label: string;
  readonly plane: DepthPlane;
  readonly yStartRatio: number;
  readonly yEndRatio: number;
  readonly fillHex: string;
}

/** Proportionally scaled `Subject` mannequin placed on the spatial canvas. */
export interface SubjectMannequin {
  readonly id: string;
  readonly subjectSlug: string;
  readonly label: string;
  readonly xRatio: number;
  readonly yRatio: number;
  readonly heightMeters: number;
  readonly scaleFactor: number;
  readonly facing: MannequinFacing;
  readonly poseHint: string;
  readonly paletteHex: string;
}

/** Haloed trajectory or facing action vector drawn onto a `SpatialGuide`. */
export interface ActionVector {
  readonly id: string;
  readonly label: string;
  readonly fromXRatio: number;
  readonly fromYRatio: number;
  readonly toXRatio: number;
  readonly toYRatio: number;
  readonly strokeHex: string;
  readonly haloHex: string;
  readonly strokeWidthPx: number;
  readonly style: VectorStrokeStyle;
}

/** Haloed vector region or path drawn in `OverlayGuide` (or annotated in `Blockout`). */
export interface OverlayRegion {
  readonly id: string;
  readonly label: string;
  readonly kind: OverlayRegionKind;
  readonly points: readonly NormalizedPoint[];
  readonly strokeHex: string;
  readonly haloHex: string;
  readonly fillHex: string;
  readonly opacity: number;
}

/** Base fields shared by every `GuideSpec` document. */
interface GuideSpecBase {
  readonly schemaVersion: '1.0';
  readonly id: string;
  readonly title: string;
  readonly canvasWidth: number;
  readonly canvasHeight: number;
  readonly depthBands: readonly DepthBand[];
  readonly mannequins: readonly SubjectMannequin[];
  readonly actionVectors: readonly ActionVector[];
  readonly overlayRegions: readonly OverlayRegion[];
}

/** `OverlayGuide` variant drawing haloed cues onto a copy of an existing target image. */
export interface OverlayGuideSpec extends GuideSpecBase {
  readonly variant: 'OverlayGuide';
  readonly targetImagePath: string;
}

/** `Blockout` variant rendered from scratch with depth bands, mannequins, and action vectors. */
export interface BlockoutGuideSpec extends GuideSpecBase {
  readonly variant: 'Blockout';
  readonly targetImagePath: null;
}

/** Shared `GuideSpec` JSON schema edited in the Web Canvas SPA and consumed by `GuideRenderer`. */
export type GuideSpec = OverlayGuideSpec | BlockoutGuideSpec;

/** Single structured validation failure produced during `GuideSpec` validation. */
export interface GuideSpecValidationError {
  readonly path: string;
  readonly message: string;
}

/** Result of parsing or validating a `GuideSpec` payload. */
export type GuideSpecParseResult =
  | { readonly ok: true; readonly value: GuideSpec }
  | { readonly ok: false; readonly errors: readonly GuideSpecValidationError[] };

/** Thrown when attempting to serialize an invalid `GuideSpec` object. */
export class GuideSpecValidationException extends Error {
  public readonly errors: readonly GuideSpecValidationError[];

  public constructor(errors: readonly GuideSpecValidationError[]) {
    const summary = errors.map((entry) => `${entry.path}: ${entry.message}`).join('; ');
    super(`Invalid GuideSpec: ${summary}`);
    this.name = 'GuideSpecValidationException';
    this.errors = errors;
  }
}

const HEX_COLOR_RE = /^#[0-9a-fA-F]{6}$/;

function isDepthPlane(value: unknown): value is DepthPlane {
  return value === 'background' || value === 'midground' || value === 'foreground';
}

function isMannequinFacing(value: unknown): value is MannequinFacing {
  return value === 'left' || value === 'right' || value === 'front' || value === 'back';
}

function isVectorStrokeStyle(value: unknown): value is VectorStrokeStyle {
  return value === 'solid' || value === 'dashed';
}

function isOverlayRegionKind(value: unknown): value is OverlayRegionKind {
  return value === 'rect' || value === 'ellipse' || value === 'polyline';
}

function isPlainRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function readNonEmptyString(
  record: Record<string, unknown>,
  key: string,
  path: string,
  errors: GuideSpecValidationError[],
): string {
  const raw = record[key];
  if (typeof raw !== 'string' || raw.trim().length === 0) {
    errors.push({ path, message: 'must be a non-empty string' });
    return '';
  }
  return raw;
}

function readHexColor(
  record: Record<string, unknown>,
  key: string,
  path: string,
  errors: GuideSpecValidationError[],
): string {
  const raw = record[key];
  if (typeof raw !== 'string' || !HEX_COLOR_RE.test(raw)) {
    errors.push({ path, message: 'must be a 6-digit hex color (#RRGGBB)' });
    return '#000000';
  }
  return raw.toUpperCase();
}

function readPositiveInt(
  record: Record<string, unknown>,
  key: string,
  path: string,
  errors: GuideSpecValidationError[],
): number {
  const raw = record[key];
  if (typeof raw !== 'number' || !Number.isInteger(raw) || raw <= 0 || raw > 16384) {
    errors.push({ path, message: 'must be an integer between 1 and 16384' });
    return 1;
  }
  return raw;
}

function readPositiveFiniteNumber(
  record: Record<string, unknown>,
  key: string,
  path: string,
  errors: GuideSpecValidationError[],
): number {
  const raw = record[key];
  if (typeof raw !== 'number' || !Number.isFinite(raw) || raw <= 0) {
    errors.push({ path, message: 'must be a positive finite number' });
    return 1;
  }
  return raw;
}

function readUnitRatio(
  record: Record<string, unknown>,
  key: string,
  path: string,
  errors: GuideSpecValidationError[],
): number {
  const raw = record[key];
  if (typeof raw !== 'number' || !Number.isFinite(raw) || raw < 0 || raw > 1) {
    errors.push({ path, message: 'must be a finite number in [0, 1]' });
    return 0;
  }
  return raw;
}

function validateDepthBand(
  item: unknown,
  index: number,
  errors: GuideSpecValidationError[],
): DepthBand | null {
  const basePath = `depthBands[${String(index)}]`;
  if (!isPlainRecord(item)) {
    errors.push({ path: basePath, message: 'must be an object' });
    return null;
  }
  const id = readNonEmptyString(item, 'id', `${basePath}.id`, errors);
  const label = readNonEmptyString(item, 'label', `${basePath}.label`, errors);
  const rawPlane = item['plane'];
  let plane: DepthPlane = 'midground';
  if (isDepthPlane(rawPlane)) {
    plane = rawPlane;
  } else {
    errors.push({
      path: `${basePath}.plane`,
      message: 'must be one of background | midground | foreground',
    });
  }
  const yStartRatio = readUnitRatio(item, 'yStartRatio', `${basePath}.yStartRatio`, errors);
  const yEndRatio = readUnitRatio(item, 'yEndRatio', `${basePath}.yEndRatio`, errors);
  if (yStartRatio >= yEndRatio) {
    errors.push({
      path: `${basePath}.yEndRatio`,
      message: 'must be strictly greater than yStartRatio',
    });
  }
  const fillHex = readHexColor(item, 'fillHex', `${basePath}.fillHex`, errors);
  return {
    id,
    label,
    plane,
    yStartRatio,
    yEndRatio,
    fillHex,
  };
}

function validateMannequin(
  item: unknown,
  index: number,
  errors: GuideSpecValidationError[],
): SubjectMannequin | null {
  const basePath = `mannequins[${String(index)}]`;
  if (!isPlainRecord(item)) {
    errors.push({ path: basePath, message: 'must be an object' });
    return null;
  }
  const id = readNonEmptyString(item, 'id', `${basePath}.id`, errors);
  const subjectSlug = readNonEmptyString(item, 'subjectSlug', `${basePath}.subjectSlug`, errors);
  const label = readNonEmptyString(item, 'label', `${basePath}.label`, errors);
  const xRatio = readUnitRatio(item, 'xRatio', `${basePath}.xRatio`, errors);
  const yRatio = readUnitRatio(item, 'yRatio', `${basePath}.yRatio`, errors);
  const heightMeters = readPositiveFiniteNumber(
    item,
    'heightMeters',
    `${basePath}.heightMeters`,
    errors,
  );
  const scaleFactor = readPositiveFiniteNumber(
    item,
    'scaleFactor',
    `${basePath}.scaleFactor`,
    errors,
  );
  const rawFacing = item['facing'];
  let facing: MannequinFacing = 'front';
  if (isMannequinFacing(rawFacing)) {
    facing = rawFacing;
  } else {
    errors.push({
      path: `${basePath}.facing`,
      message: 'must be one of left | right | front | back',
    });
  }
  const poseHint =
    typeof item['poseHint'] === 'string'
      ? item['poseHint']
      : (errors.push({ path: `${basePath}.poseHint`, message: 'must be a string' }), '');
  const paletteHex = readHexColor(item, 'paletteHex', `${basePath}.paletteHex`, errors);
  return {
    id,
    subjectSlug,
    label,
    xRatio,
    yRatio,
    heightMeters,
    scaleFactor,
    facing,
    poseHint,
    paletteHex,
  };
}

function validateActionVector(
  item: unknown,
  index: number,
  errors: GuideSpecValidationError[],
): ActionVector | null {
  const basePath = `actionVectors[${String(index)}]`;
  if (!isPlainRecord(item)) {
    errors.push({ path: basePath, message: 'must be an object' });
    return null;
  }
  const id = readNonEmptyString(item, 'id', `${basePath}.id`, errors);
  const label = readNonEmptyString(item, 'label', `${basePath}.label`, errors);
  const fromXRatio = readUnitRatio(item, 'fromXRatio', `${basePath}.fromXRatio`, errors);
  const fromYRatio = readUnitRatio(item, 'fromYRatio', `${basePath}.fromYRatio`, errors);
  const toXRatio = readUnitRatio(item, 'toXRatio', `${basePath}.toXRatio`, errors);
  const toYRatio = readUnitRatio(item, 'toYRatio', `${basePath}.toYRatio`, errors);
  const strokeHex = readHexColor(item, 'strokeHex', `${basePath}.strokeHex`, errors);
  const haloHex = readHexColor(item, 'haloHex', `${basePath}.haloHex`, errors);
  const strokeWidthPx = readPositiveFiniteNumber(
    item,
    'strokeWidthPx',
    `${basePath}.strokeWidthPx`,
    errors,
  );
  const rawStyle = item['style'];
  let style: VectorStrokeStyle = 'solid';
  if (isVectorStrokeStyle(rawStyle)) {
    style = rawStyle;
  } else {
    errors.push({ path: `${basePath}.style`, message: 'must be one of solid | dashed' });
  }
  return {
    id,
    label,
    fromXRatio,
    fromYRatio,
    toXRatio,
    toYRatio,
    strokeHex,
    haloHex,
    strokeWidthPx,
    style,
  };
}

function validateOverlayRegion(
  item: unknown,
  index: number,
  errors: GuideSpecValidationError[],
): OverlayRegion | null {
  const basePath = `overlayRegions[${String(index)}]`;
  if (!isPlainRecord(item)) {
    errors.push({ path: basePath, message: 'must be an object' });
    return null;
  }
  const id = readNonEmptyString(item, 'id', `${basePath}.id`, errors);
  const label = readNonEmptyString(item, 'label', `${basePath}.label`, errors);
  const rawKind = item['kind'];
  let kind: OverlayRegionKind = 'rect';
  if (isOverlayRegionKind(rawKind)) {
    kind = rawKind;
  } else {
    errors.push({
      path: `${basePath}.kind`,
      message: 'must be one of rect | ellipse | polyline',
    });
  }
  const rawPoints = item['points'];
  const points: NormalizedPoint[] = [];
  if (!Array.isArray(rawPoints) || rawPoints.length < 2) {
    errors.push({ path: `${basePath}.points`, message: 'must contain at least 2 points' });
  } else {
    rawPoints.forEach((pt, pointIndex) => {
      const pointPath = `${basePath}.points[${String(pointIndex)}]`;
      if (!isPlainRecord(pt)) {
        errors.push({ path: pointPath, message: 'must be an object' });
        return;
      }
      points.push({
        xRatio: readUnitRatio(pt, 'xRatio', `${pointPath}.xRatio`, errors),
        yRatio: readUnitRatio(pt, 'yRatio', `${pointPath}.yRatio`, errors),
      });
    });
  }
  const strokeHex = readHexColor(item, 'strokeHex', `${basePath}.strokeHex`, errors);
  const haloHex = readHexColor(item, 'haloHex', `${basePath}.haloHex`, errors);
  const fillHex = readHexColor(item, 'fillHex', `${basePath}.fillHex`, errors);
  const opacity = readUnitRatio(item, 'opacity', `${basePath}.opacity`, errors);
  return {
    id,
    label,
    kind,
    points,
    strokeHex,
    haloHex,
    fillHex,
    opacity,
  };
}

/** Validate an arbitrary parsed value against the canonical `GuideSpec` schema. */
export function validateGuideSpec(input: unknown): GuideSpecParseResult {
  const errors: GuideSpecValidationError[] = [];
  if (!isPlainRecord(input)) {
    return {
      ok: false,
      errors: [{ path: '$', message: 'GuideSpec root must be a JSON object' }],
    };
  }

  if (input['schemaVersion'] !== '1.0') {
    errors.push({ path: 'schemaVersion', message: 'must equal "1.0"' });
  }
  const id = readNonEmptyString(input, 'id', 'id', errors);
  const title = readNonEmptyString(input, 'title', 'title', errors);
  const canvasWidth = readPositiveInt(input, 'canvasWidth', 'canvasWidth', errors);
  const canvasHeight = readPositiveInt(input, 'canvasHeight', 'canvasHeight', errors);

  const rawVariant = input['variant'];
  if (rawVariant !== 'OverlayGuide' && rawVariant !== 'Blockout') {
    errors.push({
      path: 'variant',
      message: 'must be either "OverlayGuide" or "Blockout"',
    });
  }

  const rawTargetImagePath = input['targetImagePath'];
  if (rawVariant === 'OverlayGuide') {
    if (typeof rawTargetImagePath !== 'string' || rawTargetImagePath.trim().length === 0) {
      errors.push({
        path: 'targetImagePath',
        message: 'OverlayGuide requires a non-empty targetImagePath string',
      });
    }
  } else if (rawVariant === 'Blockout' && rawTargetImagePath !== null) {
    errors.push({
      path: 'targetImagePath',
      message: 'Blockout must set targetImagePath to null',
    });
  }

  const rawDepthBands = input['depthBands'];
  const depthBands: DepthBand[] = [];
  if (!Array.isArray(rawDepthBands)) {
    errors.push({ path: 'depthBands', message: 'must be an array' });
  } else {
    rawDepthBands.forEach((band, index) => {
      const validated = validateDepthBand(band, index, errors);
      if (validated !== null) {
        depthBands.push(validated);
      }
    });
  }

  const rawMannequins = input['mannequins'];
  const mannequins: SubjectMannequin[] = [];
  if (!Array.isArray(rawMannequins)) {
    errors.push({ path: 'mannequins', message: 'must be an array' });
  } else {
    rawMannequins.forEach((mannequin, index) => {
      const validated = validateMannequin(mannequin, index, errors);
      if (validated !== null) {
        mannequins.push(validated);
      }
    });
  }

  const rawActionVectors = input['actionVectors'];
  const actionVectors: ActionVector[] = [];
  if (!Array.isArray(rawActionVectors)) {
    errors.push({ path: 'actionVectors', message: 'must be an array' });
  } else {
    rawActionVectors.forEach((vector, index) => {
      const validated = validateActionVector(vector, index, errors);
      if (validated !== null) {
        actionVectors.push(validated);
      }
    });
  }

  const rawOverlayRegions = input['overlayRegions'];
  const overlayRegions: OverlayRegion[] = [];
  if (!Array.isArray(rawOverlayRegions)) {
    errors.push({ path: 'overlayRegions', message: 'must be an array' });
  } else {
    rawOverlayRegions.forEach((region, index) => {
      const validated = validateOverlayRegion(region, index, errors);
      if (validated !== null) {
        overlayRegions.push(validated);
      }
    });
  }

  if (errors.length > 0) {
    return { ok: false, errors };
  }

  if (rawVariant === 'OverlayGuide' && typeof rawTargetImagePath === 'string') {
    return {
      ok: true,
      value: {
        schemaVersion: '1.0',
        id,
        title,
        variant: 'OverlayGuide',
        targetImagePath: rawTargetImagePath,
        canvasWidth,
        canvasHeight,
        depthBands,
        mannequins,
        actionVectors,
        overlayRegions,
      },
    };
  }

  return {
    ok: true,
    value: {
      schemaVersion: '1.0',
      id,
      title,
      variant: 'Blockout',
      targetImagePath: null,
      canvasWidth,
      canvasHeight,
      depthBands,
      mannequins,
      actionVectors,
      overlayRegions,
    },
  };
}

/** Defensively deserialize a raw JSON string into a validated `GuideSpec`. */
export function parseGuideSpecJson(rawJson: string): GuideSpecParseResult {
  if (rawJson.trim().length === 0) {
    return {
      ok: false,
      errors: [{ path: '$', message: 'Input JSON string must not be empty' }],
    };
  }
  let parsed: unknown;
  try {
    parsed = JSON.parse(rawJson) as unknown;
  } catch (err) {
    const detail = err instanceof Error ? err.message : 'Syntax error';
    return {
      ok: false,
      errors: [{ path: '$', message: `Malformed JSON: ${detail}` }],
    };
  }
  return validateGuideSpec(parsed);
}

/** Validate and serialize a `GuideSpec` into canonical JSON (fail-closed on invalid specs). */
export function serializeGuideSpecJson(spec: GuideSpec, pretty = true): string {
  const check = validateGuideSpec(spec);
  if (!check.ok) {
    throw new GuideSpecValidationException(check.errors);
  }
  return JSON.stringify(check.value, null, pretty ? 2 : undefined);
}

/** Create a canonical default `GuideSpec` for either `Blockout` or `OverlayGuide` editing. */
export function createDefaultGuideSpec(variant: SpatialGuideVariant = 'Blockout'): GuideSpec {
  const base = {
    schemaVersion: '1.0' as const,
    id: variant === 'Blockout' ? 'guide-blockout-001' : 'guide-overlay-001',
    title:
      variant === 'Blockout'
        ? 'Default Blockout SpatialGuide'
        : 'Default OverlayGuide Trajectory Cue',
    canvasWidth: 1024,
    canvasHeight: 1024,
    depthBands:
      variant === 'Blockout'
        ? ([
            {
              id: 'band-bg',
              label: 'Background Horizon',
              plane: 'background',
              yStartRatio: 0,
              yEndRatio: 0.36,
              fillHex: '#2C3344',
            },
            {
              id: 'band-mg',
              label: 'Midground Stage',
              plane: 'midground',
              yStartRatio: 0.36,
              yEndRatio: 0.72,
              fillHex: '#3B4254',
            },
            {
              id: 'band-fg',
              label: 'Foreground Anchor',
              plane: 'foreground',
              yStartRatio: 0.72,
              yEndRatio: 1,
              fillHex: '#4A5164',
            },
          ] satisfies DepthBand[])
        : [],
    mannequins:
      variant === 'Blockout'
        ? ([
            {
              id: 'mannequin-01',
              subjectSlug: 'subject-primary',
              label: 'Primary Subject (1.75m)',
              xRatio: 0.42,
              yRatio: 0.68,
              heightMeters: 1.75,
              scaleFactor: 1,
              facing: 'right',
              poseHint: 'Three-quarter ready stance',
              paletteHex: '#63B3ED',
            },
          ] satisfies SubjectMannequin[])
        : [],
    actionVectors: [
      {
        id: 'vector-01',
        label: 'Primary Motion Vector',
        fromXRatio: 0.42,
        fromYRatio: 0.56,
        toXRatio: 0.72,
        toYRatio: 0.44,
        strokeHex: '#F6AD55',
        haloHex: '#1A1D24',
        strokeWidthPx: 4,
        style: 'solid',
      },
    ] satisfies ActionVector[],
    overlayRegions:
      variant === 'OverlayGuide'
        ? ([
            {
              id: 'region-01',
              label: 'Target Adjustment ROI',
              kind: 'rect',
              points: [
                { xRatio: 0.3, yRatio: 0.25 },
                { xRatio: 0.7, yRatio: 0.65 },
              ],
              strokeHex: '#68D391',
              haloHex: '#1A1D24',
              fillHex: '#68D391',
              opacity: 0.18,
            },
          ] satisfies OverlayRegion[])
        : [],
  };

  if (variant === 'OverlayGuide') {
    return {
      ...base,
      variant: 'OverlayGuide',
      targetImagePath: 'runs/v001.png',
    };
  }

  return {
    ...base,
    variant: 'Blockout',
    targetImagePath: null,
  };
}
