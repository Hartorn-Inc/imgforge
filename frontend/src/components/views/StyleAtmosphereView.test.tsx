import { render } from 'preact';
import { act } from 'preact/test-utils';
import { afterEach, beforeEach, describe, expect, test, vi } from 'vitest';
import type {
  AsyncViewState,
  LifecycleState,
  LibrarySource,
  ReferencePackSummary,
  WorkspaceSummary,
} from '../../types/domain';
import { StyleAtmosphereView } from './StyleAtmosphereView';

function makeMultiSheetPack(): ReferencePackSummary {
  return {
    subjectSlug: 'courier-ren',
    styleSlug: 'franco-belgian-line',
    sheetKind: 'turnaround',
    sliceCount: 3,
    lifecycle: 'draft',
    version: 1,
    visibility: 'public',
    sheets: [
      {
        role: 'turnaround',
        filename: 'turnaround.png',
        imageUrl: '/api/assets/turnaround.png',
        sha256: 't1',
        width: 1536,
        height: 1024,
        bgCoverage: 0.9,
        borderBgCoverage: 1.0,
        foregroundPixels: 1000,
        stripY: 952,
        stripHeight: 72,
      },
      {
        role: 'poses_expressions',
        filename: 'poses_expressions.png',
        imageUrl: '/api/assets/poses.png',
        sha256: 'p1',
        width: 1536,
        height: 1024,
        bgCoverage: 0.9,
        borderBgCoverage: 1.0,
        foregroundPixels: 1000,
        stripY: 952,
        stripHeight: 72,
      },
    ],
    slices: [
      {
        id: 'turnaround_001',
        file: 'slices/turnaround_001.png',
        imageUrl: '/api/assets/slices/turnaround_001.png',
        rect: { x: 20, y: 20, width: 100, height: 200 },
        source: 'auto',
        label: 'Front',
        sha256: 's1',
      },
      {
        id: 'poses_001',
        file: 'slices/poses_001.png',
        imageUrl: '/api/assets/slices/poses_001.png',
        rect: { x: 40, y: 40, width: 120, height: 180 },
        source: 'auto',
        label: 'Sprint',
        sha256: 's2',
      },
    ],
    createdAtIso: '2026-11-13T00:00:00Z',
  };
}

function makeWorkspace(
  subjectSource: LibrarySource = 'project',
  subjectLifecycle: LifecycleState = 'draft',
): WorkspaceSummary {
  return {
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
        paletteLock: [{ label: 'coat_slate', hex: '#2C3E50', surfaceSlot: 'coat' }],
        visibility: 'public',
        lifecycle: subjectLifecycle,
        source: subjectSource,
        asymmetricMarkers: [],
        positiveWeaponGeometry: [],
        tags: [],
      },
    ],
    styles: [
      {
        slug: 'franco-belgian-line',
        name: 'Franco-Belgian Clear Line',
        medium: 'ink',
        exemplarCount: 1,
        lifecycle: 'canonical',
        source: 'builtin',
        linework: 'crisp',
        shadingAndTexture: 'flat',
        paletteProfile: 'warm',
        framingAndBorder: 'clean',
        styleGuardrails: ['crisp'],
        exemplarUrls: ['/api/presets/styles/franco-belgian-line/exemplars/0'],
      },
    ],
    atmospheres: [
      {
        slug: 'golden-dawn',
        name: 'Golden Dawn',
        lightDirection: 'east',
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
    referencePacks: [makeMultiSheetPack()],
    runs: [],
    manifestLock: {
      schemaVersion: 1,
      canonicalCount: 0,
      supersededCount: 0,
      canonical: {},
      superseded: [],
    },
  };
}

async function flushMicrotasks(): Promise<void> {
  for (let i = 0; i < 4; i += 1) {
    await new Promise((resolve) => {
      setTimeout(resolve, 0);
    });
  }
}

