/** === STYLE, ATMOSPHERE & REFERENCEPACK HELPERS ===
 *
 * PURPOSE:
 * Provides pure, unit-testable form state builders, validation guards,
 * `Style` mutability checks, and `ReferencePack` `RectOverride` normalization
 * for `StyleAtmosphereView.tsx`.
 */

import type {
  CreateAtmospherePayload,
  CreateStylePayload,
  UpdateAtmospherePayload,
  UpdateStylePayload,
} from '../../api/client';
import type {
  AtmosphereSummary,
  OverrideMode,
  PixelRect,
  RectOverride,
  ReferencePackSummary,
  SliceRecordDto,
  StyleSummary,
} from '../../types/domain';

export interface StyleFormDraft {
  readonly slug: string;
  readonly displayName: string;
  readonly cloneFromSlug: string;
  readonly medium: string;
  readonly linework: string;
  readonly shadingAndTexture: string;
  readonly paletteProfile: string;
  readonly framingAndBorder: string;
  readonly guardrailsText: string;
}

export interface AtmosphereFormDraft {
  readonly slug: string;
  readonly displayName: string;
  readonly keyLightDirection: string;
  readonly secondaryLightDirection: string;
  readonly colorTemperatureK: number;
  readonly maxAmbientTintBlend: number;
  readonly minMeanLStar: number;
  readonly minP10LStar: number;
  readonly weatherAndParticles: string;
}

export interface EditableSliceOverride {
  readonly id: string;
  readonly label: string;
  readonly x: number;
  readonly y: number;
  readonly width: number;
  readonly height: number;
}

/** Return true only when a `Style` belongs to `project` and is not `canonical`/`superseded`. */
export function isStyleMutable(style: StyleSummary, hasWorkspace: boolean): boolean {
  if (!hasWorkspace || style.source !== 'project') {
    return false;
  }
  return style.lifecycle !== 'canonical' && style.lifecycle !== 'superseded';
}

/** Build an editable `StyleFormDraft` from an existing `StyleSummary` (or empty clone). */
export function styleToFormDraft(style: StyleSummary, cloneSlug = ''): StyleFormDraft {
  return {
    slug: cloneSlug.length > 0 ? cloneSlug : style.slug,
    displayName: cloneSlug.length > 0 ? `${style.name} Copy` : style.name,
    cloneFromSlug: cloneSlug.length > 0 ? style.slug : '',
    medium: style.medium,
    linework: style.linework,
    shadingAndTexture: style.shadingAndTexture,
    paletteProfile: style.paletteProfile,
    framingAndBorder: style.framingAndBorder,
    guardrailsText: style.styleGuardrails.join('\n'),
  };
}

/** Parse newline-separated guardrails into non-empty trimmed lines. */
export function parseGuardrailsText(text: string): readonly string[] {
  return text
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter((line) => line.length > 0);
}

/** Build exact `POST /api/styles` request payload. */
export function buildCreateStylePayload(
  draft: StyleFormDraft,
  exemplarsBase64: readonly string[],
): CreateStylePayload | { readonly error: string } {
  const slug = draft.slug.trim();
  const displayName = draft.displayName.trim();
  if (slug.length === 0 || displayName.length === 0) {
    return { error: 'Style slug and displayName are required.' };
  }
  const cloneFromSlug = draft.cloneFromSlug.trim();
  if (cloneFromSlug.length === 0 && (exemplarsBase64.length < 1 || exemplarsBase64.length > 3)) {
    return { error: 'Provide 1..=3 PNG exemplars or select a Style to clone.' };
  }

  const guardrails = parseGuardrailsText(draft.guardrailsText);
  const payload: {
    slug: string;
    displayName: string;
    medium?: string;
    linework?: string;
    shadingAndTexture?: string;
    paletteProfile?: string;
    framingAndBorder?: string;
    styleGuardrails?: readonly string[];
    cloneFromSlug?: string;
    exemplarsBase64?: readonly string[];
  } = {
    slug,
    displayName,
  };

  if (cloneFromSlug.length > 0) {
    payload.cloneFromSlug = cloneFromSlug;
  }
  if (exemplarsBase64.length > 0) {
    payload.exemplarsBase64 = exemplarsBase64;
  }
  if (draft.medium.trim().length > 0) {
    payload.medium = draft.medium.trim();
  }
  if (draft.linework.trim().length > 0) {
    payload.linework = draft.linework.trim();
  }
  if (draft.shadingAndTexture.trim().length > 0) {
    payload.shadingAndTexture = draft.shadingAndTexture.trim();
  }
  if (draft.paletteProfile.trim().length > 0) {
    payload.paletteProfile = draft.paletteProfile.trim();
  }
  if (draft.framingAndBorder.trim().length > 0) {
    payload.framingAndBorder = draft.framingAndBorder.trim();
  }
  if (guardrails.length > 0) {
    payload.styleGuardrails = guardrails;
  }

  return payload;
}

/** Build exact `PUT /api/styles/{slug}` request payload. */
export function buildUpdateStylePayload(draft: StyleFormDraft): UpdateStylePayload {
  return {
    displayName: draft.displayName.trim(),
    medium: draft.medium.trim(),
    linework: draft.linework.trim(),
    shadingAndTexture: draft.shadingAndTexture.trim(),
    paletteProfile: draft.paletteProfile.trim(),
    framingAndBorder: draft.framingAndBorder.trim(),
    styleGuardrails: parseGuardrailsText(draft.guardrailsText),
  };
}

