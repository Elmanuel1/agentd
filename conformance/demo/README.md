# The extraction demo on agentd

`run_demo.py` replays the tender-extraction demo (the agents first run on Claude
Managed Agents, then in local simulations under `harness-sim/`) against agentd,
using only the official Anthropic Python SDK with its base URL changed. It
creates the orchestrator and its five roster agents with their skills, mounts
the documents and helpers as session file resources, sends each upload of the
plan as a `user.define_outcome`, records the event stream, downloads the
outputs, and grades each upload's `result.json` with the demo's own grader
prompt and rubric in a grader session.

## Runs

Each session's agent runs in its sandbox, as the session's harness, and calls
the model through the egress service with the operator's model key. The script
uses only the official Python SDK, with its base URL changed.

Scripted (CI): `mock-model.json` scripts every model reply, served by
`conformance/mock_model.py`; `fixture/` is the slice of the demo data it needs
(`make_fixture.py` rebuilds it; each document keeps its first three pages).

```bash
python conformance/mock_model.py --port 18766 conformance/demo/mock-model.json &
echo mock | agentd model-key set --host mock-model.agentd.test
AGENTD_MODEL_BASE_URL=http://mock-model.agentd.test:18766/v1 agentd &
AGENTD_TEST_DEMO=1 AGENTD_TEST_KEY=<key> pytest conformance/demo
```

Live, on the full inputs under `~/tosspaper-eval-data`, with
`openai/gpt-6-luna` through OpenRouter:

```bash
agentd model-key set < openrouter-key          # stored in Secrets Manager, never printed
AGENTD_MODELS=openai/gpt-6-luna agentd &
OPENROUTER_API_KEY=<same key, read only for its spend> AGENTD_API_KEY=<key> \
  python conformance/demo/run_demo.py --out runs/agentd-2025-45-r1
python conformance/demo/compare.py runs/agentd-2025-45-r1
```

`summary.json` has each upload's outcome, the grader's criteria passed, the
token usage, the key's spend and the wall time; `compare.py` sets them beside
the earlier `harness-sim` runs.
