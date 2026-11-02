"""Sessions, events, the stream and resources through the SDK."""

import io
import threading
import time
import uuid

import anthropic
import pytest


def name(prefix: str) -> str:
    return f"{prefix}-{uuid.uuid4().hex[:8]}"


@pytest.fixture
def setup(client: anthropic.Anthropic):
    agent = client.beta.agents.create(name=name("sdk-sess-agent"), model="claude-sonnet-4-6")
    env = client.beta.environments.create(name=name("sdk-sess-env"))
    return agent, env


def test_the_stream_names_every_event_and_ends_with_session_deleted(client, setup) -> None:
    agent, env = setup
    session = client.beta.sessions.create(agent=agent.id, environment_id=env.id)
    seen: list[str] = []

    def read() -> None:
        for event in client.beta.sessions.events.stream(session.id):
            seen.append(event.type)

    reader = threading.Thread(target=read)
    reader.start()
    time.sleep(0.5)
    client.beta.sessions.events.send(session.id, events=[{"type": "user.message", "content": [{"type": "text", "text": "hi"}]}])
    time.sleep(0.3)
    client.beta.sessions.delete(session.id)
    reader.join(timeout=10)
    assert not reader.is_alive()
    assert seen == ["user.message", "session.deleted"]


def test_create_takes_budget_initial_events_and_overrides(client, setup) -> None:
    agent, env = setup
    session = client.beta.sessions.create(
        agent={"type": "agent_with_overrides", "id": agent.id, "system": "override"},
        environment_id=env.id,
        budget={"type": "limit", "max_list_cost": {"amount": "5.00", "currency": "USD"}},
        initial_events=[{"type": "user.message", "content": [{"type": "text", "text": "start"}]}],
    )
    assert session.agent.system == "override"
    assert session.budget.max_list_cost.amount == "5.00"
    events = client.beta.sessions.events.list(session.id)
    assert [e.type for e in events.data] == ["user.message"]


def test_update_merges_metadata_and_clears_on_null(client, setup) -> None:
    agent, env = setup
    session = client.beta.sessions.create(
        agent=agent.id, environment_id=env.id, title="t", metadata={"a": "1", "b": "2"},
        budget={"type": "limit", "max_list_cost": {"amount": "1", "currency": "USD"}},
    )
    updated = client.beta.sessions.update(session.id, metadata={"a": None}, title=None, budget=None)
    assert (updated.metadata, updated.title, updated.budget) == ({"b": "2"}, None, None)


def test_the_list_filters_by_status_and_hides_archived(client, setup) -> None:
    agent, env = setup
    a = client.beta.sessions.create(agent=agent.id, environment_id=env.id)
    b = client.beta.sessions.create(agent=agent.id, environment_id=env.id)
    client.beta.sessions.archive(b.id)
    ids = [s.id for s in client.beta.sessions.list(agent_id=agent.id, statuses=["idle"])]
    assert ids == [a.id]
    with_archived = [s.id for s in client.beta.sessions.list(agent_id=agent.id, include_archived=True, order="asc")]
    assert with_archived == [a.id, b.id]


def test_events_take_the_runner_limit_and_filters(client, setup) -> None:
    agent, env = setup
    session = client.beta.sessions.create(agent=agent.id, environment_id=env.id)
    client.beta.sessions.events.send(session.id, events=[
        {"type": "user.message", "content": [{"type": "text", "text": "a"}]},
        {"type": "user.interrupt"},
    ])
    page = client.beta.sessions.events.list(session.id, limit=1000, types=["user.interrupt"])
    assert [e.type for e in page.data] == ["user.interrupt"]
    newest = client.beta.sessions.events.list(session.id, order="desc")
    assert [e.type for e in newest.data] == ["user.interrupt", "user.message"]


def test_tool_results_and_rich_content_are_accepted(client, setup) -> None:
    agent, env = setup
    session = client.beta.sessions.create(agent=agent.id, environment_id=env.id)
    sent = client.beta.sessions.events.send(session.id, events=[
        {"type": "user.tool_result", "tool_use_id": "sevt_011CZZZZZZZZZZZZZZZZZZZZ", "content": [{"type": "text", "text": "ok"}]},
        {"type": "user.custom_tool_result", "custom_tool_use_id": "sevt_011CZZZZZZZZZZZZZZZZZZZZ"},
        {"type": "user.message", "content": [
            {"type": "image", "source": {"type": "url", "url": "https://example.com/a.png"}},
            {"type": "document", "source": {"type": "text", "media_type": "text/plain", "data": "d"}},
        ]},
    ])
    assert [e.type for e in sent.data] == ["user.tool_result", "user.custom_tool_result", "user.message"]


def test_resources_can_be_added_listed_and_deleted(client, setup) -> None:
    agent, env = setup
    session = client.beta.sessions.create(agent=agent.id, environment_id=env.id)
    file = client.beta.files.upload(file=("r.txt", io.BytesIO(b"r"), "text/plain"))
    resource = client.beta.sessions.resources.add(session.id, type="file", file_id=file.id)
    assert resource.file_id == file.id and resource.created_at is not None
    assert [r.id for r in client.beta.sessions.resources.list(session.id)] == [resource.id]
    assert client.beta.sessions.resources.retrieve(resource.id, session_id=session.id).id == resource.id
    deleted = client.beta.sessions.resources.delete(resource.id, session_id=session.id)
    assert (deleted.id, deleted.type) == (resource.id, "session_resource_deleted")


def test_an_unknown_thread_is_not_found(client, setup) -> None:
    agent, env = setup
    session = client.beta.sessions.create(agent=agent.id, environment_id=env.id)
    assert list(client.beta.sessions.threads.list(session.id)) == []
    with pytest.raises(anthropic.NotFoundError):
        client.beta.sessions.threads.retrieve("sthr_011CZZZZZZZZZZZZZZZZZZZZ", session_id=session.id)
