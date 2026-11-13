import { describe, expect, test } from 'vitest';
import type {
  AtmosphereSummary,
  LifecycleState,
  LibrarySource,
  StyleSummary,
} from '../../types/domain';
import {
  atmosphereToFormDraft,
  buildCreateAtmospherePayload,
  buildCreateStylePayload,
  buildResliceOverrides,
  clampRectToSheet,
  isStyleMutable,
  parseGuardrailsText,
  type StyleFormDraft,
} from './styleAtmosphereHelpers';

function makeStyle(source: LibrarySource, lifecycle: LifecycleState): StyleSummary {
  return {
    slug: 'harbor-ink',
    name: 'Harbor Ink',
    medium: 'pen and ink',
    exemplarCount: 1,
    lifecycle,
    source,
    linework: 'crisp',
    shadingAndTexture: 'flat',
    paletteProfile: 'coastal',
    framingAndBorder: 'clean',
    styleGuardrails: ['crisp contour'],
    exemplarUrls: ['/api/presets/styles/harbor-ink/exemplars/0'],
  };
}

describe('Style, Atmosphere, and ReferencePack helpers', () => {
  test.each<{
    id: string;
    source: LibrarySource;
    lifecycle: LifecycleState;
    hasWorkspace: boolean;
    expected: boolean;
  }>([
    {
      id: 'project-draft-style-is-mutable',
      source: 'project',
      lifecycle: 'draft',
      hasWorkspace: true,
      expected: true,
    },
    {
      id: 'project-validated-style-is-mutable',
      source: 'project',
      lifecycle: 'validated',
      hasWorkspace: true,
      expected: true,
    },
    {
      id: 'project-canonical-style-is-immutable',
      source: 'project',
      lifecycle: 'canonical',
      hasWorkspace: true,
      expected: false,
    },
    {
      id: 'builtin-style-is-immutable',
      source: 'builtin',
      lifecycle: 'canonical',
      hasWorkspace: true,
      expected: false,
    },
    {
      id: 'user-style-is-immutable',
      source: 'user',
      lifecycle: 'draft',
      hasWorkspace: true,
      expected: false,
    },
    {
      id: 'project-style-immutable-without-workspace',
      source: 'project',
      lifecycle: 'draft',
      hasWorkspace: false,
      expected: false,
    },
  ])('$id', ({ source, lifecycle, hasWorkspace, expected }) => {
    expect(isStyleMutable(makeStyle(source, lifecycle), hasWorkspace)).toBe(expected);
  });

  test.each<{
    id: string;
    draft: StyleFormDraft;
    exemplars: readonly string[];
    expectError: boolean;
    expectedClone?: string;
  }>([
    {
      id: 'creates-style-cloned-from-builtin-without-new-exemplars',
      draft: {
        slug: 'harbor-line',
        displayName: 'Harbor Line',
        cloneFromSlug: 'franco-belgian-line',
        medium: 'ink',
        linework: '',
        shadingAndTexture: '',
        paletteProfile: '',
        framingAndBorder: '',
        guardrailsText: 'rule one\n\nrule two',
      },
      exemplars: [],
      expectError: false,
      expectedClone: 'franco-belgian-line',
    },
    {
      id: 'creates-style-from-scratch-with-one-exemplar',
      draft: {
        slug: 'custom-ink',
        displayName: 'Custom Ink',
        cloneFromSlug: '',
        medium: 'ink',
        linework: 'fine',
        shadingAndTexture: 'halftone',
        paletteProfile: 'warm',
        framingAndBorder: 'borderless',
        guardrailsText: 'no smudges',
      },
      exemplars: ['iVBORw0KGgo='],
      expectError: false,
    },
    {
      id: 'rejects-scratch-style-without-exemplars',
      draft: {
        slug: 'custom-ink',
        displayName: 'Custom Ink',
        cloneFromSlug: '',
        medium: 'ink',
        linework: '',
        shadingAndTexture: '',
        paletteProfile: '',
        framingAndBorder: '',
        guardrailsText: '',
      },
      exemplars: [],
      expectError: true,
    },
  ])('$id', ({ draft, exemplars, expectError, expectedClone }) => {
    const res = buildCreateStylePayload(draft, exemplars);
    expect('error' in res).toBe(expectError);
    if (!('error' in res) && expectedClone !== undefined) {
      expect(res.cloneFromSlug).toBe(expectedClone);
      expect(res.styleGuardrails).toEqual(['rule one', 'rule two']);
    }
  });

  test('maps AtmosphereDto to CreateAtmosphereRequest with exact server.rs keys', () => {
    const atmo: AtmosphereSummary = {
      slug: 'golden-dawn',
      name: 'Golden Dawn',
      lightDirection: 'east horizon',
      colorTemperatureKelvin: 4200,
      exposureFloor: 32,
      maxAmbientTintBlend: 0.22,
      particles: 'morning mist',
      source: 'builtin',
      secondaryLightDirection: 'zenith sky',
      minMeanLStar: 32,
      minP10LStar: 12,
    };
    const draft = atmosphereToFormDraft(atmo);
    const payload = buildCreateAtmospherePayload({ ...draft, slug: 'harbor-dawn' });
    expect('error' in payload).toBe(false);
    if (!('error' in payload)) {
      expect(payload).toEqual({
        slug: 'harbor-dawn',
        displayName: 'Golden Dawn',
        keyLightDirection: 'east horizon',
        secondaryLightDirection: 'zenith sky',
        colorTemperatureK: 4200,
        maxAmbientTintBlend: 0.22,
        minMeanLStar: 32,
        minP10LStar: 12,
        weatherAndParticles: 'morning mist',
      });
    }
  });

  test.each<{
    id: string;
    patch: Partial<ReturnType<typeof atmosphereToFormDraft>>;
    expectedErrorPart: string;
  }>([
    {
      id: 'rejects-out-of-range-kelvin',
      patch: { colorTemperatureK: 25000 },
      expectedErrorPart: 'colorTemperatureK',
    },
    {
      id: 'rejects-p10-higher-than-mean-l-star',
      patch: { minMeanLStar: 20, minP10LStar: 35 },
      expectedErrorPart: 'minP10LStar',
    },
  ])('$id', ({ patch, expectedErrorPart }) => {
    const res = buildCreateAtmospherePayload({
      slug: 'test-atmo',
      displayName: 'Test',
      keyLightDirection: 'overhead',
      secondaryLightDirection: 'fill',
      colorTemperatureK: 5500,
      maxAmbientTintBlend: 0.2,
      minMeanLStar: 35,
      minP10LStar: 12,
      weatherAndParticles: 'none',
      ...patch,
    });
    expect('error' in res).toBe(true);
    if ('error' in res) {
      expect(res.error).toContain(expectedErrorPart);
    }
  });

  test('clamps ReferencePack RectOverride bounds cleanly inside sheet dimensions', () => {
    expect(clampRectToSheet({ x: -10, y: 900, width: 2000, height: 300 }, 1536, 1024)).toEqual({
      x: 0,
      y: 900,
      width: 1536,
      height: 124,
    });

    const overrides = buildResliceOverrides(
      [
        { id: 'front', label: 'Front', x: 100, y: 120, width: 200, height: 300 },
        { id: '   ', label: 'Ignored', x: 0, y: 0, width: 50, height: 50 },
      ],
      1536,
      1024,
    );
    expect(overrides).toEqual([
      { id: 'front', rect: { x: 100, y: 120, width: 200, height: 300 }, label: 'Front' },
    ]);
    expect(parseGuardrailsText('  a \n\n b ')).toEqual(['a', 'b']);
  });
});
