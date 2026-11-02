// agentd load test: the SDK's calls, at a steady rate, with event streams held
// open. Runs against BASE_URL with API_KEY; no model is ever called (agentd's
// step 1 runs no engine), and nothing reaches AWS.
//
//   crud     agents, sessions, files and skills: create, list, read
//   events   user.message events sent to existing sessions
//   streams  STREAMS concurrent event streams, each delivered 5 events
//   files    1 MB uploads, metadata reads, and skill bundle downloads
import { check, sleep } from 'k6';
import http from 'k6/http';
import { Counter, Trend } from 'k6/metrics';

import { BASE_URL, MANAGED, headers, json, unique } from './lib/client.js';

const RATE = Number(__ENV.RATE || 20);
const STREAMS = Number(__ENV.STREAMS || 50);
const DURATION = __ENV.DURATION || '2m';

const streamDelivered = new Counter('stream_events_delivered');
const streamDuration = new Trend('stream_duration', true);

export const options = {
  scenarios: {
    crud: { executor: 'constant-arrival-rate', exec: 'crud', rate: RATE, timeUnit: '1s', duration: DURATION, preAllocatedVUs: 20, maxVUs: 200 },
    events: { executor: 'constant-arrival-rate', exec: 'events', rate: RATE * 2, timeUnit: '1s', duration: DURATION, preAllocatedVUs: 20, maxVUs: 200 },
    streams: { executor: 'constant-vus', exec: 'streams', vus: STREAMS, duration: DURATION },
    files: { executor: 'constant-arrival-rate', exec: 'files', rate: Math.max(1, Math.round(RATE / 4)), timeUnit: '1s', duration: DURATION, preAllocatedVUs: 10, maxVUs: 100 },
  },
  thresholds: JSON.parse(open('./thresholds.json')),
  summaryTrendStats: ['avg', 'p(50)', 'p(95)', 'p(99)', 'max'],
};

const SKILL_MD = '---\nname: perf-skill\ndescription: A load-test skill.\n---\n# perf\n';
const MEGABYTE = 'x'.repeat(1024 * 1024);

export function setup() {
  // agentd may still be booting: wait up to a minute for it to answer.
  for (let i = 0; i < 60; i++) {
    if (json('GET', '/v1/agents?limit=1', null, MANAGED, 'setup.ready').status === 200) break;
    sleep(1);
  }
  const agent = json('POST', '/v1/agents', { name: unique('perf-agent'), model: 'claude-sonnet-4-6' }, MANAGED, 'agents.create').json();
  const env = json('POST', '/v1/environments', { name: unique('perf-env') }, MANAGED, 'environments.create').json();
  const sessions = [];
  for (let i = 0; i < 20; i++) {
    sessions.push(json('POST', '/v1/sessions', { agent: agent.id, environment_id: env.id }, MANAGED, 'sessions.create').json().id);
  }
  const skill = http.post(`${BASE_URL}/v1/skills`, {
    'files[]': http.file(SKILL_MD, 'perf-skill/SKILL.md', 'text/markdown'),
    display_title: unique('perf-skill'),
  }, { headers: headers(null), tags: { endpoint: 'skills.create' } }).json();
  // The bundle download takes the numeric version, not the skver_ id (live),
  // and only the beta version shape carries it.
  const versions = http.get(`${BASE_URL}/v1/skills/${skill.id}/versions`, {
    headers: headers('skills-2025-10-02'),
    tags: { endpoint: 'skills.versions' },
  }).json();
  return { agent: agent.id, env: env.id, sessions, skill: skill.id, version: versions.data[0].version };
}

const ok = (res, name) => check(res, { [`${name} 2xx`]: (r) => r.status >= 200 && r.status < 300 });

export function crud(s) {
  switch (__ITER % 6) {
    case 0: ok(json('POST', '/v1/agents', { name: unique('a'), model: 'claude-sonnet-4-6' }, MANAGED, 'agents.create'), 'agents.create'); break;
    case 1: ok(json('GET', '/v1/agents?limit=20', null, MANAGED, 'agents.list'), 'agents.list'); break;
    case 2: ok(json('POST', '/v1/sessions', { agent: s.agent, environment_id: s.env }, MANAGED, 'sessions.create'), 'sessions.create'); break;
    case 3: ok(json('GET', '/v1/sessions?limit=20', null, MANAGED, 'sessions.list'), 'sessions.list'); break;
    case 4: ok(http.get(`${BASE_URL}/v1/files?limit=20`, { headers: headers(null), tags: { endpoint: 'files.list' } }), 'files.list'); break;
    default: ok(http.get(`${BASE_URL}/v1/skills?limit=20`, { headers: headers(null), tags: { endpoint: 'skills.list' } }), 'skills.list');
  }
}

export function events(s) {
  const id = s.sessions[__ITER % s.sessions.length];
  const body = { events: [{ type: 'user.message', content: [{ type: 'text', text: 'hello' }] }] };
  ok(json('POST', `/v1/sessions/${id}/events`, body, MANAGED, 'events.send'), 'events.send');
}

// One stream per VU at a time: open it, send 5 events, delete the session
// (agentd then ends the stream with session.deleted), and check every event
// arrived, in order, on the stream.
export async function streams(s) {
  const session = json('POST', '/v1/sessions', { agent: s.agent, environment_id: s.env }, MANAGED, 'sessions.create').json();
  const started = Date.now();
  const stream = http.asyncRequest('GET', `${BASE_URL}/v1/sessions/${session.id}/events/stream`, null, {
    headers: headers(MANAGED, { accept: 'text/event-stream' }),
    tags: { endpoint: 'events.stream' },
    timeout: '60s',
  });
  await new Promise((r) => setTimeout(r, 200));
  for (let i = 0; i < 5; i++) {
    const body = { events: [{ type: 'user.message', content: [{ type: 'text', text: `n${i}` }] }] };
    json('POST', `/v1/sessions/${session.id}/events`, body, MANAGED, 'events.send');
    await new Promise((r) => setTimeout(r, 500));
  }
  json('DELETE', `/v1/sessions/${session.id}`, null, MANAGED, 'sessions.delete');
  const res = await stream;
  streamDuration.add(Date.now() - started);
  const frames = (res.body || '').split('\n').filter((l) => l.startsWith('event: ')).map((l) => l.slice(7));
  const delivered = frames.filter((f) => f === 'user.message').length;
  streamDelivered.add(delivered);
  check(res, {
    'stream 200': (r) => r.status === 200,
    'stream delivered every event': () => delivered === 5,
    'stream ended with session.deleted': () => frames[frames.length - 1] === 'session.deleted',
  });
}

export function files(s) {
  const up = http.post(`${BASE_URL}/v1/files`, { file: http.file(MEGABYTE, `${unique('f')}.txt`, 'text/plain') }, {
    headers: headers(null),
    tags: { endpoint: 'files.upload' },
  });
  if (!ok(up, 'files.upload')) return;
  ok(http.get(`${BASE_URL}/v1/files/${up.json().id}`, { headers: headers(null), tags: { endpoint: 'files.metadata' } }), 'files.metadata');
  // Uploaded files are not downloadable (live: file_not_downloadable), so the
  // download path is a skill version's bundle.
  ok(http.get(`${BASE_URL}/v1/skills/${s.skill}/versions/${s.version}/content`, {
    headers: headers(null),
    tags: { endpoint: 'skills.download' },
    responseType: 'binary',
  }), 'skills.download');
}
