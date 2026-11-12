"""The agent loop through the SDK: its tools run by the official worker on a
`self_hosted` environment, events on the stream, confirmations, custom tools,
delegation to threads, outcomes and the budget.

Runs when the loop calls the scripted model
(`conformance/mock_model.py conformance/fixtures/mock-model.json`, where each
agent below plays its script, as model `mock/scripted`) and AGENTD_TEST_WORKER=1.
"""

import io
import os
import time
import uuid

import anthropic
import pytest

pytestmark = pytest.mark.skipif(os.environ.get("AGENTD_TEST_WORKER") != "1", reason="needs AGENTD_TEST_WORKER=1")

MODEL = "mock/scripted"

TOOLSET = [{"type": "agent_toolset_20260401", "default_config": {"permission_policy": {"type": "always_allow"}}}]


def message(text: str) -> dict:
    return {"type": "user.message", "content": [{"type": "text", "text": text}]}


def idles(events) -> list:
    return [e for e in events if e.type == "session.status_idle"]


def run(client, session, *events, stop: str = "end_turn", timeout: float = 60):
    """Send `events` and poll until a new idle stops the turn: `stop`, or `budget_reached`. On a
    `self_hosted` environment a built-in call idles `requires_action` until the worker answers it."""
    before = len(idles(client.beta.sessions.events.list(session.id, limit=1000)))
    client.beta.sessions.events.send(session.id, events=list(events))
    deadline = time.time() + timeout
    while time.time() < deadline:
        seen = list(client.beta.sessions.events.list(session.id, limit=1000))
        new = idles(seen)[before:]
        if new and new[-1].stop_reason.type in (stop, "budget_reached") and seen[-1].type == "session.status_idle":
            return seen
        time.sleep(0.5)
    raise TimeoutError(f"session {session.id} did not stop with {stop} in {timeout}s")


def test_a_built_in_tool_runs_on_the_worker_and_its_result_continues_the_turn(client: anthropic.Anthropic, worker) -> None:
    agent = client.beta.agents.create(name="rt-tool", model=MODEL, tools=TOOLSET)
    session = client.beta.sessions.create(agent=agent.id, environment_id=worker().id)
    events = run(client, session, message("run it"))
    types = [e.type for e in events]
    # Live's order: the turn's running status, then the message it takes.
    assert types[:2] == ["session.status_running", "session.thread_status_running"] and types[-1] == "session.status_idle"
    for t in ("user.message", "span.model_request_start", "span.model_request_end", "agent.tool_use", "user.tool_result", "agent.message"):
        assert t in types, t
    result = next(e for e in events if e.type == "user.tool_result")
    assert "tool-ok" in result.content[0].text
    assert idles(events)[0].stop_reason.type == "requires_action", "a self_hosted built-in call waits for the worker"
    assert events[-1].stop_reason.type == "end_turn"
    session = client.beta.sessions.retrieve(session.id)
    assert session.status == "idle" and session.usage.input_tokens >= 20


def test_an_always_ask_tool_waits_for_the_users_confirmation(client: anthropic.Anthropic, worker) -> None:
    tools = [{"type": "agent_toolset_20260401", "default_config": {"permission_policy": {"type": "always_ask"}}}]
    agent = client.beta.agents.create(name="rt-ask", model=MODEL, tools=tools)
    session = client.beta.sessions.create(agent=agent.id, environment_id=worker().id)
    seen = run(client, session, message("remove it"), stop="requires_action")
    idle = idles(seen)[-1]
    assert idle.stop_reason.type == "requires_action"
    tool_use = next(e for e in seen if e.type == "agent.tool_use")
    assert tool_use.evaluated_permission == "ask" and idle.stop_reason.event_ids == [tool_use.id]
    run(client, session, {"type": "user.tool_confirmation", "tool_use_id": tool_use.id, "result": "allow"})
    events = list(client.beta.sessions.events.list(session.id, limit=1000))
    assert any(e.type == "user.tool_result" and e.tool_use_id == tool_use.id for e in events)
    assert events[-1].stop_reason.type == "end_turn"


def test_a_custom_tool_is_answered_by_the_client(client: anthropic.Anthropic, worker) -> None:
    tools = [{"type": "custom", "name": "lookup", "description": "Look a value up.", "input_schema": {"type": "object"}}]
    agent = client.beta.agents.create(name="rt-custom", model=MODEL, tools=tools)
    session = client.beta.sessions.create(agent=agent.id, environment_id=worker().id)
    seen = run(client, session, message("look it up"), stop="requires_action")
    custom = next(e for e in seen if e.type == "agent.custom_tool_use")
    run(client, session, {"type": "user.custom_tool_result", "custom_tool_use_id": custom.id, "content": [{"type": "text", "text": "42"}]})
    events = list(client.beta.sessions.events.list(session.id, limit=1000))
    assert [e.content[0].text for e in events if e.type == "agent.message"] == ["it is 42"]


SKILL_MD = b"---\nname: rt-skill\ndescription: A test skill.\n---\n# Test skill\n"


