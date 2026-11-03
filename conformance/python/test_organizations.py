"""The Admin API through the SDK: an admin key reaches /v1/organizations and
nothing else, and a workspace key does not reach it."""

import uuid

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


def test_a_created_workspace_is_retrieved_and_listed(admin: anthropic.Anthropic) -> None:
    name = f"sdk-{uuid.uuid4().hex[:8]}"
    created = admin.organization.workspaces.create(name=name, tags={"suite": "python"})
    assert created.type == "workspace" and created.id.startswith("wrkspc_")
    assert created.name == name and created.archived_at is None
    assert created.data_residency.allowed_inference_geos == "unrestricted"
    assert created.data_residency.default_inference_geo == "global"
    assert created.tags == {"suite": "python"}
    assert admin.organization.workspaces.retrieve(created.id) == created
    page = admin.organization.workspaces.list(limit=1000)
    assert created.id in [w.id for w in page.data]
    first = admin.organization.workspaces.list(limit=1)
    assert first.first_id == first.last_id == first.data[0].id


def test_a_bad_workspace_is_a_bad_request(admin: anthropic.Anthropic) -> None:
    with pytest.raises(anthropic.BadRequestError):
        admin.organization.workspaces.create(name="x", display_color="red")
    with pytest.raises(anthropic.NotFoundError):
        admin.organization.workspaces.retrieve("wrkspc_01missingmissingmissin")