describe('StyleAtmosphereView', () => {
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

  test('retains newly created Style and Atmosphere selection while waiting for onRefreshWorkspace', async () => {
    let workspace = makeWorkspace();
    let resolveRefresh!: () => void;

    const baseStyle = workspace.styles[0];
    const baseAtmo = workspace.atmospheres[0];
    if (baseStyle === undefined || baseAtmo === undefined) {
      throw new Error('Expected seeded Style and Atmosphere');
    }

    const clonedStyle = {
      ...baseStyle,
      slug: 'harbor-copy',
      name: 'Harbor Copy',
      lifecycle: 'draft' as const,
      source: 'project' as const,
    };
    const createdAtmo = {
      ...baseAtmo,
      slug: 'custom-atmo',
      name: 'Custom Atmosphere',
      source: 'project' as const,
    };

    vi.stubGlobal(
      'fetch',
      vi.fn((url: string) => {
        if (url === '/api/styles') {
          return Promise.resolve(new Response(JSON.stringify(clonedStyle), { status: 201 }));
        }
        if (url === '/api/atmospheres') {
          return Promise.resolve(new Response(JSON.stringify(createdAtmo), { status: 201 }));
        }
        return Promise.resolve(new Response(null, { status: 404 }));
      }),
    );

    const rerenderView = () => {
      const wsState: AsyncViewState<WorkspaceSummary> = { status: 'ready', data: workspace };
      render(
        <StyleAtmosphereView
          workspaceState={wsState}
          onRefreshWorkspace={() =>
            new Promise<void>((resolve) => {
              resolveRefresh = () => {
                workspace = {
                  ...workspace,
                  styles: [...workspace.styles, clonedStyle],
                  atmospheres: [...workspace.atmospheres, createdAtmo],
                };
                rerenderView();
                resolve();
              };
            })
          }
        />,
        mountNode,
      );
    };

    await act(async () => {
      rerenderView();
      await flushMicrotasks();
    });

    const cloneBtn = Array.from(mountNode.querySelectorAll('button')).find((b) =>
      b.textContent.includes('Clone Selected'),
    );
    expect(cloneBtn).not.toBeUndefined();
    await act(() => {
      cloneBtn?.click();
    });

    const createStyleBtn = Array.from(mountNode.querySelectorAll('button')).find(
      (b) => b.textContent === 'Create Style',
    );
    await act(async () => {
      createStyleBtn?.click();
      await flushMicrotasks();
    });

    // Before onRefreshWorkspace resolves, newly created slug must NOT be overwritten by useEffect.
    expect(mountNode.querySelector<HTMLInputElement>('#style-slug-input')?.value).toBe(
      'harbor-copy',
    );

    await act(async () => {
      resolveRefresh();
      await flushMicrotasks();
    });

    expect(mountNode.querySelector<HTMLInputElement>('#style-slug-input')?.value).toBe(
      'harbor-copy',
    );

    const newAtmoBtn = Array.from(mountNode.querySelectorAll('button')).find(
      (b) => b.textContent === 'New Atmosphere',
    );
    await act(() => {
      newAtmoBtn?.click();
    });
    expect(mountNode.querySelector<HTMLInputElement>('#atmo-display-name')?.value).toBe(
      'Custom Atmosphere',
    );

    const createAtmoBtn = Array.from(mountNode.querySelectorAll('button')).find(
      (b) => b.textContent === 'Create Atmosphere',
    );
    await act(async () => {
      createAtmoBtn?.click();
      await flushMicrotasks();
    });

    expect(mountNode.querySelector<HTMLInputElement>('#atmo-slug')?.value).toBe('custom-atmo');
    await act(async () => {
      resolveRefresh();
      await flushMicrotasks();
    });
    expect(mountNode.querySelector<HTMLInputElement>('#atmo-slug')?.value).toBe('custom-atmo');
  });

  test('scopes ReferencePack slices per sheetRole on init, switch, and reslice and keeps tr DOM identity on ID edit', async () => {
    const resliceBodies: unknown[] = [];
    vi.stubGlobal(
      'fetch',
      vi.fn((_url: string, init?: RequestInit) => {
        resliceBodies.push(typeof init?.body === 'string' ? JSON.parse(init.body) : null);
        return Promise.resolve(
          new Response(
            JSON.stringify({
              schemaVersion: 1,
              sheetFilename: 'poses_expressions.png',
              sheetWidth: 1536,
              sheetHeight: 1024,
              calibrationStripHeight: 72,
              slices: makeMultiSheetPack().slices,
            }),
            { status: 200 },
          ),
        );
      }),
    );

    const wsState: AsyncViewState<WorkspaceSummary> = { status: 'ready', data: makeWorkspace() };
    await act(async () => {
      render(
        <StyleAtmosphereView
          workspaceState={wsState}
          onRefreshWorkspace={() => Promise.resolve()}
        />,
        mountNode,
      );
      await flushMicrotasks();
    });

    // Initialized with turnaround sheet -> only turnaround_001 should appear in the table.
    const initialIds = Array.from(
      mountNode.querySelectorAll<HTMLInputElement>('input[aria-label$=" ID"]'),
    ).map((el) => el.value);
    expect(initialIds).toEqual(['turnaround_001']);

    // Switch #pack-sheet-role to poses_expressions.
    const roleSelect = mountNode.querySelector<HTMLSelectElement>('#pack-sheet-role');
    expect(roleSelect).not.toBeNull();
    if (roleSelect === null) {
      return;
    }
    await act(() => {
      roleSelect.value = 'poses_expressions';
      roleSelect.dispatchEvent(new Event('change', { bubbles: true }));
    });

    const posesIds = Array.from(
      mountNode.querySelectorAll<HTMLInputElement>('input[aria-label$=" ID"]'),
    ).map((el) => el.value);
    expect(posesIds).toEqual(['poses_001']);

    // Verify editing the Slice ID keeps the exact same <tr> DOM node (stable rowKey).
    const rowBefore = mountNode.querySelector('tbody tr');
    const idInput = mountNode.querySelector<HTMLInputElement>('input[aria-label="Slice 1 ID"]');
    expect(idInput).not.toBeNull();
    if (idInput === null) {
      return;
    }
    await act(() => {
      idInput.value = 'poses_hero';
      idInput.dispatchEvent(new Event('input', { bubbles: true }));
    });
    const rowAfter = mountNode.querySelector('tbody tr');
    expect(rowAfter).toBe(rowBefore);

    // Apply reslice and verify only poses_* overrides are submitted and retained after response.
    const resliceBtn = Array.from(mountNode.querySelectorAll('button')).find(
      (b) => b.textContent === 'Apply Reslice',
    );
    await act(async () => {
      resliceBtn?.click();
      await flushMicrotasks();
    });

    expect(resliceBodies[0]).toEqual({
      sheetRole: 'poses_expressions',
      mode: 'replace_all',
      overrides: [
        { id: 'poses_hero', rect: { x: 40, y: 40, width: 120, height: 180 }, label: 'Sprint' },
      ],
    });
    const postResliceIds = Array.from(
      mountNode.querySelectorAll<HTMLInputElement>('input[aria-label$=" ID"]'),
    ).map((el) => el.value);
    expect(postResliceIds).toEqual(['poses_001']);
  });

  test.each<{
    id: string;
    source: LibrarySource;
    lifecycle: LifecycleState;
    expectedDisabled: boolean;
  }>([
    {
      id: 'enables-palette-lock-inputs-for-project-draft-subject',
      source: 'project',
      lifecycle: 'draft',
      expectedDisabled: false,
    },
    {
      id: 'disables-palette-lock-inputs-for-user-tier-subject',
      source: 'user',
      lifecycle: 'draft',
      expectedDisabled: true,
    },
    {
      id: 'disables-palette-lock-inputs-for-canonical-subject',
      source: 'project',
      lifecycle: 'canonical',
      expectedDisabled: true,
    },
    {
      id: 'disables-palette-lock-inputs-for-superseded-subject',
      source: 'project',
      lifecycle: 'superseded',
      expectedDisabled: true,
    },
  ])('$id', async ({ source, lifecycle, expectedDisabled }) => {
    const wsState: AsyncViewState<WorkspaceSummary> = {
      status: 'ready',
      data: makeWorkspace(source, lifecycle),
    };
    await act(async () => {
      render(
        <StyleAtmosphereView
          workspaceState={wsState}
          onRefreshWorkspace={() => Promise.resolve()}
        />,
        mountNode,
      );
      await flushMicrotasks();
    });

    const swatchInput = mountNode.querySelector<HTMLInputElement>('#swatch-name');
    expect(swatchInput?.disabled).toBe(expectedDisabled);
  });

  test('surfaces error banner when reading a dropped PNG exemplar fails', async () => {
    const wsState: AsyncViewState<WorkspaceSummary> = { status: 'ready', data: makeWorkspace() };
    await act(async () => {
      render(
        <StyleAtmosphereView
          workspaceState={wsState}
          onRefreshWorkspace={() => Promise.resolve()}
        />,
        mountNode,
      );
      await flushMicrotasks();
    });

    const fileInput = mountNode.querySelector<HTMLInputElement>('#exemplar-file-input');
    expect(fileInput).not.toBeNull();
    const brokenFile = {
      name: 'broken.png',
      type: 'image/png',
      arrayBuffer: () => Promise.reject(new Error('Disk read failed')),
    } as unknown as File;

    Object.defineProperty(fileInput, 'files', {
      configurable: true,
      value: {
        length: 1,
        item: () => brokenFile,
      },
    });

    await act(async () => {
      fileInput?.dispatchEvent(new Event('change', { bubbles: true }));
      await flushMicrotasks();
    });

    expect(mountNode.querySelector('[role="alert"]')?.textContent).toContain('Disk read failed');
  });
});
