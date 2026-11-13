import { render } from 'preact';
import { act } from 'preact/test-utils';
import { afterEach, beforeEach, describe, expect, test, vi } from 'vitest';
import { createDefaultGuideSpec, type OverlayGuideSpec } from '../types/guideSpec';
import { StageCanvas } from './StageCanvas';

interface RecordedEllipseConfig {
  readonly x: number;
  readonly y: number;
  readonly radiusX: number;
  readonly radiusY: number;
  readonly stroke?: string;
  readonly strokeWidth?: number;
  readonly fill?: string;
  readonly opacity?: number;
}

interface RecordedStageConfig {
  readonly container: HTMLDivElement;
  readonly width: number;
  readonly height: number;
}

const mockState = vi.hoisted(() => ({
  throwOnStageCreate: false,
  stages: [] as RecordedStageConfig[],
  ellipses: [] as RecordedEllipseConfig[],
  destroyCount: 0,
  drawCount: 0,
  resizeCallback: null as ResizeObserverCallback | null,
}));

vi.mock('konva', () => {
  class MockNode {
    public readonly config: Record<string, unknown>;
    public constructor(config: Record<string, unknown>) {
      this.config = config;
    }
  }

  class MockEllipse extends MockNode {
    public constructor(config: RecordedEllipseConfig) {
      super({ ...config });
      mockState.ellipses.push(config);
    }
  }

  class MockLayer {
    public readonly children: MockNode[] = [];
    public add(node: MockNode): void {
      this.children.push(node);
    }
  }

  class MockStage {
    public readonly layers: MockLayer[] = [];
    public constructor(config: RecordedStageConfig) {
      if (mockState.throwOnStageCreate) {
        throw new Error('2D context unavailable');
      }
      mockState.stages.push(config);
    }
    public add(layer: MockLayer): void {
      this.layers.push(layer);
    }
    public draw(): void {
      mockState.drawCount += 1;
    }
    public destroy(): void {
      mockState.destroyCount += 1;
    }
  }

  return {
    default: {
      Stage: MockStage,
      Layer: MockLayer,
      Rect: MockNode,
      Ellipse: MockEllipse,
      Line: MockNode,
      Text: MockNode,
      Circle: MockNode,
      Arrow: MockNode,
    },
  };
});

class MockResizeObserver implements ResizeObserver {
  public constructor(callback: ResizeObserverCallback) {
    mockState.resizeCallback = callback;
  }
  public observe(): void {}
  public unobserve(): void {}
  public disconnect(): void {
    mockState.resizeCallback = null;
  }
}

function createOverlaySpecWithEllipse(
  points: readonly { readonly xRatio: number; readonly yRatio: number }[],
): OverlayGuideSpec {
  const base = createDefaultGuideSpec('OverlayGuide');
  if (base.variant !== 'OverlayGuide') {
    throw new Error('Expected OverlayGuide default spec');
  }
  return {
    ...base,
    overlayRegions: [
      {
        id: 'region-ellipse-01',
        label: 'Ellipse Cue',
        kind: 'ellipse',
        points,
        strokeHex: '#68D391',
        haloHex: '#1A1D24',
        fillHex: '#68D391',
        opacity: 0.25,
      },
    ],
  };
}

