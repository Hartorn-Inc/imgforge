/** === EXPLICIT ASYNC STATE PANEL ===
 *
 * PURPOSE:
 * Renders uniform, accessible `loading`, `empty`, `error`, and `disabled` states
 * across every data view in the `imgforge` Web Canvas SPA.
 *
 * DESIGN RATIONALE ("WHY"):
 * Enforcing a shared component guarantees consistent ARIA announcements
 * (`role="status"` vs `role="alert"`), keyboard-reachable retry actions, and
 * zero blank panels when the local Rust server or workspace data is absent.
 */

import type { JSX } from 'preact';
import type { AsyncViewState } from '../types/domain';
import { describeAsyncState } from '../state/workspace';

export interface StatePanelProps<T> {
  readonly state: Exclude<AsyncViewState<T>, { readonly status: 'ready' }>;
  readonly onRetry?: () => void;
  readonly emptyActionLabel?: string;
  readonly onEmptyAction?: () => void;
}

/** Accessible fallback panel for non-ready `AsyncViewState<T>` variants. */
export function StatePanel<T>({
  state,
  onRetry,
  emptyActionLabel,
  onEmptyAction,
}: StatePanelProps<T>): JSX.Element {
  const descriptor = describeAsyncState(state);

  return (
    <section
      class={`state-panel state-panel--${state.status}`}
      role={descriptor.ariaRole ?? 'status'}
      aria-live={state.status === 'error' ? 'assertive' : 'polite'}
    >
      <div class="state-panel__header">
        <span class={`badge badge--${state.status}`}>{descriptor.badge}</span>
      </div>

      {state.status === 'loading' && <p class="state-panel__message">{state.message}</p>}

      {state.status === 'empty' && (
        <div class="state-panel__body">
          <h3 class="state-panel__title">{state.title}</h3>
          <p class="state-panel__message">{state.detail}</p>
          {emptyActionLabel && onEmptyAction && (
            <button type="button" class="btn btn--secondary" onClick={onEmptyAction}>
              {emptyActionLabel}
            </button>
          )}
        </div>
      )}

      {state.status === 'error' && (
        <div class="state-panel__body">
          <p class="state-panel__message">{state.message}</p>
          {state.retryable && onRetry && (
            <button type="button" class="btn btn--primary" onClick={onRetry}>
              Retry request
            </button>
          )}
        </div>
      )}

      {state.status === 'disabled' && (
        <div class="state-panel__body">
          <p class="state-panel__message">{state.reason}</p>
        </div>
      )}
    </section>
  );
}
