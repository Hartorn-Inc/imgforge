/** === KONVA SPATIALGUIDE CANVAS STAGE ===
 *
 * PURPOSE:
 * Renders the interactive Konva `Stage` for `GuideSpec` layouts
 * (`OverlayGuide` and `Blockout`), supporting drag-and-drop repositioning of
 * `SubjectMannequin` nodes, `DepthBand` boundary resizing, `OverlayRegion`
 * vertex handles, `ActionVector` endpoint dragging / drawing, and freehand
 * alpha-mask brush painting exported as PNG for `PatchEdit`.
 *
 * DESIGN RATIONALE ("WHY"):
 * Mounting `konva` directly via `useRef` + `useEffect` decouples Konva's
 * imperative canvas lifecycle from Preact virtual-DOM reconciliation and keeps
 * deterministic `GuideRenderer` rasterization strictly in Rust core.
 */

import Konva from 'konva';
import type { JSX } from 'preact';
import { useCallback, useEffect, useRef, useState } from 'preact/hooks';
import type { GuideSpec } from '../types/guideSpec';
import {
  appendActionVectorToSpec,
  moveMannequinInSpec,
  resizeDepthBandInSpec,
  stripPngDataUrlPrefix,
  updateActionVectorInSpec,
  updateOverlayRegionPointInSpec,
} from './stageGeometry';

export type StageToolMode = 'select' | 'draw_vector' | 'paint_mask';

export interface StageCanvasProps {
  readonly spec: GuideSpec;
  readonly showGrid: boolean;
  readonly disabled?: boolean;
  readonly toolMode?: StageToolMode;
  readonly brushRadiusPx?: number;
  readonly backgroundImageUrl?: string | null;
  readonly maskPngBase64?: string | null;
  readonly onSpecChange?: (nextSpec: GuideSpec) => void;
  readonly onMaskExport?: (maskPngBase64: string) => void;
  readonly onMaskClear?: () => void;
}

const STAGE_VIEWPORT_SIZE = 512;
const GRID_DIVISIONS = 8;

function attachEventIfSupported(node: unknown, eventName: string, handler: () => void): void {
  const candidate = node as { on?: (ev: string, cb: () => void) => void } | null;
  if (candidate !== null && typeof candidate.on === 'function') {
    candidate.on(eventName, handler);
  }
}

