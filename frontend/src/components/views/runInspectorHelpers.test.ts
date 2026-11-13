import { describe, expect, test } from 'vitest';
import type { LifecycleState, RunRecord, Visibility } from '../../types/domain';
import {
  buildExportPayload,
  computeLensSourceRect,
  projectOklabToPlot,
  rgbBytesToHex,
  type ExportFormDraft,
} from './runInspectorHelpers';

function makeRun(lifecycle: LifecycleState, visibility: Visibility): RunRecord {
  return {
    runId: 'harbor-v002',
    slug: 'harbor',
    version: 2,
    versionLabel: 'v002',
    lifecycle,
    visibility,
    widthPx: 1024,
    heightPx: 1024,
    visionCriticPassed: true,
    colorQaMaxDeltaE2000: 1.4,
    createdAtIso: '2026-11-13T00:00:00Z',
    imageUrl: '/api/assets/runs/harbor/v002.png',
    compiledPrompt: 'harbor',
    backendName: 'codex-native',
    structuralChecks: [],
  };
}

const DEFAULT_EXPORT_DRAFT: ExportFormDraft = {
  profile: 'web_webp',
  visibility: 'public',
  outputSlug: 'harbor-web',
  tokenSize: 512,
  ringColorHex: '#D4A843',
  cols: 24,
  rows: 18,
  gridPx: 140,
  jpegQuality: 90,
  matchGradeEnabled: false,
  gradeRefPath: '',
  gradeStrength: 0.35,
  cubeLutPath: '',
};

describe('Run Inspector eyedropper, lens, vectorscope, and Export helpers', () => {
  test.each<{ id: string; rgb: readonly [number, number, number]; expected: string }>([
    { id: 'converts-slate-blue-to-uppercase-hex', rgb: [44, 62, 80], expected: '#2C3E50' },
    { id: 'clamps-negative-and-overflow-channels', rgb: [-5, 255, 300], expected: '#00FFFF' },
    { id: 'pads-single-digit-hex-bytes', rgb: [1, 2, 15], expected: '#01020F' },
  ])('$id', ({ rgb, expected }) => {
    expect(rgbBytesToHex(rgb[0], rgb[1], rgb[2])).toBe(expected);
  });

  test.each<{
    id: string;
    a: number;
    b: number;
    plotSize: number;
    expected: { readonly x: number; readonly y: number };
  }>([
    {
      id: 'maps-neutral-origin-to-plot-center',
      a: 0,
      b: 0,
      plotSize: 220,
      expected: { x: 110, y: 110 },
    },
    {
      id: 'maps-positive-oklab-to-top-right',
      a: 0.35,
      b: 0.35,
      plotSize: 220,
      expected: { x: 210, y: 10 },
    },
    {
      id: 'clamps-extreme-oklab-coordinates',
      a: -1.5,
      b: -1.5,
      plotSize: 220,
      expected: { x: 10, y: 210 },
    },
  ])('$id', ({ a, b, plotSize, expected }) => {
    expect(projectOklabToPlot(a, b, plotSize)).toEqual(expected);
  });

  test.each<{
    id: string;
    cx: number;
    cy: number;
    width: number;
    height: number;
    expected: {
      readonly sx: number;
      readonly sy: number;
      readonly sw: number;
      readonly sh: number;
    };
  }>([
    {
      id: 'clamps-top-left-lens-crop-to-zero',
      cx: 4,
      cy: 6,
      width: 512,
      height: 512,
      expected: { sx: 0, sy: 0, sw: 32, sh: 32 },
    },
    {
      id: 'clamps-bottom-right-lens-crop-within-image',
      cx: 510,
      cy: 510,
      width: 512,
      height: 512,
      expected: { sx: 480, sy: 480, sw: 32, sh: 32 },
    },
  ])('$id', ({ cx, cy, width, height, expected }) => {
    expect(computeLensSourceRect(cx, cy, width, height, 4, 128)).toEqual(expected);
  });

  test.each<{
    id: string;
    lifecycle: LifecycleState;
    runVis: Visibility;
    draftPatch: Partial<ExportFormDraft>;
    expectError: boolean;
  }>([
    {
      id: 'allows-validated-public-web-webp-export',
      lifecycle: 'validated',
      runVis: 'public',
      draftPatch: { profile: 'web_webp', visibility: 'public' },
      expectError: false,
    },
    {
      id: 'allows-canonical-vtt-token-with-match-grade',
      lifecycle: 'canonical',
      runVis: 'public',
      draftPatch: {
        profile: 'vtt_token',
        matchGradeEnabled: true,
        gradeStrength: 0.35,
        gradeRefPath: 'runs/anchor/v001.png',
      },
      expectError: false,
    },
    {
      id: 'blocks-draft-run-export',
      lifecycle: 'draft',
      runVis: 'public',
      draftPatch: {},
      expectError: true,
    },
    {
      id: 'blocks-internal-run-exporting-as-public',
      lifecycle: 'canonical',
      runVis: 'internal',
      draftPatch: { visibility: 'public' },
      expectError: true,
    },
    {
      id: 'rejects-out-of-range-match-grade-strength',
      lifecycle: 'canonical',
      runVis: 'public',
      draftPatch: { matchGradeEnabled: true, gradeStrength: 0.9 },
      expectError: true,
    },
  ])('$id', ({ lifecycle, runVis, draftPatch, expectError }) => {
    const res = buildExportPayload(makeRun(lifecycle, runVis), {
      ...DEFAULT_EXPORT_DRAFT,
      ...draftPatch,
    });
    expect('error' in res).toBe(expectError);
  });
});
