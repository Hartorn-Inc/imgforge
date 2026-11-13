/** === KONVA 1:1 PATCHEDIT ROI & MASK STAGE ===
 *
 * PURPOSE:
 * Interactive Konva stage for framing an aspect-locked `1:1` square ROI box
 * with a live collar-width margin ring (`16..=32 px`), or painting an inline
 * alpha mask directly over the target image for `PatchEdit`.
 */

import Konva from 'konva';
import type { JSX } from 'preact';
import { useCallback, useEffect, useRef, useState } from 'preact/hooks';
import type { PatchRegionMode } from '../types/domain';
import { stripPngDataUrlPrefix } from './stageGeometry';

export interface PatchCropStageProps {
  readonly imageUrl: string | null;
  readonly canvasWidthPx: number;
  readonly canvasHeightPx: number;
  readonly mode: PatchRegionMode;
  readonly originXPx: number;
  readonly originYPx: number;
  readonly cropSizePx: number;
  readonly collarWidthPx: number;
  readonly brushRadiusPx: number;
  readonly maskPngBase64: string | null;
  readonly onRoiChange: (originXPx: number, originYPx: number, cropSizePx: number) => void;
  readonly onMaskChange: (maskPngBase64: string | null) => void;
}

const MAX_STAGE_SIDE = 480;

