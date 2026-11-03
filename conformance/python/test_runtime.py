"""The session harness through the SDK: the agent loop and its tools in the
session's container, events on the stream, confirmations, custom tools,
delegation to threads, outcomes, and outputs as session files.

Runs when each session's harness calls the scripted model
(`conformance/mock_model.py conformance/fixtures/mock-model.json`, where each
agent below plays its script) and AGENTD_TEST_HARNESS=1.
"""

import io
import os
import time
import uuid

import anthropic
import pytest

pytestmark = pytest.mark.skipif(os.environ.get("AGENTD_TEST_HARNESS") != "1", reason="needs AGENTD_TEST_HARNESS=1")

TOOLSET = [{"type": "agent_toolset_20260401", "default_config": {"permission_policy": {"type": "always_allow"}}}]


def env(client: anthropic.Anthropic):
    return client.beta.environments.create(name=f"rt-{uuid.uuid4().hex[:8]}", config={"type": "cloud", "networking": {"type": "limited"}})


def message(text: str) -> dict:
    return {"type": "user.message", "content": [{"type": "text", "text": text}]}


def idle_count(client: anthropic.Anthropic, session_id: str) -> int:
    return sum(e.type == "session.status_idle" for e in client.beta.sessions.events.list(session_id, limit=1000))


def wait_idle(client: anthropic.Anthropic, session_id: str, after: int, timeout: float = 60):
    """Poll the event list until a new idle event follows `after` earlier ones; every event."""
    deadline = time.time() + timeout
    while time.time() < deadline:
        events = list(client.beta.sessions.events.list(session_id, limit=1000))
        if sum(e.type == "session.status_idle" for e in events) > after:
            return events
        time.sleep(0.5)
    raise TimeoutError(f"session {session_id} did not go idle in {timeout}s")


def run(client, session, *events):
    before = idle_count(client, session.id)
    client.beta.sessions.events.send(session.id, events=list(events))
    return wait_idle(client, session.id, before)


def test_a_tool_runs_in_the_container_and_its_output_is_a_session_file(client: anthropic.Anthropic) -> None:
    upload = client.files.upload(file=("data.txt", io.BytesIO(b"mounted bytes"), "text/plain"))
    agent = client.beta.agents.create(name="rt-tool", model="claude-sonnet-4-6", tools=TOOLSET)
    session = client.beta.sessions.create(
        agent=agent.id, environment_id=env(client).id,
        resources=[{"type": "file", "file_id": upload.id, "mount_path": "/in/data.txt"}],
    )
    run(client, session, message("write the result"))
    events = list(client.beta.sessions.events.list(session.id, limit=1000))
    types = [e.type for e in events]
    assert types[0] == "user.message" and types[-1] == "session.status_idle"
    for t in ("session.status_running", "span.model_request_start", "span.model_request_end", "agent.tool_use", "agent.tool_result", "agent.message"):
        assert t in types, t
    result = next(e for e in events if e.type == "agent.tool_result")
    assert "mounted bytes" in result.content[0].text
    assert events[-1].stop_reason.type == "end_turn"
    session = client.beta.sessions.retrieve(session.id)
    assert session.status == "idle" and session.usage.input_tokens >= 20
    files = []
    for _ in range(30):
        files = list(client.beta.files.list(scope_id=session.id, betas=["managed-agents-2026-04-01"]))
        if files:
            break
        time.sleep(0.5)
    assert [f.filename for f in files] == ["result.json"]
    assert files[0].downloadable and files[0].scope.id == session.id
    assert client.beta.files.download(files[0].id).read().strip() == b'{"ok": true}'


