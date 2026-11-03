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


def inspect(kind: str, name: str) -> dict | None:
    out = subprocess.run(["docker", kind, "inspect", name], capture_output=True, text=True)
    return json.loads(out.stdout)[0] if out.returncode == 0 else None


def container(session_id: str) -> dict | None:
    return inspect("container", f"agentd-{session_id}")


def network(session_id: str) -> dict | None:
    return inspect("network", f"agentd-net-{session_id}")


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
    # Its own internal network, with no route out.
    assert info["HostConfig"]["NetworkMode"] == f"agentd-net-{s.id}"
    assert network(s.id)["Internal"] is True
    # The worker runs as root with only the capabilities it needs to run each
    # command as nobody and to own files for it.
    assert info["Config"]["User"] == "0:0"
    assert info["HostConfig"]["CapDrop"] == ["ALL"]
    added = sorted(c.removeprefix("CAP_") for c in info["HostConfig"]["CapAdd"])
    assert added == ["CHOWN", "DAC_OVERRIDE", "FOWNER", "SETGID", "SETUID"]
    assert info["HostConfig"]["Memory"] == 2048 * 1024 * 1024
    client.beta.sessions.archive(s.id)
    assert container(s.id) is None and network(s.id) is None


def test_a_deleted_session_takes_its_container_with_it(client: anthropic.Anthropic) -> None:
    s = session(client, "limited")
    info = container(s.id)
    assert info is not None and info["HostConfig"]["NetworkMode"] == f"agentd-net-{s.id}"
    client.beta.sessions.delete(s.id)
    assert container(s.id) is None and network(s.id) is None
