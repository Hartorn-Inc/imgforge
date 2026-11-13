import { describe, expect, test } from 'vitest';
import {
  blobToRawBase64,
  connectWorkspaceEvents,
  createAtmosphere,
  createStyle,
  decodeAtmosphereDto,
  decodeColorQaReport,
  decodeCriticReport,
  decodeReferencePackDto,
  decodeRunDto,
  decodeStyleDto,
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

  test.each<{
    id: string;
    mode: 'create' | 'update';
    expectedMethod: string;
    expectedBody: Record<string, unknown>;
  }>([
    {
      id: 'sends-exact-create-atmosphere-request-fields',
      mode: 'create',
      expectedMethod: 'POST',
      expectedBody: {
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
    },
    {
      id: 'sends-exact-update-atmosphere-request-fields',
      mode: 'update',
      expectedMethod: 'PUT',
      expectedBody: {
        colorTemperatureK: 5900,
        weatherAndParticles: 'mist',
      },
    },
  ])('$id', async ({ mode, expectedMethod, expectedBody }) => {
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
          status: mode === 'create' ? 201 : 200,
          headers: { 'Content-Type': 'application/json' },
        }),
      );
    };

    const res =
      mode === 'create'
        ? await createAtmosphere(
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
          )
        : await updateAtmosphere(
            'harbor-noon',
            { colorTemperatureK: 5900, weatherAndParticles: 'mist' },
            { fetchImpl },
          );

    expect(res.ok).toBe(true);
    expect(calls[0]?.method).toBe(expectedMethod);
    expect(calls[0]?.body).toEqual(expectedBody);
  });

  const sampleStyle = (sampleWorkspacePayload(true)['styles'] as unknown[])[0];
  const sampleSubject = (sampleWorkspacePayload(true)['subjects'] as unknown[])[0];
  const samplePack = (sampleWorkspacePayload(true)['referencePacks'] as unknown[])[0];
  const sampleRun = (sampleWorkspacePayload(true)['runs'] as unknown[])[0];

  test.each<{
    id: string;
    expectedMethod: string;
    expectedUrl: string;
    responseBody: unknown;
    invoke: (fetchImpl: typeof fetch) => Promise<{ readonly ok: boolean }>;
  }>([
    {
      id: 'route-create-style',
      expectedMethod: 'POST',
      expectedUrl: '/api/styles',
      responseBody: sampleStyle,
      invoke: (fetchImpl) =>
        createStyle(
          { slug: 'harbor', displayName: 'Harbor', cloneFromSlug: 'franco-belgian-line' },
          { fetchImpl },
        ),
    },
    {
      id: 'route-update-style',
      expectedMethod: 'PUT',
      expectedUrl: '/api/styles/harbor',
      responseBody: sampleStyle,
      invoke: (fetchImpl) => updateStyle('harbor', { medium: 'gouache' }, { fetchImpl }),
    },
    {
      id: 'route-upload-style-exemplars',
      expectedMethod: 'POST',
      expectedUrl: '/api/styles/harbor/exemplars',
      responseBody: sampleStyle,
      invoke: (fetchImpl) =>
        uploadStyleExemplars(
          'harbor',
          { exemplarsBase64: ['iVBORw0KGgo='], replace: true },
          { fetchImpl },
        ),
    },
    {
      id: 'route-promote-style',
      expectedMethod: 'POST',
      expectedUrl: '/api/styles/harbor/promote',
      responseBody: { style: sampleStyle, manifestLock: null },
      invoke: (fetchImpl) => promoteStyle('harbor', 'candidate', { fetchImpl }),
    },
    {
      id: 'route-update-subject-palette-lock',
      expectedMethod: 'POST',
      expectedUrl: '/api/subjects/courier-ren/palette-lock',
      responseBody: sampleSubject,
      invoke: (fetchImpl) =>
        updateSubjectPaletteLock(
          'courier-ren',
          { name: 'lantern', colorHex: '#D4A843', surfaceSlot: 'lantern' },
          { fetchImpl },
        ),
    },
    {
      id: 'route-validate-guide',
      expectedMethod: 'POST',
      expectedUrl: '/api/guide/validate',
      responseBody: { valid: true, spec: createDefaultGuideSpec('Blockout'), errors: [] },
      invoke: (fetchImpl) => validateGuideRemote(createDefaultGuideSpec('Blockout'), { fetchImpl }),
    },
    {
      id: 'route-render-guide',
      expectedMethod: 'POST',
      expectedUrl: '/api/guide/render',
      responseBody: {
        id: 'guide-blockout-001',
        variant: 'Blockout',
        width: 1024,
        height: 1024,
        pngBase64: 'iVBORw0KGgo=',
        errors: [],
      },
      invoke: (fetchImpl) => renderGuideRemote(createDefaultGuideSpec('Blockout'), { fetchImpl }),
    },
    {
      id: 'route-reslice-reference-pack',
      expectedMethod: 'POST',
      expectedUrl: '/api/packs/courier-ren/franco-belgian-line/reslice',
      responseBody: {
        schemaVersion: 1,
        sheetFilename: 'turnaround.png',
        sheetWidth: 1536,
        sheetHeight: 1024,
        calibrationStripHeight: 72,
        slices: (samplePack as Record<string, unknown>)['slices'],
      },
      invoke: (fetchImpl) =>
        resliceReferencePack(
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
        ),
    },
    {
      id: 'route-validate-reference-pack',
      expectedMethod: 'POST',
      expectedUrl: '/api/packs/courier-ren/franco-belgian-line/validate',
      responseBody: {
        referencePack: samplePack,
        checks: ['ok'],
        criticReport: sampleCriticReport(true),
      },
      invoke: (fetchImpl) =>
        validateReferencePack(
          'courier-ren',
          'franco-belgian-line',
          { runCritic: true },
          { fetchImpl },
        ),
    },
    {
      id: 'route-promote-reference-pack',
      expectedMethod: 'POST',
      expectedUrl: '/api/packs/courier-ren/franco-belgian-line/promote',
      responseBody: {
        referencePack: samplePack,
        canonicalCount: 2,
        supersededCount: 0,
      },
      invoke: (fetchImpl) =>
        promoteReferencePack('courier-ren', 'franco-belgian-line', { fetchImpl }),
    },
    {
      id: 'route-execute-patch-edit',
      expectedMethod: 'POST',
      expectedUrl: '/api/edit/patch',
      responseBody: {
        run: sampleRun,
        beforeImageUrl: '/api/assets/runs/harbor-dock/v001.png',
        afterImageUrl: '/api/assets/runs/harbor-dock/v002.png',
        cropRect: { x: 16, y: 16, width: 80, height: 80 },
        roiRect: { x: 40, y: 40, width: 32, height: 32 },
        reinhardGains: { gain_l: 1.02, gain_a: 0.99, gain_b: 1.01, collar_pixel_count: 512 },
        colorQa: sampleColorQaReport(true),
      },
      invoke: (fetchImpl) =>
        executePatchEdit(
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
        ),
    },
    {
      id: 'route-inspect-run',
      expectedMethod: 'POST',
      expectedUrl: '/api/runs/harbor-dock/1/inspect',
      responseBody: {
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
      },
      invoke: (fetchImpl) =>
        inspectRun(
          'harbor-dock',
          1,
          { atmosphereSlug: 'golden-dawn', subjectSlugs: ['courier-ren'], maxSamples: 128 },
          { fetchImpl },
        ),
    },
    {
      id: 'route-validate-run',
      expectedMethod: 'POST',
      expectedUrl: '/api/runs/harbor-dock/1/validate',
      responseBody: {
        run: sampleRun,
        colorQa: sampleColorQaReport(true),
        criticReport: sampleCriticReport(false),
      },
      invoke: (fetchImpl) => validateRun('harbor-dock', 1, { runCritic: true }, { fetchImpl }),
    },
    {
      id: 'route-promote-run',
      expectedMethod: 'POST',
      expectedUrl: '/api/runs/harbor-dock/1/promote',
      responseBody: { run: sampleRun, canonicalCount: 2, supersededCount: 1 },
      invoke: (fetchImpl) => promoteRun('harbor-dock', 1, { fetchImpl }),
    },
    {
      id: 'route-export-run',
      expectedMethod: 'POST',
      expectedUrl: '/api/export',
      responseBody: {
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
      },
      invoke: (fetchImpl) =>
        exportRun(
          {
            runSlug: 'harbor-dock',
            runVersion: 1,
            profile: 'web_webp',
            visibility: 'public',
            matchGrade: { enabled: true, gradeStrength: 0.35 },
          },
          { fetchImpl },
        ),
    },
  ])('$id', async ({ expectedMethod, expectedUrl, responseBody, invoke }) => {
    const calls: { url: string; method: string }[] = [];
    const fetchImpl: typeof fetch = (input, init) => {
      calls.push({
        url:
          typeof input === 'string' ? input : input instanceof URL ? input.toString() : input.url,
        method: init?.method ?? 'GET',
      });
      return Promise.resolve(new Response(JSON.stringify(responseBody), { status: 200 }));
    };

    const res = await invoke(fetchImpl);
    expect(res.ok).toBe(true);
    expect(calls).toEqual([{ url: expectedUrl, method: expectedMethod }]);
  });

  test.each<{ id: string; input: unknown; expectedValid: boolean }>([
    {
      id: 'decodes-valid-style-dto',
      input: sampleStyle,
      expectedValid: true,
    },
    {
      id: 'rejects-style-dto-with-non-array-guardrails',
      input: { ...(sampleStyle as Record<string, unknown>), styleGuardrails: 'crisp' },
      expectedValid: false,
    },
    {
      id: 'rejects-style-dto-with-unknown-source',
      input: { ...(sampleStyle as Record<string, unknown>), source: 'remote' },
      expectedValid: false,
    },
  ])('$id', ({ input, expectedValid }) => {
    expect(decodeStyleDto(input) !== null).toBe(expectedValid);
  });

  test.each<{ id: string; input: unknown; expectedValid: boolean }>([
    {
      id: 'decodes-valid-atmosphere-dto',
      input: (sampleWorkspacePayload(true)['atmospheres'] as unknown[])[0],
      expectedValid: true,
    },
    {
      id: 'rejects-atmosphere-dto-missing-kelvin',
      input: {
        ...((sampleWorkspacePayload(true)['atmospheres'] as Record<string, unknown>[])[0] ?? {}),
        colorTemperatureKelvin: 'warm',
      },
      expectedValid: false,
    },
    {
      id: 'rejects-atmosphere-dto-non-object',
      input: null,
      expectedValid: false,
    },
  ])('$id', ({ input, expectedValid }) => {
    expect(decodeAtmosphereDto(input) !== null).toBe(expectedValid);
  });

  test.each<{ id: string; input: unknown; expectedValid: boolean }>([
    {
      id: 'decodes-valid-reference-pack-dto',
      input: samplePack,
      expectedValid: true,
    },
    {
      id: 'rejects-reference-pack-dto-with-invalid-sheet-kind',
      input: { ...(samplePack as Record<string, unknown>), sheetKind: 'poster' },
      expectedValid: false,
    },
    {
      id: 'rejects-reference-pack-dto-with-malformed-slice-rect',
      input: {
        ...(samplePack as Record<string, unknown>),
        slices: [
          {
            id: 's1',
            file: 's1.png',
            imageUrl: '/x',
            rect: null,
            source: 'auto',
            label: null,
            sha256: 'a',
          },
        ],
      },
      expectedValid: false,
    },
  ])('$id', ({ input, expectedValid }) => {
    expect(decodeReferencePackDto(input) !== null).toBe(expectedValid);
  });

  test.each<{ id: string; input: unknown; expectedValid: boolean }>([
    {
      id: 'decodes-valid-run-dto',
      input: sampleRun,
      expectedValid: true,
    },
    {
      id: 'rejects-run-dto-with-invalid-lifecycle',
      input: { ...(sampleRun as Record<string, unknown>), lifecycle: 'archived' },
      expectedValid: false,
    },
    {
      id: 'rejects-run-dto-with-string-delta-e',
      input: { ...(sampleRun as Record<string, unknown>), colorQaMaxDeltaE2000: '1.25' },
      expectedValid: false,
    },
  ])('$id', ({ input, expectedValid }) => {
    expect(decodeRunDto(input) !== null).toBe(expectedValid);
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

  test.each<{
    id: string;
    input: unknown;
    expectedLifecycle: string | null;
    expectedValid: boolean;
  }>([
    {
      id: 'decodes-valid-workspace-event',
      input: {
        eventType: 'promoted',
        resourceKind: 'run',
        id: 'harbor-dock-v001',
        lifecycle: 'canonical',
        detail: 'Promoted',
        timestampIso: '2026-11-13T00:00:00Z',
      },
      expectedLifecycle: 'canonical',
      expectedValid: true,
    },
    {
      id: 'rejects-malformed-workspace-event',
      input: { eventType: 'bad' },
      expectedLifecycle: null,
      expectedValid: false,
    },
  ])('$id', ({ input, expectedLifecycle, expectedValid }) => {
    const ev = decodeWorkspaceEvent(input);
    expect(ev !== null).toBe(expectedValid);
    if (ev !== null) {
      expect(ev.lifecycle).toBe(expectedLifecycle);
    }
  });

  test('handles WebSocket connect, event dispatch, and clean close', () => {
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

  test.each<{
    id: string;
    createTransfer: (pngBytes: Uint8Array<ArrayBuffer>) => DataTransfer | null;
    expectedNames: readonly string[];
  }>([
    {
      id: 'extracts-only-png-files-from-file-list',
      createTransfer: (pngBytes) => {
        const pngFile = new File([pngBytes], 'exemplar.png', { type: 'image/png' });
        const jpgFile = new File([pngBytes], 'ignored.jpg', { type: 'image/jpeg' });
        return {
          files: {
            length: 2,
            item: (idx: number) => (idx === 0 ? jpgFile : pngFile),
          },
          items: [],
        } as unknown as DataTransfer;
      },
      expectedNames: ['exemplar.png'],
    },
    {
      id: 'returns-empty-for-null-data-transfer',
      createTransfer: () => null,
      expectedNames: [],
    },
  ])('$id', async ({ createTransfer, expectedNames }) => {
    const pngBytes = new Uint8Array([137, 80, 78, 71, 13, 10, 26, 10]);
    const blob = new Blob([pngBytes], { type: 'image/png' });
    expect(await blobToRawBase64(blob)).toBe('iVBORw0KGgo=');
    expect(extractPngFilesFromDataTransfer(createTransfer(pngBytes)).map((f) => f.name)).toEqual(
      expectedNames,
    );
  });
});
