"""Agents and environments through the SDK, against the SDK 2.3.0 shapes."""

import uuid

import anthropic
import pytest


def name(prefix: str) -> str:
    return f"{prefix}-{uuid.uuid4().hex[:8]}"


def test_an_agent_serves_every_required_field(client: anthropic.Anthropic) -> None:
    agent = client.beta.agents.create(
        name=name("sdk-agent"),
        model="claude-sonnet-4-6",
        mcp_servers=[{"type": "url", "name": "gh", "url": "https://mcp.example.com"}],
        tools=[
            {"type": "agent_toolset_20260401", "configs": [{"name": "web_fetch"}],
             "default_config": {"permission_policy": {"type": "auto"}}},
            {"type": "mcp_toolset", "mcp_server_name": "gh"},
        ],
        skills=[{"type": "anthropic", "skill_id": "xlsx"}],
    )
    assert agent.execution_identity.type == "service_account"
    toolset, mcp = agent.tools
    assert toolset.default_config.enabled is True
    assert toolset.default_config.permission_policy.type == "auto"
    fetch = toolset.configs[0]
    assert (fetch.type, fetch.enabled, fetch.permission_policy.type) == ("web_fetch", True, "auto")
    assert fetch.url_sources is None
    assert mcp.configs == [] and mcp.default_config.permission_policy.type == "always_ask"
    assert agent.skills[0].version == "latest"


def test_an_update_needs_no_version_and_merges_metadata(client: anthropic.Anthropic) -> None:
    agent = client.beta.agents.create(name=name("sdk-upd"), model="claude-sonnet-4-6", metadata={"a": "1", "b": "2"})
    updated = client.beta.agents.update(agent.id, description="d", metadata={"a": None, "c": "3"})
    assert updated.version == agent.version + 1
    assert updated.metadata == {"b": "2", "c": "3"}
    cleared = client.beta.agents.update(agent.id, description=None)
    assert cleared.description is None


def test_up_to_256_tools_are_accepted(client: anthropic.Anthropic) -> None:
    tools = [{"type": "custom", "name": f"t{i}", "description": "d", "input_schema": {"type": "object"}} for i in range(200)]
    agent = client.beta.agents.create(name=name("sdk-tools"), model="claude-sonnet-4-6", tools=tools)
    assert len(agent.tools) == 200


def test_archived_agents_are_listed_only_on_request(client: anthropic.Anthropic) -> None:
    agent = client.beta.agents.create(name=name("sdk-arch"), model="claude-sonnet-4-6")
    client.beta.agents.archive(agent.id)
    assert agent.id not in {a.id for a in client.beta.agents.list()}
    assert agent.id in {a.id for a in client.beta.agents.list(include_archived=True)}


def test_a_coordinator_roster_resolves_to_versions(client: anthropic.Anthropic) -> None:
    worker = client.beta.agents.create(name=name("sdk-worker"), model="claude-sonnet-4-6")
    lead = client.beta.agents.create(
        name=name("sdk-lead"), model="claude-sonnet-4-6",
        multiagent={"type": "coordinator", "agents": [worker.id, {"type": "self"}]},
    )
    members = lead.multiagent.agents
    assert [(m.type, m.id, m.version) for m in members] == [("agent", worker.id, 1), ("agent", lead.id, 1)]


def test_an_environment_needs_no_config(client: anthropic.Anthropic) -> None:
    env = client.beta.environments.create(name=name("sdk-env"))
    assert env.config.type == "cloud" and env.config.networking.type == "unrestricted"
    limited = client.beta.environments.update(env.id, config={"type": "cloud", "networking": {"type": "limited"}})
    net = limited.config.networking
    assert (net.allowed_hosts, net.allow_package_managers, net.allow_mcp_servers) == ([], False, False)
    cleared = client.beta.environments.update(env.id, description=None)
    assert cleared.description is None


def test_an_account_scoped_environment_is_rejected(client: anthropic.Anthropic) -> None:
    with pytest.raises(anthropic.BadRequestError):
        client.beta.environments.create(name=name("sdk-acct"), scope="account")


def test_archived_environments_are_listed_only_on_request(client: anthropic.Anthropic) -> None:
    env = client.beta.environments.create(name=name("sdk-env-arch"))
    client.beta.environments.archive(env.id)
    assert env.id not in {e.id for e in client.beta.environments.list()}
    assert env.id in {e.id for e in client.beta.environments.list(include_archived=True)}
