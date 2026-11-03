"""The official Python SDK against a running agentd, with only the base URL changed.

AGENTD_BASE_URL (default http://127.0.0.1:8080) names the server,
AGENTD_TEST_KEY a workspace key it accepts and AGENTD_TEST_ADMIN_KEY an admin
key for the same organization.
"""

import os

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
