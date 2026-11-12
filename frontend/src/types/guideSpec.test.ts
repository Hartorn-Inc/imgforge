import { describe, expect, test } from 'vitest';
import {
  GuideSpecValidationException,
  createDefaultGuideSpec,
  parseGuideSpecJson,
  serializeGuideSpecJson,
  type GuideSpec,
  type SpatialGuideVariant,
} from './guideSpec';

describe('GuideSpec defensive serialization and validation', () => {
  test.each<{ id: string; variant: SpatialGuideVariant }>([
    { id: 'roundtrip-blockout-default', variant: 'Blockout' },
    { id: 'roundtrip-overlay-default', variant: 'OverlayGuide' },
  ])('$id', ({ variant }) => {
    const original = createDefaultGuideSpec(variant);
    const serialized = serializeGuideSpecJson(original);
    const parsed = parseGuideSpecJson(serialized);

    expect(parsed.ok).toBe(true);
    if (parsed.ok) {
      expect(parsed.value).toEqual(original);
    }
  });

  test('normalizes lowercase hex colors during validation round-trip', () => {
    const base = createDefaultGuideSpec('Blockout');
    const withLowerHex: GuideSpec = {
      ...base,
      actionVectors: [
        {
          id: 'vec-lower',
          label: 'Lowercase hex vector',
          fromXRatio: 0.1,
          fromYRatio: 0.2,
          toXRatio: 0.8,
          toYRatio: 0.7,
          strokeHex: '#abcdef',
          haloHex: '#112233',
          strokeWidthPx: 3,
          style: 'dashed',
        },
      ],
    };

    const parsed = parseGuideSpecJson(serializeGuideSpecJson(withLowerHex, false));
    expect(parsed.ok).toBe(true);
    if (parsed.ok) {
      expect(parsed.value.actionVectors[0]?.strokeHex).toBe('#ABCDEF');
      expect(parsed.value.actionVectors[0]?.haloHex).toBe('#112233');
    }
  });

  test.each<{ id: string; rawJson: string; expectedPath: string }>([
    {
      id: 'reject-empty-string',
      rawJson: '   ',
      expectedPath: '$',
    },
    {
      id: 'reject-malformed-json-syntax',
      rawJson: '{"schemaVersion": "1.0",',
      expectedPath: '$',
    },
    {
      id: 'reject-non-object-root',
      rawJson: '["not", "an", "object"]',
      expectedPath: '$',
    },
    {
      id: 'reject-unsupported-schema-version',
      rawJson: JSON.stringify({
        ...createDefaultGuideSpec('Blockout'),
        schemaVersion: '2.0',
      }),
      expectedPath: 'schemaVersion',
    },
    {
      id: 'reject-unknown-variant',
      rawJson: JSON.stringify({
        ...createDefaultGuideSpec('Blockout'),
        variant: 'UnknownGuide',
      }),
      expectedPath: 'variant',
    },
    {
      id: 'reject-overlay-guide-without-target-image',
      rawJson: JSON.stringify({
        ...createDefaultGuideSpec('OverlayGuide'),
        targetImagePath: '',
      }),
      expectedPath: 'targetImagePath',
    },
    {
      id: 'reject-blockout-with-non-null-target-image',
      rawJson: JSON.stringify({
        ...createDefaultGuideSpec('Blockout'),
        targetImagePath: 'runs/v001.png',
      }),
      expectedPath: 'targetImagePath',
    },
    {
      id: 'reject-inverted-depth-band',
      rawJson: JSON.stringify({
        ...createDefaultGuideSpec('Blockout'),
        depthBands: [
          {
            id: 'band-bad',
            label: 'Inverted Band',
            plane: 'foreground',
            yStartRatio: 0.8,
            yEndRatio: 0.4,
            fillHex: '#252830',
          },
        ],
      }),
      expectedPath: 'depthBands[0].yEndRatio',
    },
    {
      id: 'reject-out-of-range-mannequin-x-ratio',
      rawJson: JSON.stringify({
        ...createDefaultGuideSpec('Blockout'),
        mannequins: [
          {
            id: 'm1',
            subjectSlug: 'scout',
            label: 'Scout',
            xRatio: 1.25,
            yRatio: 0.5,
            heightMeters: 1.75,
            scaleFactor: 1,
            facing: 'front',
            poseHint: 'Standing',
            paletteHex: '#63B3ED',
          },
        ],
      }),
      expectedPath: 'mannequins[0].xRatio',
    },
    {
      id: 'reject-invalid-hex-on-action-vector',
      rawJson: JSON.stringify({
        ...createDefaultGuideSpec('OverlayGuide'),
        actionVectors: [
          {
            id: 'v1',
            label: 'Bad Color',
            fromXRatio: 0.1,
            fromYRatio: 0.1,
            toXRatio: 0.9,
            toYRatio: 0.9,
            strokeHex: 'red',
            haloHex: '#000000',
            strokeWidthPx: 2,
            style: 'solid',
          },
        ],
      }),
      expectedPath: 'actionVectors[0].strokeHex',
    },
    {
      id: 'reject-overlay-region-with-fewer-than-two-points',
      rawJson: JSON.stringify({
        ...createDefaultGuideSpec('OverlayGuide'),
        overlayRegions: [
          {
            id: 'r1',
            label: 'Single Point',
            kind: 'polyline',
            points: [{ xRatio: 0.5, yRatio: 0.5 }],
            strokeHex: '#68D391',
            haloHex: '#1A1D24',
            fillHex: '#68D391',
            opacity: 0.2,
          },
        ],
      }),
      expectedPath: 'overlayRegions[0].points',
    },
  ])('$id', ({ rawJson, expectedPath }) => {
    const parsed = parseGuideSpecJson(rawJson);
    expect(parsed.ok).toBe(false);
    if (!parsed.ok) {
      expect(parsed.errors.some((err) => err.path === expectedPath)).toBe(true);
    }
  });

  test('serializeGuideSpecJson fails closed on invalid in-memory GuideSpec', () => {
    const invalidSpec = {
      ...createDefaultGuideSpec('Blockout'),
      canvasWidth: -100,
    } as unknown as GuideSpec;

    expect(() => serializeGuideSpecJson(invalidSpec)).toThrow(GuideSpecValidationException);
  });
});
