"""The Admin API through the SDK: an admin key reaches /v1/organizations and
nothing else, and a workspace key does not reach it."""

import anthropic
import pytest


def test_the_admin_key_retrieves_its_organization(admin: anthropic.Anthropic) -> None:
    org = admin.organization.retrieve()
    assert org.type == "organization"
    assert org.id == "org_sdk" and org.name == "org_sdk"


def test_a_workspace_key_is_refused_on_the_admin_api(client: anthropic.Anthropic) -> None:
    with pytest.raises(anthropic.AuthenticationError):
        client.organization.retrieve()


def test_an_admin_key_is_refused_on_workspace_routes(admin: anthropic.Anthropic) -> None:
    with pytest.raises(anthropic.AuthenticationError):
        admin.beta.environments.list()
