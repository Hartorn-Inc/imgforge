import { describe, expect, test } from 'vitest';
import {
  blobToRawBase64,
  connectWorkspaceEvents,
  createAtmosphere,
  createStyle,
  decodeColorQaReport,
  decodeCriticReport,
  decodeWorkspaceEvent,
  executePatchEdit,
  exportRun,
  extractPngFilesFromDataTransfer,
  fetchWorkspace,
  inspectRun,
  promoteReferencePack,
  promoteRun,
  promoteStyle,
  renderGuideRemote,
  resliceReferencePack,
  updateAtmosphere,
  updateStyle,
  updateSubjectPaletteLock,
  uploadStyleExemplars,
  validateGuideRemote,
  validateReferencePack,
  validateRun,
} from './client';
import { createDefaultGuideSpec } from '../types/guideSpec';

function sampleWorkspacePayload(hasWorkspace: boolean): Record<string, unknown> {
  return {
    hasWorkspace,
    workspaceRoot: hasWorkspace ? '/tmp/ws' : null,
    workspaceName: hasWorkspace ? 'phase5-ws' : null,
    authProvisioned: true,
    subjects: hasWorkspace
      ? [
          {
            slug: 'courier-ren',
            name: 'Courier Ren',
            category: 'character',
            heightMeters: 1.74,
            paletteLock: [{ label: 'coat_slate', hex: '#2C3E50', surfaceSlot: 'coat' }],
            visibility: 'public',
            lifecycle: 'draft',
            source: 'project',
            asymmetricMarkers: ['brass lantern on left hip'],
            positiveWeaponGeometry: [],
            tags: ['npc'],
          },
        ]
      : [],
    styles: [
      {
        slug: 'franco-belgian-line',
        name: 'Franco-Belgian Clear Line',
        medium: 'pen and ink',
        exemplarCount: 2,
        lifecycle: 'canonical',
        source: 'builtin',
        linework: 'uniform crisp ink',
        shadingAndTexture: 'flat cel shading',
        paletteProfile: 'warm coastal',
        framingAndBorder: 'clean border',
        styleGuardrails: ['crisp contour lines'],
        exemplarUrls: ['/api/presets/styles/franco-belgian-line/exemplars/0'],
      },
    ],
    atmospheres: [
      {
        slug: 'golden-dawn',
        name: 'Golden Dawn',
        lightDirection: 'low east sun',
        colorTemperatureKelvin: 4200,
        exposureFloor: 32,
        maxAmbientTintBlend: 0.2,
        particles: 'light morning mist',
        source: 'builtin',
        secondaryLightDirection: 'cool zenith fill',
        minMeanLStar: 32,
        minP10LStar: 12,
      },
    ],
    referencePacks: hasWorkspace
      ? [
          {
            subjectSlug: 'courier-ren',
            styleSlug: 'franco-belgian-line',
            sheetKind: 'turnaround',
            sliceCount: 1,
            lifecycle: 'draft',
            version: 1,
            visibility: 'public',
            sheets: [
              {
                role: 'turnaround',
                filename: 'turnaround.png',
                imageUrl:
                  '/api/assets/library/subjects/courier-ren/packs/franco-belgian-line/turnaround.png',
                sha256: 'abc123',
                width: 1536,
                height: 1024,
                bgCoverage: 0.9,
                borderBgCoverage: 1.0,
                foregroundPixels: 50000,
                stripY: 952,
                stripHeight: 72,
              },
            ],
            slices: [
              {
                id: 'turnaround_001',
                file: 'slices/turnaround_001.png',
                imageUrl:
                  '/api/assets/library/subjects/courier-ren/packs/franco-belgian-line/slices/turnaround_001.png',
                rect: { x: 100, y: 120, width: 200, height: 300 },
                source: 'auto',
                label: 'Front',
                sha256: 'def456',
              },
            ],
            createdAtIso: '2026-11-13T00:00:00Z',
          },
        ]
      : [],
    runs: hasWorkspace
      ? [
          {
            runId: 'harbor-dock-v001',
            slug: 'harbor-dock',
            version: 1,
            versionLabel: 'v001',
            lifecycle: 'draft',
            visibility: 'public',
            widthPx: 1024,
            heightPx: 1024,
            visionCriticPassed: null,
            colorQaMaxDeltaE2000: 1.25,
            createdAtIso: '2026-11-13T00:10:00Z',
            imageUrl: '/api/assets/runs/harbor-dock/v001.png',
            compiledPrompt: 'harbor dock scene',
            backendName: 'codex-native',
            structuralChecks: ['dimensions_ok'],
          },
        ]
      : [],
    manifestLock: {
      schemaVersion: 1,
      canonicalCount: hasWorkspace ? 1 : 0,
      supersededCount: 0,
      canonical: hasWorkspace
        ? {
            'style:harbor': {
              entry_kind: 'style',
              key: 'style:harbor',
              version: 1,
              version_label: 'v001',
              lifecycle: 'canonical',
              visibility: 'public',
              artifact_hashes: { 'library/styles/harbor/card.yaml': 'deadbeef' },
              promoted_at_iso: '2026-11-13T00:05:00Z',
            },
          }
        : {},
      superseded: [],
    },
  };
}

