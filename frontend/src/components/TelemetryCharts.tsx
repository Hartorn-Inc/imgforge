/** === RUST TELEMETRY VISUALIZATIONS (L* HISTOGRAM & OKLAB VECTORSCOPE) ===
 *
 * PURPOSE:
 * Renders the 20-bin CIE `L*` histogram and OKLab `(a, b)` vectorscope
 * computed in Rust core (`compute_l_star_histogram_and_vectorscope`).
 * Zero color-space math is duplicated in TypeScript.
 */

import type { JSX } from 'preact';
import type { ColorInspectionTelemetry } from '../types/domain';
import { projectOklabToPlot } from './views/runInspectorHelpers';

export interface TelemetryChartsProps {
  readonly telemetry: ColorInspectionTelemetry;
}

const VECTORSCOPE_SIZE = 220;

/** Side-by-side `L*` luminance histogram and OKLab `(a, b)` vectorscope. */
export function TelemetryCharts({ telemetry }: TelemetryChartsProps): JSX.Element {
  const maxFraction = telemetry.lStarBins.reduce(
    (acc, bin) => (bin.fraction > acc ? bin.fraction : acc),
    0.01,
  );
  const histW = 320;
  const histH = 140;
  const barCount = Math.max(1, telemetry.lStarBins.length);
  const barStep = histW / barCount;

  return (
    <div class="split-two">
      <div class="telemetry-card">
        <header class="card__header">
          <h4>CIE L* Luminance Histogram ({telemetry.lStarBins.length} bins)</h4>
          <span class="muted-note">{telemetry.opaquePixelCount.toLocaleString()} opaque px</span>
        </header>
        <svg
          class="telemetry-svg"
          viewBox={`0 0 ${String(histW)} ${String(histH + 22)}`}
          role="img"
          aria-label="20-bin CIE L* luminance histogram"
        >
          <rect x={0} y={0} width={histW} height={histH} fill="#181B21" rx={4} />
          {telemetry.lStarBins.map((bin, idx) => {
            const barH = Math.max(1, Math.round((bin.fraction / maxFraction) * (histH - 12)));
            const x = idx * barStep + 1;
            const y = histH - barH;
            const gray = Math.round((bin.binIndex / barCount) * 215 + 30);
            return (
              <rect
                key={bin.binIndex}
                x={x}
                y={y}
                width={Math.max(2, barStep - 2)}
                height={barH}
                fill={`rgb(${String(gray)}, ${String(gray)}, ${String(gray)})`}
                stroke="#63B3ED"
                strokeWidth={0.5}
              >
                <title>
                  L* {bin.minLStar.toFixed(0)}..{bin.maxLStar.toFixed(0)}:{' '}
                  {(bin.fraction * 100).toFixed(1)}% ({bin.count} px)
                </title>
              </rect>
            );
          })}
          <text x={4} y={histH + 15} fill="#94A3B8" font-size="10" font-family="monospace">
            0 L*
          </text>
          <text
            x={histW / 2 - 14}
            y={histH + 15}
            fill="#94A3B8"
            font-size="10"
            font-family="monospace"
          >
            50 L*
          </text>
          <text x={histW - 36} y={histH + 15} fill="#94A3B8" font-size="10" font-family="monospace">
            100 L*
          </text>
        </svg>
      </div>

      <div class="telemetry-card">
        <header class="card__header">
          <h4>OKLab (a, b) Vectorscope</h4>
          <span class="muted-note">{telemetry.vectorscopePoints.length} samples</span>
        </header>
        <svg
          class="telemetry-svg telemetry-svg--square"
          viewBox={`0 0 ${String(VECTORSCOPE_SIZE)} ${String(VECTORSCOPE_SIZE)}`}
          role="img"
          aria-label="OKLab a and b vectorscope scatter plot"
        >
          <rect
            x={0}
            y={0}
            width={VECTORSCOPE_SIZE}
            height={VECTORSCOPE_SIZE}
            fill="#181B21"
            rx={6}
          />
          <circle
            cx={VECTORSCOPE_SIZE / 2}
            cy={VECTORSCOPE_SIZE / 2}
            r={VECTORSCOPE_SIZE / 2 - 14}
            fill="none"
            stroke="#3D4452"
            strokeDasharray="4 4"
          />
          <line
            x1={10}
            y1={VECTORSCOPE_SIZE / 2}
            x2={VECTORSCOPE_SIZE - 10}
            y2={VECTORSCOPE_SIZE / 2}
            stroke="#3D4452"
            strokeWidth={1}
          />
          <line
            x1={VECTORSCOPE_SIZE / 2}
            y1={10}
            x2={VECTORSCOPE_SIZE / 2}
            y2={VECTORSCOPE_SIZE - 10}
            stroke="#3D4452"
            strokeWidth={1}
          />
          {telemetry.vectorscopePoints.map((pt, idx) => {
            const { x, y } = projectOklabToPlot(pt.a, pt.b, VECTORSCOPE_SIZE);
            return (
              <circle
                key={`${pt.hex}-${String(idx)}`}
                cx={x}
                cy={y}
                r={2.2}
                fill={pt.hex}
                opacity={0.85}
              >
                <title>
                  {pt.hex} · L*={pt.lStar.toFixed(1)} a={pt.a.toFixed(3)} b={pt.b.toFixed(3)}
                </title>
              </circle>
            );
          })}
          <text
            x={VECTORSCOPE_SIZE / 2 + 4}
            y={14}
            fill="#94A3B8"
            font-size="9"
            font-family="monospace"
          >
            +b
          </text>
          <text
            x={VECTORSCOPE_SIZE - 20}
            y={VECTORSCOPE_SIZE / 2 - 4}
            fill="#94A3B8"
            font-size="9"
            font-family="monospace"
          >
            +a
          </text>
        </svg>
      </div>
    </div>
  );
}
