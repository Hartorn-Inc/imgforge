import { describe, expect, test } from 'vitest';
import { fetchHealthStatus, parseHealthPayload } from './health';

describe('GET /api/health client', () => {
  test.each<{ id: string; payload: unknown; expectedOk: boolean }>([
    {
      id: 'accepts-ok-health-payload',
      payload: {
        status: 'ok',
        version: '0.1.0',
        backend: 'CodexNativeBackend',
        authProvisioned: true,
      },
      expectedOk: true,
    },
    {
      id: 'accepts-degraded-health-payload',
      payload: {
        status: 'degraded',
        version: '0.1.0',
        backend: 'CodexNativeBackend',
        authProvisioned: false,
      },
      expectedOk: true,
    },
    {
      id: 'rejects-unknown-status-value',
      payload: {
        status: 'unknown',
        version: '0.1.0',
        backend: 'CodexNativeBackend',
        authProvisioned: true,
      },
      expectedOk: false,
    },
    {
      id: 'rejects-non-boolean-auth-provisioned',
      payload: {
        status: 'ok',
        version: '0.1.0',
        backend: 'CodexNativeBackend',
        authProvisioned: 'yes',
      },
      expectedOk: false,
    },
    {
      id: 'rejects-null-payload',
      payload: null,
      expectedOk: false,
    },
  ])('$id', ({ payload, expectedOk }) => {
    const parsed = parseHealthPayload(payload);
    expect(parsed !== null).toBe(expectedOk);
  });

  test.each<{
    id: string;
    mockFetch: typeof fetch;
    expectedOk: boolean;
    expectedHttpStatus: number | null;
  }>([
    {
      id: 'returns-health-on-200-ok',
      mockFetch: () =>
        Promise.resolve(
          new Response(
            JSON.stringify({
              status: 'ok',
              version: '0.1.0',
              backend: 'CodexNativeBackend',
              authProvisioned: true,
            }),
            { status: 200, headers: { 'Content-Type': 'application/json' } },
          ),
        ),
      expectedOk: true,
      expectedHttpStatus: null,
    },
    {
      id: 'returns-error-on-503-degraded',
      mockFetch: () =>
        Promise.resolve(
          new Response(
            JSON.stringify({
              status: 'degraded',
              version: '0.1.0',
              backend: 'CodexNativeBackend',
              authProvisioned: false,
            }),
            { status: 503, headers: { 'Content-Type': 'application/json' } },
          ),
        ),
      expectedOk: false,
      expectedHttpStatus: 503,
    },
    {
      id: 'returns-error-on-malformed-200-body',
      mockFetch: () =>
        Promise.resolve(
          new Response(JSON.stringify({ unexpected: true }), {
            status: 200,
            headers: { 'Content-Type': 'application/json' },
          }),
        ),
      expectedOk: false,
      expectedHttpStatus: 200,
    },
    {
      id: 'returns-error-on-network-rejection',
      mockFetch: () => Promise.reject(new Error('Connection refused')),
      expectedOk: false,
      expectedHttpStatus: null,
    },
  ])('$id', async ({ mockFetch, expectedOk, expectedHttpStatus }) => {
    const result = await fetchHealthStatus(undefined, mockFetch);
    expect(result.ok).toBe(expectedOk);
    if (!result.ok) {
      expect(result.httpStatus).toBe(expectedHttpStatus);
    }
  });
});
