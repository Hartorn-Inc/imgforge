/** === WORKSPACE NAVIGATION & VIEW STATE HELPERS ===
 *
 * PURPOSE:
 * Centralizes tab metadata, WAI-ARIA keyboard tablist navigation, and explicit
 * `AsyncViewState<T>` presentation descriptors for the four top-level views:
 * `Style & Atmosphere`, `SpatialGuide Studio`, `PatchEdit Cropper`, and
 * `Run Inspector`.
 *
 * DESIGN RATIONALE ("WHY"):
 * Separating pure state transitions and keyboard navigation logic from Preact
 * components keeps UI behavior deterministic and cleanly testable under Vitest.
 *
 * DATA FLOW:
 * 1. `WORKSPACE_TABS` defines ordered tab definitions and glossary-aligned copy.
 * 2. `resolveNextTabForKey()` computes roving focus targets on Arrow/Home/End.
 * 3. `describeAsyncState()` maps `AsyncViewState<T>` into consistent status labels.
 */

import type { AsyncViewState, LifecycleState, WorkspaceTab } from '../types/domain';

/** Metadata describing one top-level workspace tab in the SPA shell. */
export interface WorkspaceTabDefinition {
  readonly id: WorkspaceTab;
  readonly label: string;
  readonly summary: string;
}

/** Ordered top-level tabs matching the embedded Web Canvas workspace layout. */
export const WORKSPACE_TABS: readonly WorkspaceTabDefinition[] = [
  {
    id: 'style_atmosphere',
    label: 'Style & Atmosphere',
    summary:
      'Inspect Subject PaletteLock swatches, Style exemplars, Atmosphere mood, and ReferencePack sheets.',
  },
  {
    id: 'spatial_guide',
    label: 'SpatialGuide Studio',
    summary: 'Author OverlayGuide and Blockout GuideSpec layouts on the live Konva stage.',
  },
  {
    id: 'patch_edit',
    label: 'PatchEdit Cropper',
    summary:
      'Configure 1:1 square ROI or mask crops, blank/cue regions, and collar-ring harmonization.',
  },
  {
    id: 'run_inspector',
    label: 'Run Inspector',
    summary:
      'Review Run (vNNN) Lifecycle, Visibility, VisionCritic checklists, Color QA, and Export actions.',
  },
] as const;

/** Compute the target tab ID for WAI-ARIA `tablist` keyboard navigation keys. */
export function resolveNextTabForKey(currentTab: WorkspaceTab, key: string): WorkspaceTab | null {
  const currentIndex = WORKSPACE_TABS.findIndex((tab) => tab.id === currentTab);
  if (currentIndex === -1) {
    return null;
  }
  const count = WORKSPACE_TABS.length;
  switch (key) {
    case 'ArrowRight': {
      const next = WORKSPACE_TABS[(currentIndex + 1) % count];
      return next ? next.id : null;
    }
    case 'ArrowLeft': {
      const prev = WORKSPACE_TABS[(currentIndex - 1 + count) % count];
      return prev ? prev.id : null;
    }
    case 'Home': {
      const first = WORKSPACE_TABS[0];
      return first ? first.id : null;
    }
    case 'End': {
      const last = WORKSPACE_TABS[count - 1];
      return last ? last.id : null;
    }
    default:
      return null;
  }
}

/** Canonical five-state `Lifecycle` order from `GLOSSARY.md`. */
export const LIFECYCLE_ORDER: readonly LifecycleState[] = [
  'draft',
  'candidate',
  'validated',
  'canonical',
  'superseded',
] as const;

/** Determine whether `Export` actions are unlocked for a given `Lifecycle` state. */
export function isExportEligible(lifecycle: LifecycleState): boolean {
  return lifecycle === 'validated' || lifecycle === 'canonical';
}

/** Human-readable status summary derived from an explicit `AsyncViewState<T>`. */
export function describeAsyncState<T>(state: AsyncViewState<T>): {
  readonly badge: string;
  readonly ariaRole: 'status' | 'alert' | null;
  readonly isInteractive: boolean;
} {
  switch (state.status) {
    case 'loading':
      return { badge: 'Loading', ariaRole: 'status', isInteractive: false };
    case 'empty':
      return { badge: 'Empty', ariaRole: 'status', isInteractive: true };
    case 'error':
      return { badge: 'Error', ariaRole: 'alert', isInteractive: state.retryable };
    case 'disabled':
      return { badge: 'Disabled', ariaRole: 'status', isInteractive: false };
    case 'ready':
      return { badge: 'Ready', ariaRole: null, isInteractive: true };
  }
}
