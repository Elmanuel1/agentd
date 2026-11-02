import assert from 'node:assert/strict';
import { test } from 'node:test';

import { client, name } from './client.mjs';

test('the stream names every event and ends with session.deleted', async () => {
  const c = client();
  const agent = await c.beta.agents.create({ name: name('ts-sess'), model: 'claude-sonnet-4-6' });
  const env = await c.beta.environments.create({ name: name('ts-sess-env') });
  const session = await c.beta.sessions.create({ agent: agent.id, environment_id: env.id });
  const stream = await c.beta.sessions.events.stream(session.id);
  const seen = [];
  const reading = (async () => {
    for await (const event of stream) seen.push(event.type);
  })();
  await new Promise((r) => setTimeout(r, 300));
  await c.beta.sessions.events.send(session.id, {
    events: [{ type: 'user.message', content: [{ type: 'text', text: 'hi' }] }],
  });
  await new Promise((r) => setTimeout(r, 200));
  await c.beta.sessions.delete(session.id);
  await reading;
  assert.deepEqual(seen, ['user.message', 'session.deleted']);
});
