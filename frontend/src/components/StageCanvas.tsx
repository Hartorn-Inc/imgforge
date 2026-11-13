/** === KONVA SPATIALGUIDE CANVAS STAGE ===
 *
 * PURPOSE:
 * Renders the interactive Konva `Stage` placeholder for `GuideSpec` layouts
 * (`OverlayGuide` and `Blockout`), drawing canvas bounds, coordinate grid,
 * depth bands, proportionally scaled `Subject` mannequins, overlay regions,
 * and haloed action vectors.
 *
 * DESIGN RATIONALE ("WHY"):
 * Mounting `konva` directly via `useRef` + `useEffect` decouples Konva's
 * imperative canvas lifecycle from Preact virtual-DOM reconciliation and keeps
 * deterministic `GuideRenderer` rasterization strictly in Rust core.
 *
 * EXECUTION LIFECYCLE / DATA FLOW:
 * 1. Mounts `Konva.Stage` into the container `<div>` sized to the fitted viewport.
 * 2. Redraws deterministic grid/bounds and `GuideSpec` primitives whenever `spec`
 *    or `showGrid` changes.
 * 3. Destroys the Konva stage cleanly on unmount to prevent memory leaks.
 */

import Konva from 'konva';
import type { JSX } from 'preact';
import { useEffect, useRef, useState } from 'preact/hooks';
import type { GuideSpec } from '../types/guideSpec';

export interface StageCanvasProps {
  readonly spec: GuideSpec;
  readonly showGrid: boolean;
  readonly disabled?: boolean;
}

const STAGE_VIEWPORT_SIZE = 512;
const GRID_DIVISIONS = 8;

