import { render } from 'preact';
import { act } from 'preact/test-utils';
import { afterEach, beforeEach, describe, expect, test, vi } from 'vitest';
import {
  type GuideSpec,
  createDefaultGuideSpec,
  serializeGuideSpecJson,
} from '../../types/guideSpec';
import { SpatialGuideView } from './SpatialGuideView';

vi.mock('konva', () => {
  class MockNode {
    public on(): void {}
  }
  class MockLayer {
    public add(): void {}
  }
  class MockStage {
    public add(): void {}
    public draw(): void {}
    public destroy(): void {}
  }
  return {
    default: {
      Stage: MockStage,
      Layer: MockLayer,
      Rect: MockNode,
      Ellipse: MockNode,
      Line: MockNode,
      Text: MockNode,
      Circle: MockNode,
      Arrow: MockNode,
    },
  };
});

async function flushMicrotasks(): Promise<void> {
  for (let i = 0; i < 4; i += 1) {
    await new Promise((resolve) => {
      setTimeout(resolve, 0);
    });
  }
}

describe('SpatialGuideView', () => {
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
    vi.unstubAllGlobals();
  });

  test.each<{
    id: string;
    makeJsonDraft: (baseJson: string) => string;
    expectValidationError: boolean;
    expectRenderCalls: number;
    expectedRenderedTitle?: string;
  }>([
    {
      id: 'fails-render-with-validation-error-when-unapplied-json-draft-is-invalid',
      makeJsonDraft: () => '{ "schemaVersion": 999 }',
      expectValidationError: true,
      expectRenderCalls: 0,
    },
    {
      id: 'validates-applies-and-renders-updated-spec-when-json-draft-has-valid-unapplied-edits',
      makeJsonDraft: (baseJson) =>
        baseJson.replace('Default Blockout SpatialGuide', 'Updated Draft Title'),
      expectValidationError: false,
      expectRenderCalls: 1,
      expectedRenderedTitle: 'Updated Draft Title',
    },
  ])(
    '$id',
    async ({ makeJsonDraft, expectValidationError, expectRenderCalls, expectedRenderedTitle }) => {
      const baseSpec = createDefaultGuideSpec('Blockout');
      const baseJson = serializeGuideSpecJson(baseSpec);
      const editedDraft = makeJsonDraft(baseJson);

      const renderBodies: unknown[] = [];
      vi.stubGlobal(
        'fetch',
        vi.fn((url: string, init?: RequestInit) => {
          const parsedBody =
            typeof init?.body === 'string'
              ? (JSON.parse(init.body) as { guideSpec?: unknown })
              : {};
          if (url === '/api/guide/validate') {
            return Promise.resolve(
              new Response(
                JSON.stringify({ valid: true, spec: parsedBody.guideSpec, errors: [] }),
                { status: 200 },
              ),
            );
          }
          if (url === '/api/guide/render') {
            renderBodies.push(parsedBody.guideSpec);
            return Promise.resolve(
              new Response(
                JSON.stringify({
                  id: 'guide-01',
                  variant: 'Blockout',
                  width: 1024,
                  height: 1024,
                  pngBase64: 'iVBORw0KGgo=',
                  errors: [],
                }),
                { status: 200 },
              ),
            );
          }
          return Promise.resolve(new Response(null, { status: 404 }));
        }),
      );

      let currentSpec = baseSpec;
      let currentJson = editedDraft;
      const onSpecUpdate = vi.fn((nextSpec: GuideSpec, nextJson: string) => {
        currentSpec = nextSpec;
        currentJson = nextJson;
      });

      await act(() => {
        render(
          <SpatialGuideView
            spec={currentSpec}
            jsonDraft={currentJson}
            workspace={null}
            exportedMaskBase64={null}
            onSpecUpdate={onSpecUpdate}
            onJsonDraftChange={vi.fn()}
            onMaskCaptured={vi.fn()}
          />,
          mountNode,
        );
      });

      const renderBtn = Array.from(mountNode.querySelectorAll('button')).find(
        (b) => b.textContent === 'Render Guide PNG',
      );
      expect(renderBtn).not.toBeUndefined();
      await act(async () => {
        renderBtn?.click();
        await flushMicrotasks();
      });

      expect(mountNode.querySelector('.validation-errors') !== null).toBe(expectValidationError);
      expect(renderBodies).toHaveLength(expectRenderCalls);
      if (expectedRenderedTitle !== undefined) {
        expect((renderBodies[0] as { title?: string } | undefined)?.title).toBe(
          expectedRenderedTitle,
        );
        expect(onSpecUpdate).toHaveBeenCalledTimes(1);
      }
    },
  );
});
