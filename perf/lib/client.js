// The headers the official SDKs send (TypeScript SDK 1.3.0), so agentd sees
// SDK traffic. BASE_URL and API_KEY name the server and a key it accepts.
import http from 'k6/http';

export const BASE_URL = __ENV.BASE_URL || 'http://localhost:8080';
const API_KEY = __ENV.API_KEY;
if (!API_KEY) throw new Error('API_KEY is required: perf/run.sh makes one with agentd keys create');

export const MANAGED = 'managed-agents-2026-04-01';
export const MEMORY = 'agent-memory-2026-07-22';

export function headers(beta, extra = {}) {
  const h = {
    'x-api-key': API_KEY,
    'anthropic-version': '2023-06-01',
    'user-agent': 'Anthropic/JS 1.3.0',
    'x-stainless-lang': 'js',
    'x-stainless-package-version': '1.3.0',
    'x-stainless-runtime': 'node',
    'x-stainless-retry-count': '0',
    ...extra,
  };
  if (beta) h['anthropic-beta'] = beta;
  return h;
}

/** A JSON request, tagged with its endpoint so thresholds can name it. */
export function json(method, path, body, beta, endpoint) {
  return http.request(method, `${BASE_URL}${path}`, body === null ? null : JSON.stringify(body), {
    headers: headers(beta, { 'content-type': 'application/json' }),
    tags: { endpoint },
  });
}

export function unique(prefix) {
  return `${prefix}-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 8)}`;
}
