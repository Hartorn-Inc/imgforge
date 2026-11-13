/** === STYLE & ATMOSPHERE MANAGER VIEW ===
 *
 * PURPOSE:
 * Full interactive workspace manager for `Style` presets & `1..=3` PNG
 * `exemplars` (drag-drop, file picker, `Ctrl+V` paste), `Atmosphere` lighting
 * profiles, `Subject` `PaletteLock` swatches, and style-scoped `ReferencePack`
 * sheet inspection + manual `RectOverride` reslicing.
 *
 * DESIGN RATIONALE ("WHY"):
 * Enforces ADR 0003 separation (`Subject` identity stays style-neutral while
 * `ReferencePack`s are scoped per `(Subject, Style)`), respects `Canonical`
 * immutability (disabling mutations and offering Clone), and maps directly to
 * `crates/imgforge-cli/src/server.rs` endpoints.
 */

import type { JSX } from 'preact';
import { useEffect, useRef, useState } from 'preact/hooks';
import {
  blobToRawBase64,
  createAtmosphere,
  createStyle,
  extractPngFilesFromDataTransfer,
  promoteReferencePack,
  promoteStyle,
  resliceReferencePack,
  updateAtmosphere,
  updateStyle,
  updateSubjectPaletteLock,
  uploadStyleExemplars,
  validateReferencePack,
} from '../../api/client';
import { nextLifecycleStep } from '../../state/workspace';
import type {
  AsyncViewState,
  CriticReport,
  OverrideMode,
  ReferenceSheetKind,
  StyleSummary,
  WorkspaceSummary,
} from '../../types/domain';
import { StatePanel } from '../StatePanel';
import {
  type AtmosphereFormDraft,
  type EditableSliceOverride,
  type StyleFormDraft,
  atmosphereToFormDraft,
  buildCreateAtmospherePayload,
  buildCreateStylePayload,
  buildResliceOverrides,
  buildUpdateAtmospherePayload,
  buildUpdateStylePayload,
  createSliceRowKey,
  filterSlicesForSheetRole,
  isOverrideMode,
  isPackMutable,
  isReferenceSheetKind,
  isStyleMutable,
  isSubjectPaletteMutable,
  slicesToEditableOverrides,
  styleToFormDraft,
} from './styleAtmosphereHelpers';

export interface StyleAtmosphereViewProps {
  readonly workspaceState: AsyncViewState<WorkspaceSummary>;
  readonly onRefreshWorkspace: () => Promise<void>;
}

const EMPTY_STYLE_DRAFT: StyleFormDraft = {
  slug: '',
  displayName: '',
  cloneFromSlug: '',
  medium: '',
  linework: '',
  shadingAndTexture: '',
  paletteProfile: '',
  framingAndBorder: '',
  guardrailsText: '',
};

const EMPTY_ATMO_DRAFT: AtmosphereFormDraft = {
  slug: '',
  displayName: '',
  keyLightDirection: 'Top-left 45deg key',
  secondaryLightDirection: 'Soft cool sky fill',
  colorTemperatureK: 5600,
  maxAmbientTintBlend: 0.2,
  minMeanLStar: 34,
  minP10LStar: 12,
  weatherAndParticles: 'Clear air',
};

