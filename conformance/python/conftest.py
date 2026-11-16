"""The official Python SDK against a running agentd, with only the base URL changed.

AGENTD_BASE_URL (default http://127.0.0.1:8080) names the server,
AGENTD_TEST_KEY a workspace key it accepts and AGENTD_TEST_ADMIN_KEY an admin
key for the same organization. The `worker` fixture serves a `self_hosted`
environment with the official worker (`ant beta:worker poll`), its environment
key made with `agentd environment-keys create` (AGENTD_BIN, default `agentd`)
for AGENTD_TEST_ORG and AGENTD_TEST_WORKSPACE.
"""

import os
import subprocess
import uuid

import anthropic
import pytest


@pytest.fixture(scope="session")
def base_url() -> str:
    return os.environ.get("AGENTD_BASE_URL", "http://127.0.0.1:8080")


@pytest.fixture(scope="session")
def key() -> str:
    return os.environ["AGENTD_TEST_KEY"]


@pytest.fixture
def client(base_url: str, key: str) -> anthropic.Anthropic:
    return anthropic.Anthropic(api_key=key, base_url=base_url)


@pytest.fixture
def admin(base_url: str) -> anthropic.Anthropic:
    return anthropic.Anthropic(api_key=os.environ["AGENTD_TEST_ADMIN_KEY"], base_url=base_url)


class Counter:
    """Counts the HTTP requests a client sends, retries included."""

    def __init__(self) -> None:
        self.requests = 0

    def __call__(self, _request: object) -> None:
        self.requests += 1


@pytest.fixture
def counted(base_url: str, key: str) -> tuple[anthropic.Anthropic, Counter]:
    counter = Counter()
    http = anthropic.DefaultHttpxClient(event_hooks={"request": [counter]})
    return anthropic.Anthropic(api_key=key, base_url=base_url, http_client=http), counter


@pytest.fixture
def worker(client: anthropic.Anthropic, base_url: str, tmp_path):
    """Start an environment served by the official worker; returns the environment."""
    pollers = []

    def start():
        env = client.beta.environments.create(name=f"w-{uuid.uuid4().hex[:8]}", config={"type": "self_hosted"})
        made = subprocess.run(
            [os.environ.get("AGENTD_BIN", "agentd"), "environment-keys", "create", "--org", os.environ.get("AGENTD_TEST_ORG", "org_sdk"),
             "--workspace", os.environ.get("AGENTD_TEST_WORKSPACE", "wrkspc_sdk"), "--environment", env.id, "--name", "conformance"],
            capture_output=True, text=True, check=True)
        workdir = tmp_path / env.id
        workdir.mkdir()
        log = open(tmp_path / f"{env.id}.poller.log", "w")
        pollers.append(subprocess.Popen(
            ["ant", "beta:worker", "poll", "--environment-id", env.id, "--base-url", base_url, "--workdir", str(workdir), "--max-idle", "0"],
            env={**os.environ, "ANTHROPIC_ENVIRONMENT_KEY": made.stdout.strip()}, stdout=log, stderr=log))
        return env

    yield start
    for poller in pollers:
        poller.terminate()
        poller.wait(timeout=10)
