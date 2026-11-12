/** === SERVER HEALTH INDICATOR ===
 *
 * PURPOSE:
 * Displays live connection and auth status from `GET /api/health` on the local
 * `127.0.0.1:<port>` Rust server with an accessible manual refresh trigger.
 *
 * DESIGN RATIONALE ("WHY"):
 * Users running the SPA standalone or launching `imgforge` from WSL2 need an
 * immediate indicator showing whether the Rust server and `CodexNativeBackend`
 * auth session are reachable without inspecting terminal logs.
 */

import type { JSX } from 'preact';
import type { AsyncViewState, HealthStatus } from '../types/domain';

export interface HealthBadgeProps {
  readonly state: AsyncViewState<HealthStatus>;
  readonly onRefresh: () => void;
}

/** Header status pill reflecting `GET /api/health`. */
export function HealthBadge({ state, onRefresh }: HealthBadgeProps): JSX.Element {
  let statusText = 'Checking /api/health...';
  let pillModifier = 'loading';

  if (state.status === 'ready') {
    const authLabel = state.data.authProvisioned ? 'Auth Ready' : 'Auth Missing';
    statusText = `${state.data.status.toUpperCase()} · v${state.data.version} · ${state.data.backend} · ${authLabel}`;
    pillModifier = state.data.status === 'ok' ? 'ok' : 'degraded';
  } else if (state.status === 'error') {
    statusText = `Offline (${state.message})`;
    pillModifier = 'error';
  } else if (state.status === 'disabled') {
    statusText = `Disabled (${state.reason})`;
    pillModifier = 'disabled';
  } else if (state.status === 'empty') {
    statusText = state.title;
    pillModifier = 'empty';
  }

  return (
    <div class="health-bar" role="status" aria-live="polite" aria-label="Local server health">
      <span class={`health-pill health-pill--${pillModifier}`}>
        <span class="health-pill__dot" aria-hidden="true" />
        <span>{statusText}</span>
      </span>
      <button
        type="button"
        class="btn btn--ghost"
        onClick={onRefresh}
        disabled={state.status === 'loading'}
        aria-label="Refresh /api/health status"
      >
        Refresh Health
      </button>
    </div>
  );
}
