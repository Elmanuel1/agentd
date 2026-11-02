"""The Docker sandbox: one container per session, gone when it is archived or deleted.

Runs when agentd uses AGENTD_RUNTIME=docker and AGENTD_TEST_DOCKER=1 says the
test may read the same Docker daemon.
"""

import json
import os
import subprocess
import uuid

import anthropic
import pytest

pytestmark = pytest.mark.skipif(os.environ.get("AGENTD_TEST_DOCKER") != "1", reason="needs AGENTD_TEST_DOCKER=1")


def container(session_id: str) -> dict | None:
    out = subprocess.run(["docker", "inspect", f"agentd-{session_id}"], capture_output=True, text=True)
    return json.loads(out.stdout)[0] if out.returncode == 0 else None


def session(client: anthropic.Anthropic, networking: str):
    agent = client.beta.agents.create(name=f"sbx-{uuid.uuid4().hex[:8]}", model="claude-sonnet-4-6")
    env = client.beta.environments.create(
        name=f"sbx-{uuid.uuid4().hex[:8]}", config={"type": "cloud", "networking": {"type": networking}}
    )
    return client.beta.sessions.create(agent=agent.id, environment_id=env.id)


def test_a_session_runs_in_its_container_until_archived(client: anthropic.Anthropic) -> None:
    s = session(client, "unrestricted")
    info = container(s.id)
    assert info is not None and info["State"]["Running"]
    assert info["Config"]["Labels"]["agentd.target"] == s.id
    assert info["HostConfig"]["NetworkMode"] == "agentd-sandboxes"
    assert info["Config"]["User"] == "65534:65534"
    assert info["HostConfig"]["Memory"] == 2048 * 1024 * 1024 and info["HostConfig"]["CapDrop"] == ["ALL"]
    client.beta.sessions.archive(s.id)
    assert container(s.id) is None


def test_a_deleted_session_takes_its_container_with_it(client: anthropic.Anthropic) -> None:
    s = session(client, "limited")
    info = container(s.id)
    assert info is not None and info["HostConfig"]["NetworkMode"] == "none"
    client.beta.sessions.delete(s.id)
    assert container(s.id) is None
