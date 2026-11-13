import { render } from 'preact';
import { act } from 'preact/test-utils';
import { afterEach, beforeEach, describe, expect, test, vi } from 'vitest';
import { App } from './App';

async function flushEffects(): Promise<void> {
  for (let i = 0; i < 4; i += 1) {
    await new Promise((resolve) => {
      setTimeout(resolve, 0);
    });
  }
}

function makeWorkspacePayload(workspaceName: string) {
  return {
    hasWorkspace: true,
    workspaceRoot: '/tmp/ws',
    workspaceName,
    authProvisioned: true,
    subjects: [
      {
        slug: 'courier-ren',
        name: 'Courier Ren',
        category: 'character',
        heightMeters: 1.74,
        paletteLock: [{ label: 'coat_slate', hex: '#2C3E50', surfaceSlot: 'coat' }],
        visibility: 'public',
        lifecycle: 'draft',
        source: 'project',
        asymmetricMarkers: [],
        positiveWeaponGeometry: [],
        tags: [],
      },
    ],
    styles: [
      {
        slug: 'franco-belgian-line',
        name: 'Franco-Belgian Clear Line',
        medium: 'pen and ink',
        exemplarCount: 1,
        lifecycle: 'canonical',
        source: 'builtin',
        linework: 'crisp',
        shadingAndTexture: 'flat',
        paletteProfile: 'warm',
        framingAndBorder: 'clean',
        styleGuardrails: ['crisp contour'],
        exemplarUrls: ['/api/presets/styles/franco-belgian-line/exemplars/0'],
      },
    ],
    atmospheres: [
      {
        slug: 'golden-dawn',
        name: 'Golden Dawn',
        lightDirection: 'east sun',
        colorTemperatureKelvin: 4200,
        exposureFloor: 32,
        maxAmbientTintBlend: 0.2,
        particles: 'mist',
        source: 'builtin',
        secondaryLightDirection: 'sky',
        minMeanLStar: 32,
        minP10LStar: 12,
      },
    ],
    referencePacks: [],
    runs: [
      {
        runId: 'harbor-v001',
        slug: 'harbor',
        version: 1,
        versionLabel: 'v001',
        lifecycle: 'draft',
        visibility: 'public',
        widthPx: 1024,
        heightPx: 1024,
        visionCriticPassed: null,
        colorQaMaxDeltaE2000: 1.5,
        createdAtIso: '2026-11-13T00:00:00Z',
        imageUrl: '/api/assets/runs/harbor/v001.png',
        compiledPrompt: 'harbor',
        backendName: 'codex-native',
        structuralChecks: [],
      },
    ],
    manifestLock: {
      schemaVersion: 1,
      canonicalCount: 0,
      supersededCount: 0,
      canonical: {},
      superseded: [],
    },
  };
}

describe('App workspace shell and view states', () => {
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
    workspaceOk: boolean;
    targetTabId: string;
    expectedInitialText: string;
    expectedTabText: string;
  }>([
    {
      id: 'renders-error-panel-on-workspace-failure-and-allows-guide-tab',
      workspaceOk: false,
      targetTabId: '#tab-spatial_guide',
      expectedInitialText: 'Workspace unreadable',
      expectedTabText: 'GuideSpec Payload',
    },
    {
      id: 'loads-configured-workspace-and-switches-to-run-inspector',
      workspaceOk: true,
      targetTabId: '#tab-run_inspector',
      expectedInitialText: 'Workspace: harbor-project',
      expectedTabText: 'Review, Diff & Color Inspector',
    },
  ])('$id', async ({ workspaceOk, targetTabId, expectedInitialText, expectedTabText }) => {
    vi.stubGlobal(
      'fetch',
      vi.fn((url: string) => {
        if (url === '/api/health') {
          return Promise.resolve(
            new Response(
              JSON.stringify({
                status: 'ok',
                version: '0.1.0',
                backend: 'codex-native',
                authProvisioned: true,
              }),
              { status: 200 },
            ),
          );
        }
        if (!workspaceOk) {
          return Promise.resolve(
            new Response(JSON.stringify({ error: 'io_error', message: 'Workspace unreadable' }), {
              status: 500,
            }),
          );
        }
        return Promise.resolve(
          new Response(JSON.stringify(makeWorkspacePayload('harbor-project')), { status: 200 }),
        );
      }),
    );

    vi.stubGlobal(
      'WebSocket',
      class {
        public addEventListener(): void {}
        public close(): void {}
      },
    );

    await act(() => {
      render(<App />, mountNode);
    });
    await act(async () => {
      await flushEffects();
    });

    expect(mountNode.textContent).toContain(expectedInitialText);

    const tabButton = mountNode.querySelector<HTMLButtonElement>(targetTabId);
    expect(tabButton).not.toBeNull();
    await act(() => {
      tabButton?.click();
    });

    expect(mountNode.textContent).toContain(expectedTabText);
  });

  test('drops stale async workspace response when a WebSocket refresh resolves first', async () => {
    let resolveSlowWorkspace!: (res: Response) => void;
    let wsCall = 0;
    const wsListeners = new Map<string, ((ev: Event) => void)[]>();

    vi.stubGlobal(
      'fetch',
      vi.fn((url: string) => {
        if (url === '/api/health') {
          return Promise.resolve(
            new Response(
              JSON.stringify({
                status: 'ok',
                version: '0.2.0',
                backend: 'codex-native',
                authProvisioned: true,
              }),
              { status: 200 },
            ),
          );
        }
        wsCall += 1;
        if (wsCall === 1) {
          return new Promise<Response>((resolve) => {
            resolveSlowWorkspace = resolve;
          });
        }
        return Promise.resolve(
          new Response(JSON.stringify(makeWorkspacePayload('newer-workspace')), { status: 200 }),
        );
      }),
    );

    vi.stubGlobal(
      'WebSocket',
      class {
        public addEventListener(type: string, cb: (ev: Event) => void): void {
          const list = wsListeners.get(type) ?? [];
          list.push(cb);
          wsListeners.set(type, list);
        }
        public close(): void {}
      },
    );

    await act(async () => {
      render(<App />, mountNode);
      await flushEffects();
    });

    // Trigger a WebSocket WorkspaceEvent so workspace request #2 resolves while #1 is still pending.
    await act(async () => {
      wsListeners.get('message')?.forEach((cb) => {
        cb(
          new MessageEvent('message', {
            data: JSON.stringify({
              eventType: 'created',
              resourceKind: 'style',
              id: 'harbor',
              lifecycle: 'draft',
              detail: 'Created',
              timestampIso: '2026-11-13T00:00:00Z',
            }),
          }),
        );
      });
      await flushEffects();
    });

    expect(mountNode.textContent).toContain('Workspace: newer-workspace');
    expect(mountNode.textContent).toContain('0.2.0');

    // Now resolve the slower initial request with stale data and verify it does NOT overwrite.
    await act(async () => {
      resolveSlowWorkspace(
        new Response(JSON.stringify(makeWorkspacePayload('stale-workspace')), { status: 200 }),
      );
      await flushEffects();
    });

    expect(mountNode.textContent).toContain('Workspace: newer-workspace');
    expect(mountNode.textContent).not.toContain('stale-workspace');
  });
});
