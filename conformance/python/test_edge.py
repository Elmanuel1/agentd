"""The wire edge as the SDK sees it: auth, versions, betas, request ids, retries."""

import uuid

import anthropic
import pytest


def test_the_sdks_bearer_auth_is_accepted(client: anthropic.Anthropic) -> None:
    page = client.beta.environments.list()
    assert isinstance(page.data, list)


def test_an_unknown_key_is_an_authentication_error_with_a_request_id(base_url: str) -> None:
    bad = anthropic.Anthropic(api_key="not-a-key", base_url=base_url)
    with pytest.raises(anthropic.AuthenticationError) as caught:
        bad.beta.environments.list()
    assert caught.value.request_id is not None
    assert caught.value.request_id.startswith("req_")
    assert caught.value.body["request_id"] == caught.value.request_id


def test_the_terraform_api_version_is_accepted(client: anthropic.Anthropic) -> None:
    client.beta.environments.list(extra_headers={"anthropic-version": "2023-06-01"})


def test_an_unknown_beta_is_a_bad_request(client: anthropic.Anthropic) -> None:
    with pytest.raises(anthropic.BadRequestError) as caught:
        client.beta.environments.list(betas=["not-a-beta-2020-01-01"])
    assert "not-a-beta-2020-01-01" in caught.value.message


def test_a_successful_response_carries_a_request_id(client: anthropic.Anthropic) -> None:
    page = client.beta.environments.list()
    assert page._request_id is not None and page._request_id.startswith("req_")


def test_a_conflict_is_not_retried(counted) -> None:
    client, counter = counted
    store = client.beta.memory_stores.create(name=f"conflict-{uuid.uuid4().hex[:8]}")
    client.beta.memory_stores.memories.create(store.id, path="/a.md", content="one")
    counter.requests = 0
    with pytest.raises(anthropic.ConflictError):
        client.beta.memory_stores.memories.create(store.id, path="/a.md", content="two")
    assert counter.requests == 1
