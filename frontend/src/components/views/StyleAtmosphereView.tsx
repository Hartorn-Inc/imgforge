/** === STYLE & ATMOSPHERE VIEW ===
 *
 * PURPOSE:
 * Renders the `Style & Atmosphere` workspace tab for inspecting `Subject`
 * `PaletteLock` swatches, `Style` exemplars, `Atmosphere` parameters, and
 * style-scoped `ReferencePack` inventory.
 *
 * DESIGN RATIONALE ("WHY"):
 * Enforces ADR 0003 separation: `Subject` identity stays style-neutral while
 * `ReferencePack` sheets are scoped per `(Subject, Style)` pair. Supports
 * explicit previewing of `ready`, `loading`, `empty`, `error`, and `disabled`
 * view states during Phase 1 before live library endpoints are wired.
 */

import type { JSX } from 'preact';
import { useState } from 'preact/hooks';
import type {
  AsyncViewState,
  AtmosphereSummary,
  ReferencePackSummary,
  StyleSummary,
  SubjectSummary,
} from '../../types/domain';
import { StatePanel } from '../StatePanel';

interface StyleAtmospherePayload {
  readonly subject: SubjectSummary;
  readonly style: StyleSummary;
  readonly atmosphere: AtmosphereSummary;
  readonly referencePack: ReferencePackSummary;
}

const SAMPLE_PAYLOAD: StyleAtmospherePayload = {
  subject: {
    slug: 'subject-scout',
    name: 'Field Scout',
    category: 'character',
    heightMeters: 1.75,
    visibility: 'public',
    lifecycle: 'validated',
    paletteLock: [
      { label: 'Coat Base', hex: '#3B4D63', surfaceSlot: 'outer_garment' },
      { label: 'Leather Trim', hex: '#6E4F3A', surfaceSlot: 'belt_straps' },
      { label: 'Brass Clasp', hex: '#C79B42', surfaceSlot: 'hardware' },
    ],
  },
  style: {
    slug: 'clean-cel-ink',
    name: 'Clean Cel Ink',
    medium: 'Crisp variable-weight ink linework with flat shaded fills',
    exemplarCount: 2,
    lifecycle: 'canonical',
  },
  atmosphere: {
    slug: 'overcast-daylight',
    name: 'Overcast Daylight',
    lightDirection: 'Top-left 45deg diffuse key',
    colorTemperatureKelvin: 6200,
    exposureFloor: 0.14,
    maxAmbientTintBlend: 0.18,
    particles: 'None',
  },
  referencePack: {
    subjectSlug: 'subject-scout',
    styleSlug: 'clean-cel-ink',
    sheetKind: 'turnaround',
    sliceCount: 6,
    lifecycle: 'validated',
  },
};

type StatePreviewMode = 'ready' | 'loading' | 'empty' | 'error' | 'disabled';

function buildViewState(mode: StatePreviewMode): AsyncViewState<StyleAtmospherePayload> {
  switch (mode) {
    case 'ready':
      return { status: 'ready', data: SAMPLE_PAYLOAD };
    case 'loading':
      return {
        status: 'loading',
        message: 'Loading Subject, Style, Atmosphere, and ReferencePack catalog...',
      };
    case 'empty':
      return {
        status: 'empty',
        title: 'No Style or Atmosphere loaded',
        detail:
          'Initialize a Subject, Style, and Atmosphere in your workspace to inspect PaletteLock swatches and style-scoped ReferencePack sheets.',
      };
    case 'error':
      return {
        status: 'error',
        message: 'Failed to load ReferencePack metadata from local imgforge server.',
        retryable: true,
      };
    case 'disabled':
      return {
        status: 'disabled',
        reason: 'Style & Atmosphere editing is disabled while an active Run is locked.',
      };
  }
}

