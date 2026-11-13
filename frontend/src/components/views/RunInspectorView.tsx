/** === RUN INSPECTOR VIEW ===
 *
 * PURPOSE:
 * Displays versioned `Run` (`vNNN`) records, five-state `Lifecycle` progression
 * (`draft` -> `candidate` -> `validated` -> `canonical` -> `superseded`),
 * `Visibility` classification, `VisionCritic` + OKLab/`DeltaE_2000` QA status,
 * and `Export` gating.
 *
 * DESIGN RATIONALE ("WHY"):
 * Enforces `GLOSSARY.md` lifecycle immutability and `Visibility` boundaries:
 * `Export` actions remain disabled until a `Run` reaches `validated` or
 * `canonical`, and `canonical` runs cannot regress to earlier states.
 */

import type { JSX } from 'preact';
import { useState } from 'preact/hooks';
import { LIFECYCLE_ORDER, isExportEligible, isLifecycleState } from '../../state/workspace';
import type { AsyncViewState, LifecycleState, RunRecord } from '../../types/domain';
import { StatePanel } from '../StatePanel';

const INITIAL_RUNS: readonly RunRecord[] = [
  {
    runId: 'run-20261113-v001',
    versionLabel: 'v001',
    lifecycle: 'draft',
    visibility: 'public',
    widthPx: 1024,
    heightPx: 1024,
    visionCriticPassed: null,
    colorQaMaxDeltaE2000: 4.2,
    createdAtIso: '2026-11-13T00:15:00Z',
  },
  {
    runId: 'run-20261113-v002',
    versionLabel: 'v002',
    lifecycle: 'validated',
    visibility: 'restricted',
    widthPx: 1024,
    heightPx: 1024,
    visionCriticPassed: true,
    colorQaMaxDeltaE2000: 1.8,
    createdAtIso: '2026-11-13T00:30:00Z',
  },
];

/** `Run Inspector` tab view. */
export function RunInspectorView(): JSX.Element {
  const [runs, setRuns] = useState<readonly RunRecord[]>(INITIAL_RUNS);
  const [selectedRunId, setSelectedRunId] = useState<string>(INITIAL_RUNS[0]?.runId ?? '');

  const selectedRun = runs.find((run) => run.runId === selectedRunId) ?? null;

  const viewState: AsyncViewState<RunRecord> =
    runs.length === 0 || selectedRun === null
      ? {
          status: 'empty',
          title: 'No Run records available',
          detail: 'Execute a generation or PatchEdit Run to inspect vNNN metadata and QA gates.',
        }
      : { status: 'ready', data: selectedRun };

  const handleLifecycleChange = (runId: string, nextLifecycle: LifecycleState): void => {
    setRuns((prev) =>
      prev.map((entry) =>
        entry.runId === runId && entry.lifecycle !== 'canonical'
          ? { ...entry, lifecycle: nextLifecycle }
          : entry,
      ),
    );
  };

  return (
    <div class="workspace-panel">
      <div class="panel-toolbar">
        <div>
          <h2 class="panel-title">Run Inspector</h2>
          <p class="panel-subtitle">
            Inspect versioned <code>Run</code> (<code>vNNN</code>) records, <code>Lifecycle</code>{' '}
            transitions, <code>VisionCritic</code> results, and <code>Export</code> gates.
          </p>
        </div>
        <div class="control-group">
          <label class="field-label" for="run-selector">
            Active Run
          </label>
          <select
            id="run-selector"
            class="select-input"
            value={selectedRunId}
            disabled={runs.length === 0}
            onChange={(event) => {
              setSelectedRunId(event.currentTarget.value);
            }}
          >
            {runs.map((run) => (
              <option key={run.runId} value={run.runId}>
                {run.versionLabel} ({run.lifecycle} · {run.visibility})
              </option>
            ))}
          </select>
        </div>
      </div>

      {viewState.status !== 'ready' ? (
        <StatePanel
          state={viewState}
          emptyActionLabel="Restore sample Runs"
          onEmptyAction={() => {
            setRuns(INITIAL_RUNS);
            setSelectedRunId(INITIAL_RUNS[0]?.runId ?? '');
          }}
        />
      ) : (
        <div class="card-grid">
          <article class="card">
            <header class="card__header">
              <h3>
                Run {viewState.data.versionLabel} ({viewState.data.runId})
              </h3>
              <span class="badge badge--ready">{viewState.data.lifecycle}</span>
            </header>

            <dl class="meta-list">
              <div>
                <dt>Dimensions</dt>
                <dd>
                  {viewState.data.widthPx}×{viewState.data.heightPx} px
                </dd>
              </div>
              <div>
                <dt>Visibility</dt>
                <dd>{viewState.data.visibility}</dd>
              </div>
              <div>
                <dt>VisionCritic</dt>
                <dd>
                  {viewState.data.visionCriticPassed === null
                    ? 'Pending'
                    : viewState.data.visionCriticPassed
                      ? 'Passed'
                      : 'Failed'}
                </dd>
              </div>
              <div>
                <dt>Color QA Max DeltaE_2000</dt>
                <dd>
                  {viewState.data.colorQaMaxDeltaE2000 === null
                    ? 'Not measured'
                    : viewState.data.colorQaMaxDeltaE2000.toFixed(2)}
                </dd>
              </div>
            </dl>

            <div class="control-group">
              <label class="field-label" for="run-lifecycle-select">
                Lifecycle State
              </label>
              <select
                id="run-lifecycle-select"
                class="select-input"
                value={viewState.data.lifecycle}
                disabled={viewState.data.lifecycle === 'canonical'}
                onChange={(event) => {
                  const value = event.currentTarget.value;
                  if (isLifecycleState(value)) {
                    handleLifecycleChange(viewState.data.runId, value);
                  }
                }}
              >
                {LIFECYCLE_ORDER.map((state) => (
                  <option key={state} value={state}>
                    {state}
                  </option>
                ))}
              </select>
            </div>
          </article>

          <article class="card">
            <header class="card__header">
              <h3>Export Deliverables</h3>
              <span class="badge badge--ready">
                {isExportEligible(viewState.data.lifecycle) ? 'Unlocked' : 'Gated'}
              </span>
            </header>
            <p class="panel-subtitle">
              Exports derive deterministic deliverables without mutating canonical asset versions.
            </p>
            <div class="button-row">
              <button
                type="button"
                class="btn btn--primary"
                disabled={!isExportEligible(viewState.data.lifecycle)}
              >
                Export Deliverable
              </button>
            </div>
            {!isExportEligible(viewState.data.lifecycle) && (
              <p class="muted-note">
                Advance Lifecycle to <code>validated</code> or <code>canonical</code> to unlock
                Export.
              </p>
            )}
          </article>
        </div>
      )}
    </div>
  );
}
