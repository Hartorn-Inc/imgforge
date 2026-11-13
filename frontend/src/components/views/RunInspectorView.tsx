/** === RUN INSPECTOR VIEW ===
 *
 * PURPOSE:
 * Full interactive review, before/after diff, zoomed ROI lens, `PaletteLock`
 * eyedropper, Rust `L*` & OKLab telemetry charts, diagnostic heatmap overlays,
 * `VisionCritic` + `Lifecycle` promotion, and gated `Export` workflow.
 */

import type { JSX } from 'preact';
import { useCallback, useEffect, useRef, useState } from 'preact/hooks';
import {
  exportRun,
  inspectRun,
  promoteRun,
  updateSubjectPaletteLock,
  validateRun,
} from '../../api/client';
import { isExportEligible, isVisibilityAllowedForExport } from '../../state/workspace';
import type {
  AsyncViewState,
  ColorQaReport,
  CriticReport,
  ExportProfileName,
  ExportResponse,
  HeatmapKindKey,
  InspectRunResponse,
  RunRecord,
  Visibility,
  WorkspaceSummary,
} from '../../types/domain';
import { StatePanel } from '../StatePanel';
import { TelemetryCharts } from '../TelemetryCharts';
import {
  type ExportFormDraft,
  buildExportPayload,
  computeLensSourceRect,
  rgbBytesToHex,
} from './runInspectorHelpers';

export interface RunInspectorViewProps {
  readonly workspaceState: AsyncViewState<WorkspaceSummary>;
  readonly selectedRunId: string;
  readonly onSelectRunId: (runId: string) => void;
  readonly onRefreshWorkspace: () => Promise<void>;
  readonly onSendRunToPatchEdit: (run: RunRecord) => void;
}

type CompareMode = 'swipe' | 'onion';

const DEFAULT_EXPORT: ExportFormDraft = {
  profile: 'web_webp',
  visibility: 'public',
  outputSlug: '',
  tokenSize: 512,
  ringColorHex: '#D4A843',
  cols: 24,
  rows: 18,
  gridPx: 140,
  jpegQuality: 90,
  matchGradeEnabled: false,
  gradeRefPath: '',
  gradeStrength: 0.35,
  cubeLutPath: '',
};

function isExportProfileName(val: string): val is ExportProfileName {
  return val === 'vtt_token' || val === 'vtt_map' || val === 'web_webp' || val === 'grid_2x2';
}

function isVisibility(val: string): val is Visibility {
  return val === 'public' || val === 'restricted' || val === 'internal';
}