describe('StageCanvas', () => {
  let mountNode: HTMLDivElement;

  beforeEach(() => {
    mockState.throwOnStageCreate = false;
    mockState.stages = [];
    mockState.ellipses = [];
    mockState.destroyCount = 0;
    mockState.drawCount = 0;
    mockState.resizeCallback = null;
    mountNode = document.createElement('div');
    document.body.appendChild(mountNode);
    vi.stubGlobal('ResizeObserver', MockResizeObserver);
  });

  afterEach(async () => {
    await act(() => {
      render(null, mountNode);
    });
    mountNode.remove();
    vi.unstubAllGlobals();
  });

  test.each<{
    id: string;
    points: readonly { readonly xRatio: number; readonly yRatio: number }[];
    expectedCenter: { readonly x: number; readonly y: number };
    expectedRadii: { readonly radiusX: number; readonly radiusY: number };
  }>([
    {
      id: 'renders-halo-and-filled-ellipse-for-ordered-points',
      points: [
        { xRatio: 0.25, yRatio: 0.25 },
        { xRatio: 0.75, yRatio: 0.75 },
      ],
      expectedCenter: { x: 256, y: 256 },
      expectedRadii: { radiusX: 128, radiusY: 128 },
    },
    {
      id: 'renders-halo-and-filled-ellipse-for-reversed-points',
      points: [
        { xRatio: 0.75, yRatio: 0.5 },
        { xRatio: 0.25, yRatio: 0.25 },
      ],
      expectedCenter: { x: 256, y: 192 },
      expectedRadii: { radiusX: 128, radiusY: 64 },
    },
  ])('$id', async ({ points, expectedCenter, expectedRadii }) => {
    const spec = createOverlaySpecWithEllipse(points);
    await act(() => {
      render(<StageCanvas spec={spec} showGrid={true} />, mountNode);
    });

    expect(mockState.ellipses).toHaveLength(2);
    expect(mockState.ellipses[0]).toEqual({
      x: expectedCenter.x,
      y: expectedCenter.y,
      radiusX: expectedRadii.radiusX,
      radiusY: expectedRadii.radiusY,
      stroke: '#1A1D24',
      strokeWidth: 6,
    });
    expect(mockState.ellipses[1]).toEqual({
      x: expectedCenter.x,
      y: expectedCenter.y,
      radiusX: expectedRadii.radiusX,
      radiusY: expectedRadii.radiusY,
      fill: '#68D391',
      opacity: 0.25,
      stroke: '#68D391',
      strokeWidth: 2,
    });
  });

  test.each<{ id: string; observedWidth: number; expectedStageWidth: number }>([
    {
      id: 'clamps-stage-width-to-narrow-container',
      observedWidth: 320,
      expectedStageWidth: 320,
    },
    {
      id: 'caps-stage-width-at-512-for-wide-container',
      observedWidth: 840,
      expectedStageWidth: 512,
    },
  ])('$id', async ({ observedWidth, expectedStageWidth }) => {
    const spec = createDefaultGuideSpec('Blockout');
    await act(() => {
      render(<StageCanvas spec={spec} showGrid={false} />, mountNode);
    });

    const callback = mockState.resizeCallback;
    expect(callback).not.toBeNull();
    if (callback === null) {
      return;
    }

    await act(() => {
      const fakeRect: DOMRectReadOnly = {
        width: observedWidth,
        height: 512,
        top: 0,
        left: 0,
        bottom: 512,
        right: observedWidth,
        x: 0,
        y: 0,
        toJSON: () => ({}),
      };
      const fakeEntry: ResizeObserverEntry = {
        target: mountNode,
        contentRect: fakeRect,
        borderBoxSize: [],
        contentBoxSize: [],
        devicePixelContentBoxSize: [],
      };
      callback([fakeEntry], new MockResizeObserver(callback));
    });

    const latestStage = mockState.stages.at(-1);
    expect(latestStage?.width).toBe(expectedStageWidth);
    expect(latestStage?.height).toBe(expectedStageWidth);
  });

  test('keeps container mounted and resets canvasUnavailable on disabled and spec changes', async () => {
    mockState.throwOnStageCreate = true;
    const initialSpec = createDefaultGuideSpec('Blockout');

    await act(() => {
      render(<StageCanvas spec={initialSpec} showGrid={true} />, mountNode);
    });

    const canvasContainer = mountNode.querySelector('.stage-frame__canvas');
    expect(canvasContainer).not.toBeNull();
    expect(canvasContainer?.hasAttribute('hidden')).toBe(true);
    expect(mountNode.querySelector('[role="alert"]')?.textContent).toContain(
      '2D context unavailable',
    );

    mockState.throwOnStageCreate = false;
    const updatedSpec = { ...initialSpec, title: 'Recovered Blockout' };
    await act(() => {
      render(<StageCanvas spec={updatedSpec} showGrid={true} />, mountNode);
    });

    expect(mountNode.querySelector('[role="alert"]')).toBeNull();
    expect(canvasContainer?.hasAttribute('hidden')).toBe(false);
    expect(mockState.stages.length).toBeGreaterThan(0);

    await act(() => {
      render(<StageCanvas spec={updatedSpec} showGrid={true} disabled={true} />, mountNode);
    });
    expect(canvasContainer?.hasAttribute('hidden')).toBe(true);
    expect(mountNode.querySelector('[role="status"]')?.textContent).toContain('disabled');

    await act(() => {
      render(<StageCanvas spec={updatedSpec} showGrid={true} disabled={false} />, mountNode);
    });
    expect(canvasContainer?.hasAttribute('hidden')).toBe(false);
  });

  test('preserves mask strokes across toolMode switches and notifies onMaskClear', async () => {
    const spec = createDefaultGuideSpec('Blockout');
    const onMaskClear = vi.fn();

    await act(() => {
      render(
        <StageCanvas
          spec={spec}
          showGrid={true}
          toolMode="paint_mask"
          maskPngBase64="iVBORw0KGgo="
          onMaskClear={onMaskClear}
        />,
        mountNode,
      );
    });

    const maskCanvas = mountNode.querySelector<HTMLCanvasElement>('.mask-paint-overlay');
    expect(maskCanvas).not.toBeNull();
    expect(maskCanvas?.hasAttribute('hidden')).toBe(false);

    // Switch to select mode: canvas remains mounted (hidden) so painted bitmap is preserved.
    await act(() => {
      render(
        <StageCanvas
          spec={spec}
          showGrid={true}
          toolMode="select"
          maskPngBase64="iVBORw0KGgo="
          onMaskClear={onMaskClear}
        />,
        mountNode,
      );
    });

    const maskCanvasWhileSelect = mountNode.querySelector<HTMLCanvasElement>('.mask-paint-overlay');
    expect(maskCanvasWhileSelect).toBe(maskCanvas);
    expect(maskCanvasWhileSelect?.hasAttribute('hidden')).toBe(true);

    // Switch back to paint_mask and click Clear Mask -> calls onMaskClear.
    await act(() => {
      render(
        <StageCanvas
          spec={spec}
          showGrid={true}
          toolMode="paint_mask"
          maskPngBase64="iVBORw0KGgo="
          onMaskClear={onMaskClear}
        />,
        mountNode,
      );
    });

    const clearBtn = Array.from(mountNode.querySelectorAll('button')).find(
      (b) => b.textContent.trim() === 'Clear Mask',
    );
    expect(clearBtn).toBeDefined();
    if (clearBtn === undefined) {
      return;
    }
    expect(clearBtn.disabled).toBe(false);

    await act(() => {
      clearBtn.click();
    });
    expect(onMaskClear).toHaveBeenCalledTimes(1);
  });
});
