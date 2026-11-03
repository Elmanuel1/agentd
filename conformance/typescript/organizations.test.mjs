// The Admin API through the SDK: an admin key reaches /v1/organizations and
// nothing else, and a workspace key does not reach it.
import assert from 'node:assert/strict';
import { test } from 'node:test';
import Anthropic from '@anthropic-ai/sdk';
import { admin, client } from './client.mjs';

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