/** `Run Inspector` tab view. */
export function RunInspectorView({
  workspaceState,
  selectedRunId,
  onSelectRunId,
  onRefreshWorkspace,
  onSendRunToPatchEdit,
}: RunInspectorViewProps): JSX.Element {
  const [beforeRunVersion, setBeforeRunVersion] = useState<string>('');
  const [atmosphereSlug, setAtmosphereSlug] = useState<string>('');
  const [selectedSubjects, setSelectedSubjects] = useState<readonly string[]>([]);
  const [compareMode, setCompareMode] = useState<CompareMode>('swipe');
  const [comparePct, setComparePct] = useState<number>(50);
  const [activeHeatmaps, setActiveHeatmaps] = useState<Record<HeatmapKindKey, boolean>>({
    drift: false,
    bleed: false,
    sparkle: false,
    outOfRoi: false,
  });
  const [heatmapOpacityPct, setHeatmapOpacityPct] = useState<number>(65);

  const [inspection, setInspection] = useState<InspectRunResponse | null>(null);
  const [activeColorQa, setActiveColorQa] = useState<ColorQaReport | null>(null);
  const [criticReport, setCriticReport] = useState<CriticReport | null>(null);
  const [runCriticOnValidate, setRunCriticOnValidate] = useState<boolean>(false);

  const [eyedropperEnabled, setEyedropperEnabled] = useState<boolean>(true);
  const [sampledHex, setSampledHex] = useState<string>('#2C3E50');
  const [sampledCoords, setSampledCoords] = useState<{ readonly x: number; readonly y: number }>({
    x: 0,
    y: 0,
  });
  const [paletteSubjectSlug, setPaletteSubjectSlug] = useState<string>('');
  const [paletteSwatchName, setPaletteSwatchName] = useState<string>('');
  const [paletteSurfaceSlot, setPaletteSurfaceSlot] = useState<string>('');

  const [exportDraft, setExportDraft] = useState<ExportFormDraft>(DEFAULT_EXPORT);
  const [exportResult, setExportResult] = useState<ExportResponse | null>(null);

  const [busyLabel, setBusyLabel] = useState<string | null>(null);
  const [feedback, setFeedback] = useState<{
    readonly kind: 'ok' | 'error';
    readonly text: string;
  } | null>(null);

  const samplerCanvasRef = useRef<HTMLCanvasElement | null>(null);
  const lensCanvasRef = useRef<HTMLCanvasElement | null>(null);
  const inspectReqSeqRef = useRef<number>(0);
  const validateReqSeqRef = useRef<number>(0);

  const ws = workspaceState.status === 'ready' ? workspaceState.data : null;
  const runs = ws?.runs ?? [];
  const activeRun = runs.find((r) => r.runId === selectedRunId) ?? runs[0] ?? null;

  const activeRunIdRef = useRef<string>(activeRun?.runId ?? '');
  activeRunIdRef.current = activeRun?.runId ?? '';

  useEffect(() => {
    if (ws === null) {
      return;
    }
    if (ws.subjects[0] !== undefined && paletteSubjectSlug === '') {
      setPaletteSubjectSlug(ws.subjects[0].slug);
    }
    if (activeRun !== null && activeRun.runId !== selectedRunId) {
      onSelectRunId(activeRun.runId);
    }
  }, [ws, activeRun, selectedRunId, paletteSubjectSlug, onSelectRunId]);

  useEffect(() => {
    if (activeRun === null) {
      return;
    }
    const prevSameSlug = runs
      .filter((r) => r.slug === activeRun.slug && r.version < activeRun.version)
      .sort((a, b) => b.version - a.version)[0];
    setBeforeRunVersion(prevSameSlug !== undefined ? String(prevSameSlug.version) : '');
    setExportDraft((prev) => ({
      ...prev,
      outputSlug: activeRun.slug,
      visibility: activeRun.visibility,
    }));
  }, [activeRun?.runId]);

  // Draw activeRun image onto offscreen sampler canvas for zero-latency pixel eyedropper + 4x loupe.
  useEffect(() => {
    if (activeRun === null || typeof Image === 'undefined') {
      return;
    }
    let cancelled = false;
    const img = new Image();
    img.onload = () => {
      if (cancelled) {
        return;
      }
      const canvas = samplerCanvasRef.current;
      if (canvas !== null) {
        canvas.width = img.naturalWidth || activeRun.widthPx;
        canvas.height = img.naturalHeight || activeRun.heightPx;
        const ctx = canvas.getContext('2d');
        if (ctx !== null) {
          ctx.clearRect(0, 0, canvas.width, canvas.height);
          ctx.drawImage(img, 0, 0);
        }
      }
    };
    img.src = activeRun.imageUrl;
    return () => {
      cancelled = true;
    };
  }, [activeRun?.imageUrl, activeRun?.widthPx, activeRun?.heightPx]);

  const sampleAtImagePixel = useCallback(
    (rawX: number, rawY: number): void => {
      const sampler = samplerCanvasRef.current;
      const maxW =
        sampler !== null && sampler.width > 0 ? sampler.width : (activeRun?.widthPx ?? 1);
      const maxH =
        sampler !== null && sampler.height > 0 ? sampler.height : (activeRun?.heightPx ?? 1);
      const px = Math.max(0, Math.min(maxW - 1, Math.round(rawX)));
      const py = Math.max(0, Math.min(maxH - 1, Math.round(rawY)));
      setSampledCoords({ x: px, y: py });

      if (sampler === null || sampler.width === 0 || sampler.height === 0) {
        return;
      }
      const ctx = sampler.getContext('2d');
      if (ctx === null) {
        return;
      }
      const data = ctx.getImageData(px, py, 1, 1).data;
      const hex = rgbBytesToHex(data[0] ?? 0, data[1] ?? 0, data[2] ?? 0);
      setSampledHex(hex);

      const lens = lensCanvasRef.current;
      if (lens !== null) {
        const lensCtx = lens.getContext('2d');
        if (lensCtx !== null) {
          const { sx, sy, sw, sh } = computeLensSourceRect(
            px,
            py,
            sampler.width,
            sampler.height,
            4,
            lens.width,
          );
          lensCtx.imageSmoothingEnabled = false;
          lensCtx.clearRect(0, 0, lens.width, lens.height);
          lensCtx.drawImage(sampler, sx, sy, sw, sh, 0, 0, lens.width, lens.height);
        }
      }
    },
    [activeRun?.widthPx, activeRun?.heightPx],
  );

  const sampleAtClientPoint = useCallback(
    (clientX: number, clientY: number, element: HTMLElement): void => {
      const sampler = samplerCanvasRef.current;
      if (sampler === null || sampler.width === 0 || sampler.height === 0) {
        return;
      }
      const rect = element.getBoundingClientRect();
      if (rect.width <= 0 || rect.height <= 0) {
        return;
      }
      const px = Math.floor(((clientX - rect.left) / rect.width) * sampler.width);
      const py = Math.floor(((clientY - rect.top) / rect.height) * sampler.height);
      sampleAtImagePixel(px, py);
    },
    [sampleAtImagePixel],
  );

  if (workspaceState.status !== 'ready') {
    return (
      <div class="workspace-panel">
        <div class="panel-toolbar">
          <div>
            <h2 class="panel-title">Run Inspector</h2>
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

  if (!workspaceState.data.hasWorkspace || activeRun === null) {
    return (
      <div class="workspace-panel">
        <div class="panel-toolbar">
          <div>
            <h2 class="panel-title">Run Inspector</h2>
          </div>
        </div>
        <StatePanel
          state={
            !workspaceState.data.hasWorkspace
              ? {
                  status: 'disabled',
                  reason:
                    'Run Inspector requires an active workspace. Launch imgforge ui --workspace <PATH>.',
                }
              : {
                  status: 'empty',
                  title: 'No Run records available',
                  detail:
                    'Execute a generation or PatchEdit Run to inspect vNNN telemetry, heatmaps, VisionCritic, and Export.',
                }
          }
        />
      </div>
    );
  }

  const beforeRun =
    beforeRunVersion.trim().length > 0
      ? (runs.find(
          (r) => r.slug === activeRun.slug && String(r.version) === beforeRunVersion.trim(),
        ) ?? null)
      : null;

  const handleInspect = async (): Promise<void> => {
    const targetRun = activeRun;
    const targetRunId = targetRun.runId;
    const reqSeq = ++inspectReqSeqRef.current;
    setBusyLabel(`Inspecting ${targetRun.slug} ${targetRun.versionLabel}...`);
    setFeedback(null);
    const payload: {
      atmosphereSlug?: string;
      subjectSlugs?: readonly string[];
      beforeRunVersion?: number;
      maxSamples?: number;
    } = {
      maxSamples: 1024,
    };
    if (atmosphereSlug.trim().length > 0) {
      payload.atmosphereSlug = atmosphereSlug;
    }
    if (selectedSubjects.length > 0) {
      payload.subjectSlugs = selectedSubjects;
    }
    if (beforeRun !== null) {
      payload.beforeRunVersion = beforeRun.version;
    }
    const res = await inspectRun(targetRun.slug, targetRun.version, payload);
    if (reqSeq !== inspectReqSeqRef.current || activeRunIdRef.current !== targetRunId) {
      return;
    }
    setBusyLabel(null);
    if (!res.ok) {
      setFeedback({ kind: 'error', text: `${res.error.code}: ${res.error.message}` });
      return;
    }
    setInspection(res.data);
    setActiveColorQa(res.data.colorQa);
    setFeedback({
      kind: 'ok',
      text: `Loaded telemetry (${String(res.data.telemetry.vectorscopePoints.length)} vectorscope samples) & diagnostic heatmaps.`,
    });
  };

  const handleValidateRun = async (): Promise<void> => {
    const targetRun = activeRun;
    const targetRunId = targetRun.runId;
    const reqSeq = ++validateReqSeqRef.current;
    setBusyLabel(`Validating ${targetRun.slug} ${targetRun.versionLabel}...`);
    setFeedback(null);
    const payload: {
      atmosphereSlug?: string;
      subjectSlugs?: readonly string[];
      runCritic?: boolean;
    } = {
      runCritic: runCriticOnValidate,
    };
    if (atmosphereSlug.trim().length > 0) {
      payload.atmosphereSlug = atmosphereSlug;
    }
    if (selectedSubjects.length > 0) {
      payload.subjectSlugs = selectedSubjects;
    }
    const res = await validateRun(targetRun.slug, targetRun.version, payload);
    if (reqSeq !== validateReqSeqRef.current || activeRunIdRef.current !== targetRunId) {
      return;
    }
    setBusyLabel(null);
    if (!res.ok) {
      setFeedback({ kind: 'error', text: `${res.error.code}: ${res.error.message}` });
      return;
    }
    setActiveColorQa(res.data.colorQa);
    setCriticReport(res.data.criticReport);
    setFeedback({
      kind: 'ok',
      text: `Validated ${res.data.run.runId} -> ${res.data.run.lifecycle}.`,
    });
    await onRefreshWorkspace();
  };

  const handlePromoteRun = async (): Promise<void> => {
    setBusyLabel(`Promoting ${activeRun.slug} ${activeRun.versionLabel} to canonical...`);
    setFeedback(null);
    const res = await promoteRun(activeRun.slug, activeRun.version);
    setBusyLabel(null);
    if (!res.ok) {
      setFeedback({ kind: 'error', text: `${res.error.code}: ${res.error.message}` });
      return;
    }
    setFeedback({
      kind: 'ok',
      text: `Promoted ${res.data.run.runId} to canonical (lock entries: ${String(res.data.canonicalCount)}).`,
    });
    await onRefreshWorkspace();
  };

  const handleSaveEyedropperSwatch = async (): Promise<void> => {
    if (paletteSubjectSlug.trim().length === 0 || paletteSwatchName.trim().length === 0) {
      setFeedback({
        kind: 'error',
        text: 'Select a Subject and enter a swatch name before saving PaletteLock.',
      });
      return;
    }
    setBusyLabel(`Saving ${sampledHex} to ${paletteSubjectSlug} PaletteLock...`);
    setFeedback(null);
    const payload =
      paletteSurfaceSlot.trim().length > 0
        ? {
            name: paletteSwatchName.trim(),
            colorHex: sampledHex,
            surfaceSlot: paletteSurfaceSlot.trim(),
          }
        : { name: paletteSwatchName.trim(), colorHex: sampledHex };
    const res = await updateSubjectPaletteLock(paletteSubjectSlug, payload);
    setBusyLabel(null);
    if (!res.ok) {
      setFeedback({ kind: 'error', text: `${res.error.code}: ${res.error.message}` });
      return;
    }
    setPaletteSwatchName('');
    setPaletteSurfaceSlot('');
    setFeedback({
      kind: 'ok',
      text: `Saved ${sampledHex} to Subject '${res.data.slug}' PaletteLock.`,
    });
    await onRefreshWorkspace();
  };

  const handleExport = async (): Promise<void> => {
    const payload = buildExportPayload(activeRun, exportDraft);
    if ('error' in payload) {
      setFeedback({ kind: 'error', text: payload.error });
      return;
    }
    setBusyLabel(`Exporting ${exportDraft.profile} from ${activeRun.runId}...`);
    setFeedback(null);
    const res = await exportRun(payload);
    setBusyLabel(null);
    if (!res.ok) {
      setFeedback({ kind: 'error', text: `${res.error.code}: ${res.error.message}` });
      return;
    }
    setExportResult(res.data);
    setFeedback({
      kind: 'ok',
      text: `Exported ${String(res.data.artifacts.length)} deliverable(s) from ${res.data.sourceId}.`,
    });
    await onRefreshWorkspace();
  };

  const sameSlugPriorRuns = runs.filter(
    (r) => r.slug === activeRun.slug && r.version !== activeRun.version,
  );
  const visibilityOk = isVisibilityAllowedForExport(activeRun.visibility, exportDraft.visibility);
  const exportUnlocked = isExportEligible(activeRun.lifecycle) && visibilityOk;

  return (
    <div class="workspace-panel">
      <div class="panel-toolbar">
        <div>
          <h2 class="panel-title">Review, Diff &amp; Color Inspector</h2>
          <p class="panel-subtitle">
            Compare before/after <code>Run</code>s, inspect Rust <code>L*</code> &amp; OKLab
            telemetry, toggle diagnostic heatmaps, sample <code>PaletteLock</code> swatches, and
            promote or export deliverables.
          </p>
        </div>
        <div class="control-row">
          <div class="control-group">
            <label class="field-label" for="inspector-run-select">
              Active Run (vNNN)
            </label>
            <select
              id="inspector-run-select"
              class="select-input"
              value={activeRun.runId}
              onChange={(e) => {
                inspectReqSeqRef.current += 1;
                validateReqSeqRef.current += 1;
                onSelectRunId(e.currentTarget.value);
                setInspection(null);
                setActiveColorQa(null);
                setCriticReport(null);
                setExportResult(null);
                setBusyLabel(null);
              }}
            >
              {runs.map((r) => (
                <option key={r.runId} value={r.runId}>
                  {r.slug} {r.versionLabel} ({r.lifecycle} · {r.visibility})
                </option>
              ))}
            </select>
          </div>
          <button
            type="button"
            class="btn btn--secondary"
            onClick={() => {
              onSendRunToPatchEdit(activeRun);
            }}
          >
            PatchEdit This Run
          </button>
        </div>
      </div>

      {busyLabel !== null && (
        <div class="notice-banner" role="status">
          {busyLabel}
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

      {/* === 1. Inspection Target Controls === */}
      <section class="card" aria-labelledby="inspect-controls-heading">
        <header class="card__header">
          <h3 id="inspect-controls-heading">
            Run {activeRun.slug} {activeRun.versionLabel}
          </h3>
          <span class="badge badge--ready">
            {activeRun.lifecycle} · {activeRun.visibility}
          </span>
        </header>

        <div class="form-row-3">
          <div class="control-group">
            <label class="field-label" for="inspect-before-ver">
              Compare Against Baseline Version
            </label>
            <select
              id="inspect-before-ver"
              class="select-input"
              value={beforeRunVersion}
              onChange={(e) => {
                setBeforeRunVersion(e.currentTarget.value);
              }}
            >
              <option value="">None (single image)</option>
              {sameSlugPriorRuns.map((r) => (
                <option key={r.runId} value={String(r.version)}>
                  {r.versionLabel} ({r.lifecycle})
                </option>
              ))}
            </select>
          </div>

          <div class="control-group">
            <label class="field-label" for="inspect-atmo-select">
              Atmosphere Exposure Floor
            </label>
            <select
              id="inspect-atmo-select"
              class="select-input"
              value={atmosphereSlug}
              onChange={(e) => {
                setAtmosphereSlug(e.currentTarget.value);
              }}
            >
              <option value="">None</option>
              {workspaceState.data.atmospheres.map((a) => (
                <option key={a.slug} value={a.slug}>
                  {a.name} (L* ≥ {a.minMeanLStar})
                </option>
              ))}
            </select>
          </div>

          <div class="control-group">
            <span class="field-label">Subject PaletteLock Checks</span>
            <div class="chip-row">
              {workspaceState.data.subjects.map((subj) => {
                const checked = selectedSubjects.includes(subj.slug);
                return (
                  <label key={subj.slug} class="checkbox-label">
                    <input
                      type="checkbox"
                      checked={checked}
                      onChange={(e) => {
                        const isChecked = e.currentTarget.checked;
                        setSelectedSubjects((prev) =>
                          isChecked ? [...prev, subj.slug] : prev.filter((s) => s !== subj.slug),
                        );
                      }}
                    />
                    <span>{subj.slug}</span>
                  </label>
                );
              })}
            </div>
          </div>
        </div>

        <div class="button-row">
          <button
            type="button"
            class="btn btn--primary"
            onClick={() => {
              void handleInspect();
            }}
          >
            Inspect Telemetry &amp; Heatmaps
          </button>
          <label class="checkbox-label">
            <input
              type="checkbox"
              checked={runCriticOnValidate}
              onChange={(e) => {
                setRunCriticOnValidate(e.currentTarget.checked);
              }}
            />
            <span>Run VisionCritic on Validate</span>
          </label>
          <button
            type="button"
            class="btn btn--secondary"
            disabled={activeRun.lifecycle === 'validated' || activeRun.lifecycle === 'canonical'}
            onClick={() => {
              void handleValidateRun();
            }}
          >
            Validate Run
          </button>
          <button
            type="button"
            class="btn btn--primary"
            disabled={activeRun.lifecycle !== 'validated'}
            onClick={() => {
              void handlePromoteRun();
            }}
          >
            Promote → Canonical
          </button>
        </div>
      </section>

      {/* === 2. Swipe / Onion-Skin Comparator, Heatmap Overlays, Lens & Eyedropper === */}
      <div class="studio-layout">
        <section class="card" aria-labelledby="diff-viewport-heading">
          <header class="card__header">
            <h3 id="diff-viewport-heading">Visual Comparator &amp; Heatmap Overlays</h3>
            <div class="control-row">
              <label class="field-label" for="compare-mode-select">
                Diff Mode
              </label>
              <select
                id="compare-mode-select"
                class="select-input"
                value={compareMode}
                onChange={(e) => {
                  setCompareMode(e.currentTarget.value === 'onion' ? 'onion' : 'swipe');
                }}
              >
                <option value="swipe">Swipe Curtain</option>
                <option value="onion">Onion-Skin Overlay</option>
              </select>
            </div>
          </header>

          {beforeRun !== null && (
            <div class="control-group">
              <label class="field-label" for="compare-slider">
                {compareMode === 'swipe'
                  ? `Curtain Split: ${String(comparePct)}%`
                  : `After Opacity: ${String(comparePct)}%`}
              </label>
              <input
                id="compare-slider"
                type="range"
                min={0}
                max={100}
                value={comparePct}
                onInput={(e) => {
                  setComparePct(Number.parseInt(e.currentTarget.value, 10));
                }}
              />
            </div>
          )}

          <div
            class="compare-stage"
            tabIndex={0}
            role="region"
            aria-label="Run visual comparator and PaletteLock eyedropper stage; use arrow keys to step sample coordinates"
            onMouseMove={(e) => {
              if (eyedropperEnabled) {
                sampleAtClientPoint(e.clientX, e.clientY, e.currentTarget);
              }
            }}
            onClick={(e) => {
              sampleAtClientPoint(e.clientX, e.clientY, e.currentTarget);
            }}
            onKeyDown={(e) => {
              const step = e.shiftKey ? 10 : 1;
              if (e.key === 'ArrowLeft') {
                e.preventDefault();
                sampleAtImagePixel(sampledCoords.x - step, sampledCoords.y);
              } else if (e.key === 'ArrowRight') {
                e.preventDefault();
                sampleAtImagePixel(sampledCoords.x + step, sampledCoords.y);
              } else if (e.key === 'ArrowUp') {
                e.preventDefault();
                sampleAtImagePixel(sampledCoords.x, sampledCoords.y - step);
              } else if (e.key === 'ArrowDown') {
                e.preventDefault();
                sampleAtImagePixel(sampledCoords.x, sampledCoords.y + step);
              }
            }}
          >
            {beforeRun !== null && (
              <img
                src={beforeRun.imageUrl}
                alt={`Baseline ${beforeRun.versionLabel}`}
                class="compare-stage__base"
              />
            )}
            <img
              src={activeRun.imageUrl}
              alt={`Active ${activeRun.versionLabel}`}
              class="compare-stage__after"
              style={
                beforeRun === null
                  ? undefined
                  : compareMode === 'swipe'
                    ? { clipPath: `inset(0 ${String(100 - comparePct)}% 0 0)` }
                    : { opacity: comparePct / 100 }
              }
            />

            {(['drift', 'bleed', 'sparkle', 'outOfRoi'] as const).map((kind) => {
              const b64 = inspection?.heatmaps[kind] ?? null;
              if (!activeHeatmaps[kind] || b64 === null) {
                return null;
              }
              return (
                <img
                  key={kind}
                  src={`data:image/png;base64,${b64}`}
                  alt={`${kind} diagnostic heatmap`}
                  class="compare-stage__heatmap"
                  style={{ opacity: heatmapOpacityPct / 100 }}
                />
              );
            })}

            {beforeRun !== null && compareMode === 'swipe' && (
              <div
                class="compare-stage__divider"
                style={{ left: `${String(comparePct)}%` }}
                aria-hidden="true"
              />
            )}
          </div>

          <canvas ref={samplerCanvasRef} hidden />

          <div class="heatmap-bar">
            <span class="field-label">Diagnostic Heatmaps:</span>
            {(['drift', 'bleed', 'sparkle', 'outOfRoi'] as const).map((kind) => {
              const available = inspection?.heatmaps[kind] !== null && inspection !== null;
              return (
                <label key={kind} class="checkbox-label">
                  <input
                    type="checkbox"
                    disabled={!available}
                    checked={activeHeatmaps[kind]}
                    onChange={(e) => {
                      const checked = e.currentTarget.checked;
                      setActiveHeatmaps((prev) => ({ ...prev, [kind]: checked }));
                    }}
                  />
                  <span>{kind}</span>
                </label>
              );
            })}
            <label class="field-label" for="heatmap-opacity">
              Opacity {heatmapOpacityPct}%
            </label>
            <input
              id="heatmap-opacity"
              type="range"
              min={10}
              max={100}
              value={heatmapOpacityPct}
              onInput={(e) => {
                setHeatmapOpacityPct(Number.parseInt(e.currentTarget.value, 10));
              }}
            />
          </div>
        </section>

        {/* === 3. Zoomed ROI Lens & PaletteLock Eyedropper === */}
        <section class="card" aria-labelledby="eyedropper-heading">
          <header class="card__header">
            <h3 id="eyedropper-heading">4× Zoom Lens &amp; PaletteLock Eyedropper</h3>
            <label class="checkbox-label">
              <input
                type="checkbox"
                checked={eyedropperEnabled}
                onChange={(e) => {
                  setEyedropperEnabled(e.currentTarget.checked);
                }}
              />
              <span>Live Hover Sample</span>
            </label>
          </header>

          <div class="lens-row">
            <canvas
              ref={lensCanvasRef}
              width={128}
              height={128}
              class="lens-canvas"
              aria-label="4x zoomed ROI pixel inspection lens"
            />
            <div class="form-grid">
              <div class="swatch-item">
                <span
                  class="swatch-chip swatch-chip--lg"
                  style={{ backgroundColor: sampledHex }}
                  aria-hidden="true"
                />
                <code>{sampledHex}</code>
                <span class="muted-note">
                  @ ({sampledCoords.x}, {sampledCoords.y})
                </span>
              </div>

              <div class="form-row-2">
                <div class="control-group">
                  <label class="field-label" for="eyedropper-x">
                    Sample X (px)
                  </label>
                  <input
                    id="eyedropper-x"
                    type="number"
                    min={0}
                    max={Math.max(0, activeRun.widthPx - 1)}
                    step={1}
                    class="text-input"
                    value={sampledCoords.x}
                    onInput={(e) => {
                      const n = Number.parseInt(e.currentTarget.value, 10);
                      if (Number.isFinite(n)) {
                        sampleAtImagePixel(n, sampledCoords.y);
                      }
                    }}
                  />
                </div>
                <div class="control-group">
                  <label class="field-label" for="eyedropper-y">
                    Sample Y (px)
                  </label>
                  <input
                    id="eyedropper-y"
                    type="number"
                    min={0}
                    max={Math.max(0, activeRun.heightPx - 1)}
                    step={1}
                    class="text-input"
                    value={sampledCoords.y}
                    onInput={(e) => {
                      const n = Number.parseInt(e.currentTarget.value, 10);
                      if (Number.isFinite(n)) {
                        sampleAtImagePixel(sampledCoords.x, n);
                      }
                    }}
                  />
                </div>
              </div>

              <div class="control-group">
                <label class="field-label" for="eyedropper-subject">
                  Target Subject
                </label>
                <select
                  id="eyedropper-subject"
                  class="select-input"
                  value={paletteSubjectSlug}
                  onChange={(e) => {
                    setPaletteSubjectSlug(e.currentTarget.value);
                  }}
                >
                  {workspaceState.data.subjects.map((s) => (
                    <option key={s.slug} value={s.slug}>
                      {s.name} ({s.slug})
                    </option>
                  ))}
                </select>
              </div>

              <div class="form-row-2">
                <div class="control-group">
                  <label class="field-label" for="eyedropper-swatch-name">
                    Swatch Name
                  </label>
                  <input
                    id="eyedropper-swatch-name"
                    class="text-input"
                    value={paletteSwatchName}
                    placeholder="coat_slate"
                    onInput={(e) => {
                      setPaletteSwatchName(e.currentTarget.value);
                    }}
                  />
                </div>
                <div class="control-group">
                  <label class="field-label" for="eyedropper-slot">
                    Surface Slot
                  </label>
                  <input
                    id="eyedropper-slot"
                    class="text-input"
                    value={paletteSurfaceSlot}
                    placeholder="coat"
                    onInput={(e) => {
                      setPaletteSurfaceSlot(e.currentTarget.value);
                    }}
                  />
                </div>
              </div>

              <button
                type="button"
                class="btn btn--secondary"
                disabled={workspaceState.data.subjects.length === 0}
                onClick={() => {
                  void handleSaveEyedropperSwatch();
                }}
              >
                Bind {sampledHex} to PaletteLock
              </button>
            </div>
          </div>
        </section>
      </div>

      {/* === 4. Rust L* Histogram & OKLab Vectorscope === */}
      {inspection !== null && (
        <section class="card" aria-labelledby="telemetry-heading">
          <header class="card__header">
            <h3 id="telemetry-heading">Rust Color Telemetry</h3>
            <span class="badge badge--ready">
              {inspection.telemetry.width}×{inspection.telemetry.height} px
            </span>
          </header>
          <TelemetryCharts telemetry={inspection.telemetry} />
        </section>
      )}

      {/* === 5. ColorQA & VisionCritic Breakdown === */}
      {(activeColorQa !== null || criticReport !== null) && (
        <div class="split-two">
          {activeColorQa !== null && (
            <section class="card" aria-labelledby="colorqa-heading">
              <header class="card__header">
                <h3 id="colorqa-heading">Color QA Report</h3>
                <span class={`badge ${activeColorQa.passed ? 'badge--ready' : 'badge--error'}`}>
                  {activeColorQa.passed ? 'PASSED' : 'FAILED'} · Max ΔE{' '}
                  {activeColorQa.max_delta_e2000.toFixed(2)}
                </span>
              </header>
              {activeColorQa.exposure !== null && (
                <p class="muted-note">
                  Exposure: mean L*={activeColorQa.exposure.metrics.mean_l_star.toFixed(1)}, P10 L*=
                  {activeColorQa.exposure.metrics.p10_l_star.toFixed(1)}, clipped black=
                  {(activeColorQa.exposure.metrics.clipped_black_ratio * 100).toFixed(2)}%
                </p>
              )}
              {activeColorQa.sparkle !== null && (
                <p class="muted-note">
                  Sparkle QA: {activeColorQa.sparkle.passed ? 'Clean' : 'Flagged'} (
                  {activeColorQa.sparkle.flagged_blobs.length} blob(s))
                </p>
              )}
              {activeColorQa.notes.length > 0 && (
                <ul class="check-list">
                  {activeColorQa.notes.map((n) => (
                    <li key={n}>{n}</li>
                  ))}
                </ul>
              )}
            </section>
          )}

          {criticReport !== null && (
            <section class="card" aria-labelledby="critic-heading">
              <header class="card__header">
                <h3 id="critic-heading">VisionCritic Checklist</h3>
                <span class={`badge ${criticReport.passed ? 'badge--ready' : 'badge--error'}`}>
                  {criticReport.passed ? 'PASSED' : 'FAILED'}
                </span>
              </header>
              <p class="muted-note">{criticReport.summary}</p>
              <table class="data-table">
                <thead>
                  <tr>
                    <th>Trait</th>
                    <th>Category</th>
                    <th>Verdict</th>
                    <th>Observation</th>
                  </tr>
                </thead>
                <tbody>
                  {criticReport.checks.map((chk) => (
                    <tr key={chk.trait_id}>
                      <td>
                        <code>{chk.trait_id}</code>
                      </td>
                      <td>{chk.category}</td>
                      <td>{chk.passed ? 'PASS' : 'FAIL'}</td>
                      <td>{chk.observation}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </section>
          )}
        </div>
      )}

      {/* === 6. Export Deliverables Form === */}
      <section class="card" aria-labelledby="export-heading">
        <header class="card__header">
          <h3 id="export-heading">Export Deliverables</h3>
          <span class={`badge ${exportUnlocked ? 'badge--ready' : 'badge--error'}`}>
            {exportUnlocked ? 'Unlocked' : 'Gated'}
          </span>
        </header>

        <div class="form-row-3">
          <div class="control-group">
            <label class="field-label" for="export-profile">
              Export Profile
            </label>
            <select
              id="export-profile"
              class="select-input"
              value={exportDraft.profile}
              onChange={(e) => {
                const value = e.currentTarget.value;
                if (isExportProfileName(value)) {
                  setExportDraft((prev) => ({
                    ...prev,
                    profile: value,
                  }));
                }
              }}
            >
              <option value="web_webp">web_webp</option>
              <option value="vtt_token">vtt_token</option>
              <option value="vtt_map">vtt_map</option>
              <option value="grid_2x2">grid_2x2</option>
            </select>
          </div>

          <div class="control-group">
            <label class="field-label" for="export-visibility">
              Target Visibility
            </label>
            <select
              id="export-visibility"
              class="select-input"
              value={exportDraft.visibility}
              onChange={(e) => {
                const value = e.currentTarget.value;
                if (isVisibility(value)) {
                  setExportDraft((prev) => ({
                    ...prev,
                    visibility: value,
                  }));
                }
              }}
            >
              <option value="public">public</option>
              <option value="restricted">restricted</option>
              <option value="internal">internal</option>
            </select>
          </div>

          <div class="control-group">
            <label class="field-label" for="export-output-slug">
              Output Slug
            </label>
            <input
              id="export-output-slug"
              class="text-input"
              value={exportDraft.outputSlug}
              onInput={(e) => {
                setExportDraft((prev) => ({ ...prev, outputSlug: e.currentTarget.value }));
              }}
            />
          </div>
        </div>

        {exportDraft.profile === 'vtt_token' && (
          <div class="form-row-3">
            <div class="control-group">
              <label class="field-label" for="export-token-size">
                Token Size (px)
              </label>
              <input
                id="export-token-size"
                type="number"
                min={16}
                max={4096}
                class="text-input"
                value={exportDraft.tokenSize}
                onInput={(e) => {
                  const n = Number.parseInt(e.currentTarget.value, 10) || 512;
                  setExportDraft((prev) => ({ ...prev, tokenSize: n }));
                }}
              />
            </div>
            <div class="control-group">
              <label class="field-label" for="export-ring-hex">
                Ring Color (#RRGGBB)
              </label>
              <input
                id="export-ring-hex"
                class="text-input"
                value={exportDraft.ringColorHex}
                onInput={(e) => {
                  setExportDraft((prev) => ({ ...prev, ringColorHex: e.currentTarget.value }));
                }}
              />
            </div>
            <div class="control-group">
              <label class="field-label" for="export-jpeg-qual">
                JPEG Quality (1..100)
              </label>
              <input
                id="export-jpeg-qual"
                type="number"
                min={1}
                max={100}
                class="text-input"
                value={exportDraft.jpegQuality}
                onInput={(e) => {
                  const n = Number.parseInt(e.currentTarget.value, 10) || 90;
                  setExportDraft((prev) => ({ ...prev, jpegQuality: n }));
                }}
              />
            </div>
          </div>
        )}

        {exportDraft.profile === 'vtt_map' && (
          <div class="form-row-3">
            <div class="control-group">
              <label class="field-label" for="export-map-cols">
                Grid Columns
              </label>
              <input
                id="export-map-cols"
                type="number"
                min={1}
                class="text-input"
                value={exportDraft.cols}
                onInput={(e) => {
                  const n = Number.parseInt(e.currentTarget.value, 10) || 24;
                  setExportDraft((prev) => ({ ...prev, cols: n }));
                }}
              />
            </div>
            <div class="control-group">
              <label class="field-label" for="export-map-rows">
                Grid Rows
              </label>
              <input
                id="export-map-rows"
                type="number"
                min={1}
                class="text-input"
                value={exportDraft.rows}
                onInput={(e) => {
                  const n = Number.parseInt(e.currentTarget.value, 10) || 18;
                  setExportDraft((prev) => ({ ...prev, rows: n }));
                }}
              />
            </div>
            <div class="control-group">
              <label class="field-label" for="export-map-grid">
                Grid Cell (px)
              </label>
              <input
                id="export-map-grid"
                type="number"
                min={16}
                class="text-input"
                value={exportDraft.gridPx}
                onInput={(e) => {
                  const n = Number.parseInt(e.currentTarget.value, 10) || 140;
                  setExportDraft((prev) => ({ ...prev, gridPx: n }));
                }}
              />
            </div>
          </div>
        )}

        <div class="form-grid">
          <label class="checkbox-label">
            <input
              type="checkbox"
              checked={exportDraft.matchGradeEnabled}
              onChange={(e) => {
                setExportDraft((prev) => ({ ...prev, matchGradeEnabled: e.currentTarget.checked }));
              }}
            />
            <span>Enable opt-in Export --match-grade (L* CDF 0.25..=0.45 / .cube LUT)</span>
          </label>

          {exportDraft.matchGradeEnabled && (
            <div class="form-row-3">
              <div class="control-group">
                <label class="field-label" for="export-grade-ref">
                  Grade Reference Image (optional)
                </label>
                <input
                  id="export-grade-ref"
                  class="text-input"
                  value={exportDraft.gradeRefPath}
                  placeholder="runs/anchor/v001.png"
                  onInput={(e) => {
                    setExportDraft((prev) => ({ ...prev, gradeRefPath: e.currentTarget.value }));
                  }}
                />
              </div>
              <div class="control-group">
                <label class="field-label" for="export-grade-strength">
                  CDF Damp Strength ({exportDraft.gradeStrength.toFixed(2)})
                </label>
                <input
                  id="export-grade-strength"
                  type="range"
                  min={0.25}
                  max={0.45}
                  step={0.01}
                  value={exportDraft.gradeStrength}
                  onInput={(e) => {
                    const n = Number.parseFloat(e.currentTarget.value);
                    if (Number.isFinite(n)) {
                      setExportDraft((prev) => ({ ...prev, gradeStrength: n }));
                    }
                  }}
                />
              </div>
              <div class="control-group">
                <label class="field-label" for="export-cube-lut">
                  3D .cube LUT Path (optional)
                </label>
                <input
                  id="export-cube-lut"
                  class="text-input"
                  value={exportDraft.cubeLutPath}
                  placeholder="luts/coastal.cube"
                  onInput={(e) => {
                    setExportDraft((prev) => ({ ...prev, cubeLutPath: e.currentTarget.value }));
                  }}
                />
              </div>
            </div>
          )}
        </div>

        <div class="button-row">
          <button
            type="button"
            class="btn btn--primary"
            disabled={!exportUnlocked}
            onClick={() => {
              void handleExport();
            }}
          >
            Export Deliverable
          </button>
        </div>

        {!isExportEligible(activeRun.lifecycle) && (
          <p class="muted-note">
            Advance Lifecycle to <code>validated</code> or <code>canonical</code> before exporting.
          </p>
        )}
        {!visibilityOk && (
          <p class="muted-note">
            Visibility gate: Run is <code>{activeRun.visibility}</code> and cannot export as{' '}
            <code>{exportDraft.visibility}</code>.
          </p>
        )}

        {exportResult !== null && (
          <div class="export-artifacts">
            {exportResult.artifacts.map((art) => (
              <div key={art.path} class="artifact-card">
                <div class="artifact-card__meta">
                  <strong>{art.role}</strong> ({art.width}×{art.height})<code>{art.path}</code>
                </div>
                {art.assetUrl !== null && (
                  <a href={art.assetUrl} target="_blank" rel="noreferrer" class="btn btn--ghost">
                    Open Asset
                  </a>
                )}
              </div>
            ))}
          </div>
        )}
      </section>
    </div>
  );
}
