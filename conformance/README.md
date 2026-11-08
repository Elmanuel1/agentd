# Conformance

Everything here proves agentd matches the live Anthropic API.

- `fixtures/`: captures from the live API, which the other suites compare against.
- `rust/`: replays the captured requests through the composed router and diffs the responses (gated by
  `AGENTD_PG_TESTS=1`).
- `python/`: the official Python SDK, `anthropic==2.3.0`, under pytest.
- `typescript/`: the official TypeScript SDK, `@anthropic-ai/sdk@1.3.0`, under `node:test`.

The SDK suites run against a running agentd, with only the base URL changed:

```sh
export AGENTD_BASE_URL=http://127.0.0.1:8080
export AGENTD_TEST_KEY=$(agentd keys create --org org_sdk --workspace wrkspc_sdk --name sdk)
export AGENTD_TEST_ADMIN_KEY=$(agentd admin-keys create --org org_sdk --name sdk)
(cd conformance/python && pip install -r requirements.txt && pytest -q)
(cd conformance/typescript && npm ci && npm test)
```

CI runs both on every pull request, against the binary built from the branch.
