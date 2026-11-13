/** === SPATIALGUIDE STUDIO VIEW ===
 *
 * PURPOSE:
 * Hosts the interactive Konva `StageCanvas`, `GuideSpec` JSON editor,
 * server-side `GuideSpec` validation (`POST /api/guide/validate`), live Rust
 * `GuideRenderer` preview (`POST /api/guide/render`), and freehand PNG mask
 * export for `PatchEdit`.
 */

import type { JSX } from 'preact';
import { useState } from 'preact/hooks';
import { renderGuideRemote, validateGuideRemote } from '../../api/client';
import type { AsyncViewState, RunRecord, WorkspaceSummary } from '../../types/domain';
import {
  type GuideSpec,
  type GuideSpecValidationError,
  type SpatialGuideVariant,
  createDefaultGuideSpec,
  parseGuideSpecJson,
  serializeGuideSpecJson,
} from '../../types/guideSpec';
import { StageCanvas, type StageToolMode } from '../StageCanvas';
import { StatePanel } from '../StatePanel';

export interface SpatialGuideViewProps {
  readonly spec: GuideSpec;
  readonly jsonDraft: string;
  readonly workspace: WorkspaceSummary | null;
  readonly exportedMaskBase64: string | null;
  readonly onSpecUpdate: (nextSpec: GuideSpec, nextJson: string) => void;
  readonly onJsonDraftChange: (nextJson: string) => void;
  readonly onMaskCaptured: (maskPngBase64: string, switchToPatchEdit: boolean) => void;
}

function isSpatialGuideVariant(value: string): value is SpatialGuideVariant {
  return value === 'Blockout' || value === 'OverlayGuide';
}

function isStageToolMode(value: string): value is StageToolMode {
  return value === 'select' || value === 'draw_vector' || value === 'paint_mask';
}

/** Resolve a workspace-relative `targetImagePath` (like `runs/foo/v001.png`) to its `/api/assets/...` URL. */
function resolveTargetAssetUrl(
  targetImagePath: string | null,
  runs: readonly RunRecord[],
): string | null {
  if (targetImagePath === null || targetImagePath.trim().length === 0) {
    return null;
  }
  const normalized = targetImagePath.trim().replace(/^\/+/, '');
  const matchingRun = runs.find((r) => `runs/${r.slug}/${r.versionLabel}.png` === normalized);
  if (matchingRun !== undefined && matchingRun.imageUrl.length > 0) {
    return matchingRun.imageUrl;
  }
  return `/api/assets/${normalized}`;
}

