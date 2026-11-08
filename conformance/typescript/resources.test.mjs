import assert from 'node:assert/strict';
import { test } from 'node:test';

import { toFile } from '@anthropic-ai/sdk';

import { client, name } from './client.mjs';

test('memories work with the agent-memory beta alone', async () => {
  const c = client();
  const store = await c.beta.memoryStores.create({ name: name('ts-mem') });
  const memory = await c.beta.memoryStores.memories.create(store.id, { path: '/notes/a.md', content: 'hello' });
  assert.equal(memory.path, '/notes/a.md');
  const deleted = await c.beta.memoryStores.memories.delete(memory.id, { memory_store_id: store.id });
  assert.deepEqual([deleted.id, deleted.type], [memory.id, 'memory_deleted']);
  const gone = await c.beta.memoryStores.delete(store.id);
  assert.deepEqual([gone.id, gone.type], [store.id, 'memory_store_deleted']);
});

test('environment delete returns the deleted body', async () => {
  const c = client();
  const env = await c.beta.environments.create({
    name: name('ts-env'),
    config: { type: 'cloud', networking: { type: 'unrestricted' } },
  });
  const deleted = await c.beta.environments.delete(env.id);
  assert.deepEqual([deleted.id, deleted.type], [env.id, 'environment_deleted']);
});

test('agents auto-paginate across pages', async () => {
  const c = client();
  const created = new Set();
  for (let i = 0; i < 3; i++) {
    created.add((await c.beta.agents.create({ name: name('ts-page'), model: 'claude-sonnet-4-6' })).id);
  }
  const seen = new Set();
  for await (const agent of c.beta.agents.list({ limit: 1 })) seen.add(agent.id);
  for (const id of created) assert.ok(seen.has(id), id);
});

test('an upload over 2 MB keeps its bytes', async () => {
  const c = client();
  const payload = Buffer.alloc(3 * 1024 * 1024, 'a');
  const meta = await c.beta.files.upload({ file: await toFile(payload, 'big.bin') });
  assert.equal(meta.size_bytes, payload.length);
  const deleted = await c.files.delete(meta.id);
  assert.deepEqual([deleted.id, deleted.type], [meta.id, 'file_deleted']);
});