function sampleColorQaReport(passed: boolean): Record<string, unknown> {
  return {
    passed,
    exposure: {
      passed: true,
      metrics: {
        mean_l_star: 48.2,
        p10_l_star: 18.4,
        p90_l_star: 82.1,
        clipped_black_ratio: 0.01,
        pixel_count: 10000,
      },
      floor: { min_mean_l_star: 32, min_p10_l_star: 12 },
      reasons: [],
    },
    palette_adherence: [
      {
        passed,
        subject_slug: 'courier-ren',
        clusters: [
          {
            centroid_oklab: [0.45, -0.02, -0.05],
            centroid_hex: '#2C3E50',
            weight: 0.42,
            pixel_count: 4200,
            closest_swatch_name: 'coat_slate',
            closest_swatch_hex: '#2C3E50',
            delta_e2000: passed ? 1.2 : 28.4,
            lightness_shift: 1.1,
            chroma_shift: 0.9,
          },
        ],
        max_swatch_delta_e2000: passed ? 1.2 : 28.4,
        weighted_mean_delta_e2000: passed ? 1.1 : 18.0,
        forbidden_hue_intrusions: [],
        reasons: passed ? [] : ['DeltaE_2000 exceeded threshold'],
      },
    ],
    untouched_drift: {
      passed: true,
      untouched_pixel_count: 8500,
      mean_delta_e2000: 0.0,
      p95_delta_e2000: 0.0,
      max_delta_e2000: 0.0,
      fraction_exceeding_threshold: 0.0,
      threshold_delta_e2000: 2.0,
      reasons: [],
    },
    sparkle: {
      passed: true,
      flagged_blobs: [],
      total_flagged_pixels: 0,
      reasons: [],
    },
    max_delta_e2000: passed ? 1.2 : 28.4,
    notes: ['checked'],
  };
}

function sampleCriticReport(passed: boolean): Record<string, unknown> {
  return {
    schema_version: 1,
    passed,
    checks: [
      {
        trait_id: 'zero-leaked-text',
        category: 'zero_leaked_text',
        passed,
        observation: passed ? 'clean' : 'text watermark detected',
      },
    ],
    summary: passed ? 'All checks passed' : 'Text leak detected',
  };
}