def test_an_always_ask_tool_waits_for_the_users_confirmation(client: anthropic.Anthropic) -> None:
    tools = [{"type": "agent_toolset_20260401", "default_config": {"permission_policy": {"type": "always_ask"}}}]
    agent = client.beta.agents.create(name="rt-ask", model="claude-sonnet-4-6", tools=tools)
    session = client.beta.sessions.create(agent=agent.id, environment_id=env(client).id)
    seen = run(client, session, message("remove it"))
    idle = [e for e in seen if e.type == "session.status_idle"][-1]
    assert idle.stop_reason.type == "requires_action"
    tool_use = next(e for e in seen if e.type == "agent.tool_use")
    assert tool_use.evaluated_permission == "ask" and idle.stop_reason.event_ids == [tool_use.id]
    run(client, session, {"type": "user.tool_confirmation", "tool_use_id": tool_use.id, "result": "allow"})
    events = list(client.beta.sessions.events.list(session.id, limit=1000))
    assert any(e.type == "agent.tool_result" and e.tool_use_id == tool_use.id for e in events)
    assert events[-1].stop_reason.type == "end_turn"


def test_a_custom_tool_is_answered_by_the_client(client: anthropic.Anthropic) -> None:
    tools = [{"type": "custom", "name": "lookup", "description": "Look a value up.", "input_schema": {"type": "object"}}]
    agent = client.beta.agents.create(name="rt-custom", model="claude-sonnet-4-6", tools=tools)
    session = client.beta.sessions.create(agent=agent.id, environment_id=env(client).id)
    seen = run(client, session, message("look it up"))
    custom = next(e for e in seen if e.type == "agent.custom_tool_use")
    run(client, session, {"type": "user.custom_tool_result", "custom_tool_use_id": custom.id, "content": [{"type": "text", "text": "42"}]})
    events = list(client.beta.sessions.events.list(session.id, limit=1000))
    assert [e.content[0].text for e in events if e.type == "agent.message"] == ["it is 42"]


SKILL_MD = b"---\nname: rt-skill\ndescription: A test skill.\n---\n# Test skill\n"


def test_a_coordinator_delegates_to_a_roster_thread(client: anthropic.Anthropic) -> None:
    skill = client.skills.create(files=[("rt-skill/SKILL.md", io.BytesIO(SKILL_MD), "text/markdown")], display_name=f"rt-skill-{uuid.uuid4().hex[:6]}")
    worker = client.beta.agents.create(
        name="rt-worker", model="claude-sonnet-4-6", tools=TOOLSET,
        skills=[{"type": "custom", "skill_id": skill.id, "version": "latest"}],
    )
    coord = client.beta.agents.create(
        name="rt-coord", model="claude-sonnet-4-6", tools=TOOLSET,
        multiagent={"type": "coordinator", "agents": [{"type": "agent", "id": worker.id, "version": worker.version}]},
    )
    session = client.beta.sessions.create(agent=coord.id, environment_id=env(client).id)
    assert session.agent.multiagent.agents[0].name == "rt-worker"
    run(client, session, message("run it"))
    events = list(client.beta.sessions.events.list(session.id, limit=1000))
    types = [e.type for e in events]
    assert "session.thread_created" in types and "agent.thread_message_sent" in types and "agent.thread_message_received" in types
    assert [e.content[0].text for e in events if e.type == "agent.message"][-1] == "all done"
    assert not any(e.type == "agent.tool_use" and e.name == "read" for e in events), "the worker's tools stay on its thread"
    threads = list(client.beta.sessions.threads.list(session.id))
    assert [t.agent.name for t in threads] == ["rt-coord", "rt-worker"]
    assert threads[1].parent_thread_id == threads[0].id
    worker_events = list(client.beta.sessions.threads.events.list(threads[1].id, session_id=session.id))
    worker_types = [e.type for e in worker_events]
    assert worker_types[0] == "session.thread_status_running" and "agent.thread_message_received" in worker_types
    read = next(e for e in worker_events if e.type == "agent.tool_result")
    assert "# Test skill" in read.content[0].text


def test_an_outcome_is_graded_and_revised(client: anthropic.Anthropic) -> None:
    agent = client.beta.agents.create(name="rt-outcome", model="claude-sonnet-4-6", tools=TOOLSET)
    session = client.beta.sessions.create(agent=agent.id, environment_id=env(client).id)
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
    assert [e.content[0].text for e in events if e.type == "agent.message"] == ["draft", "final"]
    assert s.outcome_evaluations[0].result == "satisfied"
