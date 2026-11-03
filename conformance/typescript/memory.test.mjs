import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { test } from 'node:test';

import { client, name } from './client.mjs';

test('a memory store updates and leaves the default list once archived', async () => {
  const c = client();
  const s = await c.beta.memoryStores.create({ name: name('ts-store'), metadata: { a: '1' } });
  const updated = await c.beta.memoryStores.update(s.id, { name: 'renamed', metadata: { a: null, b: '2' } });
  assert.deepEqual([updated.name, updated.metadata], ['renamed', { b: '2' }]);
  await c.beta.memoryStores.archive(s.id);
  const live = [];
  for await (const x of c.beta.memoryStores.list({ limit: 100 })) live.push(x.id);
  assert.ok(!live.includes(s.id));
  const all = await c.beta.memoryStores.list({ limit: 100, include_archived: true });
  assert.ok(all.data.some((x) => x.id === s.id));
});

test('memories list with depth, full view and pages', async () => {
  const c = client();
  const s = await c.beta.memoryStores.create({ name: name('ts-mem') });
  for (const path of ['/a.md', '/n/b.md', '/n/d/c.md']) {
    await c.beta.memoryStores.memories.create(s.id, { path, content: path });
  }
  const top = await c.beta.memoryStores.memories.list(s.id, { depth: 1 });
  assert.deepEqual(top.data.map((m) => [m.type, m.path]).sort(), [['memory', '/a.md'], ['memory_prefix', '/n/']]);
  const full = await c.beta.memoryStores.memories.list(s.id, { view: 'full', path_prefix: '/n/' });
  assert.deepEqual(full.data.map((m) => m.content).sort(), ['/n/b.md', '/n/d/c.md']);
  const seen = [];
  for await (const m of c.beta.memoryStores.memories.list(s.id, { limit: 1 })) seen.push(m.path);
  assert.equal(seen.length, 3);
});

test('versions filter, an unchanged update adds none, and the head cannot be redacted', async () => {
  const c = client();
  const s = await c.beta.memoryStores.create({ name: name('ts-ver') });
  const m = await c.beta.memoryStores.memories.create(s.id, { path: '/v.md', content: 'one' });
  const sha = createHash('sha256').update('one').digest('hex');
  await c.beta.memoryStores.memories.update(m.id, {
    memory_store_id: s.id,
    content: 'one',
    precondition: { type: 'content_sha256', content_sha256: sha },
  });
  await c.beta.memoryStores.memories.update(m.id, { memory_store_id: s.id, content: 'two' });
  const versions = await c.beta.memoryStores.memoryVersions.list(s.id, { memory_id: m.id });
  assert.deepEqual(versions.data.map((v) => v.operation), ['modified', 'created']);
  const [head, old] = versions.data;
  await assert.rejects(c.beta.memoryStores.memoryVersions.redact(head.id, { memory_store_id: s.id }), (err) => {
    assert.equal(err.status, 409);
    return true;
  });
  const redacted = await c.beta.memoryStores.memoryVersions.redact(old.id, { memory_store_id: s.id });
  assert.ok(redacted.redacted_at);
});
