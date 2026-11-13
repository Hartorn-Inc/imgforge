/** === IMGFORGE WEB CANVAS APPLICATION SHELL ===
 *
 * PURPOSE:
 * Top-level Preact shell coordinating workspace tab navigation (`Style &
 * Atmosphere`, `SpatialGuide Studio`, `PatchEdit Cropper`, `Run Inspector`),
 * shared `GET /api/workspace` catalog loading, `GET /api/health` polling,
 * and live `GET /api/ws` `WorkspaceEvent` auto-refresh.
 *
 * DESIGN RATIONALE ("WHY"):
 * Lifting `workspaceState`, `GuideSpec` draft, `PatchEditDraft`, and selected
 * `Run` ID above the tabpanel switcher guarantees user drafts survive tab
 * switches and WebSocket updates keep all panels synchronized.
 */

import type { JSX } from 'preact';
import { useCallback, useEffect, useRef, useState } from 'preact/hooks';
import { type WsConnectionStatus, connectWorkspaceEvents, fetchWorkspace } from './api/client';
import { fetchHealthStatus } from './api/health';
import { HealthBadge } from './components/HealthBadge';
import { PatchEditView } from './components/views/PatchEditView';
import { RunInspectorView } from './components/views/RunInspectorView';
import { SpatialGuideView } from './components/views/SpatialGuideView';
import { StyleAtmosphereView } from './components/views/StyleAtmosphereView';
import {
  WORKSPACE_TABS,
  clampPatchEditDraft,
  createDefaultPatchEditDraft,
  resolveNextTabForKey,
} from './state/workspace';
import type {
  AsyncViewState,
  HealthStatus,
  PatchEditDraft,
  WorkspaceEvent,
  WorkspaceSummary,
  WorkspaceTab,
} from './types/domain';
import { type GuideSpec, createDefaultGuideSpec, serializeGuideSpecJson } from './types/guideSpec';

