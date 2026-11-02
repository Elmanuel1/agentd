"""Resource calls through the SDK: memory under its own beta, deletes, uploads."""

import io
import uuid

import anthropic


def name(prefix: str) -> str:
    return f"{prefix}-{uuid.uuid4().hex[:8]}"


def test_memories_work_with_the_agent_memory_beta_alone(client: anthropic.Anthropic) -> None:
    store = client.beta.memory_stores.create(name=name("sdk-mem"))
    memory = client.beta.memory_stores.memories.create(store.id, path="/notes/a.md", content="hello")
    assert memory.path == "/notes/a.md"
    deleted = client.beta.memory_stores.memories.delete(memory.id, memory_store_id=store.id)
    assert (deleted.id, deleted.type) == (memory.id, "memory_deleted")
    gone = client.beta.memory_stores.delete(store.id)
    assert (gone.id, gone.type) == (store.id, "memory_store_deleted")


def test_environment_delete_returns_the_deleted_body(client: anthropic.Anthropic) -> None:
    env = client.beta.environments.create(
        name=name("sdk-env"), config={"type": "cloud", "networking": {"type": "unrestricted"}}
    )
    deleted = client.beta.environments.delete(env.id)
    assert (deleted.id, deleted.type) == (env.id, "environment_deleted")


def test_vault_delete_returns_the_deleted_body(client: anthropic.Anthropic) -> None:
    vault = client.beta.vaults.create(display_name=name("sdk-vault"))
    deleted = client.beta.vaults.delete(vault.id)
    assert (deleted.id, deleted.type) == (vault.id, "vault_deleted")


def test_a_credential_secret_is_stored_and_never_returned(client: anthropic.Anthropic) -> None:
    vault = client.beta.vaults.create(display_name=name("sdk-cred-vault"))
    token = f"sdk-token-{uuid.uuid4().hex}"
    credential = client.beta.vaults.credentials.create(
        vault.id,
        display_name=name("sdk-cred"),
        auth={"type": "static_bearer", "mcp_server_url": "https://mcp.example.com/mcp", "token": token},
    )
    assert (credential.auth.type, credential.auth.mcp_server_url) == ("static_bearer", "https://mcp.example.com/mcp")
    fetched = client.beta.vaults.credentials.retrieve(credential.id, vault_id=vault.id)
    assert token not in fetched.model_dump_json()
    archived = client.beta.vaults.credentials.archive(credential.id, vault_id=vault.id)
    assert archived.archived_at is not None
    client.beta.vaults.delete(vault.id)


def test_agents_auto_paginate_across_pages(client: anthropic.Anthropic) -> None:
    created = {client.beta.agents.create(name=name("sdk-page"), model="claude-sonnet-4-6").id for _ in range(3)}
    seen = {a.id for a in client.beta.agents.list(limit=1)}
    assert created <= seen


def test_an_upload_over_2_mb_keeps_its_bytes(client: anthropic.Anthropic) -> None:
    payload = b"a" * (3 * 1024 * 1024)
    meta = client.beta.files.upload(file=("big.bin", io.BytesIO(payload), "application/octet-stream"))
    assert meta.size_bytes == len(payload)
    deleted = client.files.delete(meta.id)
    assert (deleted.id, deleted.type) == (meta.id, "file_deleted")
