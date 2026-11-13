import { describe, expect, test } from 'vitest';
import type {
  AsyncViewState,
  LifecycleState,
  PatchEditDraft,
  RunRecord,
  Visibility,
  WorkspaceTab,
} from '../types/domain';
import {
  WORKSPACE_TABS,
  buildPatchEditPayload,
  clampPatchEditDraft,
  createDefaultPatchEditDraft,
  describeAsyncState,
  isExportEligible,
  isVisibilityAllowedForExport,
  nextLifecycleStep,
  resolveNextTabForKey,
} from './workspace';

const SAMPLE_RUN: RunRecord = {
  runId: 'harbor-dock-v001',
  slug: 'harbor-dock',
  version: 1,
  versionLabel: 'v001',
  lifecycle: 'draft',
  visibility: 'restricted',
  widthPx: 1024,
  heightPx: 768,
  visionCriticPassed: null,
  colorQaMaxDeltaE2000: 1.5,
  createdAtIso: '2026-11-13T00:10:00Z',
  imageUrl: '/api/assets/runs/harbor-dock/v001.png',
  compiledPrompt: 'harbor dock',
  backendName: 'codex-native',
  structuralChecks: [],
};

describe('workspace navigation, lifecycle, visibility, and PatchEdit helpers', () => {
  test('exposes all four Phase 5 top-level workspace tabs', () => {
    expect(WORKSPACE_TABS.map((tab) => tab.label)).toEqual([
      'Style & Atmosphere',
      'SpatialGuide Studio',
      'PatchEdit Cropper',
      'Run Inspector',
    ]);
  });

  test.each<{
    id: string;
    currentTab: WorkspaceTab;
    key: string;
    expected: WorkspaceTab | null;
  }>([
    {
      id: 'arrow-right-advances-tab',
      currentTab: 'style_atmosphere',
      key: 'ArrowRight',
      expected: 'spatial_guide',
    },
    {
      id: 'arrow-right-wraps-from-last-to-first',
      currentTab: 'run_inspector',
      key: 'ArrowRight',
      expected: 'style_atmosphere',
    },
    {
      id: 'arrow-left-moves-backward',
      currentTab: 'patch_edit',
      key: 'ArrowLeft',
      expected: 'spatial_guide',
    },
    {
      id: 'arrow-left-wraps-from-first-to-last',
      currentTab: 'style_atmosphere',
      key: 'ArrowLeft',
      expected: 'run_inspector',
    },
    {
      id: 'home-jumps-to-first-tab',
      currentTab: 'patch_edit',
      key: 'Home',
      expected: 'style_atmosphere',
    },
    {
      id: 'end-jumps-to-last-tab',
      currentTab: 'style_atmosphere',
      key: 'End',
      expected: 'run_inspector',
    },
    {
      id: 'unrelated-key-returns-null',
      currentTab: 'spatial_guide',
      key: 'Enter',
      expected: null,
    },
  ])('$id', ({ currentTab, key, expected }) => {
    expect(resolveNextTabForKey(currentTab, key)).toBe(expected);
  });

  test.each<{ id: string; lifecycle: LifecycleState; expected: boolean }>([
    { id: 'draft-blocks-export', lifecycle: 'draft', expected: false },
    { id: 'candidate-blocks-export', lifecycle: 'candidate', expected: false },
    { id: 'validated-unlocks-export', lifecycle: 'validated', expected: true },
    { id: 'canonical-unlocks-export', lifecycle: 'canonical', expected: true },
    { id: 'superseded-blocks-export', lifecycle: 'superseded', expected: false },
  ])('$id', ({ lifecycle, expected }) => {
    expect(isExportEligible(lifecycle)).toBe(expected);
  });

  test.each<{ id: string; current: LifecycleState; expectedNext: LifecycleState | null }>([
    { id: 'draft-to-candidate', current: 'draft', expectedNext: 'candidate' },
    { id: 'candidate-to-validated', current: 'candidate', expectedNext: 'validated' },
    { id: 'validated-to-canonical', current: 'validated', expectedNext: 'canonical' },
    { id: 'canonical-is-terminal', current: 'canonical', expectedNext: null },
    { id: 'superseded-is-terminal', current: 'superseded', expectedNext: null },
  ])('$id', ({ current, expectedNext }) => {
    expect(nextLifecycleStep(current)).toBe(expectedNext);
  });

  test.each<{ id: string; source: Visibility; target: Visibility; allowed: boolean }>([
    { id: 'public-to-public-allowed', source: 'public', target: 'public', allowed: true },
    { id: 'public-to-internal-allowed', source: 'public', target: 'internal', allowed: true },
    { id: 'restricted-to-public-blocked', source: 'restricted', target: 'public', allowed: false },
    {
      id: 'internal-to-restricted-blocked',
      source: 'internal',
      target: 'restricted',
      allowed: false,
    },
    { id: 'internal-to-internal-allowed', source: 'internal', target: 'internal', allowed: true },
  ])('$id', ({ source, target, allowed }) => {
    expect(isVisibilityAllowedForExport(source, target)).toBe(allowed);
  });

  test.each<{
    id: string;
    patch: Partial<PatchEditDraft>;
    expected: {
      readonly cropSizePx: number;
      readonly originXPx: number;
      readonly originYPx: number;
      readonly collarWidthPx: number;
      readonly featherPx: number;
    };
  }>([
    {
      id: 'clamps-oversized-square-to-shortest-canvas-edge',
      patch: { cropSizePx: 1200, originXPx: 500, originYPx: 400 },
      expected: {
        cropSizePx: 768,
        originXPx: 256,
        originYPx: 0,
        collarWidthPx: 24,
        featherPx: 12,
      },
    },
    {
      id: 'clamps-negative-origin-and-collar-bounds',
      patch: { originXPx: -50, originYPx: -20, collarWidthPx: 8, featherPx: 40 },
      expected: {
        cropSizePx: 269,
        originXPx: 0,
        originYPx: 0,
        collarWidthPx: 16,
        featherPx: 16,
      },
    },
    {
      id: 'clamps-collar-max-32-and-feather-min-2',
      patch: { collarWidthPx: 64, featherPx: 0 },
      expected: {
        cropSizePx: 269,
        originXPx: 377,
        originYPx: 249,
        collarWidthPx: 32,
        featherPx: 2,
      },
    },
  ])('$id', ({ patch, expected }) => {
    const base = createDefaultPatchEditDraft(SAMPLE_RUN);
    const clamped = clampPatchEditDraft({ ...base, ...patch });
    expect({
      cropSizePx: clamped.cropSizePx,
      originXPx: clamped.originXPx,
      originYPx: clamped.originYPx,
      collarWidthPx: clamped.collarWidthPx,
      featherPx: clamped.featherPx,
    }).toEqual(expected);
  });

  test.each<{
    id: string;
    draftPatch: Partial<PatchEditDraft>;
    run: RunRecord | null;
    expectError: boolean;
    expectedMode?: 'roi' | 'mask';
  }>([
    {
      id: 'builds-run-roi-patch-payload',
      draftPatch: { mode: 'roi', prompt: 'refine brass lantern' },
      run: SAMPLE_RUN,
      expectError: false,
      expectedMode: 'roi',
    },
    {
      id: 'builds-target-image-mask-payload',
      draftPatch: {
        mode: 'mask',
        targetImagePath: 'runs/harbor-dock/v001.png',
        maskPngBase64: 'iVBORw0KGgo=',
        prompt: 'fix lantern',
      },
      run: null,
      expectError: false,
      expectedMode: 'mask',
    },
    {
      id: 'rejects-empty-prompt',
      draftPatch: { prompt: '   ' },
      run: SAMPLE_RUN,
      expectError: true,
    },
    {
      id: 'rejects-mask-mode-without-mask-png',
      draftPatch: { mode: 'mask', maskPngBase64: null, prompt: 'fix' },
      run: SAMPLE_RUN,
      expectError: true,
    },
  ])('$id', ({ draftPatch, run, expectError, expectedMode }) => {
    const draft = { ...createDefaultPatchEditDraft(run), ...draftPatch };
    const result = buildPatchEditPayload(draft, run);
    if (expectError) {
      expect('error' in result).toBe(true);
    } else {
      expect('error' in result).toBe(false);
      if (!('error' in result)) {
        if (expectedMode === 'roi') {
          expect(result.roi).toBeDefined();
          expect(result.maskPngBase64).toBeUndefined();
        } else {
          expect(result.maskPngBase64).toBeDefined();
          expect(result.roi).toBeUndefined();
        }
      }
    }
  });

  test.each<{
    id: string;
    state: AsyncViewState<string>;
    expectedBadge: string;
    expectedRole: 'status' | 'alert' | null;
    expectedInteractive: boolean;
  }>([
    {
      id: 'loading-state-descriptor',
      state: { status: 'loading', message: 'Loading...' },
      expectedBadge: 'Loading',
      expectedRole: 'status',
      expectedInteractive: false,
    },
    {
      id: 'empty-state-descriptor',
      state: { status: 'empty', title: 'Empty', detail: 'Nothing yet' },
      expectedBadge: 'Empty',
      expectedRole: 'status',
      expectedInteractive: true,
    },
    {
      id: 'retryable-error-state-descriptor',
      state: { status: 'error', message: 'Network failed', retryable: true },
      expectedBadge: 'Error',
      expectedRole: 'alert',
      expectedInteractive: true,
    },
    {
      id: 'fatal-error-state-descriptor',
      state: { status: 'error', message: 'Invalid workspace', retryable: false },
      expectedBadge: 'Error',
      expectedRole: 'alert',
      expectedInteractive: false,
    },
    {
      id: 'disabled-state-descriptor',
      state: { status: 'disabled', reason: 'Locked' },
      expectedBadge: 'Disabled',
      expectedRole: 'status',
      expectedInteractive: false,
    },
    {
      id: 'ready-state-descriptor',
      state: { status: 'ready', data: 'ok' },
      expectedBadge: 'Ready',
      expectedRole: null,
      expectedInteractive: true,
    },
  ])('$id', ({ state, expectedBadge, expectedRole, expectedInteractive }) => {
    expect(describeAsyncState(state)).toEqual({
      badge: expectedBadge,
      ariaRole: expectedRole,
      isInteractive: expectedInteractive,
    });
  });
});
