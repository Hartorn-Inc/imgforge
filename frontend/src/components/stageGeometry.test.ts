import { describe, expect, test } from 'vitest';
import { createDefaultGuideSpec } from '../types/guideSpec';
import {
  appendActionVectorToSpec,
  clampUnitRatio,
  moveMannequinInSpec,
  resizeDepthBandInSpec,
  stripPngDataUrlPrefix,
  updateActionVectorInSpec,
  updateOverlayRegionPointInSpec,
} from './stageGeometry';

describe('SpatialGuide interactive geometry helpers', () => {
  test.each<{ id: string; input: number; expected: number }>([
    { id: 'clamps-negative-to-zero', input: -0.25, expected: 0 },
    { id: 'clamps-above-one-to-one', input: 1.5, expected: 1 },
    { id: 'rounds-to-four-decimals', input: 0.123456, expected: 0.1235 },
    { id: 'handles-nan-as-zero', input: Number.NaN, expected: 0 },
  ])('$id', ({ input, expected }) => {
    expect(clampUnitRatio(input)).toBe(expected);
  });

  test.each<{
    id: string;
    xRatio: number;
    yRatio: number;
    expectedX: number;
    expectedY: number;
  }>([
    {
      id: 'moves-mannequin-within-unit-bounds',
      xRatio: 0.42,
      yRatio: 0.78,
      expectedX: 0.42,
      expectedY: 0.78,
    },
    {
      id: 'clamps-mannequin-overflow-and-negative-coords',
      xRatio: 1.2,
      yRatio: -0.1,
      expectedX: 1,
      expectedY: 0,
    },
  ])('$id', ({ xRatio, yRatio, expectedX, expectedY }) => {
    const spec = createDefaultGuideSpec('Blockout');
    const updated = moveMannequinInSpec(spec, 'mannequin-01', xRatio, yRatio);
    expect(updated.mannequins[0]?.xRatio).toBe(expectedX);
    expect(updated.mannequins[0]?.yRatio).toBe(expectedY);
  });

  test.each<{
    id: string;
    yStart: number;
    yEnd: number;
    expectedStart: number;
    expectedEnd: number;
  }>([
    {
      id: 'resizes-depth-band-within-valid-range',
      yStart: 0.1,
      yEnd: 0.45,
      expectedStart: 0.1,
      expectedEnd: 0.45,
    },
    {
      id: 'enforces-minimum-depth-band-thickness',
      yStart: 0.5,
      yEnd: 0.505,
      expectedStart: 0.5,
      expectedEnd: 0.52,
    },
  ])('$id', ({ yStart, yEnd, expectedStart, expectedEnd }) => {
    const spec = createDefaultGuideSpec('Blockout');
    const updated = resizeDepthBandInSpec(spec, 'band-bg', yStart, yEnd);
    expect(updated.depthBands[0]?.yStartRatio).toBe(expectedStart);
    expect(updated.depthBands[0]?.yEndRatio).toBe(expectedEnd);
  });

  test.each<{
    id: string;
    action: 'update' | 'append';
    coords: readonly [number, number, number, number];
    expectedCount: number;
    expectedToY: number;
  }>([
    {
      id: 'updates-existing-action-vector-endpoints',
      action: 'update',
      coords: [0.1, 0.2, 0.8, 0.9],
      expectedCount: 1,
      expectedToY: 0.9,
    },
    {
      id: 'appends-new-action-vector-with-sequential-id',
      action: 'append',
      coords: [0.2, 0.3, 0.6, 0.4],
      expectedCount: 2,
      expectedToY: 0.4,
    },
  ])('$id', ({ action, coords, expectedCount, expectedToY }) => {
    const spec = createDefaultGuideSpec('Blockout');
    const result =
      action === 'update'
        ? updateActionVectorInSpec(spec, 'vector-01', coords[0], coords[1], coords[2], coords[3])
        : appendActionVectorToSpec(spec, coords[0], coords[1], coords[2], coords[3]);
    expect(result.actionVectors).toHaveLength(expectedCount);
    expect(result.actionVectors.at(-1)?.toYRatio).toBe(expectedToY);
    if (action === 'append') {
      expect(result.actionVectors[1]?.id).toBe('vector-02');
    }
  });

  test.each<{
    id: string;
    pointIndex: number;
    xRatio: number;
    yRatio: number;
    dataUrl: string;
    expectedBase64: string;
  }>([
    {
      id: 'updates-overlay-point-and-strips-png-data-url-prefix',
      pointIndex: 1,
      xRatio: 0.85,
      yRatio: 0.9,
      dataUrl: 'data:image/png;base64,iVBORw0KGgo=',
      expectedBase64: 'iVBORw0KGgo=',
    },
    {
      id: 'clamps-overlay-point-and-preserves-raw-base64',
      pointIndex: 0,
      xRatio: -0.2,
      yRatio: 1.4,
      dataUrl: 'iVBORw0KGgo=',
      expectedBase64: 'iVBORw0KGgo=',
    },
  ])('$id', ({ pointIndex, xRatio, yRatio, dataUrl, expectedBase64 }) => {
    const spec = createDefaultGuideSpec('OverlayGuide');
    const updated = updateOverlayRegionPointInSpec(spec, 'region-01', pointIndex, xRatio, yRatio);
    expect(updated.overlayRegions[0]?.points[pointIndex]).toEqual({
      xRatio: clampUnitRatio(xRatio),
      yRatio: clampUnitRatio(yRatio),
    });
    expect(stripPngDataUrlPrefix(dataUrl)).toBe(expectedBase64);
  });
});