/** Interactive `1:1` square ROI cropper and mask painter on Konva. */
export function PatchCropStage({
  imageUrl,
  canvasWidthPx,
  canvasHeightPx,
  mode,
  originXPx,
  originYPx,
  cropSizePx,
  collarWidthPx,
  brushRadiusPx,
  maskPngBase64,
  onRoiChange,
  onMaskChange,
}: PatchCropStageProps): JSX.Element {
  const containerRef = useRef<HTMLDivElement | null>(null);
  const maskCanvasRef = useRef<HTMLCanvasElement | null>(null);
  const isPaintingRef = useRef<boolean>(false);
  const [imgElement, setImgElement] = useState<HTMLImageElement | null>(null);
  const [canvasUnavailable, setCanvasUnavailable] = useState<string | null>(null);

  const safeW = Math.max(32, canvasWidthPx);
  const safeH = Math.max(32, canvasHeightPx);
  const scale = MAX_STAGE_SIDE / Math.max(safeW, safeH);
  const stageWidth = Math.max(1, Math.round(safeW * scale));
  const stageHeight = Math.max(1, Math.round(safeH * scale));

  const onRoiChangeRef = useRef(onRoiChange);
  onRoiChangeRef.current = onRoiChange;

  useEffect(() => {
    if (imageUrl === null || imageUrl.trim().length === 0 || typeof Image === 'undefined') {
      setImgElement(null);
      return;
    }
    let cancelled = false;
    const img = new Image();
    img.onload = () => {
      if (!cancelled) {
        setImgElement(img);
      }
    };
    img.onerror = () => {
      if (!cancelled) {
        setImgElement(null);
      }
    };
    img.src = imageUrl;
    return () => {
      cancelled = true;
    };
  }, [imageUrl]);

  useEffect(() => {
    if (mode !== 'mask' || maskPngBase64 === null || typeof Image === 'undefined') {
      return;
    }
    const canvas = maskCanvasRef.current;
    if (canvas === null) {
      return;
    }
    const ctx = canvas.getContext('2d');
    if (ctx === null) {
      return;
    }
    const img = new Image();
    img.onload = () => {
      ctx.clearRect(0, 0, canvas.width, canvas.height);
      ctx.drawImage(img, 0, 0, canvas.width, canvas.height);
    };
    img.src = `data:image/png;base64,${maskPngBase64}`;
  }, [mode, maskPngBase64]);

  useEffect(() => {
    const container = containerRef.current;
    if (container === null) {
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
      setCanvasUnavailable(err instanceof Error ? err.message : '2D context unavailable');
      return;
    }
    setCanvasUnavailable(null);

    const bgLayer = new Konva.Layer();
    const roiLayer = new Konva.Layer();

    bgLayer.add(
      new Konva.Rect({
        x: 0,
        y: 0,
        width: stageWidth,
        height: stageHeight,
        fill: '#252830',
        stroke: '#3D4452',
        strokeWidth: 1,
      }),
    );

    if (imgElement !== null && typeof Konva.Image === 'function') {
      bgLayer.add(
        new Konva.Image({
          x: 0,
          y: 0,
          width: stageWidth,
          height: stageHeight,
          image: imgElement,
        }),
      );
    }

    if (mode === 'roi') {
      const boxX = originXPx * scale;
      const boxY = originYPx * scale;
      const boxSize = cropSizePx * scale;
      const collarScaled = collarWidthPx * scale;

      const collarRect = new Konva.Rect({
        x: Math.max(0, boxX - collarScaled),
        y: Math.max(0, boxY - collarScaled),
        width: Math.min(stageWidth, boxSize + collarScaled * 2),
        height: Math.min(stageHeight, boxSize + collarScaled * 2),
        stroke: '#63B3ED',
        strokeWidth: 1.5,
        dash: [6, 4],
        listening: false,
      });

      const roiRect = new Konva.Rect({
        x: boxX,
        y: boxY,
        width: boxSize,
        height: boxSize,
        stroke: '#F6AD55',
        strokeWidth: 2,
        fill: 'rgba(246, 173, 85, 0.14)',
        draggable: true,
        dragBoundFunc: (pos) => ({
          x: Math.max(0, Math.min(stageWidth - boxSize, pos.x)),
          y: Math.max(0, Math.min(stageHeight - boxSize, pos.y)),
        }),
      });

      const resizeHandle = new Konva.Circle({
        x: boxX + boxSize,
        y: boxY + boxSize,
        radius: 7,
        fill: '#F6AD55',
        stroke: '#1A1D24',
        strokeWidth: 2,
        draggable: true,
        dragBoundFunc: (pos) => {
          const minSide = Math.max(16, Math.round(32 * scale));
          const maxSide = Math.min(stageWidth - boxX, stageHeight - boxY);
          const delta = Math.max(pos.x - boxX, pos.y - boxY);
          const clampedSide = Math.max(minSide, Math.min(maxSide, delta));
          return { x: boxX + clampedSide, y: boxY + clampedSide };
        },
      });

      roiRect.on('dragmove', () => {
        collarRect.position({
          x: Math.max(0, roiRect.x() - collarScaled),
          y: Math.max(0, roiRect.y() - collarScaled),
        });
        resizeHandle.position({
          x: roiRect.x() + boxSize,
          y: roiRect.y() + boxSize,
        });
      });

      roiRect.on('dragend', () => {
        onRoiChangeRef.current(
          Math.round(roiRect.x() / scale),
          Math.round(roiRect.y() / scale),
          cropSizePx,
        );
      });

      resizeHandle.on('dragmove', () => {
        const nextScaledSize = Math.max(16 * scale, resizeHandle.x() - boxX);
        roiRect.size({ width: nextScaledSize, height: nextScaledSize });
        collarRect.position({
          x: Math.max(0, boxX - collarScaled),
          y: Math.max(0, boxY - collarScaled),
        });
        collarRect.size({
          width: Math.min(
            stageWidth - Math.max(0, boxX - collarScaled),
            nextScaledSize + collarScaled * 2,
          ),
          height: Math.min(
            stageHeight - Math.max(0, boxY - collarScaled),
            nextScaledSize + collarScaled * 2,
          ),
        });
      });

      resizeHandle.on('dragend', () => {
        const nextSizePx = Math.max(32, Math.round((resizeHandle.x() - boxX) / scale));
        onRoiChangeRef.current(originXPx, originYPx, nextSizePx);
      });

      roiLayer.add(collarRect);
      roiLayer.add(roiRect);
      roiLayer.add(resizeHandle);
    }

    stage.add(bgLayer);
    stage.add(roiLayer);
    stage.draw();

    return () => {
      stage.destroy();
    };
  }, [
    imgElement,
    stageWidth,
    stageHeight,
    scale,
    mode,
    originXPx,
    originYPx,
    cropSizePx,
    collarWidthPx,
  ]);

  const paintAt = useCallback(
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
      ctx.fillStyle = 'rgba(255, 255, 255, 1)';
      ctx.beginPath();
      ctx.arc(x, y, Math.max(4, brushRadiusPx), 0, Math.PI * 2);
      ctx.fill();
    },
    [brushRadiusPx],
  );

  const commitMask = useCallback((): void => {
    const canvas = maskCanvasRef.current;
    if (canvas === null) {
      return;
    }
    onMaskChange(stripPngDataUrlPrefix(canvas.toDataURL('image/png')));
  }, [onMaskChange]);

  return (
    <div class="stage-frame">
      <div class="stage-frame__meta">
        <span>
          Canvas: {safeW}×{safeH} px
        </span>
        <span>
          {mode === 'roi'
            ? `1:1 ROI (${String(originXPx)}, ${String(originYPx)}) ${String(cropSizePx)}×${String(cropSizePx)}px · Collar ${String(collarWidthPx)}px`
            : 'Painted Alpha Mask Mode'}
        </span>
      </div>
      {canvasUnavailable !== null ? (
        <div class="stage-frame__placeholder" role="alert">
          Konva stage unavailable: {canvasUnavailable}
        </div>
      ) : (
        <div class="stage-canvas-stack">
          <div
            ref={containerRef}
            class="stage-frame__canvas"
            role="img"
            aria-label="PatchEdit 1:1 cropper stage"
          />
          {mode === 'mask' && (
            <canvas
              ref={maskCanvasRef}
              width={safeW}
              height={safeH}
              class="mask-paint-overlay"
              style={{ width: `${String(stageWidth)}px`, height: `${String(stageHeight)}px` }}
              aria-label="PatchEdit mask painting overlay"
              onPointerDown={(e) => {
                isPaintingRef.current = true;
                paintAt(e.clientX, e.clientY);
              }}
              onPointerMove={(e) => {
                if (isPaintingRef.current) {
                  paintAt(e.clientX, e.clientY);
                }
              }}
              onPointerUp={() => {
                if (isPaintingRef.current) {
                  isPaintingRef.current = false;
                  commitMask();
                }
              }}
              onPointerLeave={() => {
                if (isPaintingRef.current) {
                  isPaintingRef.current = false;
                  commitMask();
                }
              }}
            />
          )}
        </div>
      )}
      {mode === 'mask' && (
        <div class="button-row">
          <button
            type="button"
            class="btn btn--ghost"
            onClick={() => {
              const canvas = maskCanvasRef.current;
              const ctx = canvas?.getContext('2d');
              if (canvas && ctx) {
                ctx.clearRect(0, 0, canvas.width, canvas.height);
              }
              onMaskChange(null);
            }}
          >
            Clear Mask
          </button>
        </div>
      )}
    </div>
  );
}
