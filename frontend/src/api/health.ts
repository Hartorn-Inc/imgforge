/** === HEALTH ENDPOINT CLIENT ===
 *
 * PURPOSE:
 * Queries and validates `GET /api/health` on the embedded `127.0.0.1:<port>`
 * Rust server so the Web Canvas SPA accurately surfaces backend availability.
 *
 * DESIGN RATIONALE ("WHY"):
 * When the SPA runs standalone during UI development or when the local server
 * restarts, `GET /api/health` may fail or return malformed JSON. Returning a
 * typed discriminated union guarantees the UI never crashes on partial data.
 *
 * EXECUTION LIFECYCLE / DATA FLOW:
 * 1. `fetchHealthStatus()` invokes `fetch('/api/health')` with optional `AbortSignal`.
 * 2. `parseHealthPayload()` checks required fields fail-closed.
 * 3. Returns `{ ok: true, health }` or `{ ok: false, error, httpStatus }`.
 */

import type { HealthStatus } from '../types/domain';

/** Discriminated response from `fetchHealthStatus`. */
export type HealthCheckResult =
  | { readonly ok: true; readonly health: HealthStatus }
  | { readonly ok: false; readonly error: string; readonly httpStatus: number | null };

/** Validate an untyped JSON object returned by `GET /api/health`. */
export function parseHealthPayload(payload: unknown): HealthStatus | null {
  if (typeof payload !== 'object' || payload === null || Array.isArray(payload)) {
    return null;
  }
  const record = payload as Record<string, unknown>;
  const status = record['status'];
  const version = record['version'];
  const backend = record['backend'];
  const authProvisioned = record['authProvisioned'];

  if (status !== 'ok' && status !== 'degraded') {
    return null;
  }
  if (typeof version !== 'string' || version.trim().length === 0) {
    return null;
  }
  if (typeof backend !== 'string' || backend.trim().length === 0) {
    return null;
  }
  if (typeof authProvisioned !== 'boolean') {
    return null;
  }
  return {
    status,
    version,
    backend,
    authProvisioned,
  };
}

/** Fetch `GET /api/health` using the provided fetch implementation (injectable for tests). */
export async function fetchHealthStatus(
  signal?: AbortSignal,
  fetchImpl: typeof fetch = fetch,
): Promise<HealthCheckResult> {
  try {
    const requestInit: RequestInit = {
      method: 'GET',
      headers: { Accept: 'application/json' },
    };
    if (signal !== undefined) {
      requestInit.signal = signal;
    }
    const response = await fetchImpl('/api/health', requestInit);
    let body: unknown = null;
    try {
      body = (await response.json()) as unknown;
    } catch (err) {
      if (signal?.aborted) {
        throw err;
      }
      body = null;
    }
    const health = parseHealthPayload(body);

    if (!response.ok) {
      return {
        ok: false,
        error: health
          ? `Server reported degraded health (HTTP ${String(response.status)})`
          : `Health check failed with HTTP ${String(response.status)}`,
        httpStatus: response.status,
      };
    }

    if (health === null) {
      return {
        ok: false,
        error: 'Malformed payload from GET /api/health',
        httpStatus: response.status,
      };
    }

    return { ok: true, health };
  } catch (err) {
    const message = err instanceof Error ? err.message : 'Unable to reach local imgforge server';
    return {
      ok: false,
      error: message,
      httpStatus: null,
    };
  }
}
