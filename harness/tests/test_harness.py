"""The harness's parts on their own: the prompt, the tool policy, the model adapter, the tool host,
the outcome verdict, and one thread turn against the scripted model with a recording agentd."""

import asyncio
import json
import os
import signal
import sys
import threading
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "conformance"))

from agentd_harness import __main__ as entry
from agentd_harness import mounts, prompt, toolhost
from agentd_harness.budget import Budget, BudgetReached
from agentd_harness.model import Model, text, usage
from agentd_harness.outcome import explain
from agentd_harness.thread import Thread, thread_id
from agentd_harness.tools import Policy


def test_skills_and_credentials_go_into_the_prompt():
    assert prompt.skill_description("---\nname: x\ndescription: Does x.\n---\n# X") == "Does x."
    assert prompt.skill_description("# none") == ""
    text_ = prompt.system_prompt({"system": "Be brief."}, [("x", "Does x.")], [{"name": "w", "description": "Works."}],
                                 [{"name": "GH", "description": "GitHub", "hosts": ["api.github.com"]},
                                  {"name": "NONE", "description": "n", "hosts": None}])
    assert text_.startswith("Be brief.") and "/workspace/skills/x/SKILL.md" in text_ and "`w`: Works." in text_
    assert "`GH`: GitHub (sent to: api.github.com)" in text_ and "no host; it lists none" in text_


def test_the_policy_follows_the_agent_config():
    agent = {"tools": [
        {"type": "agent_toolset_20260401", "default_config": {"permission_policy": {"type": "always_allow"}},
         "configs": [{"name": "write", "permission_policy": {"type": "always_ask"}}, {"name": "grep", "enabled": False}]},
        {"type": "custom", "name": "lookup", "description": "Look up.", "input_schema": {"type": "object"}}]}
    p = Policy(agent)
    assert p.allowed["write"] == "always_ask" and p.allowed["bash"] == "always_allow" and "grep" not in p.allowed
    names = [f["function"]["name"] for f in p.functions()]
    assert "bash" in names and "lookup" in names and "grep" not in names
    assert Policy({}).functions() == []


def test_tool_calls_and_usage_map_from_chat_completions():
    m = Model(model_name="m", functions=[], base_url="http://x/v1", user="a")
    response = {"choices": [{"message": {"tool_calls": [
        {"id": "c1", "function": {"name": "bash", "arguments": "{\"command\": \"ls\"}"}}]}}],
        "usage": {"prompt_tokens": 100, "completion_tokens": 7, "prompt_tokens_details": {"cached_tokens": 40}}}
    assert m._parse_actions(response) == [{"tool": "bash", "input": {"command": "ls"}, "tool_call_id": "c1"}]
    assert usage(response) == {"input_tokens": 60, "cache_read_input_tokens": 40, "cache_creation_input_tokens": 0, "output_tokens": 7}
    sent = m._prepare_messages_for_api([{"role": "assistant", "content": None, "tool_calls": [{"id": "c9"}]},
                                       {"role": "exit", "content": "x"}])
    assert sent[-1] == {"role": "tool", "tool_call_id": "c9", "content": "Not run."}
    assert text({"content": [{"type": "text", "text": "boom"}], "is_error": True}) == "Error: boom"


def test_the_tool_host_runs_the_sdk_toolset(tmp_path):
    lines = [json.dumps({"id": 1, "name": "write", "input": {"file_path": str(tmp_path / "a.txt"), "content": "hi\n"}}),
             json.dumps({"id": 2, "name": "read", "input": {"file_path": str(tmp_path / "a.txt")}}),
             json.dumps({"id": 3, "name": "nope", "input": {}})]
    out = []

    async def feed():
        for line in lines:
            yield line

    asyncio.run(toolhost.serve(feed(), out.append, workdir=tmp_path, roots=[tmp_path], read_only=[]))
    replies = [json.loads(o) for o in out]
    assert not replies[0]["is_error"] and "hi" in replies[1]["content"][0]["text"]
    assert replies[2]["is_error"]


def test_a_verdict_becomes_a_result_and_explanation():
    assert explain({"result": "satisfied"}) == ("satisfied", "All criteria are met.")
    result, why = explain({"result": "needs_revision", "findings": [{"criterion": "A", "finding": "wrong total"}]})
    assert result == "needs_revision" and why == "- A: wrong total"
    assert explain({"result": "garbage"})[0] == "needs_revision"


def test_thread_ids_parse_as_agentd_ids():
    tid = thread_id()
    assert tid.startswith("sthr_01") and len(tid) == 29 and tid[5:].isalnum()


class Recorder:
    """agentd as a thread sees it: recorded events get ids."""

    def __init__(self):
        self.events = []

    def emit(self, events, thread=None):
        out = []
        for e in events:
            e = {**e, "id": f"sevt_{len(self.events)}", "session_thread_id": thread}
            self.events.append(e)
            out.append(e)
        return out


