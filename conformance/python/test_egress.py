"""Credentials through the egress proxy: the sandbox sees each credential's
name and a placeholder, never the value; the proxy puts the value into
requests to the hosts the credential allows, refuses it anywhere else, and
masks it in responses.

Runs with the agent loop (AGENTD_TEST_RUNTIME=1, the scripted model) and
AGENTD_EGRESS_INTERNAL_HOSTS=host.docker.internal, so the sandbox may reach
the echo server this test starts on the runner.
"""

import http.server
import json
import os
import threading
import time
import uuid

import anthropic
import pytest

pytestmark = pytest.mark.skipif(os.environ.get("AGENTD_TEST_RUNTIME") != "1", reason="needs AGENTD_TEST_RUNTIME=1")

SECRET = f"s3cret-{uuid.uuid4().hex}"
SEEN: list[str] = []


class Echo(http.server.BaseHTTPRequestHandler):
    def do_GET(self) -> None:  # noqa: N802
        auth = self.headers.get("authorization", "")
        SEEN.append(auth)
        body = json.dumps({"you_sent": auth}).encode()
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *_args) -> None:
        pass


@pytest.fixture(scope="module", autouse=True)
def echo_server():
    server = http.server.ThreadingHTTPServer(("0.0.0.0", 18765), Echo)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    yield
    server.shutdown()


def test_a_credential_is_a_placeholder_in_the_sandbox_and_the_value_on_the_wire(client: anthropic.Anthropic) -> None:
    vault = client.beta.vaults.create(display_name=f"rt-egress-{uuid.uuid4().hex[:6]}")
    client.beta.vaults.credentials.create(
        vault.id,
        display_name="Echo service token",
        auth={
            "type": "environment_variable",
            "secret_name": "ECHO_TOKEN",
            "secret_value": SECRET,
            "networking": {"type": "limited", "allowed_hosts": ["host.docker.internal"]},
        },
    )
    env = client.beta.environments.create(
        name=f"rt-egress-{uuid.uuid4().hex[:6]}",
        config={"type": "cloud", "networking": {"type": "limited", "allowed_hosts": ["host.docker.internal"]}},
    )
    tools = [{"type": "agent_toolset_20260401", "default_config": {"permission_policy": {"type": "always_allow"}}}]
    agent = client.beta.agents.create(name="rt-egress", model="claude-sonnet-4-6", tools=tools)
    session = client.beta.sessions.create(agent=agent.id, environment_id=env.id, vault_ids=[vault.id])
    client.beta.sessions.events.send(session.id, events=[{"type": "user.message", "content": [{"type": "text", "text": "go"}]}])
    for _ in range(120):
        events = list(client.beta.sessions.events.list(session.id, limit=1000))
        if events and events[-1].type == "session.status_idle":
            break
        time.sleep(1)
    results = [e.content[0].text for e in events if e.type == "agent.tool_result"]
    assert len(results) == 3, results
    env_out, allowed, blocked = results
    assert "ECHO_TOKEN=agentd-ph-" in env_out and SECRET not in env_out
    assert SEEN == [f"Bearer {SECRET}"]
    assert "agentd-ph-" in allowed and SECRET not in allowed
    assert blocked.strip().startswith("403")
    everything = json.dumps([e.model_dump() for e in events])
    assert SECRET not in everything
