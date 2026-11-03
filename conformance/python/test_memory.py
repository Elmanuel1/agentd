"""Memory stores, memories and versions through the SDK."""

import hashlib
import json
import urllib.request
import uuid

import anthropic
import pytest


def store(client: anthropic.Anthropic):
    return client.beta.memory_stores.create(name=f"sdk-mem-{uuid.uuid4().hex[:8]}")


def test_a_store_updates_and_archived_stores_leave_the_list(client: anthropic.Anthropic) -> None:
    s = client.beta.memory_stores.create(name="before", description="d", metadata={"a": "1"})
    updated = client.beta.memory_stores.update(s.id, name="after", description=None, metadata={"a": None, "b": "2"})
    assert (updated.name, updated.description, updated.metadata) == ("after", "", {"b": "2"})
    client.beta.memory_stores.archive(s.id)
    assert s.id not in {x.id for x in client.beta.memory_stores.list(limit=100)}
    assert s.id in {x.id for x in client.beta.memory_stores.list(limit=100, include_archived=True)}
    later = client.beta.memory_stores.list(include_archived=True, created_at_gte="2999-01-01T00:00:00Z")
    assert later.data == []
    with pytest.raises(anthropic.ConflictError):
        client.beta.memory_stores.update(s.id, name="again")


def test_memories_page_and_collapse_by_depth(client: anthropic.Anthropic) -> None:
    s = store(client)
    for path in ["/a.md", "/notes/b.md", "/notes/deep/c.md"]:
        client.beta.memory_stores.memories.create(s.id, path=path, content=path)
    first = client.beta.memory_stores.memories.list(s.id, limit=2)
    assert len(first.data) == 2 and first.next_page
    rest = client.beta.memory_stores.memories.list(s.id, limit=2, page=first.next_page)
    assert len(rest.data) == 1
    top = client.beta.memory_stores.memories.list(s.id, depth=1)
    assert sorted((m.type, m.path) for m in top.data) == [("memory", "/a.md"), ("memory_prefix", "/notes/")]
    full = client.beta.memory_stores.memories.list(s.id, view="full", path_prefix="/notes/")
    assert {m.content for m in full.data} == {"/notes/b.md", "/notes/deep/c.md"}


def test_a_path_cannot_sit_above_or_below_another(client: anthropic.Anthropic) -> None:
    s = store(client)
    client.beta.memory_stores.memories.create(s.id, path="/x/y.md", content="1")
    with pytest.raises(anthropic.ConflictError):
        client.beta.memory_stores.memories.create(s.id, path="/x", content="2")
    with pytest.raises(anthropic.ConflictError):
        client.beta.memory_stores.memories.create(s.id, path="/x/y.md/z.md", content="3")


def test_an_unchanged_update_writes_no_version(client: anthropic.Anthropic) -> None:
    s = store(client)
    m = client.beta.memory_stores.memories.create(s.id, path="/same.md", content="same")
    sha = hashlib.sha256(b"same").hexdigest()
    again = client.beta.memory_stores.memories.update(
        m.id, memory_store_id=s.id, content="same", precondition={"type": "content_sha256", "content_sha256": sha}
    )
    assert again.content_sha256 == sha
    versions = client.beta.memory_stores.memory_versions.list(s.id, memory_id=m.id)
    assert [v.operation for v in versions.data] == ["created"]


def test_versions_filter_and_redact(client: anthropic.Anthropic) -> None:
    s = store(client)
    m = client.beta.memory_stores.memories.create(s.id, path="/v.md", content="one")
    client.beta.memory_stores.memories.update(m.id, memory_store_id=s.id, content="two")
    versions = client.beta.memory_stores.memory_versions
    assert [v.operation for v in versions.list(s.id, memory_id=m.id)] == ["modified", "created"]
    assert [v.operation for v in versions.list(s.id, operation="created").data] == ["created"]
    assert versions.list(s.id, session_id="sesn_011CZZZZZZZZZZZZZZZZZZZZ").data == []
    head, old = versions.list(s.id, memory_id=m.id).data
    with pytest.raises(anthropic.ConflictError):
        versions.redact(head.id, memory_store_id=s.id)
    redacted = versions.redact(old.id, memory_store_id=s.id)
    assert redacted.redacted_at is not None and redacted.content is None


def test_the_managed_agents_beta_alone_also_opens_memory(client: anthropic.Anthropic, base_url: str, key: str) -> None:
    s = store(client)
    headers = {"x-api-key": key, "anthropic-version": "2023-06-01", "anthropic-beta": "managed-agents-2026-04-01"}
    req = urllib.request.Request(f"{base_url}/v1/memory_stores/{s.id}", headers=headers)
    with urllib.request.urlopen(req) as res:
        assert res.status == 200 and json.load(res)["id"] == s.id


def test_versions_full_view_carries_content_and_archived_stores_refuse_deletes(client: anthropic.Anthropic) -> None:
    s = store(client)
    m = client.beta.memory_stores.memories.create(s.id, path="/f.md", content="body")
    full = client.beta.memory_stores.memory_versions.list(s.id, view="full")
    assert [v.content for v in full.data] == ["body"]
    client.beta.memory_stores.archive(s.id)
    with pytest.raises(anthropic.ConflictError):
        client.beta.memory_stores.memories.delete(m.id, memory_store_id=s.id)


def test_a_version_of_a_deleted_memory_can_be_redacted(client: anthropic.Anthropic) -> None:
    s = store(client)
    m = client.beta.memory_stores.memories.create(s.id, path="/gone.md", content="secret")
    client.beta.memory_stores.memories.delete(m.id, memory_store_id=s.id)
    created = client.beta.memory_stores.memory_versions.list(s.id, memory_id=m.id, operation="created").data[0]
    redacted = client.beta.memory_stores.memory_versions.redact(created.id, memory_store_id=s.id)
    assert redacted.redacted_at is not None
    again = client.beta.memory_stores.memory_versions.retrieve(created.id, memory_store_id=s.id)
    assert again.content is None and again.content_sha256 is None
