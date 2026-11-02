# The extraction demo on agentd

`run_demo.py` replays the tender-extraction demo (the agents first run on Claude
Managed Agents, then in local simulations under `harness-sim/`) against agentd,
using only the official Anthropic Python SDK with its base URL changed. It
creates the orchestrator and its five roster agents with their skills, mounts
the documents and helpers as session file resources, sends each upload of the
plan as a `user.define_outcome`, records the event stream, downloads the
outputs, and grades each upload's `result.json` with the demo's own grader
prompt and rubric in a grader session.

## Scripted run (CI)

`mock-model.json` answers every model call, so the run is free and
deterministic. `fixture/` is the slice of the demo data it needs
(`make_fixture.py` rebuilds it; each document keeps its first three pages).

```bash
docker build -f docker/sandbox/Dockerfile -t agentd-sandbox .
AGENTD_RUNTIME=docker AGENTD_SANDBOX_IMAGE=agentd-sandbox \
  AGENTD_MOCK_MODEL=conformance/demo/mock-model.json ./target/debug/agentd &
AGENTD_TEST_DEMO=1 AGENTD_BASE_URL=http://127.0.0.1:8080 AGENTD_TEST_KEY=<key> \
  pytest conformance/demo
```

## Live run

agentd calls the model itself, so the model key goes to agentd, never to the
script or the sandbox. For `openai/gpt-6-luna` through OpenRouter:

```bash
AGENTD_RUNTIME=docker AGENTD_SANDBOX_IMAGE=agentd-sandbox \
  AGENTD_OPENAI_API_KEY=<openrouter key> AGENTD_MODELS=openai/gpt-6-luna ./target/debug/agentd &
OPENROUTER_API_KEY=<same key, read only for its spend> AGENTD_BASE_URL=http://127.0.0.1:8080 \
  AGENTD_API_KEY=<key> python conformance/demo/run_demo.py --out runs/agentd-2025-45-r1
python conformance/demo/compare.py runs/agentd-2025-45-r1
```

`summary.json` has each upload's outcome, the grader's criteria passed, the
token usage, the key's spend and the wall time; `compare.py` sets them beside
the earlier `harness-sim` runs.
