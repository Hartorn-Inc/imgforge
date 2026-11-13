/** === PATCHEDIT CROPPER VIEW ===
 *
 * PURPOSE:
 * Provides controls for localized `PatchEdit` operations (`1:1` square ROI
 * extraction or painted mask, optional blank/cue target region, and collar-ring
 * OKLab harmonization parameters).
 *
 * DESIGN RATIONALE ("WHY"):
 * Per ADR 0002 and ADR 0005, `CodexNativeBackend` does not support native alpha
 * mask inpainting, so `PatchEdit` crops a square ROI and blends the output in
 * Rust via collar-ring Reinhard transfer and Laplacian pyramid blending.
 */

import type { JSX } from 'preact';
import { useState } from 'preact/hooks';
import type { AsyncViewState, PatchEditDraft, PatchRegionMode } from '../../types/domain';
import { StatePanel } from '../StatePanel';

const DEFAULT_PATCH_DRAFT: PatchEditDraft = {
  targetRunId: 'run-20261113-v001',
  mode: 'roi',
  cropSizePx: 512,
  originXPx: 256,
  originYPx: 192,
  blankTargetRegion: false,
  collarWidthPx: 32,
};

function isPatchRegionMode(value: string): value is PatchRegionMode {
  return value === 'roi' || value === 'mask';
}

/** `PatchEdit Cropper` tab view. */
export function PatchEditView(): JSX.Element {
  const [hasTargetRun, setHasTargetRun] = useState<boolean>(true);
  const [draft, setDraft] = useState<PatchEditDraft>(DEFAULT_PATCH_DRAFT);

  const viewState: AsyncViewState<PatchEditDraft> = hasTargetRun
    ? { status: 'ready', data: draft }
    : {
        status: 'empty',
        title: 'No target Run selected for PatchEdit',
        detail:
          'Select an existing Run (vNNN) or attach a local image before extracting a 1:1 square PatchEdit crop.',
      };

  return (
    <div class="workspace-panel">
      <div class="panel-toolbar">
        <div>
          <h2 class="panel-title">PatchEdit Cropper</h2>
          <p class="panel-subtitle">
            Configure <code>1:1</code> square ROI crops or painted masks for localized{' '}
            <code>PatchEdit</code> compositing (versus <code>WholeImageEdit</code>).
          </p>
        </div>
        <button
          type="button"
          class="btn btn--secondary"
          onClick={() => {
            setHasTargetRun((prev) => !prev);
          }}
        >
          {hasTargetRun ? 'Clear Target Run' : 'Attach Sample Run'}
        </button>
      </div>

      {viewState.status !== 'ready' ? (
        <StatePanel
          state={viewState}
          emptyActionLabel="Attach sample Run (v001)"
          onEmptyAction={() => {
            setHasTargetRun(true);
          }}
        />
      ) : (
        <div class="card-grid">
          <article class="card">
            <header class="card__header">
              <h3>1:1 Square Extraction Controls</h3>
              <span class="badge badge--ready">{viewState.data.targetRunId ?? 'Unbound'}</span>
            </header>

            <div class="form-grid">
              <div class="control-group">
                <label class="field-label" for="patchedit-mode-select">
                  Selection Mode
                </label>
                <select
                  id="patchedit-mode-select"
                  class="select-input"
                  value={viewState.data.mode}
                  onChange={(event) => {
                    const value = event.currentTarget.value;
                    if (isPatchRegionMode(value)) {
                      setDraft((prev) => ({ ...prev, mode: value }));
                    }
                  }}
                >
                  <option value="roi">ROI Square Crop</option>
                  <option value="mask">Painted Mask</option>
                </select>
              </div>

              <div class="control-group">
                <label class="field-label" for="patchedit-crop-size">
                  Square Crop Size (px)
                </label>
                <input
                  id="patchedit-crop-size"
                  type="number"
                  min={128}
                  max={2048}
                  step={64}
                  class="text-input"
                  value={viewState.data.cropSizePx}
                  onInput={(event) => {
                    const parsed = Number.parseInt(event.currentTarget.value, 10);
                    if (Number.isFinite(parsed) && parsed > 0) {
                      setDraft((prev) => ({ ...prev, cropSizePx: parsed }));
                    }
                  }}
                />
              </div>

              <div class="control-group">
                <label class="field-label" for="patchedit-collar-width">
                  Collar-Ring Blend Width (px)
                </label>
                <input
                  id="patchedit-collar-width"
                  type="number"
                  min={8}
                  max={128}
                  step={4}
                  class="text-input"
                  value={viewState.data.collarWidthPx}
                  onInput={(event) => {
                    const parsed = Number.parseInt(event.currentTarget.value, 10);
                    if (Number.isFinite(parsed) && parsed > 0) {
                      setDraft((prev) => ({ ...prev, collarWidthPx: parsed }));
                    }
                  }}
                />
              </div>

              <label class="checkbox-label">
                <input
                  type="checkbox"
                  checked={viewState.data.blankTargetRegion}
                  onChange={(event) => {
                    const checked = event.currentTarget.checked;
                    setDraft((prev) => ({ ...prev, blankTargetRegion: checked }));
                  }}
                />
                <span>Blank / Cue Target Region Before Generation</span>
              </label>
            </div>
          </article>

          <article class="card">
            <header class="card__header">
              <h3>Compositing Contract</h3>
              <span class="badge badge--ready">Host Blended</span>
            </header>
            <dl class="meta-list">
              <div>
                <dt>ROI Origin</dt>
                <dd>
                  ({viewState.data.originXPx}px, {viewState.data.originYPx}px)
                </dd>
              </div>
              <div>
                <dt>Patch Bounds</dt>
                <dd>
                  {viewState.data.cropSizePx}×{viewState.data.cropSizePx} px (1:1)
                </dd>
              </div>
              <div>
                <dt>Harmonization</dt>
                <dd>Collar-ring OKLab Reinhard transfer + Laplacian pyramid blend</dd>
              </div>
            </dl>
            <div class="button-row">
              <button
                type="button"
                class="btn btn--primary"
                disabled
                aria-describedby="patchedit-submit-note"
              >
                Queue PatchEdit Run
              </button>
            </div>
            <p id="patchedit-submit-note" class="muted-note">
              Run execution binds once the Rust Run endpoint is connected in Phase 2+.
            </p>
          </article>
        </div>
      )}
    </div>
  );
}
