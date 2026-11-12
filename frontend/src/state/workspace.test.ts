import { describe, expect, test } from 'vitest';
import type { AsyncViewState, LifecycleState, WorkspaceTab } from '../types/domain';
import {
  WORKSPACE_TABS,
  describeAsyncState,
  isExportEligible,
  resolveNextTabForKey,
} from './workspace';

describe('workspace navigation and view state helpers', () => {
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
