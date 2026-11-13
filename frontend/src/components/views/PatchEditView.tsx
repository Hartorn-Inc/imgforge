/** === PATCHEDIT CROPPER VIEW ===
 *
 * PURPOSE:
 * Interactive `1:1` square `PatchEdit` cropper and mask-edit launcher connected
 * to `POST /api/edit/patch`, displaying live before/after composites,
 * OKLab Reinhard gains, and `outOfRoi` `ColorQaReport` metrics.
 *
 * DESIGN RATIONALE ("WHY"):
 * Per ADR 0002 and ADR 0005, `CodexNativeBackend` does not support native alpha
 * mask inpainting, so `PatchEdit` extracts a `1:1` square crop around an ROI
 * or painted mask and blends the output in Rust via collar-ring Reinhard
 * transfer (`16..=32 px`) and 5-level Laplacian pyramid blending.
 */

import type { JSX } from 'preact';
import { useState } from 'preact/hooks';
import { executePatchEdit } from '../../api/client';
import { buildPatchEditPayload, clampPatchEditDraft } from '../../state/workspace';
import type {
  AsyncViewState,
  PatchEditDraft,
  PatchEditResponse,
  PatchPrepMode,
  PatchRegionMode,
  RunRecord,
  Visibility,
  WorkspaceSummary,
} from '../../types/domain';
import { PatchCropStage } from '../PatchCropStage';
import { StatePanel } from '../StatePanel';

export interface PatchEditViewProps {
  readonly workspaceState: AsyncViewState<WorkspaceSummary>;
  readonly draft: PatchEditDraft;
  readonly onDraftChange: (next: PatchEditDraft) => void;
  readonly onRefreshWorkspace: () => Promise<void>;
  readonly onPatchCompleted: (res: PatchEditResponse) => void;
}

function isPatchRegionMode(value: string): value is PatchRegionMode {
  return value === 'roi' || value === 'mask';
}

function isPatchPrepMode(value: string): value is PatchPrepMode {
  return value === 'crop_only' || value === 'blank_inside_mask' || value === 'cue_outline';
}

function isVisibility(value: string): value is Visibility {
  return value === 'public' || value === 'restricted' || value === 'internal';
}

