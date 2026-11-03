// The official TypeScript SDK against a running agentd, with only the base URL
// changed. AGENTD_BASE_URL (default http://127.0.0.1:8080) and AGENTD_TEST_KEY
// name the server and a key it accepts.
import Anthropic from '@anthropic-ai/sdk';

export const baseURL = process.env.AGENTD_BASE_URL ?? 'http://127.0.0.1:8080';
export const apiKey = process.env.AGENTD_TEST_KEY;

export function client(options = {}) {
  return new Anthropic({ apiKey, baseURL, ...options });
}

export function name(prefix) {
  return `${prefix}-${Math.random().toString(16).slice(2, 10)}`;
}

// A fetch that counts the requests the client sends, retries included.
export function countedFetch() {
  const counter = { requests: 0 };
  const fetchFn = (input, init) => {
    counter.requests += 1;
    return fetch(input, init);
  };
  return { counter, fetchFn };
}
