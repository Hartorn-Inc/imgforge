import { render } from 'preact';
import { act } from 'preact/test-utils';
import { afterEach, beforeEach, describe, expect, test, vi } from 'vitest';
import type { RunRecord, WorkspaceSummary } from '../../types/domain';
import { RunInspectorView } from './RunInspectorView';

function makeRun(runId: string, version: number, label: string): RunRecord {
  return {
    runId,
    slug: 'harbor',
    version,
    versionLabel: label,
    lifecycle: 'draft',
    visibility: 'public',
    widthPx: 1024,
    heightPx: 1024,
    visionCriticPassed: null,
    colorQaMaxDeltaE2000: 1.5,
    createdAtIso: '2026-11-13T00:00:00Z',
    imageUrl: `/api/assets/runs/harbor/${label}.png`,
    compiledPrompt: 'harbor',
    backendName: 'codex-native',
    structuralChecks: [],
  };
}

function makeWorkspaceState(): { readonly status: 'ready'; readonly data: WorkspaceSummary } {
  return {
    status: 'ready',
    data: {
      hasWorkspace: true,
      workspaceRoot: '/tmp/ws',
      workspaceName: 'ws',
      authProvisioned: true,
      subjects: [
        {
          slug: 'courier-ren',
          name: 'Courier Ren',
          category: 'character',
          heightMeters: 1.74,
          paletteLock: [],
          visibility: 'public',
          lifecycle: 'draft',
          source: 'project',
          asymmetricMarkers: [],
          positiveWeaponGeometry: [],
          tags: [],
        },
      ],
      styles: [],
      atmospheres: [],
      referencePacks: [],
      runs: [makeRun('harbor-v001', 1, 'v001'), makeRun('harbor-v002', 2, 'v002')],
      manifestLock: {
        schemaVersion: 1,
        canonicalCount: 0,
        supersededCount: 0,
        canonical: {},
        superseded: [],
      },
    },
  };
}

function makeInspectPayload(run: RunRecord, maxDelta: number) {
  return {
    run,
    telemetry: {
      width: 1024,
      height: 1024,
      opaquePixelCount: 1000,
      lStarBins: [{ binIndex: 0, minLStar: 0, maxLStar: 5, count: 100, fraction: 1 }],
      vectorscopePoints: [{ oklabL: 0.5, a: 0, b: 0, lStar: 50, hex: '#808080' }],
    },
    colorQa: {
      passed: maxDelta < 5,
      exposure: null,
      palette_adherence: [],
      untouched_drift: null,
      sparkle: null,
      max_delta_e2000: maxDelta,
      notes: [`qa-${run.runId}`],
    },
    heatmaps: { drift: null, bleed: null, sparkle: null, outOfRoi: null },
  };
}

async function flushMicrotasks(): Promise<void> {
  for (let i = 0; i < 4; i += 1) {
    await new Promise((resolve) => {
      setTimeout(resolve, 0);
    });
  }
}

