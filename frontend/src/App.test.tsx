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

  test('renders error StatePanel when /api/workspace fails and preserves tab switching', async () => {
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
        return Promise.resolve(
          new Response(JSON.stringify({ error: 'io_error', message: 'Workspace unreadable' }), {
            status: 500,
          }),
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

    expect(mountNode.querySelector('[role="alert"]')?.textContent).toContain(
      'Workspace unreadable',
    );

    const guideTab = mountNode.querySelector<HTMLButtonElement>('#tab-spatial_guide');
    expect(guideTab).not.toBeNull();
    await act(() => {
      guideTab?.click();
    });

    const jsonEditor = mountNode.querySelector<HTMLTextAreaElement>('#guidespec-json-editor');
    expect(jsonEditor).not.toBeNull();
  });

  test('loads configured workspace and renders Style, Atmosphere, Subject, and Run Inspector states', async () => {
    const workspacePayload = {
      hasWorkspace: true,
      workspaceRoot: '/tmp/ws',
      workspaceName: 'harbor-project',
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
        return Promise.resolve(new Response(JSON.stringify(workspacePayload), { status: 200 }));
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

    expect(mountNode.textContent).toContain('Workspace: harbor-project');
    expect(mountNode.textContent).toContain('Franco-Belgian Clear Line');
    expect(mountNode.textContent).toContain('Courier Ren');

    const inspectorTab = mountNode.querySelector<HTMLButtonElement>('#tab-run_inspector');
    await act(() => {
      inspectorTab?.click();
    });
    expect(mountNode.textContent).toContain('Review, Diff & Color Inspector');
    expect(mountNode.textContent).toContain('harbor v001');
  });
});