/** `Style & Atmosphere` tab view. */
export function StyleAtmosphereView({
  workspaceState,
  onRefreshWorkspace,
}: StyleAtmosphereViewProps): JSX.Element {
  const [selectedStyleSlug, setSelectedStyleSlug] = useState<string>('');
  const [styleMode, setStyleMode] = useState<'edit' | 'create'>('edit');
  const [styleDraft, setStyleDraft] = useState<StyleFormDraft>(EMPTY_STYLE_DRAFT);
  const [pendingExemplarsB64, setPendingExemplarsB64] = useState<readonly string[]>([]);
  const [replaceExemplars, setReplaceExemplars] = useState<boolean>(true);
  const [isDragOver, setIsDragOver] = useState<boolean>(false);

  const [selectedAtmoSlug, setSelectedAtmoSlug] = useState<string>('');
  const [atmoMode, setAtmoMode] = useState<'edit' | 'create'>('edit');
  const [atmoDraft, setAtmoDraft] = useState<AtmosphereFormDraft>(EMPTY_ATMO_DRAFT);

  const [selectedSubjectSlug, setSelectedSubjectSlug] = useState<string>('');
  const [swatchName, setSwatchName] = useState<string>('');
  const [swatchHex, setSwatchHex] = useState<string>('#2C3E50');
  const [swatchSlot, setSwatchSlot] = useState<string>('');

  const [selectedPackKey, setSelectedPackKey] = useState<string>('');
  const [selectedSheetRole, setSelectedSheetRole] = useState<ReferenceSheetKind>('turnaround');
  const [overrideMode, setOverrideMode] = useState<OverrideMode>('replace_all');
  const [sliceOverrides, setSliceOverrides] = useState<readonly EditableSliceOverride[]>([]);
  const [selectedSliceId, setSelectedSliceId] = useState<string | null>(null);
  const [runPackCritic, setRunPackCritic] = useState<boolean>(false);
  const [packCriticReport, setPackCriticReport] = useState<CriticReport | null>(null);
  const [packChecks, setPackChecks] = useState<readonly string[]>([]);

  const [busyMessage, setBusyMessage] = useState<string | null>(null);
  const [feedback, setFeedback] = useState<{
    readonly kind: 'ok' | 'error';
    readonly text: string;
  } | null>(null);

  const workspace = workspaceState.status === 'ready' ? workspaceState.data : null;

  const selectedStyleSlugRef = useRef(selectedStyleSlug);
  selectedStyleSlugRef.current = selectedStyleSlug;
  const styleModeRef = useRef(styleMode);
  styleModeRef.current = styleMode;
  const selectedAtmoSlugRef = useRef(selectedAtmoSlug);
  selectedAtmoSlugRef.current = selectedAtmoSlug;
  const atmoModeRef = useRef(atmoMode);
  atmoModeRef.current = atmoMode;
  const selectedSubjectSlugRef = useRef(selectedSubjectSlug);
  selectedSubjectSlugRef.current = selectedSubjectSlug;
  const selectedPackKeyRef = useRef(selectedPackKey);
  selectedPackKeyRef.current = selectedPackKey;

  useEffect(() => {
    if (workspace === null) {
      return;
    }
    const currentStyleSlug = selectedStyleSlugRef.current;
    const firstStyle = workspace.styles[0];
    if (
      firstStyle !== undefined &&
      styleModeRef.current !== 'create' &&
      !workspace.styles.some((st) => st.slug === currentStyleSlug)
    ) {
      setSelectedStyleSlug(firstStyle.slug);
      setStyleDraft(styleToFormDraft(firstStyle));
      setStyleMode('edit');
    }

    const currentAtmoSlug = selectedAtmoSlugRef.current;
    const firstAtmo = workspace.atmospheres[0];
    if (
      firstAtmo !== undefined &&
      atmoModeRef.current !== 'create' &&
      !workspace.atmospheres.some((a) => a.slug === currentAtmoSlug)
    ) {
      setSelectedAtmoSlug(firstAtmo.slug);
      setAtmoDraft(atmosphereToFormDraft(firstAtmo));
      setAtmoMode('edit');
    }

    const currentSubjectSlug = selectedSubjectSlugRef.current;
    const firstSubj = workspace.subjects[0];
    if (firstSubj !== undefined && !workspace.subjects.some((s) => s.slug === currentSubjectSlug)) {
      setSelectedSubjectSlug(firstSubj.slug);
    }

    const currentPackKey = selectedPackKeyRef.current;
    const firstPack = workspace.referencePacks[0];
    if (
      firstPack !== undefined &&
      !workspace.referencePacks.some((p) => `${p.subjectSlug}/${p.styleSlug}` === currentPackKey)
    ) {
      const nextRole = firstPack.sheets[0]?.role ?? firstPack.sheetKind;
      setSelectedPackKey(`${firstPack.subjectSlug}/${firstPack.styleSlug}`);
      setSelectedSheetRole(nextRole);
      setSliceOverrides(
        slicesToEditableOverrides(filterSlicesForSheetRole(firstPack.slices, nextRole)),
      );
    }
  }, [workspace]);

  if (workspaceState.status !== 'ready') {
    return (
      <div class="workspace-panel">
        <div class="panel-toolbar">
          <div>
            <h2 class="panel-title">Style &amp; Atmosphere</h2>
            <p class="panel-subtitle">
              Manage <code>Style</code> presets, <code>Atmosphere</code> cards, <code>Subject</code>{' '}
              <code>PaletteLock</code> swatches, and <code>ReferencePack</code> slicing.
            </p>
          </div>
        </div>
        <StatePanel
          state={workspaceState}
          onRetry={() => {
            void onRefreshWorkspace();
          }}
        />
      </div>
    );
  }

  const ws = workspaceState.data;
  const activeStyle: StyleSummary | null =
    ws.styles.find((st) => st.slug === selectedStyleSlug) ?? null;
  const activeAtmo = ws.atmospheres.find((a) => a.slug === selectedAtmoSlug) ?? null;
  const activeSubject = ws.subjects.find((s) => s.slug === selectedSubjectSlug) ?? null;
  const activePack =
    ws.referencePacks.find((p) => `${p.subjectSlug}/${p.styleSlug}` === selectedPackKey) ?? null;

  const styleEditable =
    styleMode === 'create'
      ? ws.hasWorkspace
      : activeStyle !== null && isStyleMutable(activeStyle, ws.hasWorkspace);
  const atmoEditable =
    atmoMode === 'create'
      ? ws.hasWorkspace
      : ws.hasWorkspace && activeAtmo !== null && activeAtmo.source === 'project';

  const ingestPngFiles = async (files: readonly File[]): Promise<void> => {
    if (files.length === 0) {
      setFeedback({ kind: 'error', text: 'Only .png files are supported for Style exemplars.' });
      return;
    }
    const limited = files.slice(0, 3);
    const encoded: string[] = [];
    try {
      for (const file of limited) {
        encoded.push(await blobToRawBase64(file));
      }
    } catch (err) {
      setFeedback({
        kind: 'error',
        text: err instanceof Error ? err.message : 'Failed to decode PNG exemplar file.',
      });
      return;
    }
    setPendingExemplarsB64((prev) => [...prev, ...encoded].slice(0, 3));
    setFeedback({
      kind: 'ok',
      text: `Staged ${String(encoded.length)} PNG exemplar(s) ready to upload.`,
    });
  };

  const handleSaveStyle = async (): Promise<void> => {
    setFeedback(null);
    if (styleMode === 'create') {
      const payload = buildCreateStylePayload(styleDraft, pendingExemplarsB64);
      if ('error' in payload) {
        setFeedback({ kind: 'error', text: payload.error });
        return;
      }
      setBusyMessage('Creating Style...');
      const res = await createStyle(payload);
      setBusyMessage(null);
      if (!res.ok) {
        setFeedback({ kind: 'error', text: res.error.message });
        return;
      }
      setSelectedStyleSlug(res.data.slug);
      setStyleDraft(styleToFormDraft(res.data));
      setStyleMode('edit');
      setPendingExemplarsB64([]);
      setFeedback({ kind: 'ok', text: `Created Style '${res.data.slug}'.` });
      await onRefreshWorkspace();
      return;
    }

    if (activeStyle === null) {
      return;
    }
    setBusyMessage(`Saving Style '${activeStyle.slug}'...`);
    const res = await updateStyle(activeStyle.slug, buildUpdateStylePayload(styleDraft));
    setBusyMessage(null);
    if (!res.ok) {
      setFeedback({ kind: 'error', text: res.error.message });
      return;
    }
    setStyleDraft(styleToFormDraft(res.data));
    setFeedback({ kind: 'ok', text: `Updated Style '${res.data.slug}'.` });
    await onRefreshWorkspace();
  };

  const handleUploadExemplars = async (): Promise<void> => {
    if (activeStyle === null || pendingExemplarsB64.length === 0) {
      return;
    }
    setBusyMessage(`Uploading exemplars to '${activeStyle.slug}'...`);
    setFeedback(null);
    const res = await uploadStyleExemplars(activeStyle.slug, {
      exemplarsBase64: pendingExemplarsB64,
      replace: replaceExemplars,
    });
    setBusyMessage(null);
    if (!res.ok) {
      setFeedback({ kind: 'error', text: res.error.message });
      return;
    }
    setPendingExemplarsB64([]);
    setFeedback({
      kind: 'ok',
      text: `Uploaded exemplars (${String(res.data.exemplarCount)}/3) for '${res.data.slug}'.`,
    });
    await onRefreshWorkspace();
  };

  const handlePromoteStyle = async (): Promise<void> => {
    if (activeStyle === null) {
      return;
    }
    setBusyMessage(`Promoting Style '${activeStyle.slug}'...`);
    setFeedback(null);
    const res = await promoteStyle(activeStyle.slug);
    setBusyMessage(null);
    if (!res.ok) {
      setFeedback({ kind: 'error', text: res.error.message });
      return;
    }
    setFeedback({
      kind: 'ok',
      text: `Style '${res.data.style.slug}' promoted to ${res.data.style.lifecycle}.`,
    });
    await onRefreshWorkspace();
  };

  const handleSaveAtmosphere = async (): Promise<void> => {
    setFeedback(null);
    if (atmoMode === 'create') {
      const payload = buildCreateAtmospherePayload(atmoDraft);
      if ('error' in payload) {
        setFeedback({ kind: 'error', text: payload.error });
        return;
      }
      setBusyMessage(`Creating Atmosphere '${payload.slug}'...`);
      const res = await createAtmosphere(payload);
      setBusyMessage(null);
      if (!res.ok) {
        setFeedback({ kind: 'error', text: res.error.message });
        return;
      }
      setSelectedAtmoSlug(res.data.slug);
      setAtmoDraft(atmosphereToFormDraft(res.data));
      setAtmoMode('edit');
      setFeedback({ kind: 'ok', text: `Created Atmosphere '${res.data.slug}'.` });
      await onRefreshWorkspace();
      return;
    }

    if (activeAtmo === null) {
      return;
    }
    setBusyMessage(`Updating Atmosphere '${activeAtmo.slug}'...`);
    const res = await updateAtmosphere(activeAtmo.slug, buildUpdateAtmospherePayload(atmoDraft));
    setBusyMessage(null);
    if (!res.ok) {
      setFeedback({ kind: 'error', text: res.error.message });
      return;
    }
    setAtmoDraft(atmosphereToFormDraft(res.data));
    setFeedback({ kind: 'ok', text: `Updated Atmosphere '${res.data.slug}'.` });
    await onRefreshWorkspace();
  };

  const handleAddPaletteSwatch = async (): Promise<void> => {
    if (activeSubject === null) {
      return;
    }
    if (swatchName.trim().length === 0) {
      setFeedback({ kind: 'error', text: 'Swatch name is required.' });
      return;
    }
    setBusyMessage(`Updating PaletteLock on '${activeSubject.slug}'...`);
    setFeedback(null);
    const payload =
      swatchSlot.trim().length > 0
        ? { name: swatchName.trim(), colorHex: swatchHex.trim(), surfaceSlot: swatchSlot.trim() }
        : { name: swatchName.trim(), colorHex: swatchHex.trim() };
    const res = await updateSubjectPaletteLock(activeSubject.slug, payload);
    setBusyMessage(null);
    if (!res.ok) {
      setFeedback({ kind: 'error', text: res.error.message });
      return;
    }
    setSwatchName('');
    setSwatchSlot('');
    setFeedback({ kind: 'ok', text: `Updated PaletteLock for '${res.data.slug}'.` });
    await onRefreshWorkspace();
  };

  const activeSheet =
    activePack?.sheets.find((sh) => sh.role === selectedSheetRole) ?? activePack?.sheets[0] ?? null;

  const handleReslicePack = async (): Promise<void> => {
    if (activePack === null) {
      return;
    }
    const sheetW = activeSheet?.width ?? 1536;
    const sheetH = activeSheet?.height ?? 1024;
    const overrides = buildResliceOverrides(sliceOverrides, sheetW, sheetH);
    setBusyMessage(`Reslicing ${activePack.subjectSlug}/${activePack.styleSlug}...`);
    setFeedback(null);
    const res = await resliceReferencePack(activePack.subjectSlug, activePack.styleSlug, {
      sheetRole: selectedSheetRole,
      mode: overrideMode,
      overrides,
    });
    setBusyMessage(null);
    if (!res.ok) {
      setFeedback({ kind: 'error', text: res.error.message });
      return;
    }
    const scopedSlices = filterSlicesForSheetRole(res.data.slices, selectedSheetRole);
    setSliceOverrides(slicesToEditableOverrides(scopedSlices));
    setFeedback({
      kind: 'ok',
      text: `Resliced ${res.data.sheetFilename}: ${String(scopedSlices.length)} slice(s).`,
    });
    await onRefreshWorkspace();
  };

  const handleValidatePack = async (): Promise<void> => {
    if (activePack === null) {
      return;
    }
    setBusyMessage(`Validating ReferencePack ${activePack.subjectSlug}/${activePack.styleSlug}...`);
    setFeedback(null);
    const res = await validateReferencePack(activePack.subjectSlug, activePack.styleSlug, {
      runCritic: runPackCritic,
    });
    setBusyMessage(null);
    if (!res.ok) {
      setFeedback({ kind: 'error', text: res.error.message });
      return;
    }
    setPackChecks(res.data.checks);
    setPackCriticReport(res.data.criticReport);
    setFeedback({
      kind: 'ok',
      text: `ReferencePack transitioned to ${res.data.referencePack.lifecycle}.`,
    });
    await onRefreshWorkspace();
  };

  const handlePromotePack = async (): Promise<void> => {
    if (activePack === null) {
      return;
    }
    setBusyMessage(`Promoting ReferencePack ${activePack.subjectSlug}/${activePack.styleSlug}...`);
    setFeedback(null);
    const res = await promoteReferencePack(activePack.subjectSlug, activePack.styleSlug);
    setBusyMessage(null);
    if (!res.ok) {
      setFeedback({ kind: 'error', text: res.error.message });
      return;
    }
    setFeedback({
      kind: 'ok',
      text: `Promoted ReferencePack to canonical (lock entries: ${String(res.data.canonicalCount)}).`,
    });
    await onRefreshWorkspace();
  };

  const subjectPaletteMutable =
    activeSubject !== null && isSubjectPaletteMutable(activeSubject, ws.hasWorkspace);

  return (
    <div class="workspace-panel">
      <div class="panel-toolbar">
        <div>
          <h2 class="panel-title">Style &amp; Atmosphere Manager</h2>
          <p class="panel-subtitle">
            Browse presets, edit project <code>Style</code> &amp; <code>Atmosphere</code> cards,
            attach <code>1..=3</code> PNG <code>exemplars</code>, manage <code>Subject</code>{' '}
            <code>PaletteLock</code> swatches, and override <code>ReferencePack</code> slice bounds.
          </p>
        </div>
        <button
          type="button"
          class="btn btn--secondary"
          onClick={() => {
            void onRefreshWorkspace();
          }}
        >
          Refresh Workspace
        </button>
      </div>

      {!ws.hasWorkspace && (
        <div class="notice-banner notice-banner--warn" role="status">
          No workspace is open (launched outside an <code>imgforge.toml</code> directory). Built-in{' '}
          <code>Style</code> and <code>Atmosphere</code> presets are shown read-only; launch{' '}
          <code>imgforge ui --workspace &lt;PATH&gt;</code> to create or mutate project assets.
        </div>
      )}

      {busyMessage !== null && (
        <div class="notice-banner" role="status">
          {busyMessage}
        </div>
      )}

      {feedback !== null && (
        <div
          class={`notice-banner ${feedback.kind === 'error' ? 'notice-banner--error' : 'notice-banner--ok'}`}
          role={feedback.kind === 'error' ? 'alert' : 'status'}
        >
          {feedback.text}
        </div>
      )}

      {/* === 1. Style Gallery & Editor === */}
      <section class="card" aria-labelledby="style-section-heading">
        <header class="card__header">
          <h3 id="style-section-heading">Style Presets &amp; Project Cards ({ws.styles.length})</h3>
          <div class="button-row button-row--tight">
            <button
              type="button"
              class="btn btn--secondary"
              disabled={!ws.hasWorkspace}
              onClick={() => {
                setStyleMode('create');
                setStyleDraft({
                  ...EMPTY_STYLE_DRAFT,
                  slug: 'custom-style',
                  displayName: 'Custom Style',
                });
                setPendingExemplarsB64([]);
              }}
            >
              New Style
            </button>
            {activeStyle !== null && (
              <button
                type="button"
                class="btn btn--secondary"
                disabled={!ws.hasWorkspace}
                onClick={() => {
                  setStyleMode('create');
                  setStyleDraft(styleToFormDraft(activeStyle, `${activeStyle.slug}-copy`));
                  setPendingExemplarsB64([]);
                }}
              >
                Clone Selected
              </button>
            )}
          </div>
        </header>

        <div class="preset-gallery" role="list" aria-label="Style presets">
          {ws.styles.map((st) => {
            const isSelected = styleMode === 'edit' && st.slug === selectedStyleSlug;
            return (
              <div key={st.slug} role="listitem">
                <button
                  type="button"
                  aria-pressed={isSelected}
                  class={`preset-tile${isSelected ? ' preset-tile--active' : ''}`}
                  onClick={() => {
                    setStyleMode('edit');
                    setSelectedStyleSlug(st.slug);
                    setStyleDraft(styleToFormDraft(st));
                  }}
                >
                  <div class="preset-tile__header">
                    <strong>{st.name}</strong>
                    <span class="badge">{st.source}</span>
                  </div>
                  <div class="preset-tile__meta">
                    <span>{st.slug}</span>
                    <span class="badge badge--ready">{st.lifecycle}</span>
                  </div>
                  <div class="thumb-row">
                    {st.exemplarUrls.slice(0, 3).map((url, idx) => (
                      <img
                        key={url}
                        src={url}
                        alt={`${st.name} exemplar ${String(idx + 1)}`}
                        class="exemplar-thumb"
                        loading="lazy"
                      />
                    ))}
                  </div>
                </button>
              </div>
            );
          })}
        </div>

        <div class="split-two">
          <div class="form-grid">
            <div class="form-row-2">
              <div class="control-group">
                <label class="field-label" for="style-slug-input">
                  Style Slug
                </label>
                <input
                  id="style-slug-input"
                  class="text-input"
                  value={styleDraft.slug}
                  disabled={styleMode === 'edit' || !ws.hasWorkspace}
                  onInput={(e) => {
                    setStyleDraft((prev) => ({ ...prev, slug: e.currentTarget.value }));
                  }}
                />
              </div>
              <div class="control-group">
                <label class="field-label" for="style-name-input">
                  Display Name
                </label>
                <input
                  id="style-name-input"
                  class="text-input"
                  value={styleDraft.displayName}
                  disabled={!styleEditable}
                  onInput={(e) => {
                    setStyleDraft((prev) => ({ ...prev, displayName: e.currentTarget.value }));
                  }}
                />
              </div>
            </div>

            {styleMode === 'create' && (
              <div class="control-group">
                <label class="field-label" for="style-clone-from">
                  Clone From Slug (optional)
                </label>
                <input
                  id="style-clone-from"
                  class="text-input"
                  value={styleDraft.cloneFromSlug}
                  placeholder="franco-belgian-line"
                  onInput={(e) => {
                    setStyleDraft((prev) => ({ ...prev, cloneFromSlug: e.currentTarget.value }));
                  }}
                />
              </div>
            )}

            <div class="control-group">
              <label class="field-label" for="style-medium-input">
                Medium
              </label>
              <input
                id="style-medium-input"
                class="text-input"
                value={styleDraft.medium}
                disabled={!styleEditable}
                onInput={(e) => {
                  setStyleDraft((prev) => ({ ...prev, medium: e.currentTarget.value }));
                }}
              />
            </div>

            <div class="control-group">
              <label class="field-label" for="style-linework-input">
                Linework
              </label>
              <input
                id="style-linework-input"
                class="text-input"
                value={styleDraft.linework}
                disabled={!styleEditable}
                onInput={(e) => {
                  setStyleDraft((prev) => ({ ...prev, linework: e.currentTarget.value }));
                }}
              />
            </div>

            <div class="control-group">
              <label class="field-label" for="style-shading-input">
                Shading &amp; Texture
              </label>
              <input
                id="style-shading-input"
                class="text-input"
                value={styleDraft.shadingAndTexture}
                disabled={!styleEditable}
                onInput={(e) => {
                  setStyleDraft((prev) => ({
                    ...prev,
                    shadingAndTexture: e.currentTarget.value,
                  }));
                }}
              />
            </div>

            <div class="form-row-2">
              <div class="control-group">
                <label class="field-label" for="style-palette-input">
                  Palette Profile
                </label>
                <input
                  id="style-palette-input"
                  class="text-input"
                  value={styleDraft.paletteProfile}
                  disabled={!styleEditable}
                  onInput={(e) => {
                    setStyleDraft((prev) => ({ ...prev, paletteProfile: e.currentTarget.value }));
                  }}
                />
              </div>
              <div class="control-group">
                <label class="field-label" for="style-framing-input">
                  Framing &amp; Border
                </label>
                <input
                  id="style-framing-input"
                  class="text-input"
                  value={styleDraft.framingAndBorder}
                  disabled={!styleEditable}
                  onInput={(e) => {
                    setStyleDraft((prev) => ({
                      ...prev,
                      framingAndBorder: e.currentTarget.value,
                    }));
                  }}
                />
              </div>
            </div>

            <div class="control-group">
              <label class="field-label" for="style-guardrails-input">
                Style Guardrails (one positive rule per line)
              </label>
              <textarea
                id="style-guardrails-input"
                class="code-editor"
                rows={4}
                value={styleDraft.guardrailsText}
                disabled={!styleEditable}
                onInput={(e) => {
                  setStyleDraft((prev) => ({ ...prev, guardrailsText: e.currentTarget.value }));
                }}
              />
            </div>

            <div class="button-row">
              <button
                type="button"
                class="btn btn--primary"
                disabled={!styleEditable}
                onClick={() => {
                  void handleSaveStyle();
                }}
              >
                {styleMode === 'create' ? 'Create Style' : 'Save Style Changes'}
              </button>
              {styleMode === 'edit' && activeStyle !== null && (
                <button
                  type="button"
                  class="btn btn--secondary"
                  disabled={
                    !isStyleMutable(activeStyle, ws.hasWorkspace) ||
                    nextLifecycleStep(activeStyle.lifecycle) === null
                  }
                  onClick={() => {
                    void handlePromoteStyle();
                  }}
                >
                  Promote Style ({nextLifecycleStep(activeStyle.lifecycle) ?? activeStyle.lifecycle}
                  )
                </button>
              )}
            </div>
            {styleMode === 'edit' && activeStyle !== null && !styleEditable && (
              <p class="muted-note">
                {activeStyle.lifecycle === 'canonical'
                  ? 'Canonical Styles are immutable in manifest.lock.json. Use "Clone Selected" to create an editable draft.'
                  : 'Built-in and user-tier Styles are read-only here. Use "Clone Selected" to customize inside this project.'}
              </p>
            )}
          </div>

          <div class="form-grid">
            <h4>Exemplar PNG Uploader (1..=3)</h4>
            <div
              class={`dropzone${isDragOver ? ' dropzone--active' : ''}`}
              tabIndex={0}
              role="region"
              aria-label="Drop PNG exemplars or press Ctrl+V to paste clipboard PNG"
              onDragOver={(e) => {
                e.preventDefault();
                setIsDragOver(true);
              }}
              onDragLeave={() => {
                setIsDragOver(false);
              }}
              onDrop={(e) => {
                e.preventDefault();
                setIsDragOver(false);
                const files = extractPngFilesFromDataTransfer(e.dataTransfer);
                void ingestPngFiles(files);
              }}
              onPaste={(e) => {
                const files = extractPngFilesFromDataTransfer(e.clipboardData);
                if (files.length > 0) {
                  e.preventDefault();
                  void ingestPngFiles(files);
                }
              }}
            >
              <p class="muted-note">
                Drag &amp; drop 1..=3 PNG files here, focus &amp; press <strong>Ctrl+V</strong> to
                paste a clipboard image, or choose files below.
              </p>
              <label class="field-label" for="exemplar-file-input">
                Select PNG Exemplars
              </label>
              <input
                id="exemplar-file-input"
                type="file"
                accept="image/png"
                multiple
                onChange={(e) => {
                  const list = e.currentTarget.files;
                  if (list !== null) {
                    const files: File[] = [];
                    for (let i = 0; i < list.length; i += 1) {
                      const f = list.item(i);
                      if (f !== null) {
                        files.push(f);
                      }
                    }
                    void ingestPngFiles(files);
                  }
                }}
              />
            </div>

            {pendingExemplarsB64.length > 0 && (
              <div class="thumb-row" aria-label="Staged PNG exemplars">
                {pendingExemplarsB64.map((b64, idx) => (
                  <img
                    key={`${String(idx)}-${b64.slice(0, 12)}`}
                    src={`data:image/png;base64,${b64}`}
                    alt={`Staged exemplar ${String(idx + 1)}`}
                    class="exemplar-thumb"
                  />
                ))}
              </div>
            )}

            <label class="checkbox-label">
              <input
                type="checkbox"
                checked={replaceExemplars}
                onChange={(e) => {
                  setReplaceExemplars(e.currentTarget.checked);
                }}
              />
              <span>Replace existing exemplars on upload</span>
            </label>

            <div class="button-row">
              <button
                type="button"
                class="btn btn--secondary"
                disabled={
                  styleMode === 'create' ||
                  activeStyle === null ||
                  !isStyleMutable(activeStyle, ws.hasWorkspace) ||
                  pendingExemplarsB64.length === 0
                }
                onClick={() => {
                  void handleUploadExemplars();
                }}
              >
                Upload Staged Exemplars ({pendingExemplarsB64.length}/3)
              </button>
              {pendingExemplarsB64.length > 0 && (
                <button
                  type="button"
                  class="btn btn--ghost"
                  onClick={() => {
                    setPendingExemplarsB64([]);
                  }}
                >
                  Clear Staged
                </button>
              )}
            </div>
          </div>
        </div>
      </section>

      {/* === 2. Atmosphere Cards & Editor === */}
      <section class="card" aria-labelledby="atmo-section-heading">
        <header class="card__header">
          <h3 id="atmo-section-heading">Atmospheres ({ws.atmospheres.length})</h3>
          <button
            type="button"
            class="btn btn--secondary"
            disabled={!ws.hasWorkspace}
            onClick={() => {
              setAtmoMode('create');
              setAtmoDraft({
                ...EMPTY_ATMO_DRAFT,
                slug: 'custom-atmo',
                displayName: 'Custom Atmosphere',
              });
            }}
          >
            New Atmosphere
          </button>
        </header>

        <div class="preset-gallery" role="list" aria-label="Atmosphere cards">
          {ws.atmospheres.map((atmo) => {
            const isSelected = atmoMode === 'edit' && atmo.slug === selectedAtmoSlug;
            return (
              <div key={atmo.slug} role="listitem">
                <button
                  type="button"
                  aria-pressed={isSelected}
                  class={`preset-tile${isSelected ? ' preset-tile--active' : ''}`}
                  onClick={() => {
                    setAtmoMode('edit');
                    setSelectedAtmoSlug(atmo.slug);
                    setAtmoDraft(atmosphereToFormDraft(atmo));
                  }}
                >
                  <div class="preset-tile__header">
                    <strong>{atmo.name}</strong>
                    <span class="badge">{atmo.source}</span>
                  </div>
                  <div class="preset-tile__meta">
                    <span>{atmo.colorTemperatureKelvin} K</span>
                    <span>L* ≥ {atmo.minMeanLStar.toFixed(0)}</span>
                  </div>
                </button>
              </div>
            );
          })}
        </div>

        <div class="form-row-3">
          <div class="control-group">
            <label class="field-label" for="atmo-slug">
              Slug
            </label>
            <input
              id="atmo-slug"
              class="text-input"
              value={atmoDraft.slug}
              disabled={atmoMode === 'edit' || !ws.hasWorkspace}
              onInput={(e) => {
                setAtmoDraft((prev) => ({ ...prev, slug: e.currentTarget.value }));
              }}
            />
          </div>
          <div class="control-group">
            <label class="field-label" for="atmo-display-name">
              Display Name
            </label>
            <input
              id="atmo-display-name"
              class="text-input"
              value={atmoDraft.displayName}
              disabled={!atmoEditable}
              onInput={(e) => {
                setAtmoDraft((prev) => ({ ...prev, displayName: e.currentTarget.value }));
              }}
            />
          </div>
          <div class="control-group">
            <label class="field-label" for="atmo-kelvin">
              Color Temperature (K)
            </label>
            <input
              id="atmo-kelvin"
              type="number"
              min={1000}
              max={20000}
              step={100}
              class="text-input"
              value={atmoDraft.colorTemperatureK}
              disabled={!atmoEditable}
              onInput={(e) => {
                const n = Number.parseInt(e.currentTarget.value, 10);
                if (Number.isFinite(n)) {
                  setAtmoDraft((prev) => ({ ...prev, colorTemperatureK: n }));
                }
              }}
            />
          </div>
          <div class="control-group">
            <label class="field-label" for="atmo-key-light">
              Key Light Direction
            </label>
            <input
              id="atmo-key-light"
              class="text-input"
              value={atmoDraft.keyLightDirection}
              disabled={!atmoEditable}
              onInput={(e) => {
                setAtmoDraft((prev) => ({ ...prev, keyLightDirection: e.currentTarget.value }));
              }}
            />
          </div>
          <div class="control-group">
            <label class="field-label" for="atmo-sec-light">
              Secondary Light Direction
            </label>
            <input
              id="atmo-sec-light"
              class="text-input"
              value={atmoDraft.secondaryLightDirection}
              disabled={!atmoEditable}
              onInput={(e) => {
                setAtmoDraft((prev) => ({
                  ...prev,
                  secondaryLightDirection: e.currentTarget.value,
                }));
              }}
            />
          </div>
          <div class="control-group">
            <label class="field-label" for="atmo-tint-blend">
              Max Ambient Tint Blend (0..1)
            </label>
            <input
              id="atmo-tint-blend"
              type="number"
              min={0}
              max={1}
              step={0.05}
              class="text-input"
              value={atmoDraft.maxAmbientTintBlend}
              disabled={!atmoEditable}
              onInput={(e) => {
                const n = Number.parseFloat(e.currentTarget.value);
                if (Number.isFinite(n)) {
                  setAtmoDraft((prev) => ({ ...prev, maxAmbientTintBlend: n }));
                }
              }}
            />
          </div>
          <div class="control-group">
            <label class="field-label" for="atmo-min-mean-l">
              Min Mean L*
            </label>
            <input
              id="atmo-min-mean-l"
              type="number"
              min={0}
              max={100}
              step={1}
              class="text-input"
              value={atmoDraft.minMeanLStar}
              disabled={!atmoEditable}
              onInput={(e) => {
                const n = Number.parseFloat(e.currentTarget.value);
                if (Number.isFinite(n)) {
                  setAtmoDraft((prev) => ({ ...prev, minMeanLStar: n }));
                }
              }}
            />
          </div>
          <div class="control-group">
            <label class="field-label" for="atmo-min-p10-l">
              Min P10 L*
            </label>
            <input
              id="atmo-min-p10-l"
              type="number"
              min={0}
              max={100}
              step={1}
              class="text-input"
              value={atmoDraft.minP10LStar}
              disabled={!atmoEditable}
              onInput={(e) => {
                const n = Number.parseFloat(e.currentTarget.value);
                if (Number.isFinite(n)) {
                  setAtmoDraft((prev) => ({ ...prev, minP10LStar: n }));
                }
              }}
            />
          </div>
          <div class="control-group">
            <label class="field-label" for="atmo-particles">
              Weather &amp; Particles
            </label>
            <input
              id="atmo-particles"
              class="text-input"
              value={atmoDraft.weatherAndParticles}
              disabled={!atmoEditable}
              onInput={(e) => {
                setAtmoDraft((prev) => ({ ...prev, weatherAndParticles: e.currentTarget.value }));
              }}
            />
          </div>
        </div>

        <div class="button-row">
          <button
            type="button"
            class="btn btn--primary"
            disabled={!atmoEditable}
            onClick={() => {
              void handleSaveAtmosphere();
            }}
          >
            {atmoMode === 'create' ? 'Create Atmosphere' : 'Save Atmosphere'}
          </button>
        </div>
      </section>

      {/* === 3. Subject PaletteLock & ReferencePack Slice Editor === */}
      <div class="split-two">
        <section class="card" aria-labelledby="subject-section-heading">
          <header class="card__header">
            <h3 id="subject-section-heading">Subjects &amp; PaletteLock ({ws.subjects.length})</h3>
          </header>

          {ws.subjects.length === 0 ? (
            <p class="muted-note">No Subject cards found in this workspace.</p>
          ) : (
            <>
              <div class="control-group">
                <label class="field-label" for="subject-select">
                  Selected Subject
                </label>
                <select
                  id="subject-select"
                  class="select-input"
                  value={selectedSubjectSlug}
                  onChange={(e) => {
                    setSelectedSubjectSlug(e.currentTarget.value);
                  }}
                >
                  {ws.subjects.map((subj) => (
                    <option key={subj.slug} value={subj.slug}>
                      {subj.name} ({subj.category} · {subj.visibility} · {subj.lifecycle})
                    </option>
                  ))}
                </select>
              </div>

              {activeSubject !== null && (
                <>
                  <dl class="meta-list">
                    <div>
                      <dt>Height</dt>
                      <dd>{activeSubject.heightMeters.toFixed(2)} m</dd>
                    </div>
                    <div>
                      <dt>Visibility / Lifecycle</dt>
                      <dd>
                        {activeSubject.visibility} / {activeSubject.lifecycle}
                      </dd>
                    </div>
                  </dl>
                  <ul class="swatch-list" aria-label="PaletteLock swatches">
                    {activeSubject.paletteLock.map((sw) => (
                      <li key={`${sw.label}-${sw.surfaceSlot}`} class="swatch-item">
                        <span
                          class="swatch-chip"
                          style={{ backgroundColor: sw.hex }}
                          aria-hidden="true"
                        />
                        <span>
                          <strong>{sw.label}</strong> ({sw.hex}) · {sw.surfaceSlot || 'unbound'}
                        </span>
                      </li>
                    ))}
                  </ul>
                  <div class="form-row-3">
                    <div class="control-group">
                      <label class="field-label" for="swatch-name">
                        Swatch Name
                      </label>
                      <input
                        id="swatch-name"
                        class="text-input"
                        value={swatchName}
                        placeholder="lantern_brass"
                        disabled={!subjectPaletteMutable}
                        onInput={(e) => {
                          setSwatchName(e.currentTarget.value);
                        }}
                      />
                    </div>
                    <div class="control-group">
                      <label class="field-label" for="swatch-hex">
                        Hex (#RRGGBB)
                      </label>
                      <input
                        id="swatch-hex"
                        class="text-input"
                        value={swatchHex}
                        disabled={!subjectPaletteMutable}
                        onInput={(e) => {
                          setSwatchHex(e.currentTarget.value);
                        }}
                      />
                    </div>
                    <div class="control-group">
                      <label class="field-label" for="swatch-slot">
                        Surface Slot
                      </label>
                      <input
                        id="swatch-slot"
                        class="text-input"
                        value={swatchSlot}
                        placeholder="lantern"
                        disabled={!subjectPaletteMutable}
                        onInput={(e) => {
                          setSwatchSlot(e.currentTarget.value);
                        }}
                      />
                    </div>
                  </div>
                  <div class="button-row">
                    <button
                      type="button"
                      class="btn btn--secondary"
                      disabled={!subjectPaletteMutable}
                      onClick={() => {
                        void handleAddPaletteSwatch();
                      }}
                    >
                      Save PaletteLock Swatch
                    </button>
                  </div>
                  {!subjectPaletteMutable && (
                    <p class="muted-note">
                      PaletteLock swatches are read-only for non-project, canonical, or superseded
                      Subjects.
                    </p>
                  )}
                </>
              )}
            </>
          )}
        </section>

        <section class="card" aria-labelledby="pack-section-heading">
          <header class="card__header">
            <h3 id="pack-section-heading">
              ReferencePack Reslice &amp; Lifecycle ({ws.referencePacks.length})
            </h3>
            {activePack !== null && <span class="badge badge--ready">{activePack.lifecycle}</span>}
          </header>

          {ws.referencePacks.length === 0 ? (
            <p class="muted-note">
              No ReferencePacks detected in workspace. Generate one via CLI or MCP <code>pack</code>{' '}
              tool first.
            </p>
          ) : (
            <>
              <div class="form-row-3">
                <div class="control-group">
                  <label class="field-label" for="pack-select">
                    ReferencePack
                  </label>
                  <select
                    id="pack-select"
                    class="select-input"
                    value={selectedPackKey}
                    onChange={(e) => {
                      const key = e.currentTarget.value;
                      setSelectedPackKey(key);
                      const pack = ws.referencePacks.find(
                        (p) => `${p.subjectSlug}/${p.styleSlug}` === key,
                      );
                      if (pack !== undefined) {
                        const nextRole = pack.sheets[0]?.role ?? pack.sheetKind;
                        setSelectedSheetRole(nextRole);
                        setSliceOverrides(
                          slicesToEditableOverrides(
                            filterSlicesForSheetRole(pack.slices, nextRole),
                          ),
                        );
                        setPackChecks([]);
                        setPackCriticReport(null);
                      }
                    }}
                  >
                    {ws.referencePacks.map((p) => {
                      const k = `${p.subjectSlug}/${p.styleSlug}`;
                      return (
                        <option key={k} value={k}>
                          {k} ({p.lifecycle} · {p.sliceCount} slices)
                        </option>
                      );
                    })}
                  </select>
                </div>

                {activePack !== null && (
                  <div class="control-group">
                    <label class="field-label" for="pack-sheet-role">
                      Sheet Role
                    </label>
                    <select
                      id="pack-sheet-role"
                      class="select-input"
                      value={selectedSheetRole}
                      onChange={(e) => {
                        const value = e.currentTarget.value;
                        if (isReferenceSheetKind(value)) {
                          setSelectedSheetRole(value);
                          setSliceOverrides(
                            slicesToEditableOverrides(
                              filterSlicesForSheetRole(activePack.slices, value),
                            ),
                          );
                          setSelectedSliceId(null);
                        }
                      }}
                    >
                      {activePack.sheets.map((sh) => (
                        <option key={sh.role} value={sh.role}>
                          {sh.role} ({sh.width}×{sh.height})
                        </option>
                      ))}
                    </select>
                  </div>
                )}

                <div class="control-group">
                  <label class="field-label" for="pack-override-mode">
                    Override Mode
                  </label>
                  <select
                    id="pack-override-mode"
                    class="select-input"
                    value={overrideMode}
                    onChange={(e) => {
                      if (isOverrideMode(e.currentTarget.value)) {
                        setOverrideMode(e.currentTarget.value);
                      }
                    }}
                  >
                    <option value="replace_all">replace_all</option>
                    <option value="merge_by_id">merge_by_id</option>
                  </select>
                </div>
              </div>

              {activeSheet !== null && (
                <div class="sheet-preview-box">
                  <img
                    src={activeSheet.imageUrl}
                    alt={`${activeSheet.filename} preview`}
                    class="sheet-preview-img"
                  />
                  <svg
                    class="sheet-preview-overlay"
                    viewBox={`0 0 ${String(activeSheet.width)} ${String(activeSheet.height)}`}
                    preserveAspectRatio="none"
                    aria-hidden="true"
                  >
                    {sliceOverrides.map((ov) => (
                      <rect
                        key={ov.rowKey}
                        x={ov.x}
                        y={ov.y}
                        width={ov.width}
                        height={ov.height}
                        fill={selectedSliceId === ov.id ? 'rgba(99, 179, 237, 0.25)' : 'none'}
                        stroke={selectedSliceId === ov.id ? '#63B3ED' : '#68D391'}
                        strokeWidth={4}
                      />
                    ))}
                  </svg>
                </div>
              )}

              <div class="override-table-wrap">
                <table class="data-table">
                  <thead>
                    <tr>
                      <th>Slice ID</th>
                      <th>Label</th>
                      <th>X</th>
                      <th>Y</th>
                      <th>W</th>
                      <th>H</th>
                      <th>Action</th>
                    </tr>
                  </thead>
                  <tbody>
                    {sliceOverrides.map((ov, idx) => (
                      <tr
                        key={ov.rowKey}
                        onClick={() => {
                          setSelectedSliceId(ov.id);
                        }}
                      >
                        <td>
                          <input
                            aria-label={`Slice ${String(idx + 1)} ID`}
                            class="text-input text-input--compact"
                            value={ov.id}
                            onInput={(e) => {
                              const val = e.currentTarget.value;
                              setSliceOverrides((prev) =>
                                prev.map((item, i) => (i === idx ? { ...item, id: val } : item)),
                              );
                            }}
                          />
                        </td>
                        <td>
                          <input
                            aria-label={`Slice ${String(idx + 1)} Label`}
                            class="text-input text-input--compact"
                            value={ov.label}
                            onInput={(e) => {
                              const val = e.currentTarget.value;
                              setSliceOverrides((prev) =>
                                prev.map((item, i) => (i === idx ? { ...item, label: val } : item)),
                              );
                            }}
                          />
                        </td>
                        <td>
                          <input
                            aria-label={`Slice ${String(idx + 1)} X`}
                            type="number"
                            class="text-input text-input--num"
                            value={ov.x}
                            onInput={(e) => {
                              const n = Number.parseInt(e.currentTarget.value, 10) || 0;
                              setSliceOverrides((prev) =>
                                prev.map((item, i) => (i === idx ? { ...item, x: n } : item)),
                              );
                            }}
                          />
                        </td>
                        <td>
                          <input
                            aria-label={`Slice ${String(idx + 1)} Y`}
                            type="number"
                            class="text-input text-input--num"
                            value={ov.y}
                            onInput={(e) => {
                              const n = Number.parseInt(e.currentTarget.value, 10) || 0;
                              setSliceOverrides((prev) =>
                                prev.map((item, i) => (i === idx ? { ...item, y: n } : item)),
                              );
                            }}
                          />
                        </td>
                        <td>
                          <input
                            aria-label={`Slice ${String(idx + 1)} Width`}
                            type="number"
                            class="text-input text-input--num"
                            value={ov.width}
                            onInput={(e) => {
                              const n = Number.parseInt(e.currentTarget.value, 10) || 1;
                              setSliceOverrides((prev) =>
                                prev.map((item, i) => (i === idx ? { ...item, width: n } : item)),
                              );
                            }}
                          />
                        </td>
                        <td>
                          <input
                            aria-label={`Slice ${String(idx + 1)} Height`}
                            type="number"
                            class="text-input text-input--num"
                            value={ov.height}
                            onInput={(e) => {
                              const n = Number.parseInt(e.currentTarget.value, 10) || 1;
                              setSliceOverrides((prev) =>
                                prev.map((item, i) => (i === idx ? { ...item, height: n } : item)),
                              );
                            }}
                          />
                        </td>
                        <td>
                          <button
                            type="button"
                            class="btn btn--ghost"
                            onClick={() => {
                              setSliceOverrides((prev) => prev.filter((_, i) => i !== idx));
                            }}
                          >
                            Remove
                          </button>
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>

              <div class="button-row">
                <button
                  type="button"
                  class="btn btn--secondary"
                  disabled={activePack === null || !isPackMutable(activePack)}
                  onClick={() => {
                    setSliceOverrides((prev) => [
                      ...prev,
                      {
                        rowKey: createSliceRowKey(),
                        id: `slice_${String(prev.length + 1)}`,
                        label: '',
                        x: 64,
                        y: 64,
                        width: 200,
                        height: 200,
                      },
                    ]);
                  }}
                >
                  Add Override Rect
                </button>
                <button
                  type="button"
                  class="btn btn--primary"
                  disabled={activePack === null || !isPackMutable(activePack)}
                  onClick={() => {
                    void handleReslicePack();
                  }}
                >
                  Apply Reslice
                </button>
              </div>

              <div class="button-row">
                <label class="checkbox-label">
                  <input
                    type="checkbox"
                    checked={runPackCritic}
                    onChange={(e) => {
                      setRunPackCritic(e.currentTarget.checked);
                    }}
                  />
                  <span>Run VisionCritic</span>
                </label>
                <button
                  type="button"
                  class="btn btn--secondary"
                  disabled={activePack === null || !isPackMutable(activePack)}
                  onClick={() => {
                    void handleValidatePack();
                  }}
                >
                  Validate Pack
                </button>
                <button
                  type="button"
                  class="btn btn--primary"
                  disabled={activePack === null || activePack.lifecycle !== 'validated'}
                  onClick={() => {
                    void handlePromotePack();
                  }}
                >
                  Promote Pack → Canonical
                </button>
              </div>

              {packChecks.length > 0 && (
                <ul class="check-list">
                  {packChecks.map((chk) => (
                    <li key={chk}>{chk}</li>
                  ))}
                </ul>
              )}
              {packCriticReport !== null && (
                <div class="critic-summary">
                  <strong>
                    VisionCritic: {packCriticReport.passed ? 'PASSED' : 'FAILED'} —{' '}
                    {packCriticReport.summary}
                  </strong>
                </div>
              )}
            </>
          )}
        </section>
      </div>
    </div>
  );
}