/** `PatchEdit Cropper` tab view. */
export function PatchEditView({
  workspaceState,
  draft,
  onDraftChange,
  onRefreshWorkspace,
  onPatchCompleted,
}: PatchEditViewProps): JSX.Element {
  const [brushRadiusPx, setBrushRadiusPx] = useState<number>(22);
  const [isSubmitting, setIsSubmitting] = useState<boolean>(false);
  const [submitError, setSubmitError] = useState<string | null>(null);
  const [lastResult, setLastResult] = useState<PatchEditResponse | null>(null);

  if (workspaceState.status !== 'ready') {
    return (
      <div class="workspace-panel">
        <div class="panel-toolbar">
          <div>
            <h2 class="panel-title">PatchEdit Cropper</h2>
            <p class="panel-subtitle">
              Frame a <code>1:1</code> square ROI or painted mask for localized{' '}
              <code>PatchEdit</code> compositing.
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
  if (!ws.hasWorkspace) {
    return (
      <div class="workspace-panel">
        <div class="panel-toolbar">
          <div>
            <h2 class="panel-title">PatchEdit Cropper</h2>
          </div>
        </div>
        <StatePanel
          state={{
            status: 'disabled',
            reason:
              'PatchEdit requires an active workspace. Launch imgforge ui --workspace <PATH>.',
          }}
        />
      </div>
    );
  }

  const selectedRun: RunRecord | null =
    draft.targetRunKey !== null
      ? (ws.runs.find((r) => r.runId === draft.targetRunKey) ?? null)
      : null;

  const targetPreviewUrl =
    selectedRun !== null
      ? selectedRun.imageUrl
      : draft.targetImagePath.trim().length > 0
        ? `/api/assets/${draft.targetImagePath.trim().replace(/^\/+/, '')}`
        : null;

  const updateDraft = (patch: Partial<PatchEditDraft>): void => {
    onDraftChange(clampPatchEditDraft({ ...draft, ...patch }));
  };

  const handleSubmitPatch = async (): Promise<void> => {
    setSubmitError(null);
    const payload = buildPatchEditPayload(draft, selectedRun);
    if ('error' in payload) {
      setSubmitError(payload.error);
      return;
    }

    setIsSubmitting(true);
    const res = await executePatchEdit(payload);
    setIsSubmitting(false);
    if (!res.ok) {
      setSubmitError(`${res.error.code}: ${res.error.message}`);
      return;
    }
    setLastResult(res.data);
    onPatchCompleted(res.data);
  };

  const hasTarget = selectedRun !== null || draft.targetImagePath.trim().length > 0;

  return (
    <div class="workspace-panel">
      <div class="panel-toolbar">
        <div>
          <h2 class="panel-title">1:1 PatchEdit Cropper</h2>
          <p class="panel-subtitle">
            Frame a square ROI with a <code>16..=32 px</code> collar ring or switch to painted mask
            mode, choose <code>PatchPrepMode</code>, and composite via Rust Laplacian pyramid.
          </p>
        </div>
      </div>

      {!hasTarget && (
        <StatePanel
          state={{
            status: 'empty',
            title: 'No target Run or workspace image selected',
            detail:
              'Choose an existing Run (vNNN) or enter a workspace-relative PNG path (e.g. runs/harbor/v001.png).',
          }}
          {...(ws.runs[0]
            ? {
                emptyActionLabel: `Bind ${ws.runs[0].runId}`,
                onEmptyAction: () => {
                  const first = ws.runs[0];
                  if (first !== undefined) {
                    updateDraft({
                      targetRunKey: first.runId,
                      slug: first.slug,
                      visibility: first.visibility,
                      canvasWidthPx: first.widthPx,
                      canvasHeightPx: first.heightPx,
                    });
                  }
                },
              }
            : {})}
        />
      )}

      <div class="studio-layout">
        <div class="studio-layout__canvas">
          <PatchCropStage
            imageUrl={targetPreviewUrl}
            canvasWidthPx={draft.canvasWidthPx}
            canvasHeightPx={draft.canvasHeightPx}
            mode={draft.mode}
            originXPx={draft.originXPx}
            originYPx={draft.originYPx}
            cropSizePx={draft.cropSizePx}
            collarWidthPx={draft.collarWidthPx}
            brushRadiusPx={brushRadiusPx}
            maskPngBase64={draft.maskPngBase64}
            onRoiChange={(x, y, size) => {
              updateDraft({ originXPx: x, originYPx: y, cropSizePx: size });
            }}
            onMaskChange={(b64) => {
              updateDraft({ maskPngBase64: b64 });
            }}
            onImageDimensions={(width, height) => {
              if (
                draft.targetRunKey === null &&
                (draft.canvasWidthPx !== width || draft.canvasHeightPx !== height)
              ) {
                updateDraft({ canvasWidthPx: width, canvasHeightPx: height });
              }
            }}
          />
        </div>

        <div class="studio-layout__inspector">
          <article class="card">
            <header class="card__header">
              <h3>Target &amp; Patch Parameters</h3>
              <span class="badge badge--ready">
                {draft.mode === 'roi' ? '1:1 Square ROI' : 'Painted Mask'}
              </span>
            </header>

            <div class="form-grid">
              <div class="control-group">
                <label class="field-label" for="patchedit-run-select">
                  Target Run (or Manual Path)
                </label>
                <select
                  id="patchedit-run-select"
                  class="select-input"
                  value={draft.targetRunKey ?? ''}
                  onChange={(e) => {
                    const key = e.currentTarget.value;
                    if (key === '') {
                      updateDraft({ targetRunKey: null });
                      return;
                    }
                    const run = ws.runs.find((r) => r.runId === key);
                    if (run !== undefined) {
                      updateDraft({
                        targetRunKey: run.runId,
                        slug: run.slug,
                        visibility: run.visibility,
                        canvasWidthPx: run.widthPx,
                        canvasHeightPx: run.heightPx,
                      });
                    }
                  }}
                >
                  <option value="">Manual targetImagePath...</option>
                  {ws.runs.map((r) => (
                    <option key={r.runId} value={r.runId}>
                      {r.slug} {r.versionLabel} ({r.widthPx}×{r.heightPx} · {r.visibility})
                    </option>
                  ))}
                </select>
              </div>

              {draft.targetRunKey === null && (
                <div class="control-group">
                  <label class="field-label" for="patchedit-target-path">
                    Target Image Path (workspace relative)
                  </label>
                  <input
                    id="patchedit-target-path"
                    class="text-input"
                    value={draft.targetImagePath}
                    placeholder="runs/harbor/v001.png"
                    onInput={(e) => {
                      updateDraft({ targetImagePath: e.currentTarget.value });
                    }}
                  />
                </div>
              )}

              <div class="form-row-2">
                <div class="control-group">
                  <label class="field-label" for="patchedit-slug">
                    Output Run Slug
                  </label>
                  <input
                    id="patchedit-slug"
                    class="text-input"
                    value={draft.slug}
                    onInput={(e) => {
                      updateDraft({ slug: e.currentTarget.value });
                    }}
                  />
                </div>
                <div class="control-group">
                  <label class="field-label" for="patchedit-visibility">
                    Visibility
                  </label>
                  <select
                    id="patchedit-visibility"
                    class="select-input"
                    value={draft.visibility}
                    onChange={(e) => {
                      if (isVisibility(e.currentTarget.value)) {
                        updateDraft({ visibility: e.currentTarget.value });
                      }
                    }}
                  >
                    <option value="public">public</option>
                    <option value="restricted">restricted</option>
                    <option value="internal">internal</option>
                  </select>
                </div>
              </div>

              <div class="form-row-2">
                <div class="control-group">
                  <label class="field-label" for="patchedit-mode-select">
                    Selection Mode
                  </label>
                  <select
                    id="patchedit-mode-select"
                    class="select-input"
                    value={draft.mode}
                    onChange={(e) => {
                      if (isPatchRegionMode(e.currentTarget.value)) {
                        updateDraft({ mode: e.currentTarget.value });
                      }
                    }}
                  >
                    <option value="roi">1:1 Square ROI</option>
                    <option value="mask">Painted Alpha Mask</option>
                  </select>
                </div>

                <div class="control-group">
                  <label class="field-label" for="patchedit-prep-mode">
                    PatchPrepMode
                  </label>
                  <select
                    id="patchedit-prep-mode"
                    class="select-input"
                    value={draft.prepMode}
                    onChange={(e) => {
                      if (isPatchPrepMode(e.currentTarget.value)) {
                        updateDraft({ prepMode: e.currentTarget.value });
                      }
                    }}
                  >
                    <option value="crop_only">crop_only</option>
                    <option value="blank_inside_mask">blank_inside_mask</option>
                    <option value="cue_outline">cue_outline</option>
                  </select>
                </div>
              </div>

              {draft.mode === 'roi' ? (
                <div class="form-row-3">
                  <div class="control-group">
                    <label class="field-label" for="patchedit-origin-x">
                      ROI X (px)
                    </label>
                    <input
                      id="patchedit-origin-x"
                      type="number"
                      class="text-input"
                      value={draft.originXPx}
                      onInput={(e) => {
                        const n = Number.parseInt(e.currentTarget.value, 10);
                        if (Number.isFinite(n)) {
                          updateDraft({ originXPx: n });
                        }
                      }}
                    />
                  </div>
                  <div class="control-group">
                    <label class="field-label" for="patchedit-origin-y">
                      ROI Y (px)
                    </label>
                    <input
                      id="patchedit-origin-y"
                      type="number"
                      class="text-input"
                      value={draft.originYPx}
                      onInput={(e) => {
                        const n = Number.parseInt(e.currentTarget.value, 10);
                        if (Number.isFinite(n)) {
                          updateDraft({ originYPx: n });
                        }
                      }}
                    />
                  </div>
                  <div class="control-group">
                    <label class="field-label" for="patchedit-crop-size">
                      1:1 Size (px)
                    </label>
                    <input
                      id="patchedit-crop-size"
                      type="number"
                      min={16}
                      step={16}
                      class="text-input"
                      value={draft.cropSizePx}
                      onInput={(e) => {
                        const n = Number.parseInt(e.currentTarget.value, 10);
                        if (Number.isFinite(n)) {
                          updateDraft({ cropSizePx: n });
                        }
                      }}
                    />
                  </div>
                </div>
              ) : (
                <div class="control-group">
                  <label class="field-label" for="patchedit-brush-radius">
                    Mask Brush Radius ({brushRadiusPx} px) ·{' '}
                    {draft.maskPngBase64 !== null ? 'Mask PNG Ready' : 'Paint mask on stage'}
                  </label>
                  <input
                    id="patchedit-brush-radius"
                    type="range"
                    min={6}
                    max={72}
                    value={brushRadiusPx}
                    onInput={(e) => {
                      const n = Number.parseInt(e.currentTarget.value, 10);
                      if (Number.isFinite(n)) {
                        setBrushRadiusPx(n);
                      }
                    }}
                  />
                </div>
              )}

              <div class="form-row-2">
                <div class="control-group">
                  <label class="field-label" for="patchedit-collar-width">
                    Collar Width (16..=32 px): {draft.collarWidthPx} px
                  </label>
                  <input
                    id="patchedit-collar-width"
                    type="range"
                    min={16}
                    max={32}
                    value={draft.collarWidthPx}
                    onInput={(e) => {
                      const n = Number.parseInt(e.currentTarget.value, 10);
                      if (Number.isFinite(n)) {
                        updateDraft({ collarWidthPx: n });
                      }
                    }}
                  />
                </div>
                <div class="control-group">
                  <label class="field-label" for="patchedit-feather">
                    Feather (2..={draft.collarWidthPx} px): {draft.featherPx} px
                  </label>
                  <input
                    id="patchedit-feather"
                    type="range"
                    min={2}
                    max={draft.collarWidthPx}
                    value={draft.featherPx}
                    onInput={(e) => {
                      const n = Number.parseInt(e.currentTarget.value, 10);
                      if (Number.isFinite(n)) {
                        updateDraft({ featherPx: n });
                      }
                    }}
                  />
                </div>
              </div>

              <div class="control-group">
                <label class="field-label" for="patchedit-prompt">
                  Positive Edit Prompt
                </label>
                <textarea
                  id="patchedit-prompt"
                  class="code-editor"
                  rows={3}
                  value={draft.prompt}
                  placeholder="refine brass lantern gleam matching warm coastal ink"
                  onInput={(e) => {
                    updateDraft({ prompt: e.currentTarget.value });
                  }}
                />
              </div>

              {submitError !== null && (
                <div class="validation-errors" role="alert">
                  {submitError}
                </div>
              )}

              <div class="button-row">
                <button
                  type="button"
                  class="btn btn--primary"
                  disabled={isSubmitting || !hasTarget}
                  onClick={() => {
                    void handleSubmitPatch();
                  }}
                >
                  {isSubmitting ? 'Running PatchEdit...' : 'Execute PatchEdit'}
                </button>
              </div>
            </div>
          </article>
        </div>
      </div>

      {lastResult !== null && (
        <section class="card" aria-labelledby="patchedit-result-heading">
          <header class="card__header">
            <h3 id="patchedit-result-heading">
              PatchEdit Result: {lastResult.run.slug} {lastResult.run.versionLabel}
            </h3>
            <span class={`badge ${lastResult.colorQa.passed ? 'badge--ready' : 'badge--error'}`}>
              outOfRoi QA: {lastResult.colorQa.passed ? 'PASSED' : 'FAILED'}
            </span>
          </header>

          <div class="split-two">
            <figure class="compare-figure">
              <figcaption>Before</figcaption>
              <img src={lastResult.beforeImageUrl} alt="Before PatchEdit" class="preview-img" />
            </figure>
            <figure class="compare-figure">
              <figcaption>After ({lastResult.run.versionLabel})</figcaption>
              <img src={lastResult.afterImageUrl} alt="After PatchEdit" class="preview-img" />
            </figure>
          </div>

          <dl class="meta-list">
            <div>
              <dt>1:1 Crop Rect</dt>
              <dd>
                ({lastResult.cropRect.x}, {lastResult.cropRect.y}) {lastResult.cropRect.width}×
                {lastResult.cropRect.height} px
              </dd>
            </div>
            <div>
              <dt>OKLab Reinhard Gains (L, a, b)</dt>
              <dd>
                ({lastResult.reinhardGains.gain_l.toFixed(3)},{' '}
                {lastResult.reinhardGains.gain_a.toFixed(3)},{' '}
                {lastResult.reinhardGains.gain_b.toFixed(3)}) ·{' '}
                {lastResult.reinhardGains.collar_pixel_count} collar px
              </dd>
            </div>
            <div>
              <dt>Out-of-ROI Drift (Max / Mean DeltaE_2000)</dt>
              <dd>
                {lastResult.colorQa.untouched_drift
                  ? `${lastResult.colorQa.untouched_drift.max_delta_e2000.toFixed(3)} / ${lastResult.colorQa.untouched_drift.mean_delta_e2000.toFixed(3)}`
                  : lastResult.colorQa.max_delta_e2000.toFixed(3)}
              </dd>
            </div>
          </dl>
        </section>
      )}
    </div>
  );
}
