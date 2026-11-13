/** === INTERACTIVE SPATIALGUIDE GEOMETRY HELPERS ===
 *
 * PURPOSE:
 * Pure immutable `GuideSpec` update functions for interactive Konva drag,
 * resize, and vector-drawing gestures in `StageCanvas.tsx`.
 */

import type { ActionVector, GuideSpec } from '../types/guideSpec';

/** Clamp a normalized coordinate ratio strictly into `[0, 1]`. */
export function clampUnitRatio(value: number): number {
  if (!Number.isFinite(value)) {
    return 0;
  }
  return Math.max(0, Math.min(1, Number(value.toFixed(4))));
}

/** Reposition a `SubjectMannequin` anchor (`xRatio`, `yRatio`) clamped to `[0, 1]`. */
export function moveMannequinInSpec(
  spec: GuideSpec,
  mannequinId: string,
  xRatio: number,
  yRatio: number,
): GuideSpec {
  const nextX = clampUnitRatio(xRatio);
  const nextY = clampUnitRatio(yRatio);
  return {
    ...spec,
    mannequins: spec.mannequins.map((m) =>
      m.id === mannequinId ? { ...m, xRatio: nextX, yRatio: nextY } : m,
    ),
  };
}

/** Update a `DepthBand`'s vertical bounds (`yEndRatio > yStartRatio + 0.02`). */
export function resizeDepthBandInSpec(
  spec: GuideSpec,
  bandId: string,
  yStartRatio: number,
  yEndRatio: number,
): GuideSpec {
  const start = clampUnitRatio(Math.min(yStartRatio, 0.98));
  const end = clampUnitRatio(Math.max(start + 0.02, yEndRatio));
  return {
    ...spec,
    depthBands: spec.depthBands.map((b) =>
      b.id === bandId ? { ...b, yStartRatio: start, yEndRatio: end } : b,
    ),
  };
}

/** Move either endpoint of an `ActionVector` clamped to `[0, 1]`. */
export function updateActionVectorInSpec(
  spec: GuideSpec,
  vectorId: string,
  fromXRatio: number,
  fromYRatio: number,
  toXRatio: number,
  toYRatio: number,
): GuideSpec {
  return {
    ...spec,
    actionVectors: spec.actionVectors.map((v) =>
      v.id === vectorId
        ? {
            ...v,
            fromXRatio: clampUnitRatio(fromXRatio),
            fromYRatio: clampUnitRatio(fromYRatio),
            toXRatio: clampUnitRatio(toXRatio),
            toYRatio: clampUnitRatio(toYRatio),
          }
        : v,
    ),
  };
}

/** Append a newly drawn `ActionVector` to `GuideSpec`. */
export function appendActionVectorToSpec(
  spec: GuideSpec,
  fromXRatio: number,
  fromYRatio: number,
  toXRatio: number,
  toYRatio: number,
): GuideSpec {
  const nextNum = spec.actionVectors.length + 1;
  const newVector: ActionVector = {
    id: `vector-${String(nextNum).padStart(2, '0')}`,
    label: `Action Vector ${String(nextNum)}`,
    fromXRatio: clampUnitRatio(fromXRatio),
    fromYRatio: clampUnitRatio(fromYRatio),
    toXRatio: clampUnitRatio(toXRatio),
    toYRatio: clampUnitRatio(toYRatio),
    strokeHex: '#F6AD55',
    haloHex: '#1A1D24',
    strokeWidthPx: 4,
    style: 'solid',
  };
  return {
    ...spec,
    actionVectors: [...spec.actionVectors, newVector],
  };
}

/** Move a vertex point inside an `OverlayRegion`. */
export function updateOverlayRegionPointInSpec(
  spec: GuideSpec,
  regionId: string,
  pointIndex: number,
  xRatio: number,
  yRatio: number,
): GuideSpec {
  const nx = clampUnitRatio(xRatio);
  const ny = clampUnitRatio(yRatio);
  return {
    ...spec,
    overlayRegions: spec.overlayRegions.map((reg) =>
      reg.id === regionId
        ? {
            ...reg,
            points: reg.points.map((pt, idx) =>
              idx === pointIndex ? { xRatio: nx, yRatio: ny } : pt,
            ),
          }
        : reg,
    ),
  };
}

/** Strip a `data:image/png;base64,...` data URL to raw base64 bytes. */
export function stripPngDataUrlPrefix(dataUrl: string): string {
  const idx = dataUrl.indexOf(';base64,');
  return idx >= 0 ? dataUrl.slice(idx + ';base64,'.length) : dataUrl;
}