describe('imgforge API client serialization and defensive decoding', () => {
  test.each<{ id: string; hasWorkspace: boolean }>([
    { id: 'decodes-configured-workspace-summary', hasWorkspace: true },
    { id: 'decodes-unconfigured-workspace-summary', hasWorkspace: false },
  ])('$id', async ({ hasWorkspace }) => {
    const fetchImpl: typeof fetch = () =>
      Promise.resolve(
        new Response(JSON.stringify(sampleWorkspacePayload(hasWorkspace)), {
          status: 200,
          headers: { 'Content-Type': 'application/json' },
        }),
      );

    const res = await fetchWorkspace({ fetchImpl });
    expect(res.ok).toBe(true);
    if (res.ok) {
      expect(res.data.hasWorkspace).toBe(hasWorkspace);
      expect(res.data.styles).toHaveLength(1);
      expect(res.data.manifestLock.canonicalCount).toBe(hasWorkspace ? 1 : 0);
    }
  });

  test.each<{
    id: string;
    status: number;
    body: unknown;
    expectedCode: string;
    expectedMessagePart: string;
  }>([
    {
      id: 'maps-409-canonical-immutable',
      status: 409,
      body: { error: 'canonical_immutable', message: 'Style is canonical' },
      expectedCode: 'canonical_immutable',
      expectedMessagePart: 'Style is canonical',
    },
    {
      id: 'maps-403-visibility-violation',
      status: 403,
      body: { error: 'visibility_violation', message: 'internal asset cannot export public' },
      expectedCode: 'visibility_violation',
      expectedMessagePart: 'internal asset',
    },
    {
      id: 'maps-428-workspace-unconfigured',
      status: 428,
      body: { error: 'workspace_unconfigured', message: 'no imgforge workspace is open' },
      expectedCode: 'workspace_unconfigured',
      expectedMessagePart: 'no imgforge workspace',
    },
    {
      id: 'maps-422-qa-failed',
      status: 422,
      body: { error: 'qa_failed', message: 'DeltaE exceeded' },
      expectedCode: 'qa_failed',
      expectedMessagePart: 'DeltaE exceeded',
    },
  ])('$id', async ({ status, body, expectedCode, expectedMessagePart }) => {
    const fetchImpl: typeof fetch = () =>
      Promise.resolve(
        new Response(JSON.stringify(body), {
          status,
          headers: { 'Content-Type': 'application/json' },
        }),
      );

    const res = await promoteStyle('harbor', undefined, { fetchImpl });
    expect(res.ok).toBe(false);
    if (!res.ok) {
      expect(res.error.code).toBe(expectedCode);
      expect(res.error.httpStatus).toBe(status);
      expect(res.error.message).toContain(expectedMessagePart);
    }
  });

  test('sends exact CreateAtmosphereRequest and UpdateAtmosphereRequest fields', async () => {
    const calls: { url: string; method: string; body: unknown }[] = [];
    const dto = {
      slug: 'harbor-noon',
      name: 'Harbor Noon',
      lightDirection: 'high overhead sun',
      colorTemperatureKelvin: 5600,
      exposureFloor: 38,
      maxAmbientTintBlend: 0.15,
      particles: 'crisp sea breeze',
      source: 'project',
      secondaryLightDirection: 'sea bounce',
      minMeanLStar: 38,
      minP10LStar: 14,
    };

    const fetchImpl: typeof fetch = (input, init) => {
      calls.push({
        url:
          typeof input === 'string' ? input : input instanceof URL ? input.toString() : input.url,
        method: init?.method ?? 'GET',
        body: typeof init?.body === 'string' ? (JSON.parse(init.body) as unknown) : null,
      });
      return Promise.resolve(
        new Response(JSON.stringify(dto), {
          status: 201,
          headers: { 'Content-Type': 'application/json' },
        }),
      );
    };

    const createRes = await createAtmosphere(
      {
        slug: 'harbor-noon',
        displayName: 'Harbor Noon',
        keyLightDirection: 'high overhead sun',
        secondaryLightDirection: 'sea bounce',
        colorTemperatureK: 5600,
        maxAmbientTintBlend: 0.15,
        minMeanLStar: 38,
        minP10LStar: 14,
        weatherAndParticles: 'crisp sea breeze',
      },
      { fetchImpl },
    );
    expect(createRes.ok).toBe(true);
    expect(calls[0]?.body).toEqual({
      slug: 'harbor-noon',
      displayName: 'Harbor Noon',
      keyLightDirection: 'high overhead sun',
      secondaryLightDirection: 'sea bounce',
      colorTemperatureK: 5600,
      maxAmbientTintBlend: 0.15,
      minMeanLStar: 38,
      minP10LStar: 14,
      weatherAndParticles: 'crisp sea breeze',
    });

    const updateRes = await updateAtmosphere(
      'harbor-noon',
      { colorTemperatureK: 5900, weatherAndParticles: 'mist' },
      { fetchImpl },
    );
    expect(updateRes.ok).toBe(true);
    expect(calls[1]?.method).toBe('PUT');
    expect(calls[1]?.body).toEqual({ colorTemperatureK: 5900, weatherAndParticles: 'mist' });
  });

  test('covers Style CRUD, PaletteLock, GuideSpec, ReferencePack, PatchEdit, Inspect, and Export routes', async () => {
    const routes: { url: string; method: string }[] = [];
    const sampleStyle = (sampleWorkspacePayload(true)['styles'] as unknown[])[0];
    const sampleSubject = (sampleWorkspacePayload(true)['subjects'] as unknown[])[0];
    const samplePack = (sampleWorkspacePayload(true)['referencePacks'] as unknown[])[0];
    const sampleRun = (sampleWorkspacePayload(true)['runs'] as unknown[])[0];

    const fetchImpl: typeof fetch = (input, init) => {
      const url =
        typeof input === 'string' ? input : input instanceof URL ? input.toString() : input.url;
      const method = init?.method ?? 'GET';
      routes.push({ url, method });

      if (
        url === '/api/styles' ||
        url === '/api/styles/harbor' ||
        url === '/api/styles/harbor/exemplars'
      ) {
        return Promise.resolve(new Response(JSON.stringify(sampleStyle), { status: 200 }));
      }
      if (url === '/api/styles/harbor/promote') {
        return Promise.resolve(
          new Response(JSON.stringify({ style: sampleStyle, manifestLock: null }), { status: 200 }),
        );
      }
      if (url === '/api/subjects/courier-ren/palette-lock') {
        return Promise.resolve(new Response(JSON.stringify(sampleSubject), { status: 200 }));
      }
      if (url === '/api/guide/validate') {
        return Promise.resolve(
          new Response(
            JSON.stringify({ valid: true, spec: createDefaultGuideSpec('Blockout'), errors: [] }),
            { status: 200 },
          ),
        );
      }
      if (url === '/api/guide/render') {
        return Promise.resolve(
          new Response(
            JSON.stringify({
              id: 'guide-blockout-001',
              variant: 'Blockout',
              width: 1024,
              height: 1024,
              pngBase64: 'iVBORw0KGgo=',
              errors: [],
            }),
            { status: 200 },
          ),
        );
      }
      if (url === '/api/packs/courier-ren/franco-belgian-line/reslice') {
        return Promise.resolve(
          new Response(
            JSON.stringify({
              schemaVersion: 1,
              sheetFilename: 'turnaround.png',
              sheetWidth: 1536,
              sheetHeight: 1024,
              calibrationStripHeight: 72,
              slices: (samplePack as Record<string, unknown>)['slices'],
            }),
            { status: 200 },
          ),
        );
      }
      if (url === '/api/packs/courier-ren/franco-belgian-line/validate') {
        return Promise.resolve(
          new Response(
            JSON.stringify({
              referencePack: samplePack,
              checks: ['ok'],
              criticReport: sampleCriticReport(true),
            }),
            { status: 200 },
          ),
        );
      }
      if (url === '/api/packs/courier-ren/franco-belgian-line/promote') {
        return Promise.resolve(
          new Response(
            JSON.stringify({
              referencePack: samplePack,
              canonicalCount: 2,
              supersededCount: 0,
            }),
            { status: 200 },
          ),
        );
      }
      if (url === '/api/edit/patch') {
        return Promise.resolve(
          new Response(
            JSON.stringify({
              run: sampleRun,
              beforeImageUrl: '/api/assets/runs/harbor-dock/v001.png',
              afterImageUrl: '/api/assets/runs/harbor-dock/v002.png',
              cropRect: { x: 16, y: 16, width: 80, height: 80 },
              roiRect: { x: 40, y: 40, width: 32, height: 32 },
              reinhardGains: { gain_l: 1.02, gain_a: 0.99, gain_b: 1.01, collar_pixel_count: 512 },
              colorQa: sampleColorQaReport(true),
            }),
            { status: 200 },
          ),
        );
      }
      if (url === '/api/runs/harbor-dock/1/inspect') {
        return Promise.resolve(
          new Response(
            JSON.stringify({
              run: sampleRun,
              telemetry: {
                width: 1024,
                height: 1024,
                opaquePixelCount: 1048576,
                lStarBins: [{ binIndex: 0, minLStar: 0, maxLStar: 5, count: 10, fraction: 0.01 }],
                vectorscopePoints: [{ oklabL: 0.5, a: 0.01, b: -0.02, lStar: 48, hex: '#2C3E50' }],
              },
              colorQa: sampleColorQaReport(true),
              heatmaps: { drift: 'AA==', bleed: null, sparkle: 'BB==', outOfRoi: null },
            }),
            { status: 200 },
          ),
        );
      }
      if (url === '/api/runs/harbor-dock/1/validate') {
        return Promise.resolve(
          new Response(
            JSON.stringify({
              run: sampleRun,
              colorQa: sampleColorQaReport(true),
              criticReport: sampleCriticReport(false),
            }),
            { status: 200 },
          ),
        );
      }
      if (url === '/api/runs/harbor-dock/1/promote') {
        return Promise.resolve(
          new Response(JSON.stringify({ run: sampleRun, canonicalCount: 2, supersededCount: 1 }), {
            status: 200,
          }),
        );
      }
      if (url === '/api/export') {
        return Promise.resolve(
          new Response(
            JSON.stringify({
              sourceId: 'harbor-dock-v001',
              sourceVisibility: 'public',
              profileVisibility: 'public',
              matchGradeApplied: true,
              artifacts: [
                {
                  role: 'web_webp',
                  path: 'exports/harbor-dock.webp',
                  assetUrl: '/api/assets/exports/harbor-dock.webp',
                  width: 1024,
                  height: 1024,
                  sha256: 'abc',
                },
              ],
            }),
            { status: 200 },
          ),
        );
      }
      return Promise.resolve(new Response(null, { status: 404 }));
    };

    expect(
      (
        await createStyle(
          { slug: 'harbor', displayName: 'Harbor', cloneFromSlug: 'franco-belgian-line' },
          { fetchImpl },
        )
      ).ok,
    ).toBe(true);
    expect((await updateStyle('harbor', { medium: 'gouache' }, { fetchImpl })).ok).toBe(true);
    expect(
      (
        await uploadStyleExemplars(
          'harbor',
          { exemplarsBase64: ['iVBORw0KGgo='], replace: true },
          { fetchImpl },
        )
      ).ok,
    ).toBe(true);
    expect((await promoteStyle('harbor', 'candidate', { fetchImpl })).ok).toBe(true);
    expect(
      (
        await updateSubjectPaletteLock(
          'courier-ren',
          { name: 'lantern', colorHex: '#D4A843', surfaceSlot: 'lantern' },
          { fetchImpl },
        )
      ).ok,
    ).toBe(true);
    expect((await validateGuideRemote(createDefaultGuideSpec('Blockout'), { fetchImpl })).ok).toBe(
      true,
    );
    expect((await renderGuideRemote(createDefaultGuideSpec('Blockout'), { fetchImpl })).ok).toBe(
      true,
    );
    expect(
      (
        await resliceReferencePack(
          'courier-ren',
          'franco-belgian-line',
          {
            sheetRole: 'turnaround',
            mode: 'replace_all',
            overrides: [
              { id: 'front', rect: { x: 10, y: 10, width: 100, height: 100 }, label: 'Front' },
            ],
          },
          { fetchImpl },
        )
      ).ok,
    ).toBe(true);
    const valPack = await validateReferencePack(
      'courier-ren',
      'franco-belgian-line',
      { runCritic: true },
      { fetchImpl },
    );
    expect(valPack.ok).toBe(true);
    if (valPack.ok) {
      expect(valPack.data.criticReport?.passed).toBe(true);
    }
    expect(
      (await promoteReferencePack('courier-ren', 'franco-belgian-line', { fetchImpl })).ok,
    ).toBe(true);

    const patch = await executePatchEdit(
      {
        slug: 'harbor-dock',
        runSlug: 'harbor-dock',
        runVersion: 1,
        roi: { x: 40, y: 40, width: 32, height: 32 },
        prepMode: 'cue_outline',
        collarWidthPx: 24,
        featherPx: 12,
        prompt: 'refine lantern',
        visibility: 'public',
      },
      { fetchImpl },
    );
    expect(patch.ok).toBe(true);
    if (patch.ok) {
      expect(patch.data.reinhardGains.collar_pixel_count).toBe(512);
      expect(patch.data.colorQa.untouched_drift?.max_delta_e2000).toBe(0);
    }

    const insp = await inspectRun(
      'harbor-dock',
      1,
      { atmosphereSlug: 'golden-dawn', subjectSlugs: ['courier-ren'], maxSamples: 128 },
      { fetchImpl },
    );
    expect(insp.ok).toBe(true);
    if (insp.ok) {
      expect(insp.data.telemetry.lStarBins).toHaveLength(1);
      expect(insp.data.heatmaps.drift).toBe('AA==');
    }

    const valRun = await validateRun('harbor-dock', 1, { runCritic: true }, { fetchImpl });
    expect(valRun.ok).toBe(true);
    if (valRun.ok) {
      expect(valRun.data.criticReport?.passed).toBe(false);
    }
    expect((await promoteRun('harbor-dock', 1, { fetchImpl })).ok).toBe(true);
    const exp = await exportRun(
      {
        runSlug: 'harbor-dock',
        runVersion: 1,
        profile: 'web_webp',
        visibility: 'public',
        matchGrade: { enabled: true, gradeStrength: 0.35 },
      },
      { fetchImpl },
    );
    expect(exp.ok).toBe(true);
    if (exp.ok) {
      expect(exp.data.matchGradeApplied).toBe(true);
      expect(exp.data.artifacts[0]?.assetUrl).toBe('/api/assets/exports/harbor-dock.webp');
    }
    expect(routes).toHaveLength(15);
  });

  test.each<{ id: string; input: unknown; expectedValid: boolean }>([
    {
      id: 'decodes-passing-snake-case-color-qa',
      input: sampleColorQaReport(true),
      expectedValid: true,
    },
    {
      id: 'decodes-failing-snake-case-color-qa',
      input: sampleColorQaReport(false),
      expectedValid: true,
    },
    {
      id: 'rejects-camel-case-color-qa',
      input: { passed: true, paletteAdherence: [], notes: [] },
      expectedValid: false,
    },
  ])('$id', ({ input, expectedValid }) => {
    expect(decodeColorQaReport(input) !== null).toBe(expectedValid);
  });

  test.each<{ id: string; input: unknown; expectedValid: boolean }>([
    {
      id: 'decodes-snake-case-critic-report',
      input: sampleCriticReport(true),
      expectedValid: true,
    },
    {
      id: 'rejects-unknown-critic-category',
      input: {
        schema_version: 1,
        passed: true,
        summary: 'ok',
        checks: [{ trait_id: 'x', category: 'bogus', passed: true, observation: 'x' }],
      },
      expectedValid: false,
    },
  ])('$id', ({ input, expectedValid }) => {
    expect(decodeCriticReport(input) !== null).toBe(expectedValid);
  });

  test('decodes WorkspaceEvent and handles WebSocket lifecycle cleanly', () => {
    const ev = decodeWorkspaceEvent({
      eventType: 'promoted',
      resourceKind: 'run',
      id: 'harbor-dock-v001',
      lifecycle: 'canonical',
      detail: 'Promoted',
      timestampIso: '2026-11-13T00:00:00Z',
    });
    expect(ev?.lifecycle).toBe('canonical');
    expect(decodeWorkspaceEvent({ eventType: 'bad' })).toBeNull();

    const listeners = new Map<string, ((e: unknown) => void)[]>();
    let closedCalled = false;
    const fakeWs = {
      addEventListener: (type: string, cb: (e: unknown) => void) => {
        const list = listeners.get(type) ?? [];
        list.push(cb);
        listeners.set(type, list);
      },
      close: () => {
        closedCalled = true;
      },
    } as unknown as WebSocket;

    const events: string[] = [];
    const statuses: string[] = [];
    const dispose = connectWorkspaceEvents({
      buildUrl: () => 'ws://127.0.0.1:8080/api/ws',
      webSocketFactory: () => fakeWs,
      onEvent: (received) => {
        events.push(received.id);
      },
      onStatusChange: (s) => {
        statuses.push(s);
      },
    });

    listeners.get('open')?.forEach((cb) => {
      cb(new Event('open'));
    });
    listeners.get('message')?.forEach((cb) => {
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
    expect(statuses).toEqual(['connecting', 'connected']);
    expect(events).toEqual(['harbor']);

    dispose();
    expect(closedCalled).toBe(true);
    expect(statuses.at(-1)).toBe('closed');
  });

  test('converts PNG Blob to raw base64 and extracts PNG files from DataTransfer', async () => {
    const pngBytes = new Uint8Array([137, 80, 78, 71, 13, 10, 26, 10]);
    const blob = new Blob([pngBytes], { type: 'image/png' });
    const b64 = await blobToRawBase64(blob);
    expect(b64).toBe('iVBORw0KGgo=');

    const pngFile = new File([pngBytes], 'exemplar.png', { type: 'image/png' });
    const jpgFile = new File([pngBytes], 'ignored.jpg', { type: 'image/jpeg' });
    const fakeTransfer = {
      files: {
        length: 2,
        item: (idx: number) => (idx === 0 ? jpgFile : pngFile),
      },
      items: [],
    } as unknown as DataTransfer;

    const extracted = extractPngFilesFromDataTransfer(fakeTransfer);
    expect(extracted.map((f) => f.name)).toEqual(['exemplar.png']);
    expect(extractPngFilesFromDataTransfer(null)).toEqual([]);
  });
});
