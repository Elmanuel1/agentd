import assert from 'node:assert/strict';
import { test } from 'node:test';

import Anthropic from '@anthropic-ai/sdk';

import { baseURL, client, countedFetch, name } from './client.mjs';

test('the SDK bearer auth is accepted', async () => {
  const page = await client().beta.environments.list();
  assert.ok(Array.isArray(page.data));
});

test('an unknown key is an AuthenticationError with a request id', async () => {
  const bad = new Anthropic({ apiKey: 'not-a-key', baseURL });
  await assert.rejects(bad.beta.environments.list(), (err) => {
    assert.ok(err instanceof Anthropic.AuthenticationError);
    assert.match(err.requestID, /^req_/);
    return true;
  });
});

test('an unknown beta is a BadRequestError', async () => {
  await assert.rejects(client().beta.environments.list({ betas: ['not-a-beta-2020-01-01'] }), (err) => {
    assert.ok(err instanceof Anthropic.BadRequestError);
    return true;
  });
});

test('a conflict is not retried', async () => {
  const { counter, fetchFn } = countedFetch();
  const c = client({ fetch: fetchFn });
  const store = await c.beta.memoryStores.create({ name: name('ts-conflict') });
  await c.beta.memoryStores.memories.create(store.id, { path: '/a.md', content: 'one' });
  counter.requests = 0;
  await assert.rejects(c.beta.memoryStores.memories.create(store.id, { path: '/a.md', content: 'two' }), (err) => {
    assert.ok(err instanceof Anthropic.ConflictError);
    return true;
  });
  assert.equal(counter.requests, 1);
});
