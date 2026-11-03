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

Each session's agent runs in its sandbox, as the session's harness, so the
demo runs once agentd starts sessions with it. `mock-model.json` scripts every
model reply for a free, deterministic run; `fixture/` is the slice of the demo
data it needs (`make_fixture.py` rebuilds it; each document keeps its first
three pages). A live run uses the full inputs under `~/tosspaper-eval-data`.

`summary.json` has each upload's outcome, the grader's criteria passed, the
token usage, the key's spend and the wall time; `compare.py` sets them beside
the earlier `harness-sim` runs.