/** Root Web Canvas SPA component. */
export function App(): JSX.Element {
  const [activeTab, setActiveTab] = useState<WorkspaceTab>('style_atmosphere');
  const [healthState, setHealthState] = useState<AsyncViewState<HealthStatus>>({
    status: 'loading',
    message: 'Checking GET /api/health...',
  });
  const [workspaceState, setWorkspaceState] = useState<AsyncViewState<WorkspaceSummary>>({
    status: 'loading',
    message: 'Loading GET /api/workspace...',
  });
  const [wsStatus, setWsStatus] = useState<WsConnectionStatus>('connecting');
  const [lastWsEvent, setLastWsEvent] = useState<WorkspaceEvent | null>(null);

  // Lifted shared drafts so switching tabs preserves user work.
  const [guideSpec, setGuideSpec] = useState<GuideSpec>(() => createDefaultGuideSpec('Blockout'));
  const [guideJsonDraft, setGuideJsonDraft] = useState<string>(() =>
    serializeGuideSpecJson(createDefaultGuideSpec('Blockout')),
  );
  const [patchDraft, setPatchDraft] = useState<PatchEditDraft>(() =>
    createDefaultPatchEditDraft(null),
  );
  const [selectedRunId, setSelectedRunId] = useState<string>('');

  const tabRefs = useRef<Partial<Record<WorkspaceTab, HTMLButtonElement | null>>>({});
  const healthReqSeqRef = useRef<number>(0);
  const workspaceReqSeqRef = useRef<number>(0);

  const refreshHealth = useCallback((signal?: AbortSignal): Promise<void> => {
    const seq = ++healthReqSeqRef.current;
    setHealthState({ status: 'loading', message: 'Checking GET /api/health...' });
    return fetchHealthStatus(signal).then((result) => {
      if (signal?.aborted || seq !== healthReqSeqRef.current) {
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

  const refreshWorkspace = useCallback((signal?: AbortSignal): Promise<void> => {
    const seq = ++workspaceReqSeqRef.current;
    setWorkspaceState((prev) =>
      prev.status === 'ready'
        ? prev
        : { status: 'loading', message: 'Loading workspace catalog from GET /api/workspace...' },
    );
    return fetchWorkspace(signal !== undefined ? { signal } : {}).then((result) => {
      if (signal?.aborted || seq !== workspaceReqSeqRef.current) {
        return;
      }
      if (!result.ok) {
        setWorkspaceState({
          status: 'error',
          message: result.error.message,
          retryable: true,
        });
        return;
      }
      setWorkspaceState({ status: 'ready', data: result.data });
      const firstRun = result.data.runs[0];
      if (firstRun !== undefined) {
        setSelectedRunId((prevId) => (prevId === '' ? firstRun.runId : prevId));
        setPatchDraft((prevDraft) =>
          prevDraft.targetRunKey === null && prevDraft.targetImagePath === ''
            ? createDefaultPatchEditDraft(firstRun)
            : prevDraft,
        );
      }
    });
  }, []);

  useEffect(() => {
    const controller = new AbortController();
    void refreshHealth(controller.signal);
    void refreshWorkspace(controller.signal);
    return () => {
      controller.abort();
    };
  }, [refreshHealth, refreshWorkspace]);

  useEffect(() => {
    const dispose = connectWorkspaceEvents({
      onStatusChange: setWsStatus,
      onEvent: (event) => {
        setLastWsEvent(event);
        void refreshWorkspace();
      },
    });
    return dispose;
  }, [refreshWorkspace]);

  const handleTabKeyDown = (event: KeyboardEvent): void => {
    const nextTab = resolveNextTabForKey(activeTab, event.key);
    if (nextTab === null) {
      return;
    }
    event.preventDefault();
    setActiveTab(nextTab);
    tabRefs.current[nextTab]?.focus();
  };

  const workspace = workspaceState.status === 'ready' ? workspaceState.data : null;

  return (
    <div class="app-shell">
      <a class="skip-link" href="#workspace-main">
        Skip to workspace content
      </a>

      <header class="app-header">
        <div class="app-brand">
          <h1 class="app-brand__title">imgforge</h1>
          <span class="app-brand__subtitle">
            {workspace?.workspaceName
              ? `Workspace: ${workspace.workspaceName}`
              : 'Visual Consistency Engine · Web Canvas SPA'}
          </span>
        </div>
        <div class="header-status-group">
          <span class="ws-pill" role="status" aria-label={`WebSocket status ${wsStatus}`}>
            WS: {wsStatus}
            {lastWsEvent !== null ? ` · ${lastWsEvent.resourceKind}:${lastWsEvent.eventType}` : ''}
          </span>
          <HealthBadge
            state={healthState}
            onRefresh={() => {
              void refreshHealth();
              void refreshWorkspace();
            }}
          />
        </div>
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
          {activeTab === 'style_atmosphere' && (
            <StyleAtmosphereView
              workspaceState={workspaceState}
              onRefreshWorkspace={() => refreshWorkspace()}
            />
          )}
          {activeTab === 'spatial_guide' && (
            <SpatialGuideView
              spec={guideSpec}
              jsonDraft={guideJsonDraft}
              workspace={workspace}
              exportedMaskBase64={patchDraft.maskPngBase64}
              onSpecUpdate={(nextSpec, nextJson) => {
                setGuideSpec(nextSpec);
                setGuideJsonDraft(nextJson);
              }}
              onJsonDraftChange={setGuideJsonDraft}
              onMaskCaptured={(maskB64, switchToPatchEdit) => {
                setPatchDraft((prev) =>
                  clampPatchEditDraft({
                    ...prev,
                    mode: maskB64 === null ? prev.mode : 'mask',
                    maskPngBase64: maskB64,
                  }),
                );
                if (switchToPatchEdit && maskB64 !== null) {
                  setActiveTab('patch_edit');
                }
              }}
            />
          )}
          {activeTab === 'patch_edit' && (
            <PatchEditView
              workspaceState={workspaceState}
              draft={patchDraft}
              onDraftChange={setPatchDraft}
              onRefreshWorkspace={() => refreshWorkspace()}
              onPatchCompleted={(res) => {
                setSelectedRunId(res.run.runId);
                void refreshWorkspace();
              }}
            />
          )}
          {activeTab === 'run_inspector' && (
            <RunInspectorView
              workspaceState={workspaceState}
              selectedRunId={selectedRunId}
              onSelectRunId={setSelectedRunId}
              onRefreshWorkspace={() => refreshWorkspace()}
              onSendRunToPatchEdit={(run) => {
                setPatchDraft(createDefaultPatchEditDraft(run));
                setActiveTab('patch_edit');
              }}
            />
          )}
        </div>
      </main>
    </div>
  );
}