class Tools:
    def __init__(self):
        self.calls = []

    def run(self, name, tool_input, owner=None):
        self.calls.append((name, tool_input))
        return [{"type": "text", "text": "done"}], False


class Inbox:
    def check(self):
        pass


@pytest.fixture
def scripted_model():
    from mock_model import Script, handler
    from http.server import ThreadingHTTPServer
    replies = {"worker": [
        {"content": [{"type": "tool_use", "id": "t1", "name": "bash", "input": {"command": "ls"}}],
         "usage": {"input_tokens": 10, "output_tokens": 2, "cost": 0.0612}},
        {"content": [{"type": "text", "text": "all done"}], "usage": {"input_tokens": 12, "output_tokens": 3, "cost": 0.06}}],
        "unpriced": [{"content": [{"type": "tool_use", "id": "t1", "name": "bash", "input": {"command": "ls"}}],
                      "usage": {"input_tokens": 10, "output_tokens": 2, "cost": None}}]}
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler(Script(replies)))
    threading.Thread(target=server.serve_forever, daemon=True).start()
    yield f"http://127.0.0.1:{server.server_port}/v1"
    server.shutdown()


def scripted_run(base_url, budget=None):
    class Run:
        model_base_url = base_url
        agentd = Recorder()
        tools = Tools()
        inbox = Inbox()
    Run.budget = Budget()
    Run.budget.refresh({"budget": budget, "usage": {}})
    return Run


def thread_for(run, name):
    agent = {"name": name, "model": {"id": "m"}, "tools": [{"type": "agent_toolset_20260401"}]}
    policy = Policy(agent)
    return Thread(run, agent, "sys", policy.functions(), policy)


def test_a_turn_records_model_requests_messages_and_tool_calls(scripted_model):
    Run = scripted_run(scripted_model)
    reply = thread_for(Run, "worker").send("go")
    assert reply == "all done"
    kinds = [e["type"] for e in Run.agentd.events]
    assert kinds == ["span.model_request_start", "span.model_request_end", "session.usage", "agent.tool_use",
                     "agent.tool_result", "span.model_request_start", "span.model_request_end", "session.usage",
                     "agent.message"]
    costs = [e["usage"]["list_cost"] for e in Run.agentd.events if e["type"] == "session.usage"]
    assert costs == [{"amount": "6", "currency": "USD"}, {"amount": "12", "currency": "USD"}]
    use, result = Run.agentd.events[3], Run.agentd.events[4]
    assert use["name"] == "bash" and use["input"] == {"command": "ls"} and use["evaluated_permission"] == "allow"
    assert result["tool_use_id"] == use["id"] and Run.tools.calls == [("bash", {"command": "ls"})]
    assert Run.agentd.events[1]["model_request_start_id"] == Run.agentd.events[0]["id"]


def limit(cents):
    return {"type": "limit", "max_list_cost": {"amount": str(cents), "currency": "USD"}}


def test_the_budget_stops_the_next_model_request_once_the_cost_reaches_it(scripted_model):
    Run = scripted_run(scripted_model, limit(5))
    with pytest.raises(BudgetReached):
        thread_for(Run, "worker").send("go")
    kinds = [e["type"] for e in Run.agentd.events]
    assert kinds.count("span.model_request_start") == 1 and kinds[-1] == "agent.tool_result"
    Run.budget.refresh({"budget": limit(100), "usage": {"list_cost": {"amount": "6", "currency": "USD"}}})
    assert Run.budget.limit == 100 and Run.budget.check() is None


def test_a_model_with_no_list_price_stops_a_budgeted_session(scripted_model):
    Run = scripted_run(scripted_model, limit(1000))
    with pytest.raises(BudgetReached):
        thread_for(Run, "unpriced").send("go")
    assert [e["usage"] for e in Run.agentd.events if e["type"] == "session.usage"] == [{"list_cost": None}]
    unbudgeted = scripted_run(scripted_model)
    thread_for(unbudgeted, "unpriced").send("go")
    seeded = Budget()
    seeded.refresh({"budget": limit(1000), "usage": {"input_tokens": 5}})
    with pytest.raises(BudgetReached):
        seeded.check()


def test_a_budget_changed_while_the_session_runs_applies_at_the_next_request(scripted_model):
    class Agentd(Recorder):
        def emit(self, events, thread=None):
            return [{**e, "budget": limit(5)} if e["type"] == "session.usage" else e for e in super().emit(events, thread)]

    Run = scripted_run(scripted_model, limit(1000))
    Run.agentd = Agentd()
    with pytest.raises(BudgetReached):
        thread_for(Run, "worker").send("go")
    assert [e["type"] for e in Run.agentd.events].count("span.model_request_start") == 1 and Run.budget.limit == 5