/** Map `AtmosphereDto` fields to `CreateAtmosphereRequest` / `UpdateAtmosphereRequest` draft. */
export function atmosphereToFormDraft(atmo: AtmosphereSummary): AtmosphereFormDraft {
  return {
    slug: atmo.slug,
    displayName: atmo.name,
    keyLightDirection: atmo.lightDirection,
    secondaryLightDirection: atmo.secondaryLightDirection,
    colorTemperatureK: atmo.colorTemperatureKelvin,
    maxAmbientTintBlend: atmo.maxAmbientTintBlend,
    minMeanLStar: atmo.minMeanLStar,
    minP10LStar: atmo.minP10LStar,
    weatherAndParticles: atmo.particles,
  };
}

/** Build `CreateAtmosphereRequest` with exact `server.rs` field names and validation. */
export function buildCreateAtmospherePayload(
  draft: AtmosphereFormDraft,
): CreateAtmospherePayload | { readonly error: string } {
  const slug = draft.slug.trim();
  const displayName = draft.displayName.trim();
  if (slug.length === 0 || displayName.length === 0) {
    return { error: 'Atmosphere slug and displayName are required.' };
  }
  if (
    !Number.isFinite(draft.colorTemperatureK) ||
    draft.colorTemperatureK < 1000 ||
    draft.colorTemperatureK > 20000
  ) {
    return { error: 'colorTemperatureK must be within 1000..=20000 K.' };
  }
  if (
    !Number.isFinite(draft.maxAmbientTintBlend) ||
    draft.maxAmbientTintBlend < 0 ||
    draft.maxAmbientTintBlend > 1
  ) {
    return { error: 'maxAmbientTintBlend must be within 0.0..=1.0.' };
  }
  if (draft.minP10LStar > draft.minMeanLStar) {
    return { error: 'minP10LStar cannot exceed minMeanLStar.' };
  }
  return {
    slug,
    displayName,
    keyLightDirection: draft.keyLightDirection.trim(),
    secondaryLightDirection: draft.secondaryLightDirection.trim(),
    colorTemperatureK: Math.round(draft.colorTemperatureK),
    maxAmbientTintBlend: draft.maxAmbientTintBlend,
    minMeanLStar: draft.minMeanLStar,
    minP10LStar: draft.minP10LStar,
    weatherAndParticles: draft.weatherAndParticles.trim(),
  };
}

/** Build `UpdateAtmosphereRequest` with exact `server.rs` field names. */
export function buildUpdateAtmospherePayload(draft: AtmosphereFormDraft): UpdateAtmospherePayload {
  return {
    displayName: draft.displayName.trim(),
    keyLightDirection: draft.keyLightDirection.trim(),
    secondaryLightDirection: draft.secondaryLightDirection.trim(),
    colorTemperatureK: Math.round(draft.colorTemperatureK),
    maxAmbientTintBlend: draft.maxAmbientTintBlend,
    minMeanLStar: draft.minMeanLStar,
    minP10LStar: draft.minP10LStar,
    weatherAndParticles: draft.weatherAndParticles.trim(),
  };
}

/** Convert existing `SliceRecordDto`s into editable `RectOverride` drafts. */
export function slicesToEditableOverrides(
  slices: readonly SliceRecordDto[],
): readonly EditableSliceOverride[] {
  return slices.map((sl) => ({
    id: sl.id,
    label: sl.label ?? '',
    x: sl.rect.x,
    y: sl.rect.y,
    width: sl.rect.width,
    height: sl.rect.height,
  }));
}

/** Clamp a `RectOverride` pixel rectangle inside sheet dimensions (`width > 0`, `height > 0`). */
export function clampRectToSheet(
  rect: PixelRect,
  sheetWidth: number,
  sheetHeight: number,
): PixelRect {
  const maxW = Math.max(1, Math.round(sheetWidth));
  const maxH = Math.max(1, Math.round(sheetHeight));
  const x = Math.max(0, Math.min(maxW - 1, Math.round(rect.x)));
  const y = Math.max(0, Math.min(maxH - 1, Math.round(rect.y)));
  const width = Math.max(1, Math.min(maxW - x, Math.round(rect.width)));
  const height = Math.max(1, Math.min(maxH - y, Math.round(rect.height)));
  return { x, y, width, height };
}

/** Build `RectOverride[]` payload for `POST /api/packs/{subject}/{style}/reslice`. */
export function buildResliceOverrides(
  items: readonly EditableSliceOverride[],
  sheetWidth: number,
  sheetHeight: number,
): readonly RectOverride[] {
  return items
    .filter((item) => item.id.trim().length > 0)
    .map((item) => {
      const trimmedLabel = item.label.trim();
      const rect = clampRectToSheet(
        { x: item.x, y: item.y, width: item.width, height: item.height },
        sheetWidth,
        sheetHeight,
      );
      return trimmedLabel.length > 0
        ? { id: item.id.trim(), rect, label: trimmedLabel }
        : { id: item.id.trim(), rect, label: null };
    });
}

/** Check whether a `ReferencePack` can be mutated (resliced/validated/promoted). */
export function isPackMutable(pack: ReferencePackSummary): boolean {
  return pack.lifecycle !== 'canonical' && pack.lifecycle !== 'superseded';
}

export function isOverrideMode(value: string): value is OverrideMode {
  return value === 'replace_all' || value === 'merge_by_id';
}
