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

  test('moves mannequin clamped within [0, 1]', () => {
    const spec = createDefaultGuideSpec('Blockout');
    const updated = moveMannequinInSpec(spec, 'mannequin-01', 1.2, -0.1);
    expect(updated.mannequins[0]?.xRatio).toBe(1);
    expect(updated.mannequins[0]?.yRatio).toBe(0);
  });

  test('resizes depth band while enforcing minimum thickness', () => {
    const spec = createDefaultGuideSpec('Blockout');
    const updated = resizeDepthBandInSpec(spec, 'band-bg', 0.5, 0.505);
    expect(updated.depthBands[0]?.yStartRatio).toBe(0.5);
    expect(updated.depthBands[0]?.yEndRatio).toBe(0.52);
  });

  test('updates action vector endpoints and appends new action vectors', () => {
    const spec = createDefaultGuideSpec('Blockout');
    const moved = updateActionVectorInSpec(spec, 'vector-01', 0.1, 0.2, 0.8, 0.9);
    expect(moved.actionVectors[0]?.fromXRatio).toBe(0.1);
    expect(moved.actionVectors[0]?.toYRatio).toBe(0.9);

    const added = appendActionVectorToSpec(moved, 0.2, 0.3, 0.6, 0.4);
    expect(added.actionVectors).toHaveLength(2);
    expect(added.actionVectors[1]?.id).toBe('vector-02');
  });

  test('updates overlay region control points and strips PNG data URL prefix', () => {
    const spec = createDefaultGuideSpec('OverlayGuide');
    const updated = updateOverlayRegionPointInSpec(spec, 'region-01', 1, 0.85, 0.9);
    expect(updated.overlayRegions[0]?.points[1]).toEqual({ xRatio: 0.85, yRatio: 0.9 });
    expect(stripPngDataUrlPrefix('data:image/png;base64,iVBORw0KGgo=')).toBe('iVBORw0KGgo=');
  });
});
