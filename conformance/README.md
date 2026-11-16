# Conformance

Everything here proves agentd matches the live Anthropic API.

- `fixtures/`: captures from the live API, which the other suites compare against.
- `rust/`: replays the captured requests through the composed router and diffs the responses (gated by
  `AGENTD_PG_TESTS=1`).
- `python/`: the official Python SDK, `anthropic==2.3.0`, under pytest.
- `typescript/`: the official TypeScript SDK, `@anthropic-ai/sdk@1.3.0`, under `node:test`.
- `java/`: the official Java SDK, `com.anthropic:anthropic-java` 2.52.0, under JUnit 5 on Java 21.

The SDK suites run against a running agentd, with only the base URL changed:

```sh
export AGENTD_BASE_URL=http://127.0.0.1:8080
export AGENTD_TEST_KEY=$(agentd keys create --org org_sdk --workspace wrkspc_sdk --name sdk)
export AGENTD_TEST_ADMIN_KEY=$(agentd admin-keys create --org org_sdk --name sdk)
(cd conformance/python && pip install -r requirements.txt && pytest -q)
(cd conformance/typescript && npm ci && npm test)
(cd conformance/java && ./gradlew test)
```

The Python runtime cases (`test_runtime.py`) also need the loop on the scripted model and the official
worker: start `python conformance/mock_model.py --model mock/scripted conformance/fixtures/mock-model.json`,
run agentd with `AGENTD_MODELS=mock/scripted` and `AGENTD_MODEL_BASE_URL=http://127.0.0.1:18766/v1`, put
`ant` on the `PATH`, and set `AGENTD_TEST_WORKER=1`. Each case starts `ant beta:worker poll` on a
`self_hosted` environment of its own, its key made with `agentd environment-keys create` for
`AGENTD_TEST_ORG` and `AGENTD_TEST_WORKSPACE`. Cases that need a sandbox (mounted files, session outputs,
credentials swapped at egress) and the scripted demo (`demo/`) run in `agentd-worker`'s CI.

CI runs them on every pull request, against the binary built from the branch.