describe('RunInspectorView', () => {
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

  test('clears activeColorQa when switching runs and drops stale inspect/validate replies', async () => {
    let resolveSlowV1Inspect!: (res: Response) => void;
    let resolveSlowV1Validate!: (res: Response) => void;
    const wsState = makeWorkspaceState();
    const runV1 = wsState.data.runs[0];
    const runV2 = wsState.data.runs[1];
    if (runV1 === undefined || runV2 === undefined) {
      throw new Error('Expected two seeded runs');
    }

    vi.stubGlobal(
      'fetch',
      vi.fn((url: string) => {
        if (url === '/api/runs/harbor/1/inspect') {
          return new Promise<Response>((resolve) => {
            resolveSlowV1Inspect = resolve;
          });
        }
        if (url === '/api/runs/harbor/1/validate') {
          return new Promise<Response>((resolve) => {
            resolveSlowV1Validate = resolve;
          });
        }
        if (url === '/api/runs/harbor/2/inspect') {
          return Promise.resolve(
            new Response(JSON.stringify(makeInspectPayload(runV2, 1.25)), { status: 200 }),
          );
        }
        return Promise.resolve(new Response(null, { status: 404 }));
      }),
    );

    let selectedId = 'harbor-v001';
    const renderInspector = () => {
      render(
        <RunInspectorView
          workspaceState={wsState}
          selectedRunId={selectedId}
          onSelectRunId={(nextId) => {
            selectedId = nextId;
            renderInspector();
          }}
          onRefreshWorkspace={() => Promise.resolve()}
          onSendRunToPatchEdit={vi.fn()}
        />,
        mountNode,
      );
    };

    await act(async () => {
      renderInspector();
      await flushMicrotasks();
    });

    // Start slow inspect and validate on v001.
    const inspectBtn = Array.from(mountNode.querySelectorAll('button')).find((b) =>
      b.textContent.includes('Inspect Telemetry'),
    );
    const validateBtn = Array.from(mountNode.querySelectorAll('button')).find(
      (b) => b.textContent === 'Validate Run',
    );
    await act(() => {
      inspectBtn?.click();
      validateBtn?.click();
    });

    // Switch to v002 and inspect v002 cleanly.
    const runSelect = mountNode.querySelector<HTMLSelectElement>('#inspector-run-select');
    expect(runSelect).not.toBeNull();
    if (runSelect === null) {
      return;
    }
    await act(() => {
      runSelect.value = 'harbor-v002';
      runSelect.dispatchEvent(new Event('change', { bubbles: true }));
    });
    expect(mountNode.textContent).not.toContain('Color QA Report');

    await act(async () => {
      inspectBtn?.click();
      await flushMicrotasks();
    });
    expect(mountNode.textContent).toContain('qa-harbor-v002');
    expect(mountNode.textContent).toContain('Max ΔE 1.25');

    // Resolve stale v001 inspect & validate calls -> must NOT overwrite v002 Color QA.
    await act(async () => {
      resolveSlowV1Inspect(
        new Response(JSON.stringify(makeInspectPayload(runV1, 9.99)), { status: 200 }),
      );
      resolveSlowV1Validate(
        new Response(
          JSON.stringify({
            run: runV1,
            colorQa: makeInspectPayload(runV1, 8.88).colorQa,
            criticReport: { schema_version: 1, passed: false, checks: [], summary: 'stale-critic' },
          }),
          { status: 200 },
        ),
      );
      await flushMicrotasks();
    });

    expect(mountNode.textContent).toContain('qa-harbor-v002');
    expect(mountNode.textContent).not.toContain('qa-harbor-v001');
    expect(mountNode.textContent).not.toContain('stale-critic');

    // Switching back to v001 via #inspector-run-select clears activeColorQa immediately.
    await act(() => {
      runSelect.value = 'harbor-v001';
      runSelect.dispatchEvent(new Event('change', { bubbles: true }));
    });
    expect(mountNode.textContent).not.toContain('Color QA Report');
  });

  test.each<{
    id: string;
    key: string;
    shiftKey: boolean;
    expectedCoords: string;
  }>([
    {
      id: 'arrow-right-steps-x-by-1',
      key: 'ArrowRight',
      shiftKey: false,
      expectedCoords: '@ (1, 0)',
    },
    {
      id: 'shift-arrow-down-steps-y-by-10',
      key: 'ArrowDown',
      shiftKey: true,
      expectedCoords: '@ (0, 10)',
    },
  ])('$id', async ({ key, shiftKey, expectedCoords }) => {
    const wsState = makeWorkspaceState();
    await act(async () => {
      render(
        <RunInspectorView
          workspaceState={wsState}
          selectedRunId="harbor-v001"
          onSelectRunId={vi.fn()}
          onRefreshWorkspace={() => Promise.resolve()}
          onSendRunToPatchEdit={vi.fn()}
        />,
        mountNode,
      );
      await flushMicrotasks();
    });

    const stage = mountNode.querySelector<HTMLDivElement>('.compare-stage');
    expect(stage?.getAttribute('tabindex')).toBe('0');
    await act(() => {
      stage?.dispatchEvent(new KeyboardEvent('keydown', { key, shiftKey, bubbles: true }));
    });
    expect(mountNode.textContent).toContain(expectedCoords);
  });
});
