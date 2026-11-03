import assert from 'node:assert/strict';
import { test } from 'node:test';

import { client, name } from './client.mjs';

test('an agent serves every required field', async () => {
  const agent = await client().beta.agents.create({
    name: name('ts-agent'),
    model: 'claude-sonnet-4-6',
    tools: [{ type: 'agent_toolset_20260401', configs: [{ name: 'bash', enabled: false }] }],
  });
  assert.deepEqual(agent.execution_identity, { type: 'service_account' });
  const [toolset] = agent.tools;
  assert.deepEqual(toolset.configs[0], {
    enabled: false,
    name: 'bash',
    permission_policy: { type: 'always_allow' },
    type: 'bash',
  });
});

test('an update needs no version and null clears', async () => {
  const c = client();
  const agent = await c.beta.agents.create({ name: name('ts-upd'), model: 'claude-sonnet-4-6', description: 'x' });
  const updated = await c.beta.agents.update(agent.id, { description: null });
  assert.equal(updated.description, null);
  assert.equal(updated.version, agent.version + 1);
});

test('archived environments are listed only on request', async () => {
  const c = client();
  const env = await c.beta.environments.create({ name: name('ts-env') });
  await c.beta.environments.archive(env.id);
  const active = [];
  for await (const e of c.beta.environments.list()) active.push(e.id);
  assert.ok(!active.includes(env.id));
  const all = [];
  for await (const e of c.beta.environments.list({ include_archived: true })) all.push(e.id);
  assert.ok(all.includes(env.id));
});
