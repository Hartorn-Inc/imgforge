import { describe, expect, test } from 'vitest';
import type {
  AtmosphereSummary,
  LifecycleState,
  LibrarySource,
  ReferenceSheetKind,
  SliceRecordDto,
  StyleSummary,
  SubjectSummary,
} from '../../types/domain';
import {
  atmosphereToFormDraft,
  buildCreateAtmospherePayload,
  buildCreateStylePayload,
  buildResliceOverrides,
  clampRectToSheet,
  filterSlicesForSheetRole,
  isReferenceSheetKind,
  isStyleMutable,
  isSubjectPaletteMutable,
  parseGuardrailsText,
  slicesToEditableOverrides,
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

function makeSubject(source: LibrarySource, lifecycle: LifecycleState): SubjectSummary {
  return {
    slug: 'courier-ren',
    name: 'Courier Ren',
    category: 'character',
    heightMeters: 1.74,
    paletteLock: [],
    visibility: 'public',
    lifecycle,
    source,
    asymmetricMarkers: [],
    positiveWeaponGeometry: [],
    tags: [],
  };
}

function makeSlice(id: string): SliceRecordDto {
  return {
    id,
    file: `slices/${id}.png`,
    imageUrl: `/api/assets/slices/${id}.png`,
    rect: { x: 10, y: 20, width: 100, height: 120 },
    source: 'auto',
    label: id,
    sha256: 'abc',
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
    source: LibrarySource;
    lifecycle: LifecycleState;
    hasWorkspace: boolean;
    expected: boolean;
  }>([
    {
      id: 'project-draft-subject-palette-mutable',
      source: 'project',
      lifecycle: 'draft',
      hasWorkspace: true,
      expected: true,
    },
    {
      id: 'project-canonical-subject-palette-immutable',
      source: 'project',
      lifecycle: 'canonical',
      hasWorkspace: true,
      expected: false,
    },
    {
      id: 'project-superseded-subject-palette-immutable',
      source: 'project',
      lifecycle: 'superseded',
      hasWorkspace: true,
      expected: false,
    },
    {
      id: 'user-subject-palette-immutable',
      source: 'user',
      lifecycle: 'draft',
      hasWorkspace: true,
      expected: false,
    },
    {
      id: 'builtin-subject-palette-immutable',
      source: 'builtin',
      lifecycle: 'draft',
      hasWorkspace: true,
      expected: false,
    },
  ])('$id', ({ source, lifecycle, hasWorkspace, expected }) => {
    expect(isSubjectPaletteMutable(makeSubject(source, lifecycle), hasWorkspace)).toBe(expected);
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

  test.each<{
    id: string;
    atmo: AtmosphereSummary;
    slugOverride: string;
    expectedKelvin: number;
    expectedParticles: string;
  }>([
    {
      id: 'maps-builtin-atmosphere-to-create-payload',
      atmo: {
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
      },
      slugOverride: 'harbor-dawn',
      expectedKelvin: 4200,
      expectedParticles: 'morning mist',
    },
    {
      id: 'rounds-fractional-kelvin-in-atmosphere-payload',
      atmo: {
        slug: 'storm-pier',
        name: 'Storm Pier',
        lightDirection: 'overhead cloud break',
        colorTemperatureKelvin: 6499.6,
        exposureFloor: 34,
        maxAmbientTintBlend: 0.18,
        particles: 'sea spray',
        source: 'project',
        secondaryLightDirection: 'wet stone bounce',
        minMeanLStar: 34,
        minP10LStar: 14,
      },
      slugOverride: 'storm-pier-copy',
      expectedKelvin: 6500,
      expectedParticles: 'sea spray',
    },
  ])('$id', ({ atmo, slugOverride, expectedKelvin, expectedParticles }) => {
    const draft = atmosphereToFormDraft(atmo);
    const payload = buildCreateAtmospherePayload({ ...draft, slug: slugOverride });
    expect('error' in payload).toBe(false);
    if (!('error' in payload)) {
      expect(payload).toEqual({
        slug: slugOverride,
        displayName: atmo.name,
        keyLightDirection: atmo.lightDirection,
        secondaryLightDirection: atmo.secondaryLightDirection,
        colorTemperatureK: expectedKelvin,
        maxAmbientTintBlend: atmo.maxAmbientTintBlend,
        minMeanLStar: atmo.minMeanLStar,
        minP10LStar: atmo.minP10LStar,
        weatherAndParticles: expectedParticles,
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

  test.each<{
    id: string;
    role: ReferenceSheetKind;
    expectedIds: readonly string[];
  }>([
    {
      id: 'filters-turnaround-slices-only',
      role: 'turnaround',
      expectedIds: ['turnaround_001', 'turnaround_002'],
    },
    {
      id: 'filters-poses-expressions-slices-only',
      role: 'poses_expressions',
      expectedIds: ['poses_001'],
    },
    {
      id: 'filters-inventory-flatlay-slices-only',
      role: 'inventory_flatlay',
      expectedIds: ['inventory_001'],
    },
    {
      id: 'filters-single-sheet-slices-only',
      role: 'single_sheet',
      expectedIds: ['sheet_001'],
    },
  ])('$id', ({ role, expectedIds }) => {
    const allSlices = [
      makeSlice('turnaround_001'),
      makeSlice('turnaround_002'),
      makeSlice('poses_001'),
      makeSlice('inventory_001'),
      makeSlice('sheet_001'),
    ];
    expect(filterSlicesForSheetRole(allSlices, role).map((s) => s.id)).toEqual(expectedIds);
  });

  test.each<{ id: string; rawRole: string; expectedValid: boolean }>([
    { id: 'accepts-turnaround-kind', rawRole: 'turnaround', expectedValid: true },
    { id: 'accepts-poses-expressions-kind', rawRole: 'poses_expressions', expectedValid: true },
    { id: 'accepts-inventory-flatlay-kind', rawRole: 'inventory_flatlay', expectedValid: true },
    { id: 'accepts-single-sheet-kind', rawRole: 'single_sheet', expectedValid: true },
    { id: 'rejects-unknown-sheet-kind', rawRole: 'contact_sheet', expectedValid: false },
  ])('$id', ({ rawRole, expectedValid }) => {
    expect(isReferenceSheetKind(rawRole)).toBe(expectedValid);
  });

  test.each<{
    id: string;
    rect: { x: number; y: number; width: number; height: number };
    expectedClamped: { x: number; y: number; width: number; height: number };
    guardrailsInput: string;
    expectedGuardrails: readonly string[];
  }>([
    {
      id: 'clamps-overflowing-rect-and-filters-blank-override-ids',
      rect: { x: -10, y: 900, width: 2000, height: 300 },
      expectedClamped: { x: 0, y: 900, width: 1536, height: 124 },
      guardrailsInput: '  a \n\n b ',
      expectedGuardrails: ['a', 'b'],
    },
    {
      id: 'clamps-zero-sized-rect-to-minimum-1px',
      rect: { x: 1600, y: 1200, width: 0, height: -5 },
      expectedClamped: { x: 1535, y: 1023, width: 1, height: 1 },
      guardrailsInput: '\r\n  single rule  \r\n',
      expectedGuardrails: ['single rule'],
    },
  ])('$id', ({ rect, expectedClamped, guardrailsInput, expectedGuardrails }) => {
    expect(clampRectToSheet(rect, 1536, 1024)).toEqual(expectedClamped);

    const editable = slicesToEditableOverrides([makeSlice('turnaround_001')]);
    expect(editable[0]?.rowKey).toMatch(/^slice-row-\d+$/);

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
    expect(parseGuardrailsText(guardrailsInput)).toEqual(expectedGuardrails);
  });
});