/** Live Konva canvas stage rendering `GuideSpec` bounds, grid, and spatial cues. */
export function StageCanvas({ spec, showGrid, disabled = false }: StageCanvasProps): JSX.Element {
  const containerRef = useRef<HTMLDivElement | null>(null);
  const [canvasUnavailable, setCanvasUnavailable] = useState<string | null>(null);
  const [viewportSize, setViewportSize] = useState<number>(STAGE_VIEWPORT_SIZE);

  useEffect(() => {
    const container = containerRef.current;
    if (!container) {
      return;
    }

    const updateViewportSize = (measuredWidth: number): void => {
      if (!Number.isFinite(measuredWidth) || measuredWidth <= 0) {
        return;
      }
      const clamped = Math.max(1, Math.min(STAGE_VIEWPORT_SIZE, Math.floor(measuredWidth)));
      setViewportSize((prev) => (prev === clamped ? prev : clamped));
    };

    if (container.clientWidth > 0) {
      updateViewportSize(container.clientWidth);
    }

    if (typeof ResizeObserver === 'undefined') {
      return;
    }

    const observer = new ResizeObserver((entries) => {
      for (const entry of entries) {
        updateViewportSize(entry.contentRect.width);
      }
    });
    observer.observe(container);

    return () => {
      observer.disconnect();
    };
  }, []);

  useEffect(() => {
    if (disabled) {
      setCanvasUnavailable(null);
      return;
    }

    const container = containerRef.current;
    if (!container) {
      return;
    }

    const aspectRatio = spec.canvasWidth / spec.canvasHeight;
    const stageWidth =
      aspectRatio >= 1 ? viewportSize : Math.max(1, Math.round(viewportSize * aspectRatio));
    const stageHeight =
      aspectRatio >= 1 ? Math.max(1, Math.round(viewportSize / aspectRatio)) : viewportSize;

    let stage: Konva.Stage;
    try {
      stage = new Konva.Stage({
        container,
        width: stageWidth,
        height: stageHeight,
      });
    } catch (err) {
      const detail = err instanceof Error ? err.message : '2D canvas context unavailable';
      setCanvasUnavailable(detail);
      return;
    }

    setCanvasUnavailable(null);
    const backgroundLayer = new Konva.Layer();
    const guideLayer = new Konva.Layer();

    // Neutral #252830 calibration surface matching ADR 0005.
    backgroundLayer.add(
      new Konva.Rect({
        x: 0,
        y: 0,
        width: stageWidth,
        height: stageHeight,
        fill: '#252830',
        stroke: '#63B3ED',
        strokeWidth: 2,
      }),
    );

    if (spec.variant === 'Blockout') {
      for (const band of spec.depthBands) {
        const y = band.yStartRatio * stageHeight;
        const height = (band.yEndRatio - band.yStartRatio) * stageHeight;
        backgroundLayer.add(
          new Konva.Rect({
            x: 0,
            y,
            width: stageWidth,
            height,
            fill: band.fillHex,
            opacity: 0.7,
          }),
        );
        backgroundLayer.add(
          new Konva.Text({
            x: 10,
            y: y + 6,
            text: `${band.plane.toUpperCase()}: ${band.label}`,
            fontSize: 11,
            fontFamily: 'monospace',
            fill: '#E2E8F0',
          }),
        );
      }
    } else {
      backgroundLayer.add(
        new Konva.Text({
          x: 12,
          y: 12,
          text: `OverlayGuide Target: ${spec.targetImagePath}`,
          fontSize: 12,
          fontFamily: 'monospace',
          fill: '#CBD5E0',
        }),
      );
    }

    if (showGrid) {
      for (let i = 1; i < GRID_DIVISIONS; i += 1) {
        const xPos = (stageWidth / GRID_DIVISIONS) * i;
        const yPos = (stageHeight / GRID_DIVISIONS) * i;
        backgroundLayer.add(
          new Konva.Line({
            points: [xPos, 0, xPos, stageHeight],
            stroke: '#4A5568',
            strokeWidth: 1,
            dash: [4, 4],
            opacity: 0.55,
          }),
        );
        backgroundLayer.add(
          new Konva.Line({
            points: [0, yPos, stageWidth, yPos],
            stroke: '#4A5568',
            strokeWidth: 1,
            dash: [4, 4],
            opacity: 0.55,
          }),
        );
      }
    }

    for (const region of spec.overlayRegions) {
      if (region.points.length >= 2) {
        const first = region.points[0];
        const second = region.points[1];
        if (first && second && region.kind === 'rect') {
          const x = Math.min(first.xRatio, second.xRatio) * stageWidth;
          const y = Math.min(first.yRatio, second.yRatio) * stageHeight;
          const width = Math.abs(second.xRatio - first.xRatio) * stageWidth;
          const height = Math.abs(second.yRatio - first.yRatio) * stageHeight;

          guideLayer.add(
            new Konva.Rect({
              x,
              y,
              width,
              height,
              stroke: region.haloHex,
              strokeWidth: 6,
              cornerRadius: 4,
            }),
          );
          guideLayer.add(
            new Konva.Rect({
              x,
              y,
              width,
              height,
              fill: region.fillHex,
              opacity: region.opacity,
              stroke: region.strokeHex,
              strokeWidth: 2,
              cornerRadius: 4,
            }),
          );
        } else if (first && second && region.kind === 'ellipse') {
          const x0 = first.xRatio * stageWidth;
          const y0 = first.yRatio * stageHeight;
          const x1 = second.xRatio * stageWidth;
          const y1 = second.yRatio * stageHeight;
          const centerX = (x0 + x1) / 2;
          const centerY = (y0 + y1) / 2;
          const radiusX = Math.abs(x1 - x0) / 2;
          const radiusY = Math.abs(y1 - y0) / 2;

          guideLayer.add(
            new Konva.Ellipse({
              x: centerX,
              y: centerY,
              radiusX,
              radiusY,
              stroke: region.haloHex,
              strokeWidth: 6,
            }),
          );
          guideLayer.add(
            new Konva.Ellipse({
              x: centerX,
              y: centerY,
              radiusX,
              radiusY,
              fill: region.fillHex,
              opacity: region.opacity,
              stroke: region.strokeHex,
              strokeWidth: 2,
            }),
          );
        } else {
          const flatPoints = region.points.flatMap((pt) => [
            pt.xRatio * stageWidth,
            pt.yRatio * stageHeight,
          ]);
          guideLayer.add(
            new Konva.Line({
              points: flatPoints,
              stroke: region.haloHex,
              strokeWidth: 6,
              lineCap: 'round',
              lineJoin: 'round',
            }),
          );
          guideLayer.add(
            new Konva.Line({
              points: flatPoints,
              stroke: region.strokeHex,
              strokeWidth: 2,
              lineCap: 'round',
              lineJoin: 'round',
            }),
          );
        }
      }
    }

    for (const mannequin of spec.mannequins) {
      const cx = mannequin.xRatio * stageWidth;
      const cy = mannequin.yRatio * stageHeight;
      const mannequinHeight = Math.max(
        36,
        Math.round(mannequin.heightMeters * 72 * mannequin.scaleFactor),
      );
      const mannequinWidth = Math.max(20, Math.round(mannequinHeight * 0.34));

      guideLayer.add(
        new Konva.Rect({
          x: cx - mannequinWidth / 2,
          y: cy - mannequinHeight,
          width: mannequinWidth,
          height: mannequinHeight,
          stroke: mannequin.paletteHex,
          strokeWidth: 2,
          fill: mannequin.paletteHex,
          opacity: 0.25,
          cornerRadius: 6,
        }),
      );
      guideLayer.add(
        new Konva.Circle({
          x: cx,
          y: cy - mannequinHeight - 10,
          radius: 8,
          fill: mannequin.paletteHex,
          stroke: '#1A1D24',
          strokeWidth: 2,
        }),
      );
      guideLayer.add(
        new Konva.Text({
          x: cx - 50,
          y: cy + 6,
          width: 100,
          align: 'center',
          text: `${mannequin.label} (${mannequin.facing})`,
          fontSize: 10,
          fontFamily: 'monospace',
          fill: '#F7FAFC',
        }),
      );
    }

    for (const vector of spec.actionVectors) {
      const points = [
        vector.fromXRatio * stageWidth,
        vector.fromYRatio * stageHeight,
        vector.toXRatio * stageWidth,
        vector.toYRatio * stageHeight,
      ];
      const dash = vector.style === 'dashed' ? [8, 6] : [];

      guideLayer.add(
        new Konva.Arrow({
          points,
          stroke: vector.haloHex,
          fill: vector.haloHex,
          strokeWidth: vector.strokeWidthPx + 4,
          pointerLength: 12,
          pointerWidth: 10,
          dash,
        }),
      );
      guideLayer.add(
        new Konva.Arrow({
          points,
          stroke: vector.strokeHex,
          fill: vector.strokeHex,
          strokeWidth: vector.strokeWidthPx,
          pointerLength: 10,
          pointerWidth: 8,
          dash,
        }),
      );
    }

    stage.add(backgroundLayer);
    stage.add(guideLayer);
    stage.draw();

    return () => {
      stage.destroy();
    };
  }, [spec, showGrid, disabled, viewportSize]);

  const isCanvasHidden = disabled || canvasUnavailable !== null;

  return (
    <div class="stage-frame">
      <div class="stage-frame__meta">
        <span>
          Bounds: {spec.canvasWidth}×{spec.canvasHeight}px
        </span>
        <span>Variant: {spec.variant}</span>
        <span>Grid: {showGrid ? 'Visible' : 'Hidden'}</span>
      </div>
      {disabled ? (
        <div class="stage-frame__placeholder" role="status">
          Konva canvas stage is disabled in the current view state.
        </div>
      ) : (
        canvasUnavailable !== null && (
          <div class="stage-frame__placeholder" role="alert">
            Canvas preview unavailable in this browser context: {canvasUnavailable}
          </div>
        )
      )}
      <div
        ref={containerRef}
        hidden={isCanvasHidden}
        class="stage-frame__canvas"
        role="img"
        aria-label={`Konva SpatialGuide stage for ${spec.title} (${spec.variant}, ${String(spec.canvasWidth)} by ${String(spec.canvasHeight)} pixels)`}
      />
    </div>
  );
}