/** `Style & Atmosphere` tab view. */
export function StyleAtmosphereView(): JSX.Element {
  const [previewMode, setPreviewMode] = useState<StatePreviewMode>('ready');
  const viewState = buildViewState(previewMode);

  return (
    <div class="workspace-panel">
      <div class="panel-toolbar">
        <div>
          <h2 class="panel-title">Style &amp; Atmosphere</h2>
          <p class="panel-subtitle">
            Style-neutral Subject identity, Style exemplars, Atmosphere parameters, and style-scoped
            ReferencePack sheets.
          </p>
        </div>
        <div class="control-group">
          <label class="field-label" for="style-state-select">
            View State
          </label>
          <select
            id="style-state-select"
            class="select-input"
            value={previewMode}
            onChange={(event) => {
              setPreviewMode(event.currentTarget.value as StatePreviewMode);
            }}
          >
            <option value="ready">Ready (Sample Catalog)</option>
            <option value="loading">Loading</option>
            <option value="empty">Empty</option>
            <option value="error">Error</option>
            <option value="disabled">Disabled</option>
          </select>
        </div>
      </div>

      {viewState.status !== 'ready' ? (
        <StatePanel
          state={viewState}
          onRetry={() => {
            setPreviewMode('ready');
          }}
          emptyActionLabel="Load sample catalog"
          onEmptyAction={() => {
            setPreviewMode('ready');
          }}
        />
      ) : (
        <div class="card-grid">
          <article class="card">
            <header class="card__header">
              <h3>Subject &amp; PaletteLock</h3>
              <span class="badge badge--ready">{viewState.data.subject.lifecycle}</span>
            </header>
            <dl class="meta-list">
              <div>
                <dt>Name</dt>
                <dd>{viewState.data.subject.name}</dd>
              </div>
              <div>
                <dt>Category</dt>
                <dd>{viewState.data.subject.category}</dd>
              </div>
              <div>
                <dt>Physical Height</dt>
                <dd>{viewState.data.subject.heightMeters.toFixed(2)} m</dd>
              </div>
              <div>
                <dt>Visibility</dt>
                <dd>{viewState.data.subject.visibility}</dd>
              </div>
            </dl>
            <ul class="swatch-list" aria-label="PaletteLock swatches">
              {viewState.data.subject.paletteLock.map((swatch) => (
                <li key={swatch.surfaceSlot} class="swatch-item">
                  <span
                    class="swatch-chip"
                    style={{ backgroundColor: swatch.hex }}
                    aria-hidden="true"
                  />
                  <span>
                    <strong>{swatch.label}</strong> ({swatch.hex}) · {swatch.surfaceSlot}
                  </span>
                </li>
              ))}
            </ul>
          </article>

          <article class="card">
            <header class="card__header">
              <h3>Style</h3>
              <span class="badge badge--ready">{viewState.data.style.lifecycle}</span>
            </header>
            <dl class="meta-list">
              <div>
                <dt>Name</dt>
                <dd>{viewState.data.style.name}</dd>
              </div>
              <div>
                <dt>Medium</dt>
                <dd>{viewState.data.style.medium}</dd>
              </div>
              <div>
                <dt>Exemplars</dt>
                <dd>{viewState.data.style.exemplarCount} / 3</dd>
              </div>
            </dl>
          </article>

          <article class="card">
            <header class="card__header">
              <h3>Atmosphere</h3>
              <span class="badge badge--ready">
                {viewState.data.atmosphere.colorTemperatureKelvin} K
              </span>
            </header>
            <dl class="meta-list">
              <div>
                <dt>Name</dt>
                <dd>{viewState.data.atmosphere.name}</dd>
              </div>
              <div>
                <dt>Light Direction</dt>
                <dd>{viewState.data.atmosphere.lightDirection}</dd>
              </div>
              <div>
                <dt>Exposure Floor</dt>
                <dd>{viewState.data.atmosphere.exposureFloor.toFixed(2)}</dd>
              </div>
              <div>
                <dt>Max Ambient Tint</dt>
                <dd>{viewState.data.atmosphere.maxAmbientTintBlend.toFixed(2)}</dd>
              </div>
            </dl>
          </article>

          <article class="card">
            <header class="card__header">
              <h3>ReferencePack</h3>
              <span class="badge badge--ready">{viewState.data.referencePack.lifecycle}</span>
            </header>
            <dl class="meta-list">
              <div>
                <dt>Scope</dt>
                <dd>
                  ({viewState.data.referencePack.subjectSlug},{' '}
                  {viewState.data.referencePack.styleSlug})
                </dd>
              </div>
              <div>
                <dt>Sheet Kind</dt>
                <dd>{viewState.data.referencePack.sheetKind}</dd>
              </div>
              <div>
                <dt>Detected Slices</dt>
                <dd>{viewState.data.referencePack.sliceCount}</dd>
              </div>
            </dl>
          </article>
        </div>
      )}
    </div>
  );
}
