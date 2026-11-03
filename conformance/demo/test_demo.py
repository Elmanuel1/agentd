"""The scripted demo replay: the extraction agents run on the fixture slice of
the demo data, with every model call answered by mock-model.json.

Runs when AGENTD_TEST_DEMO=1 says agentd's sessions run their harness against
that script.
"""

import json
import os
import subprocess
import sys
from pathlib import Path

import pytest

HERE = Path(__file__).resolve().parent

pytestmark = pytest.mark.skipif(os.environ.get("AGENTD_TEST_DEMO") != "1", reason="needs AGENTD_TEST_DEMO=1")


def test_the_scripted_demo_runs_every_upload_to_a_graded_result(tmp_path: Path) -> None:
    env = {**os.environ, "AGENTD_API_KEY": os.environ["AGENTD_TEST_KEY"]}
    cmd = [sys.executable, str(HERE / "run_demo.py"), "--out", str(tmp_path), "--data", str(HERE / "fixture"),
           "--model", "claude-haiku-4-5", "--timeout", "300"]
    subprocess.run(cmd, env=env, check=True, timeout=900)
    summary = json.loads((tmp_path / "summary.json").read_text())
    assert {u: o["result"] for u, o in summary["outcomes"].items()} == {"0": "satisfied", "1": "satisfied", "2": "satisfied"}
    assert sorted(summary["grades"]) == ["0", "1", "2"]
    assert all(g["criteria"] == 2 for g in summary["grades"].values())
    u0 = json.loads((tmp_path / "u0-result.json").read_text())
    assert u0["name"] == "Welland Street Trail and Road Reconstruction"
    types = [json.loads(line)["type"] for line in (tmp_path / "events-final.jsonl").read_text().splitlines()]
    for t in ("user.define_outcome", "session.thread_created", "agent.thread_message_sent", "agent.thread_message_received",
              "span.outcome_evaluation_start", "span.outcome_evaluation_end", "session.status_idle"):
        assert t in types, t
    assert summary["usage"]["input_tokens"] > 0
