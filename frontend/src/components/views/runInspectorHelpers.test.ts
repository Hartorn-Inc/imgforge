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

  test('projects OKLab (a, b) into plot coordinates with origin at center', () => {
    expect(projectOklabToPlot(0, 0, 220)).toEqual({ x: 110, y: 110 });
    const pos = projectOklabToPlot(0.35, 0.35, 220);
    expect(pos.x).toBe(210);
    expect(pos.y).toBe(10);
  });

  test('clamps zoomed ROI lens source rect inside image bounds', () => {
    expect(computeLensSourceRect(4, 6, 512, 512, 4, 128)).toEqual({
      sx: 0,
      sy: 0,
      sw: 32,
      sh: 32,
    });
    expect(computeLensSourceRect(510, 510, 512, 512, 4, 128)).toEqual({
      sx: 480,
      sy: 480,
      sw: 32,
      sh: 32,
    });
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