/** `SpatialGuide Studio` tab view. */
export function SpatialGuideView({
  spec,
  jsonDraft,
  workspace,
  exportedMaskBase64,
  onSpecUpdate,
  onJsonDraftChange,
  onMaskCaptured,
}: SpatialGuideViewProps): JSX.Element {
  const [showGrid, setShowGrid] = useState<boolean>(true);
  const [stageDisabled, setStageDisabled] = useState<boolean>(false);
  const [toolMode, setToolMode] = useState<StageToolMode>('select');
  const [brushRadiusPx, setBrushRadiusPx] = useState<number>(20);
  const [validationErrors, setValidationErrors] = useState<readonly GuideSpecValidationError[]>([]);
  const [renderStatus, setRenderStatus] = useState<string | null>(null);
  const [renderedPngB64, setRenderedPngB64] = useState<string | null>(null);
  const [isBusy, setIsBusy] = useState<boolean>(false);

  const syncSpec = (nextSpec: GuideSpec): void => {
    onSpecUpdate(nextSpec, serializeGuideSpecJson(nextSpec));
    setValidationErrors([]);
  };

  const handleVariantSwitch = (variant: SpatialGuideVariant): void => {
    const nextSpec = createDefaultGuideSpec(variant);
    if (nextSpec.variant === 'OverlayGuide' && workspace !== null && workspace.runs.length > 0) {
      const firstRun = workspace.runs[0];
      if (firstRun !== undefined) {
        syncSpec({
          ...nextSpec,
          targetImagePath: `runs/${firstRun.slug}/${firstRun.versionLabel}.png`,
          canvasWidth: firstRun.widthPx,
          canvasHeight: firstRun.heightPx,
        });
        return;
      }
    }
    syncSpec(nextSpec);
  };

  const handleApplyJson = async (): Promise<void> => {
    const result = parseGuideSpecJson(jsonDraft);
    if (!result.ok) {
      setValidationErrors(result.errors);
      return;
    }
    setIsBusy(true);
    setRenderStatus('Validating via POST /api/guide/validate...');
    const remote = await validateGuideRemote(result.value);
    setIsBusy(false);

    if (remote.ok) {
      if (!remote.data.valid) {
        setValidationErrors(remote.data.errors);
        setRenderStatus('Server GuideSpec validation failed.');
        return;
      }
      const validated = remote.data.spec ?? result.value;
      syncSpec(validated);
      setRenderStatus('GuideSpec validated cleanly against Rust schema.');
      return;
    }

    // If local server is unreachable during standalone UI work, apply local schema validation.
    syncSpec(result.value);
    setRenderStatus(`Applied locally (server validate unavailable: ${remote.error.message}).`);
  };

  const handleRenderRustGuide = async (): Promise<void> => {
    setIsBusy(true);
    setRenderStatus('Rendering via Rust GuideRenderer (POST /api/guide/render)...');
    setValidationErrors([]);
    const res = await renderGuideRemote(spec);
    setIsBusy(false);
    if (!res.ok) {
      setRenderStatus(`Render failed: ${res.error.message}`);
      return;
    }
    setRenderedPngB64(res.data.pngBase64);
    setRenderStatus(
      `Rendered ${res.data.variant} (${String(res.data.width)}×${String(res.data.height)} px) in Rust core.`,
    );
  };

  const handleResetDefault = (): void => {
    syncSpec(createDefaultGuideSpec(spec.variant));
    setRenderedPngB64(null);
    setRenderStatus(null);
  };

  const stageState: AsyncViewState<GuideSpec> = stageDisabled
    ? {
        status: 'disabled',
        reason: 'Canvas stage preview paused by user toggle.',
      }
    : { status: 'ready', data: spec };

  const runs = workspace?.runs ?? [];
  const backgroundImageUrl =
    spec.variant === 'OverlayGuide' ? resolveTargetAssetUrl(spec.targetImagePath, runs) : null;

  return (
    <div class="workspace-panel">
      <div class="panel-toolbar">
        <div>
          <h2 class="panel-title">SpatialGuide Studio</h2>
          <p class="panel-subtitle">
            Drag <code>SubjectMannequin</code>s, <code>DepthBand</code> dividers,{' '}
            <code>ActionVector</code> handles, or paint a PNG mask, then render via Rust{' '}
            <code>GuideRenderer</code>.
          </p>
        </div>
        <div class="control-row">
          <div class="control-group">
            <label class="field-label" for="guidespec-variant-select">
              SpatialGuide Variant
            </label>
            <select
              id="guidespec-variant-select"
              class="select-input"
              value={spec.variant}
              onChange={(event) => {
                const value = event.currentTarget.value;
                if (isSpatialGuideVariant(value)) {
                  handleVariantSwitch(value);
                }
              }}
            >
              <option value="Blockout">Blockout</option>
              <option value="OverlayGuide">OverlayGuide</option>
            </select>
          </div>

          <div class="control-group">
            <label class="field-label" for="guidespec-tool-select">
              Canvas Tool
            </label>
            <select
              id="guidespec-tool-select"
              class="select-input"
              value={toolMode}
              onChange={(event) => {
                if (isStageToolMode(event.currentTarget.value)) {
                  setToolMode(event.currentTarget.value);
                }
              }}
            >
              <option value="select">Select &amp; Drag Nodes</option>
              <option value="draw_vector">Draw ActionVector</option>
              <option value="paint_mask">Paint PatchEdit Mask</option>
            </select>
          </div>

          {toolMode === 'paint_mask' && (
            <div class="control-group">
              <label class="field-label" for="mask-brush-size">
                Brush Radius ({brushRadiusPx} px)
              </label>
              <input
                id="mask-brush-size"
                type="range"
                min={4}
                max={64}
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

          <label class="checkbox-label">
            <input
              type="checkbox"
              checked={showGrid}
              onChange={(event) => {
                setShowGrid(event.currentTarget.checked);
              }}
            />
            <span>Show Grid</span>
          </label>

          <label class="checkbox-label">
            <input
              type="checkbox"
              checked={stageDisabled}
              onChange={(event) => {
                setStageDisabled(event.currentTarget.checked);
              }}
            />
            <span>Pause Stage</span>
          </label>
        </div>
      </div>

      {spec.variant === 'OverlayGuide' && runs.length > 0 && (
        <div class="control-row">
          <div class="control-group">
            <label class="field-label" for="overlay-run-target">
              Bind OverlayGuide Target Run
            </label>
            <select
              id="overlay-run-target"
              class="select-input"
              value={spec.targetImagePath}
              onChange={(e) => {
                const chosen = e.currentTarget.value;
                const found = runs.find((r) => `runs/${r.slug}/${r.versionLabel}.png` === chosen);
                syncSpec({
                  ...spec,
                  targetImagePath: chosen,
                  canvasWidth: found?.widthPx ?? spec.canvasWidth,
                  canvasHeight: found?.heightPx ?? spec.canvasHeight,
                });
              }}
            >
              {runs.map((r) => {
                const relPath = `runs/${r.slug}/${r.versionLabel}.png`;
                return (
                  <option key={r.runId} value={relPath}>
                    {r.slug} {r.versionLabel} ({r.widthPx}×{r.heightPx})
                  </option>
                );
              })}
            </select>
          </div>
        </div>
      )}

      {renderStatus !== null && (
        <div class="notice-banner" role="status">
          {renderStatus}
        </div>
      )}

      <div class="studio-layout">
        <div class="studio-layout__canvas">
          {stageState.status !== 'ready' ? (
            <StatePanel state={stageState} />
          ) : (
            <StageCanvas
              spec={stageState.data}
              showGrid={showGrid}
              toolMode={toolMode}
              brushRadiusPx={brushRadiusPx}
              backgroundImageUrl={backgroundImageUrl}
              onSpecChange={syncSpec}
              onMaskExport={(b64) => {
                onMaskCaptured(b64, false);
              }}
            />
          )}

          {exportedMaskBase64 !== null && (
            <div class="card">
              <header class="card__header">
                <h3>Exported PNG Mask Ready</h3>
                <button
                  type="button"
                  class="btn btn--primary"
                  onClick={() => {
                    onMaskCaptured(exportedMaskBase64, true);
                  }}
                >
                  Open in PatchEdit Cropper
                </button>
              </header>
              <img
                src={`data:image/png;base64,${exportedMaskBase64}`}
                alt="Exported PatchEdit PNG mask preview"
                class="mask-preview-thumb"
              />
            </div>
          )}

          {renderedPngB64 !== null && (
            <div class="card">
              <header class="card__header">
                <h3>Rust GuideRenderer PNG Preview</h3>
                <span class="badge badge--ready">{spec.variant}</span>
              </header>
              <img
                src={`data:image/png;base64,${renderedPngB64}`}
                alt="Rust GuideRenderer PNG output"
                class="rendered-guide-preview"
              />
            </div>
          )}
        </div>

        <div class="studio-layout__inspector">
          <div class="card">
            <header class="card__header">
              <h3>GuideSpec JSON Schema</h3>
              <span class="badge badge--ready">v{spec.schemaVersion}</span>
            </header>

            <label class="field-label" for="guidespec-json-editor">
              GuideSpec Payload
            </label>
            <textarea
              id="guidespec-json-editor"
              class="code-editor"
              rows={18}
              value={jsonDraft}
              onInput={(event) => {
                onJsonDraftChange(event.currentTarget.value);
              }}
            />

            {validationErrors.length > 0 && (
              <div class="validation-errors" role="alert">
                <strong>Validation failed ({validationErrors.length}):</strong>
                <ul>
                  {validationErrors.map((err) => (
                    <li key={`${err.path}:${err.message}`}>
                      <code>{err.path}</code>: {err.message}
                    </li>
                  ))}
                </ul>
              </div>
            )}

            <div class="button-row">
              <button
                type="button"
                class="btn btn--secondary"
                disabled={isBusy}
                onClick={() => {
                  void handleApplyJson();
                }}
              >
                Validate &amp; Apply
              </button>
              <button
                type="button"
                class="btn btn--primary"
                disabled={isBusy}
                onClick={() => {
                  void handleRenderRustGuide();
                }}
              >
                Render Guide PNG
              </button>
              <button
                type="button"
                class="btn btn--ghost"
                disabled={isBusy}
                onClick={handleResetDefault}
              >
                Reset Default
              </button>
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}
