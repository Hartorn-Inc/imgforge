/** === SPATIALGUIDE STUDIO VIEW ===
 *
 * PURPOSE:
 * Hosts the live Konva `StageCanvas` and defensive `GuideSpec` JSON editor
 * for authoring `OverlayGuide` and `Blockout` `SpatialGuide` specifications.
 *
 * DESIGN RATIONALE ("WHY"):
 * Any syntax error or geometric violation in `GuideSpec` JSON is caught
 * immediately by `parseGuideSpecJson()` before updating the live Konva stage
 * or submitting to Rust `GuideRenderer` (`tiny-skia`/`resvg`).
 */

import type { JSX } from 'preact';
import { useState } from 'preact/hooks';
import type { AsyncViewState } from '../../types/domain';
import {
  type GuideSpec,
  type GuideSpecValidationError,
  type SpatialGuideVariant,
  createDefaultGuideSpec,
  parseGuideSpecJson,
  serializeGuideSpecJson,
} from '../../types/guideSpec';
import { StageCanvas } from '../StageCanvas';
import { StatePanel } from '../StatePanel';

function isSpatialGuideVariant(value: string): value is SpatialGuideVariant {
  return value === 'Blockout' || value === 'OverlayGuide';
}

/** `SpatialGuide Studio` tab view. */
export function SpatialGuideView(): JSX.Element {
  const [spec, setSpec] = useState<GuideSpec>(() => createDefaultGuideSpec('Blockout'));
  const [jsonDraft, setJsonDraft] = useState<string>(() =>
    serializeGuideSpecJson(createDefaultGuideSpec('Blockout')),
  );
  const [showGrid, setShowGrid] = useState<boolean>(true);
  const [stageDisabled, setStageDisabled] = useState<boolean>(false);
  const [validationErrors, setValidationErrors] = useState<readonly GuideSpecValidationError[]>([]);

  const handleVariantSwitch = (variant: SpatialGuideVariant): void => {
    const nextSpec = createDefaultGuideSpec(variant);
    setSpec(nextSpec);
    setJsonDraft(serializeGuideSpecJson(nextSpec));
    setValidationErrors([]);
  };

  const handleApplyJson = (): void => {
    const result = parseGuideSpecJson(jsonDraft);
    if (!result.ok) {
      setValidationErrors(result.errors);
      return;
    }
    setSpec(result.value);
    setJsonDraft(serializeGuideSpecJson(result.value));
    setValidationErrors([]);
  };

  const handleResetDefault = (): void => {
    const reset = createDefaultGuideSpec(spec.variant);
    setSpec(reset);
    setJsonDraft(serializeGuideSpecJson(reset));
    setValidationErrors([]);
  };

  const stageState: AsyncViewState<GuideSpec> = stageDisabled
    ? {
        status: 'disabled',
        reason: 'Canvas stage preview paused by user toggle.',
      }
    : { status: 'ready', data: spec };

  return (
    <div class="workspace-panel">
      <div class="panel-toolbar">
        <div>
          <h2 class="panel-title">SpatialGuide Studio</h2>
          <p class="panel-subtitle">
            Author <code>GuideSpec</code> JSON for <code>Blockout</code> and{' '}
            <code>OverlayGuide</code> rendering via Rust <code>GuideRenderer</code>.
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

      <div class="studio-layout">
        <div class="studio-layout__canvas">
          {stageState.status !== 'ready' ? (
            <StatePanel state={stageState} />
          ) : (
            <StageCanvas spec={stageState.data} showGrid={showGrid} />
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
              rows={16}
              value={jsonDraft}
              onInput={(event) => {
                setJsonDraft(event.currentTarget.value);
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
              <button type="button" class="btn btn--primary" onClick={handleApplyJson}>
                Apply GuideSpec
              </button>
              <button type="button" class="btn btn--secondary" onClick={handleResetDefault}>
                Reset Variant Default
              </button>
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}
