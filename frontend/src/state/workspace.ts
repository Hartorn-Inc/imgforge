/** === WORKSPACE NAVIGATION & VIEW STATE HELPERS ===
 *
 * PURPOSE:
 * Centralizes tab metadata, WAI-ARIA keyboard tablist navigation, `Lifecycle`
 * & `Visibility` guards, `PatchEdit` `1:1` ROI clamping, and explicit
 * `AsyncViewState<T>` presentation descriptors for the four top-level views.
 *
 * DESIGN RATIONALE ("WHY"):
 * Separating pure state transitions, ROI clamping, and keyboard navigation
 * logic from Preact components keeps UI behavior deterministic and cleanly
 * testable under Vitest without duplicating Rust color math.
 */

import type { PatchEditApiPayload } from '../api/client';
import type {
  AsyncViewState,
  LifecycleState,
  PatchEditDraft,
  RunRecord,
  Visibility,
  WorkspaceTab,
} from '../types/domain';

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
      'Manage Style presets & exemplars, Atmosphere lighting, Subject PaletteLock, and ReferencePack slicing.',
  },
  {
    id: 'spatial_guide',
    label: 'SpatialGuide Studio',
    summary:
      'Author OverlayGuide and Blockout GuideSpec layouts and paint PatchEdit masks on the Konva stage.',
  },
  {
    id: 'patch_edit',
    label: 'PatchEdit Cropper',
    summary:
      'Frame 1:1 square ROI or painted mask crops, PatchPrepMode, and collar-ring OKLab harmonization.',
  },
  {
    id: 'run_inspector',
    label: 'Run Inspector',
    summary:
      'Compare before/after Runs, inspect L* & OKLab telemetry, sample PaletteLock, validate, and export.',
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

/** Narrow an arbitrary string to `LifecycleState`. */
export function isLifecycleState(value: string): value is LifecycleState {
  return (
    value === 'draft' ||
    value === 'candidate' ||
    value === 'validated' ||
    value === 'canonical' ||
    value === 'superseded'
  );
}

/** Return the next forward `Lifecycle` state, or `null` when immutable (`canonical`/`superseded`). */
export function nextLifecycleStep(current: LifecycleState): LifecycleState | null {
  switch (current) {
    case 'draft':
      return 'candidate';
    case 'candidate':
      return 'validated';
    case 'validated':
      return 'canonical';
    case 'canonical':
    case 'superseded':
      return null;
  }
}

/** Determine whether `Export` actions are unlocked for a given `Lifecycle` state. */
export function isExportEligible(lifecycle: LifecycleState): boolean {
  return lifecycle === 'validated' || lifecycle === 'canonical';
}

const VISIBILITY_RANK: Readonly<Record<Visibility, number>> = {
  public: 0,
  restricted: 1,
  internal: 2,
};

/** Enforce `sourceVisibility <= targetVisibility` fail-closed `Visibility` boundary. */
export function isVisibilityAllowedForExport(source: Visibility, target: Visibility): boolean {
  return VISIBILITY_RANK[source] <= VISIBILITY_RANK[target];
}

/** Construct a default `PatchEditDraft` anchored on an optional `RunRecord`. */
export function createDefaultPatchEditDraft(run: RunRecord | null): PatchEditDraft {
  const width = run?.widthPx ?? 1024;
  const height = run?.heightPx ?? 1024;
  const minSide = Math.max(32, Math.min(width, height));
  const cropSize = Math.min(minSide, Math.max(64, Math.round(minSide * 0.35)));
  return clampPatchEditDraft({
    slug: run?.slug ?? 'patch-edit',
    targetRunKey: run?.runId ?? null,
    targetImagePath: '',
    mode: 'roi',
    cropSizePx: cropSize,
    originXPx: Math.floor((width - cropSize) / 2),
    originYPx: Math.floor((height - cropSize) / 2),
    prepMode: 'cue_outline',
    collarWidthPx: 24,
    featherPx: 12,
    prompt: '',
    visibility: run?.visibility ?? 'public',
    maskPngBase64: null,
    canvasWidthPx: width,
    canvasHeightPx: height,
  });
}

/** Clamp `PatchEditDraft` `1:1` square ROI, collar (`16..=32 px`), and feather (`2..=collar`) bounds. */
export function clampPatchEditDraft(draft: PatchEditDraft): PatchEditDraft {
  const canvasW = Math.max(32, Math.round(draft.canvasWidthPx));
  const canvasH = Math.max(32, Math.round(draft.canvasHeightPx));
  const maxSquare = Math.min(canvasW, canvasH);
  const cropSizePx = Math.max(16, Math.min(maxSquare, Math.round(draft.cropSizePx)));
  const originXPx = Math.max(0, Math.min(canvasW - cropSizePx, Math.round(draft.originXPx)));
  const originYPx = Math.max(0, Math.min(canvasH - cropSizePx, Math.round(draft.originYPx)));
  const collarWidthPx = Math.max(16, Math.min(32, Math.round(draft.collarWidthPx)));
  const featherPx = Math.max(2, Math.min(collarWidthPx, Math.round(draft.featherPx)));

  return {
    ...draft,
    canvasWidthPx: canvasW,
    canvasHeightPx: canvasH,
    cropSizePx,
    originXPx,
    originYPx,
    collarWidthPx,
    featherPx,
  };
}

/** Build a `POST /api/edit/patch` payload enforcing mutually exclusive target & mask fields. */
export function buildPatchEditPayload(
  draft: PatchEditDraft,
  selectedRun: RunRecord | null,
): PatchEditApiPayload | { readonly error: string } {
  const clamped = clampPatchEditDraft(draft);
  const slug = clamped.slug.trim();
  if (slug.length === 0) {
    return { error: 'Run output slug is required.' };
  }
  const prompt = clamped.prompt.trim();
  if (prompt.length === 0) {
    return { error: 'Edit prompt is required.' };
  }

  const base = {
    slug,
    prepMode: clamped.prepMode,
    collarWidthPx: clamped.collarWidthPx,
    featherPx: clamped.featherPx,
    prompt,
    visibility: clamped.visibility,
  };

  let withTarget: PatchEditApiPayload;
  if (selectedRun !== null) {
    withTarget = {
      ...base,
      runSlug: selectedRun.slug,
      runVersion: selectedRun.version,
    };
  } else if (clamped.targetImagePath.trim().length > 0) {
    withTarget = {
      ...base,
      targetImagePath: clamped.targetImagePath.trim(),
    };
  } else {
    return { error: 'Select a target Run or provide a workspace relative targetImagePath.' };
  }

  if (clamped.mode === 'mask') {
    if (clamped.maskPngBase64 === null || clamped.maskPngBase64.trim().length === 0) {
      return {
        error: 'Paint a mask in SpatialGuide Studio or PatchEdit before submitting in mask mode.',
      };
    }
    return {
      ...withTarget,
      maskPngBase64: clamped.maskPngBase64,
    };
  }

  return {
    ...withTarget,
    roi: {
      x: clamped.originXPx,
      y: clamped.originYPx,
      width: clamped.cropSizePx,
      height: clamped.cropSizePx,
    },
  };
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
