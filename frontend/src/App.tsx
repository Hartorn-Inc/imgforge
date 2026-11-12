/** === IMGFORGE WEB CANVAS APPLICATION SHELL ===
 *
 * PURPOSE:
 * Top-level Preact shell coordinating workspace tab navigation (`Style &
 * Atmosphere`, `SpatialGuide Studio`, `PatchEdit Cropper`, `Run Inspector`)
 * and `GET /api/health` polling against the embedded `127.0.0.1:<port>` server.
 *
 * DESIGN RATIONALE ("WHY"):
 * Implements WAI-ARIA roving-tabindex keyboard navigation across workspace views
 * and renders explicit health states without blocking offline UI inspection.
 */

import type { JSX } from 'preact';
import { useCallback, useEffect, useRef, useState } from 'preact/hooks';
import { fetchHealthStatus } from './api/health';
import { HealthBadge } from './components/HealthBadge';
import { PatchEditView } from './components/views/PatchEditView';
import { RunInspectorView } from './components/views/RunInspectorView';
import { SpatialGuideView } from './components/views/SpatialGuideView';
import { StyleAtmosphereView } from './components/views/StyleAtmosphereView';
import { WORKSPACE_TABS, resolveNextTabForKey } from './state/workspace';
import type { AsyncViewState, HealthStatus, WorkspaceTab } from './types/domain';

/** Root Web Canvas SPA component. */
export function App(): JSX.Element {
  const [activeTab, setActiveTab] = useState<WorkspaceTab>('spatial_guide');
  const [healthState, setHealthState] = useState<AsyncViewState<HealthStatus>>({
    status: 'loading',
    message: 'Checking GET /api/health...',
  });
  const tabRefs = useRef<Partial<Record<WorkspaceTab, HTMLButtonElement | null>>>({});

  const refreshHealth = useCallback((signal?: AbortSignal) => {
    setHealthState({ status: 'loading', message: 'Checking GET /api/health...' });
    void fetchHealthStatus(signal).then((result) => {
      if (signal?.aborted) {
        return;
      }
      if (result.ok) {
        setHealthState({ status: 'ready', data: result.health });
      } else {
        setHealthState({
          status: 'error',
          message: result.error,
          retryable: true,
        });
      }
    });
  }, []);

  useEffect(() => {
    const controller = new AbortController();
    refreshHealth(controller.signal);
    return () => {
      controller.abort();
    };
  }, [refreshHealth]);

  const handleTabKeyDown = (event: KeyboardEvent): void => {
    const nextTab = resolveNextTabForKey(activeTab, event.key);
    if (nextTab === null) {
      return;
    }
    event.preventDefault();
    setActiveTab(nextTab);
    tabRefs.current[nextTab]?.focus();
  };

  return (
    <div class="app-shell">
      <a class="skip-link" href="#workspace-main">
        Skip to workspace content
      </a>

      <header class="app-header">
        <div class="app-brand">
          <h1 class="app-brand__title">imgforge</h1>
          <span class="app-brand__subtitle">Visual Consistency Engine · Web Canvas SPA</span>
        </div>
        <HealthBadge
          state={healthState}
          onRefresh={() => {
            refreshHealth();
          }}
        />
      </header>

      <nav class="app-nav" aria-label="Workspace sections">
        <div class="tablist" role="tablist" aria-label="imgforge workspace views">
          {WORKSPACE_TABS.map((tab) => {
            const isSelected = activeTab === tab.id;
            return (
              <button
                key={tab.id}
                ref={(el) => {
                  tabRefs.current[tab.id] = el;
                }}
                type="button"
                role="tab"
                id={`tab-${tab.id}`}
                aria-selected={isSelected}
                aria-controls={`panel-${tab.id}`}
                tabIndex={isSelected ? 0 : -1}
                class={`tab-button${isSelected ? ' tab-button--active' : ''}`}
                onClick={() => {
                  setActiveTab(tab.id);
                }}
                onKeyDown={handleTabKeyDown}
              >
                {tab.label}
              </button>
            );
          })}
        </div>
      </nav>

      <main id="workspace-main" class="app-main">
        <div
          id={`panel-${activeTab}`}
          role="tabpanel"
          aria-labelledby={`tab-${activeTab}`}
          tabIndex={0}
          class="tabpanel"
        >
          {activeTab === 'style_atmosphere' && <StyleAtmosphereView />}
          {activeTab === 'spatial_guide' && <SpatialGuideView />}
          {activeTab === 'patch_edit' && <PatchEditView />}
          {activeTab === 'run_inspector' && <RunInspectorView />}
        </div>
      </main>
    </div>
  );
}