def test_a_coordinator_delegates_to_a_roster_thread(client: anthropic.Anthropic, worker) -> None:
    skill = client.skills.create(files=[("rt-skill/SKILL.md", io.BytesIO(SKILL_MD), "text/markdown")], display_name=f"rt-skill-{uuid.uuid4().hex[:6]}")
    helper = client.beta.agents.create(
        name="rt-worker", model=MODEL, tools=TOOLSET,
        skills=[{"type": "custom", "skill_id": skill.id, "version": "latest"}],
    )
    coord = client.beta.agents.create(
        name="rt-coord", model=MODEL, tools=TOOLSET,
        multiagent={"type": "coordinator", "agents": [{"type": "agent", "id": helper.id, "version": helper.version}]},
    )
    session = client.beta.sessions.create(agent=coord.id, environment_id=worker().id)
    assert session.agent.multiagent.agents[0].name == "rt-worker"
    run(client, session, message("run it"))
    events = list(client.beta.sessions.events.list(session.id, limit=1000))
    types = [e.type for e in events]
    assert "session.thread_created" in types and "agent.thread_message_sent" in types and "agent.thread_message_received" in types
    assert [e.content[0].text for e in events if e.type == "agent.message"][-1] == "all done"
    call = next(e for e in events if e.type == "agent.tool_use" and e.name == "bash")
    assert call.session_thread_id, "a roster thread's call shows on the session's events with its thread"
    threads = list(client.beta.sessions.threads.list(session.id))
    assert [t.agent.name for t in threads] == ["rt-coord", "rt-worker"]
    assert threads[1].parent_thread_id == threads[0].id
    worker_events = list(client.beta.sessions.threads.events.list(threads[1].id, session_id=session.id))
    worker_types = [e.type for e in worker_events]
    assert worker_types[0] == "session.thread_status_running" and "agent.thread_message_received" in worker_types
    result = next(e for e in worker_events if e.type == "user.tool_result")
    assert "page-1" in result.content[0].text, "the worker's result lands on the call's thread"


def test_an_outcome_is_graded_and_revised(client: anthropic.Anthropic, worker) -> None:
    agent = client.beta.agents.create(name="rt-outcome", model=MODEL, tools=TOOLSET)
    session = client.beta.sessions.create(agent=agent.id, environment_id=worker().id)
    outcome = {"type": "user.define_outcome", "description": "Say final.", "rubric": {"type": "text", "content": "### 1. greeting"}, "max_iterations": 3}
    run(client, session, outcome)
    for _ in range(40):
        s = client.beta.sessions.retrieve(session.id)
        if s.status == "idle" and s.outcome_evaluations and s.outcome_evaluations[0].result == "satisfied":
            break
        time.sleep(0.5)
    events = list(client.beta.sessions.events.list(session.id, limit=1000))
    ends = [e.result for e in events if e.type == "span.outcome_evaluation_end"]
    assert ends == ["needs_revision", "satisfied"]
    # Once satisfied, the agent closes the turn with one more reply.
    assert [e.content[0].text for e in events if e.type == "agent.message"] == ["draft", "final", "done"]
    assert s.outcome_evaluations[0].result == "satisfied"


def limit(minor_units: str) -> dict:
    return {"type": "limit", "max_list_cost": {"amount": minor_units, "currency": "USD"}}


def reason(call) -> str:
    with pytest.raises(anthropic.BadRequestError) as e:
        call()
    return e.value.body["error"]["reason"]


def test_the_budget_stops_model_requests_at_its_list_cost_until_it_is_raised(client: anthropic.Anthropic, worker) -> None:
    agent = client.beta.agents.create(name="rt-budget", model=MODEL, tools=TOOLSET)
    session = client.beta.sessions.create(agent=agent.id, environment_id=worker().id, budget=limit("10"))
    seen = run(client, session, message("spend"))
    idle = idles(seen)[-1]
    assert idle.stop_reason.type == "budget_reached"
    assert sum(e.type == "span.model_request_start" for e in seen) == 2
    snapshots = [e for e in seen if e.type == "session.usage"]
    amounts = [s.usage.list_cost.amount for s in snapshots]
    assert amounts[:2] == ["6", "12"] and set(amounts[2:]) <= {"12"}, "one snapshot per call, then the idle's"
    assert snapshots[-1].budget.max_list_cost.amount == "10" and snapshots[-1].usage.input_tokens == 20
    session = client.beta.sessions.retrieve(session.id)
    assert (session.usage.list_cost.amount, session.usage.list_cost.currency) == ("12", "USD")
    assert reason(lambda: client.beta.sessions.update(session.id, budget=limit("12"))) == "budget_not_raised"
    client.beta.sessions.update(session.id, budget=limit("1000"))
    run(client, session, message("go on"))
    events = list(client.beta.sessions.events.list(session.id, limit=1000))
    assert events[-1].stop_reason.type == "end_turn"
    assert [e.content[0].text for e in events if e.type == "agent.message"] == ["under budget"]
    assert client.beta.sessions.retrieve(session.id).usage.list_cost.amount == "13"


def test_a_model_with_no_list_price_stops_a_budgeted_session(client: anthropic.Anthropic, worker) -> None:
    agent = client.beta.agents.create(name="rt-unpriced", model=MODEL, tools=TOOLSET)
    session = client.beta.sessions.create(agent=agent.id, environment_id=worker().id, budget=limit("1000"))
    seen = run(client, session, message("spend"))
    assert idles(seen)[-1].stop_reason.type == "budget_reached"
    session = client.beta.sessions.retrieve(session.id)
    assert session.usage.list_cost is None and session.usage.input_tokens == 10
    assert reason(lambda: client.beta.sessions.update(session.id, budget=limit("5000"))) == "model_not_budgetable"
