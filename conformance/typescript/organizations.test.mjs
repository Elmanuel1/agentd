// The Admin API through the SDK: an admin key reaches /v1/organizations and
// nothing else, and a workspace key does not reach it.
import assert from 'node:assert/strict';
import { test } from 'node:test';
import Anthropic from '@anthropic-ai/sdk';
import { admin, client, name } from './client.mjs';

test('the admin key retrieves its organization', async () => {
  const org = await admin().organization.retrieve();
  assert.equal(org.type, 'organization');
  assert.equal(org.id, 'org_sdk');
  assert.equal(org.name, 'org_sdk');
});

test('a workspace key is refused on the admin API', async () => {
  await assert.rejects(client().organization.retrieve(), Anthropic.AuthenticationError);
});

test('an admin key is refused on workspace routes', async () => {
  await assert.rejects(admin().beta.environments.list(), Anthropic.AuthenticationError);
});

test('a created workspace is retrieved and listed', async () => {
  const workspaces = admin().organization.workspaces;
  const created = await workspaces.create({ name: name('sdk'), tags: { suite: 'typescript' } });
  assert.equal(created.type, 'workspace');
  assert.match(created.id, /^wrkspc_/);
  assert.equal(created.archived_at, null);
  assert.deepEqual(created.data_residency, {
    allowed_inference_geos: 'unrestricted',
    default_inference_geo: 'global',
    workspace_geo: 'us',
  });
  assert.deepEqual(await workspaces.retrieve(created.id), created);
  const ids = [];
  for await (const w of workspaces.list({ limit: 2 })) ids.push(w.id);
  assert.ok(ids.includes(created.id), 'auto-paging reaches every workspace');
});

test('a bad workspace is a bad request', async () => {
  const workspaces = admin().organization.workspaces;
  await assert.rejects(workspaces.create({ name: 'x', display_color: 'red' }), Anthropic.BadRequestError);
  await assert.rejects(workspaces.retrieve('wrkspc_01missingmissingmissin'), Anthropic.NotFoundError);
});

test('an update merges and an archive is one way', async () => {
  const workspaces = admin().organization.workspaces;
  const created = await workspaces.create({ name: name('sdk'), tags: { a: '1', b: '2' } });
  const updated = await workspaces.update(created.id, { name: 'renamed', tags: { b: null } });
  assert.equal(updated.name, 'renamed');
  assert.deepEqual(updated.tags, { a: '1' });
  const archived = await workspaces.archive(created.id);
  assert.notEqual(archived.archived_at, null);
  const active = (await workspaces.list({ limit: 1000 })).data.map((w) => w.id);
  const all = (await workspaces.list({ limit: 1000, include_archived: true })).data.map((w) => w.id);
  assert.ok(!active.includes(created.id) && all.includes(created.id));
});
