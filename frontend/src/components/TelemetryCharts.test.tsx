import { render } from 'preact';
import { act } from 'preact/test-utils';
import { afterEach, beforeEach, describe, expect, test } from 'vitest';
import type { ColorInspectionTelemetry } from '../types/domain';
import { TelemetryCharts } from './TelemetryCharts';

describe('TelemetryCharts', () => {
  let mountNode: HTMLDivElement;

  beforeEach(() => {
    mountNode = document.createElement('div');
    document.body.appendChild(mountNode);
  });

  afterEach(async () => {
    await act(() => {
      render(null, mountNode);
    });
    mountNode.remove();
  });

  test.each<{
    id: string;
    telemetry: ColorInspectionTelemetry;
    expectedBarCount: number;
    expectedPointCount: number;
    expectedHeaderText: string;
  }>([
    {
      id: 'renders-lstar-histogram-bars-and-oklab-vectorscope-points',
      telemetry: {
        width: 512,
        height: 512,
        opaquePixelCount: 262144,
        lStarBins: [
          { binIndex: 0, minLStar: 0, maxLStar: 5, count: 1000, fraction: 0.2 },
          { binIndex: 10, minLStar: 50, maxLStar: 55, count: 4000, fraction: 0.8 },
        ],
        vectorscopePoints: [
          { oklabL: 0.5, a: 0, b: 0, lStar: 50, hex: '#808080' },
          { oklabL: 0.7, a: 0.2, b: -0.1, lStar: 70, hex: '#C06090' },
        ],
      },
      expectedBarCount: 2,
      expectedPointCount: 2,
      expectedHeaderText: '2 samples',
    },
    {
      id: 'handles-empty-telemetry-arrays-without-crashing',
      telemetry: {
        width: 64,
        height: 64,
        opaquePixelCount: 0,
        lStarBins: [],
        vectorscopePoints: [],
      },
      expectedBarCount: 0,
      expectedPointCount: 0,
      expectedHeaderText: '0 samples',
    },
  ])('$id', async ({ telemetry, expectedBarCount, expectedPointCount, expectedHeaderText }) => {
    await act(() => {
      render(<TelemetryCharts telemetry={telemetry} />, mountNode);
    });

    const histSvg = mountNode.querySelector('svg[aria-label="20-bin CIE L* luminance histogram"]');
    const vecSvg = mountNode.querySelector(
      'svg[aria-label="OKLab a and b vectorscope scatter plot"]',
    );
    expect(histSvg).not.toBeNull();
    expect(vecSvg).not.toBeNull();

    // Background rect is index 0; remaining rects are histogram bins.
    const histRects = histSvg?.querySelectorAll('rect') ?? [];
    expect(histRects).toHaveLength(1 + expectedBarCount);

    // Outer dashed ring is index 0; remaining circles are vectorscope samples.
    const vecCircles = vecSvg?.querySelectorAll('circle') ?? [];
    expect(vecCircles).toHaveLength(1 + expectedPointCount);
    expect(mountNode.textContent).toContain(expectedHeaderText);
  });
});