def test_client_events_a_budget_stop_did_not_reach_run_with_the_next_work(monkeypatch):
    from agentd_harness.run import SessionRun

    class Agentd:
        statuses = []

        def session(self):
            return {"budget": limit(10), "usage": {"list_cost": {"amount": "0", "currency": "USD"}}}

        def emit(self, events):
            self.statuses += events

    class Work:
        batches = [["a", "b", "c"], ["d"]]

        def start(self):
            pass

        def next_work(self):
            return self.batches.pop(0) if self.batches else None

    handled = []

    def handle(event):
        handled.append(event)
        if event == "a":
            raise BudgetReached()

    run = SessionRun.__new__(SessionRun)
    run.agentd, run.inbox, run.budget, run.pending = Agentd(), Work(), Budget(), []
    run.handle, run.tools = handle, type("T", (), {"close": lambda self: None})()
    monkeypatch.setattr(mounts, "sync_outputs", lambda _agentd: 0)
    run.serve()
    assert handled == ["a", "b", "c", "d"]
    stops = [e["stop_reason"]["type"] for e in run.agentd.statuses if e["type"] == "session.status_idle"]
    assert stops == ["budget_reached", "end_turn"]


def test_a_lost_lease_is_left_alone_and_a_stale_beat_recovers(monkeypatch):
    import anthropic
    import httpx
    from agentd_harness import __main__ as entry

    monkeypatch.setattr(entry.time, "sleep", lambda _s: None)

    def status(code, body=None):
        request = httpx.Request("POST", "http://agentd/heartbeat")
        return anthropic.APIStatusError("x", response=httpx.Response(code, request=request), body=body)

    class Beat:
        def __init__(self, state="active", last="h2"):
            self.state, self.last_heartbeat, self.lease_extended = state, last, True

    class Work:
        def __init__(self, answers):
            self.answers, self.sent = list(answers), []

        def heartbeat(self, _id, *, expected_last_heartbeat, **_kw):
            self.sent.append(expected_last_heartbeat)
            answer = self.answers.pop(0)
            if isinstance(answer, Exception):
                raise answer
            return answer

    got = []
    entry.keep_lease(Work([status(412)]), "w", "e", got.append)
    assert got == ["lost"]
    stale = status(412, {"error": {"current_state": {"last_heartbeat": "h1"}}})
    work = Work([anthropic.APIConnectionError(request=httpx.Request("POST", "http://agentd")), stale, Beat("stopping")])
    got = []
    entry.keep_lease(work, "w", "e", got.append)
    assert got == ["stopping"] and work.sent == ["NO_HEARTBEAT", "NO_HEARTBEAT", "h1"]


def test_mounts_and_skills_stay_inside_their_directories(tmp_path):
    import io
    import zipfile

    from agentd_harness import mounts

    assert mounts.upload_path("file_1", "/mnt/session/uploads/docs/a.pdf") == mounts.UPLOADS / "docs/a.pdf"
    with pytest.raises(ValueError):
        mounts.upload_path("file_1", "/mnt/session/uploads/../../../etc/passwd")

    def archive(entries):
        buf = io.BytesIO()
        with zipfile.ZipFile(buf, "w") as z:
            for name, body in entries:
                z.writestr(name, body)
        return buf.getvalue()

    name, description = mounts.unpack_skill(archive([("cite/SKILL.md", "---\nname: cite\ndescription: Cites.\n---"),
                                                    ("cite/bin/run.py", "x")]), skills_dir=tmp_path)
    assert (name, description) == ("cite", "Cites.") and (tmp_path / "cite/bin/run.py").exists()
    with pytest.raises(ValueError):
        mounts.unpack_skill(archive([("SKILL.md", "---\nname: ../../etc\n---"), ("x", "y")]), skills_dir=tmp_path)
    with pytest.raises(ValueError):
        mounts.unpack_skill(archive([("SKILL.md", "---\nname: ok\n---"), ("../../escape.txt", "y")]), skills_dir=tmp_path)


def test_outputs_sync_skips_links_the_agent_made(tmp_path):
    from agentd_harness import mounts

    root, secret = tmp_path / "outputs", tmp_path / "secret"
    root.mkdir()
    secret.write_text("work token")
    (root / "result.json").write_text("{}")
    (root / "env").symlink_to(secret)

    class Agentd:
        saved = []

        def save_output(self, path, data, previous=None):
            self.saved.append((path, data))
            return f"file_{len(self.saved)}"

    agentd = Agentd()
    assert mounts.sync_outputs(agentd, root=root, state=tmp_path / "state.json") == 1
    assert agentd.saved == [("result.json", b"{}")]


def test_a_stopped_sandbox_saves_its_outputs_before_it_exits(monkeypatch):
    synced, exits = [], []
    monkeypatch.setattr(mounts, "sync_outputs", synced.append)
    monkeypatch.setattr(os, "_exit", exits.append)
    entry.on_stop("agentd")(signal.SIGTERM, None)
    assert synced == ["agentd"] and exits == [0]
