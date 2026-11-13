/** === RUN INSPECTOR & COLOR TELEMETRY HELPERS ===
 *
 * PURPOSE:
 * Pure helpers for pixel `#RRGGBB` eyedropper formatting, zoomed ROI lens
 * clamping, OKLab `(a, b)` vectorscope coordinate mapping, and `Export`
 * payload construction without duplicating Rust color math.
 */

import type { ExportApiPayload } from '../../api/client';
import { isExportEligible, isVisibilityAllowedForExport } from '../../state/workspace';
import type { ExportProfileName, RunRecord, Visibility } from '../../types/domain';

export interface ExportFormDraft {
  readonly profile: ExportProfileName;
  readonly visibility: Visibility;
  readonly outputSlug: string;
  readonly tokenSize: number;
  readonly ringColorHex: string;
  readonly cols: number;
  readonly rows: number;
  readonly gridPx: number;
  readonly jpegQuality: number;
  readonly matchGradeEnabled: boolean;
  readonly gradeRefPath: string;
  readonly gradeStrength: number;
  readonly cubeLutPath: string;
}

/** Convert clamped 8-bit `[r, g, b]` channels to uppercase `#RRGGBB` hex. */
export function rgbBytesToHex(r: number, g: number, b: number): string {
  const toHex = (n: number): string =>
    Math.max(0, Math.min(255, Math.round(n)))
      .toString(16)
      .padStart(2, '0')
      .toUpperCase();
  return `#${toHex(r)}${toHex(g)}${toHex(b)}`;
}

/** Map OKLab `(a, b)` coordinates (`~ -0.35..0.35`) into `[0, size]` SVG plot pixels. */
export function projectOklabToPlot(
  a: number,
  b: number,
  plotSize: number,
  range = 0.35,
): { readonly x: number; readonly y: number } {
  const half = plotSize / 2;
  const clampedA = Math.max(-range, Math.min(range, Number.isFinite(a) ? a : 0));
  const clampedB = Math.max(-range, Math.min(range, Number.isFinite(b) ? b : 0));
  return {
    x: Number((half + (clampedA / range) * (half - 10)).toFixed(2)),
    y: Number((half - (clampedB / range) * (half - 10)).toFixed(2)),
  };
}

/** Compute source crop rectangle for the zoomed ROI inspection lens. */
export function computeLensSourceRect(
  centerX: number,
  centerY: number,
  imageWidth: number,
  imageHeight: number,
  zoomFactor = 4,
  lensSize = 128,
): { readonly sx: number; readonly sy: number; readonly sw: number; readonly sh: number } {
  const safeW = Math.max(1, Math.round(imageWidth));
  const safeH = Math.max(1, Math.round(imageHeight));
  const sampleSpan = Math.max(8, Math.round(lensSize / Math.max(1, zoomFactor)));
  const sw = Math.min(safeW, sampleSpan);
  const sh = Math.min(safeH, sampleSpan);
  const sx = Math.max(0, Math.min(safeW - sw, Math.round(centerX - sw / 2)));
  const sy = Math.max(0, Math.min(safeH - sh, Math.round(centerY - sh / 2)));
  return { sx, sy, sw, sh };
}

/** Validate and build a `POST /api/export` payload respecting `Lifecycle` & `Visibility` gates. */
export function buildExportPayload(
  run: RunRecord,
  draft: ExportFormDraft,
): ExportApiPayload | { readonly error: string } {
  if (!isExportEligible(run.lifecycle)) {
    return {
      error: `Run ${run.versionLabel} has lifecycle '${run.lifecycle}'; advance to validated or canonical before exporting.`,
    };
  }
  if (!isVisibilityAllowedForExport(run.visibility, draft.visibility)) {
    return {
      error: `Visibility gate blocked: cannot export '${run.visibility}' Run as '${draft.visibility}'.`,
    };
  }

  const base: {
    runSlug: string;
    runVersion: number;
    profile: ExportProfileName;
    visibility: Visibility;
    outputSlug?: string;
    tokenSize?: number;
    ringColorHex?: string;
    cols?: number;
    rows?: number;
    gridPx?: number;
    jpegQuality?: number;
    matchGrade?: {
      enabled: boolean;
      gradeRefPath?: string;
      gradeStrength?: number;
      cubeLutPath?: string;
    };
  } = {
    runSlug: run.slug,
    runVersion: run.version,
    profile: draft.profile,
    visibility: draft.visibility,
  };

  if (draft.outputSlug.trim().length > 0) {
    base.outputSlug = draft.outputSlug.trim();
  }

  if (draft.profile === 'vtt_token') {
    base.tokenSize = Math.max(16, Math.min(4096, Math.round(draft.tokenSize)));
    if (draft.ringColorHex.trim().length > 0) {
      base.ringColorHex = draft.ringColorHex.trim();
    }
    base.jpegQuality = Math.max(1, Math.min(100, Math.round(draft.jpegQuality)));
  } else if (draft.profile === 'vtt_map') {
    base.cols = Math.max(1, Math.round(draft.cols));
    base.rows = Math.max(1, Math.round(draft.rows));
    base.gridPx = Math.max(16, Math.round(draft.gridPx));
    base.jpegQuality = Math.max(1, Math.min(100, Math.round(draft.jpegQuality)));
  }

  if (draft.matchGradeEnabled) {
    if (draft.gradeStrength < 0.25 || draft.gradeStrength > 0.45) {
      return { error: 'matchGrade strength must stay within 0.25..=0.45.' };
    }
    const mg: {
      enabled: boolean;
      gradeRefPath?: string;
      gradeStrength?: number;
      cubeLutPath?: string;
    } = {
      enabled: true,
      gradeStrength: draft.gradeStrength,
    };
    if (draft.gradeRefPath.trim().length > 0) {
      mg.gradeRefPath = draft.gradeRefPath.trim();
    }
    if (draft.cubeLutPath.trim().length > 0) {
      mg.cubeLutPath = draft.cubeLutPath.trim();
    }
    base.matchGrade = mg;
  }

  return base;
}