/** Live Konva canvas stage rendering `GuideSpec` bounds, interactive handles, and mask painter. */
export function StageCanvas({
  spec,
  showGrid,
  disabled = false,
  toolMode = 'select',
  brushRadiusPx = 20,
  backgroundImageUrl = null,
  maskPngBase64,
  onSpecChange,
  onMaskExport,
  onMaskClear,
}: StageCanvasProps): JSX.Element {
  const containerRef = useRef<HTMLDivElement | null>(null);
  const maskCanvasRef = useRef<HTMLCanvasElement | null>(null);
  const isPaintingRef = useRef<boolean>(false);
  const lastExportedMaskRef = useRef<string | null>(null);
  const [canvasUnavailable, setCanvasUnavailable] = useState<string | null>(null);
  const [viewportSize, setViewportSize] = useState<number>(STAGE_VIEWPORT_SIZE);
  const [bgImgElement, setBgImgElement] = useState<HTMLImageElement | null>(null);
  const [hasMaskStrokes, setHasMaskStrokes] = useState<boolean>(
    () => typeof maskPngBase64 === 'string' && maskPngBase64.length > 0,
  );

  const specRef = useRef<GuideSpec>(spec);
  specRef.current = spec;
  const onSpecChangeRef = useRef<((nextSpec: GuideSpec) => void) | undefined>(onSpecChange);
  onSpecChangeRef.current = onSpecChange;

  useEffect(() => {
    if (
      spec.variant !== 'OverlayGuide' ||
      backgroundImageUrl === null ||
      backgroundImageUrl.trim().length === 0 ||
      typeof Image === 'undefined'
    ) {
      setBgImgElement(null);
      return;
    }
    let cancelled = false;
    const img = new Image();
    img.onload = () => {
      if (!cancelled) {
        setBgImgElement(img);
      }
    };
    img.onerror = () => {
      if (!cancelled) {
        setBgImgElement(null);
      }
    };
    img.src = backgroundImageUrl;
    return () => {
      cancelled = true;
    };
  }, [spec.variant, backgroundImageUrl]);

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

  const aspectRatio = spec.canvasWidth / spec.canvasHeight;
  const stageWidth =
    aspectRatio >= 1 ? viewportSize : Math.max(1, Math.round(viewportSize * aspectRatio));
  const stageHeight =
    aspectRatio >= 1 ? Math.max(1, Math.round(viewportSize / aspectRatio)) : viewportSize;

  useEffect(() => {
    if (disabled) {
      setCanvasUnavailable(null);
      return;
    }

    const container = containerRef.current;
    if (!container) {
      return;
    }

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
    const interactiveSelect = toolMode === 'select' && onSpecChangeRef.current !== undefined;

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

    if (
      spec.variant === 'OverlayGuide' &&
      bgImgElement !== null &&
      typeof Konva.Image === 'function'
    ) {
      backgroundLayer.add(
        new Konva.Image({
          x: 0,
          y: 0,
          width: stageWidth,
          height: stageHeight,
          image: bgImgElement,
        }),
      );
    }

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

        if (interactiveSelect && band.yEndRatio < 0.999) {
          const divider = new Konva.Rect({
            x: 0,
            y: band.yEndRatio * stageHeight - 4,
            width: stageWidth,
            height: 8,
            fill: '#63B3ED',
            opacity: 0.35,
            draggable: true,
            dragBoundFunc: (pos: { x: number; y: number }) => ({
              x: 0,
              y: Math.max(band.yStartRatio * stageHeight + 8, Math.min(stageHeight - 4, pos.y)),
            }),
          });
          attachEventIfSupported(divider, 'dragend', () => {
            const nextEnd = (divider.y() + 4) / stageHeight;
            onSpecChangeRef.current?.(
              resizeDepthBandInSpec(specRef.current, band.id, band.yStartRatio, nextEnd),
            );
          });
          guideLayer.add(divider);
        }
      }
    } else if (bgImgElement === null) {
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

        if (interactiveSelect) {
          region.points.forEach((pt, pointIdx) => {
            const handle = new Konva.Circle({
              x: pt.xRatio * stageWidth,
              y: pt.yRatio * stageHeight,
              radius: 6,
              fill: region.strokeHex,
              stroke: '#1A1D24',
              strokeWidth: 2,
              draggable: true,
            });
            attachEventIfSupported(handle, 'dragend', () => {
              onSpecChangeRef.current?.(
                updateOverlayRegionPointInSpec(
                  specRef.current,
                  region.id,
                  pointIdx,
                  handle.x() / stageWidth,
                  handle.y() / stageHeight,
                ),
              );
            });
            guideLayer.add(handle);
          });
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

      const body = new Konva.Rect({
        x: cx - mannequinWidth / 2,
        y: cy - mannequinHeight,
        width: mannequinWidth,
        height: mannequinHeight,
        stroke: mannequin.paletteHex,
        strokeWidth: 2,
        fill: mannequin.paletteHex,
        opacity: 0.25,
        cornerRadius: 6,
        draggable: interactiveSelect,
      });
      if (interactiveSelect) {
        attachEventIfSupported(body, 'dragend', () => {
          const nextX = (body.x() + mannequinWidth / 2) / stageWidth;
          const nextY = (body.y() + mannequinHeight) / stageHeight;
          onSpecChangeRef.current?.(
            moveMannequinInSpec(specRef.current, mannequin.id, nextX, nextY),
          );
        });
      }
      guideLayer.add(body);

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

      if (interactiveSelect) {
        const startHandle = new Konva.Circle({
          x: vector.fromXRatio * stageWidth,
          y: vector.fromYRatio * stageHeight,
          radius: 6,
          fill: vector.strokeHex,
          stroke: '#1A1D24',
          strokeWidth: 2,
          draggable: true,
        });
        attachEventIfSupported(startHandle, 'dragend', () => {
          onSpecChangeRef.current?.(
            updateActionVectorInSpec(
              specRef.current,
              vector.id,
              startHandle.x() / stageWidth,
              startHandle.y() / stageHeight,
              vector.toXRatio,
              vector.toYRatio,
            ),
          );
        });
        guideLayer.add(startHandle);

        const endHandle = new Konva.Circle({
          x: vector.toXRatio * stageWidth,
          y: vector.toYRatio * stageHeight,
          radius: 6,
          fill: vector.strokeHex,
          stroke: '#F7FAFC',
          strokeWidth: 2,
          draggable: true,
        });
        attachEventIfSupported(endHandle, 'dragend', () => {
          onSpecChangeRef.current?.(
            updateActionVectorInSpec(
              specRef.current,
              vector.id,
              vector.fromXRatio,
              vector.fromYRatio,
              endHandle.x() / stageWidth,
              endHandle.y() / stageHeight,
            ),
          );
        });
        guideLayer.add(endHandle);
      }
    }

    if (toolMode === 'draw_vector' && onSpecChangeRef.current !== undefined) {
      let drawOrigin: { x: number; y: number } | null = null;
      let previewArrow: Konva.Arrow | null = null;

      attachEventIfSupported(stage, 'mousedown touchstart', () => {
        const pos = stage.getPointerPosition();
        if (pos === null) {
          return;
        }
        drawOrigin = { x: pos.x, y: pos.y };
        previewArrow = new Konva.Arrow({
          points: [pos.x, pos.y, pos.x, pos.y],
          stroke: '#F6AD55',
          fill: '#F6AD55',
          strokeWidth: 4,
          pointerLength: 10,
          pointerWidth: 8,
        });
        guideLayer.add(previewArrow);
        guideLayer.batchDraw();
      });

      attachEventIfSupported(stage, 'mousemove touchmove', () => {
        if (drawOrigin === null || previewArrow === null) {
          return;
        }
        const pos = stage.getPointerPosition();
        if (pos === null) {
          return;
        }
        previewArrow.points([drawOrigin.x, drawOrigin.y, pos.x, pos.y]);
        guideLayer.batchDraw();
      });

      attachEventIfSupported(stage, 'mouseup touchend', () => {
        if (drawOrigin === null) {
          return;
        }
        const pos = stage.getPointerPosition();
        const origin = drawOrigin;
        drawOrigin = null;
        if (previewArrow !== null) {
          previewArrow.destroy();
          previewArrow = null;
        }
        if (pos !== null && Math.hypot(pos.x - origin.x, pos.y - origin.y) >= 8) {
          onSpecChangeRef.current?.(
            appendActionVectorToSpec(
              specRef.current,
              origin.x / stageWidth,
              origin.y / stageHeight,
              pos.x / stageWidth,
              pos.y / stageHeight,
            ),
          );
        }
      });
    }

    stage.add(backgroundLayer);
    stage.add(guideLayer);
    stage.draw();

    return () => {
      stage.destroy();
    };
  }, [spec, showGrid, disabled, viewportSize, stageWidth, stageHeight, toolMode, bgImgElement]);

  const paintMaskAt = useCallback(
    (clientX: number, clientY: number): void => {
      const canvas = maskCanvasRef.current;
      if (canvas === null) {
        return;
      }
      const rect = canvas.getBoundingClientRect();
      if (rect.width <= 0 || rect.height <= 0) {
        return;
      }
      const x = ((clientX - rect.left) / rect.width) * canvas.width;
      const y = ((clientY - rect.top) / rect.height) * canvas.height;
      const ctx = canvas.getContext('2d');
      if (ctx === null) {
        return;
      }
      const scaledRadius = Math.max(2, Math.round((brushRadiusPx / stageWidth) * canvas.width));
      ctx.fillStyle = 'rgba(255, 255, 255, 1)';
      ctx.beginPath();
      ctx.arc(x, y, scaledRadius, 0, Math.PI * 2);
      ctx.fill();
      setHasMaskStrokes(true);
    },
    [brushRadiusPx, stageWidth],
  );

  const exportMaskPng = useCallback((): void => {
    const canvas = maskCanvasRef.current;
    if (canvas === null || onMaskExport === undefined) {
      return;
    }
    const b64 = stripPngDataUrlPrefix(canvas.toDataURL('image/png'));
    lastExportedMaskRef.current = b64;
    onMaskExport(b64);
  }, [onMaskExport]);

  const clearMaskCanvas = useCallback((): void => {
    const canvas = maskCanvasRef.current;
    if (canvas !== null) {
      const ctx = canvas.getContext('2d');
      if (ctx !== null) {
        ctx.clearRect(0, 0, canvas.width, canvas.height);
      }
    }
    lastExportedMaskRef.current = null;
    setHasMaskStrokes(false);
    onMaskClear?.();
  }, [onMaskClear]);

  const isCanvasHidden = disabled || canvasUnavailable !== null;

  useEffect(() => {
    if (isCanvasHidden) {
      lastExportedMaskRef.current = null;
      return;
    }
    if (maskPngBase64 === undefined) {
      return;
    }
    const canvas = maskCanvasRef.current;
    if (maskPngBase64 === null || maskPngBase64.length === 0) {
      lastExportedMaskRef.current = null;
      if (canvas !== null) {
        const ctx = canvas.getContext('2d');
        if (ctx !== null) {
          ctx.clearRect(0, 0, canvas.width, canvas.height);
        }
      }
      setHasMaskStrokes(false);
      return;
    }
    setHasMaskStrokes(true);
    if (
      maskPngBase64 === lastExportedMaskRef.current ||
      canvas === null ||
      typeof Image === 'undefined'
    ) {
      return;
    }
    let cancelled = false;
    const img = new Image();
    img.onload = () => {
      if (cancelled) {
        return;
      }
      const ctx = canvas.getContext('2d');
      if (ctx !== null) {
        ctx.clearRect(0, 0, canvas.width, canvas.height);
        ctx.drawImage(img, 0, 0, canvas.width, canvas.height);
      }
    };
    img.src = `data:image/png;base64,${maskPngBase64}`;
    return () => {
      cancelled = true;
    };
  }, [isCanvasHidden, maskPngBase64, spec.canvasWidth, spec.canvasHeight]);

  return (
    <div class="stage-frame">
      <div class="stage-frame__meta">
        <span>
          Bounds: {spec.canvasWidth}×{spec.canvasHeight}px
        </span>
        <span>Variant: {spec.variant}</span>
        <span>Grid: {showGrid ? 'Visible' : 'Hidden'}</span>
        <span>Tool: {toolMode}</span>
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
      <div class="stage-canvas-stack" hidden={isCanvasHidden}>
        <div
          ref={containerRef}
          hidden={isCanvasHidden}
          class="stage-frame__canvas"
          role="img"
          aria-label={`Konva SpatialGuide stage for ${spec.title} (${spec.variant}, ${String(spec.canvasWidth)} by ${String(spec.canvasHeight)} pixels)`}
        />
        {!isCanvasHidden && (
          <canvas
            ref={maskCanvasRef}
            width={spec.canvasWidth}
            height={spec.canvasHeight}
            hidden={toolMode !== 'paint_mask'}
            class="mask-paint-overlay"
            style={{ width: `${String(stageWidth)}px`, height: `${String(stageHeight)}px` }}
            aria-label="Freehand PatchEdit alpha mask painting canvas"
            onPointerDown={(e) => {
              if (toolMode !== 'paint_mask') {
                return;
              }
              isPaintingRef.current = true;
              paintMaskAt(e.clientX, e.clientY);
            }}
            onPointerMove={(e) => {
              if (toolMode === 'paint_mask' && isPaintingRef.current) {
                paintMaskAt(e.clientX, e.clientY);
              }
            }}
            onPointerUp={() => {
              if (isPaintingRef.current) {
                isPaintingRef.current = false;
                exportMaskPng();
              }
            }}
            onPointerLeave={() => {
              if (isPaintingRef.current) {
                isPaintingRef.current = false;
                exportMaskPng();
              }
            }}
          />
        )}
      </div>
      {toolMode === 'paint_mask' && !isCanvasHidden && (
        <div class="button-row">
          <button
            type="button"
            class="btn btn--secondary"
            disabled={!hasMaskStrokes}
            onClick={exportMaskPng}
          >
            Export PNG Mask
          </button>
          <button
            type="button"
            class="btn btn--ghost"
            disabled={!hasMaskStrokes}
            onClick={clearMaskCanvas}
          >
            Clear Mask
          </button>
        </div>
      )}
    </div>
  );
}
