import { render } from 'preact';
import { act } from 'preact/test-utils';
import { afterEach, beforeEach, describe, expect, test, vi } from 'vitest';
import { createDefaultPatchEditDraft } from '../state/workspace';
import type { AsyncViewState, WorkspaceSummary } from '../types/domain';
import { PatchCropStage } from './PatchCropStage';
import { PatchEditView } from './views/PatchEditView';

vi.mock('konva', () => {
  class MockNode {
    public on(): void {}
    public x(): number {
      return 0;
    }
    public y(): number {
      return 0;
    }
    public position(): void {}
    public size(): void {}
  }
  class MockLayer {
    public add(): void {}
  }
  class MockStage {
    public add(): void {}
    public draw(): void {}
    public destroy(): void {}
  }
  return {
    default: {
      Stage: MockStage,
      Layer: MockLayer,
      Rect: MockNode,
      Circle: MockNode,
      Image: MockNode,
    },
  };
});

describe('PatchCropStage and PatchEditView', () => {
  let mountNode: HTMLDivElement;

  beforeEach(() => {
    mountNode = document.createElement('div');
    document.body.appendChild(mountNode);
  });

  afterEach(async () => {
    await act(() => {
      render(null, mountNode);
    });
    mountNode.remove();
    vi.unstubAllGlobals();
  });

  test.each<{
    id: string;
    imageUrl: string | null;
    naturalWidth: number;
    naturalHeight: number;
    outcome: 'load' | 'error' | 'none';
    expectedCalls: readonly (readonly [number, number])[];
  }>([
    {
      id: 'reports-natural-dimensions-on-image-load',
      imageUrl: '/api/assets/runs/harbor/custom.png',
      naturalWidth: 1536,
      naturalHeight: 1024,
      outcome: 'load',
      expectedCalls: [[1536, 1024]],
    },
    {
      id: 'skips-dimension-callback-when-image-errors',
      imageUrl: '/api/assets/runs/harbor/missing.png',
      naturalWidth: 0,
      naturalHeight: 0,
      outcome: 'error',
      expectedCalls: [],
    },
    {
      id: 'skips-dimension-callback-when-image-url-is-null',
      imageUrl: null,
      naturalWidth: 0,
      naturalHeight: 0,
      outcome: 'none',
      expectedCalls: [],
    },
  ])('$id', async ({ imageUrl, naturalWidth, naturalHeight, outcome, expectedCalls }) => {
    const instances: {
      onload: (() => void) | null;
      onerror: (() => void) | null;
      naturalWidth: number;
      naturalHeight: number;
    }[] = [];

    class FakeImage {
      public onload: (() => void) | null = null;
      public onerror: (() => void) | null = null;
      public naturalWidth = naturalWidth;
      public naturalHeight = naturalHeight;
      public set src(_val: string) {
        instances.push(this);
        queueMicrotask(() => {
          if (outcome === 'load') {
            this.onload?.();
          } else if (outcome === 'error') {
            this.onerror?.();
          }
        });
      }
    }

    vi.stubGlobal('Image', FakeImage);
    const onImageDimensions = vi.fn();

    await act(async () => {
      render(
        <PatchCropStage
          imageUrl={imageUrl}
          canvasWidthPx={1024}
          canvasHeightPx={1024}
          mode="roi"
          originXPx={100}
          originYPx={100}
          cropSizePx={256}
          collarWidthPx={24}
          brushRadiusPx={20}
          maskPngBase64={null}
          onRoiChange={vi.fn()}
          onMaskChange={vi.fn()}
          onImageDimensions={onImageDimensions}
        />,
        mountNode,
      );
      await Promise.resolve();
    });

    expect(onImageDimensions.mock.calls).toEqual(expectedCalls);
  });

  test.each<{
    id: string;
    targetRunKey: string | null;
    targetImagePath: string;
    loadedW: number;
    loadedH: number;
    expectedW: number;
    expectedH: number;
  }>([
    {
      id: 'updates-canvas-dimensions-for-manual-target-image-path',
      targetRunKey: null,
      targetImagePath: 'custom/wide.png',
      loadedW: 1536,
      loadedH: 1024,
      expectedW: 1536,
      expectedH: 1024,
    },
    {
      id: 'keeps-bound-run-dimensions-unchanged-when-run-selected',
      targetRunKey: 'harbor-v001',
      targetImagePath: '',
      loadedW: 2048,
      loadedH: 2048,
      expectedW: 1024,
      expectedH: 1024,
    },
  ])('$id', async ({ targetRunKey, targetImagePath, loadedW, loadedH, expectedW, expectedH }) => {
    class FakeImage {
      public onload: (() => void) | null = null;
      public onerror: (() => void) | null = null;
      public naturalWidth = loadedW;
      public naturalHeight = loadedH;
      public set src(_val: string) {
        queueMicrotask(() => {
          this.onload?.();
        });
      }
    }
    vi.stubGlobal('Image', FakeImage);

    const run = {
      runId: 'harbor-v001',
      slug: 'harbor',
      version: 1,
      versionLabel: 'v001',
      lifecycle: 'draft' as const,
      visibility: 'public' as const,
      widthPx: 1024,
      heightPx: 1024,
      visionCriticPassed: null,
      colorQaMaxDeltaE2000: null,
      createdAtIso: '2026-11-13T00:00:00Z',
      imageUrl: '/api/assets/runs/harbor/v001.png',
      compiledPrompt: 'harbor',
      backendName: 'codex-native',
      structuralChecks: [],
    };
    const wsState: AsyncViewState<WorkspaceSummary> = {
      status: 'ready',
      data: {
        hasWorkspace: true,
        workspaceRoot: '/tmp/ws',
        workspaceName: 'ws',
        authProvisioned: true,
        subjects: [],
        styles: [],
        atmospheres: [],
        referencePacks: [],
        runs: [run],
        manifestLock: {
          schemaVersion: 1,
          canonicalCount: 0,
          supersededCount: 0,
          canonical: {},
          superseded: [],
        },
      },
    };

    let currentDraft = {
      ...createDefaultPatchEditDraft(null),
      targetRunKey,
      targetImagePath,
    };
    const onDraftChange = vi.fn((next: typeof currentDraft) => {
      currentDraft = next;
    });

    await act(async () => {
      render(
        <PatchEditView
          workspaceState={wsState}
          draft={currentDraft}
          onDraftChange={onDraftChange}
          onRefreshWorkspace={() => Promise.resolve()}
          onPatchCompleted={vi.fn()}
        />,
        mountNode,
      );
      await Promise.resolve();
    });

    expect(currentDraft.canvasWidthPx).toBe(expectedW);
    expect(currentDraft.canvasHeightPx).toBe(expectedH);
  });

  test('forwards onRefreshWorkspace to StatePanel Retry button in error state', async () => {
    const onRefreshWorkspace = vi.fn(() => Promise.resolve());
    const errorState: AsyncViewState<WorkspaceSummary> = {
      status: 'error',
      message: 'Failed to load workspace',
      retryable: true,
    };

    await act(() => {
      render(
        <PatchEditView
          workspaceState={errorState}
          draft={createDefaultPatchEditDraft(null)}
          onDraftChange={vi.fn()}
          onRefreshWorkspace={onRefreshWorkspace}
          onPatchCompleted={vi.fn()}
        />,
        mountNode,
      );
    });

    const retryBtn = mountNode.querySelector<HTMLButtonElement>('button');
    expect(retryBtn?.textContent).toContain('Retry request');
    await act(() => {
      retryBtn?.click();
    });
    expect(onRefreshWorkspace).toHaveBeenCalledTimes(1);
  });
});
