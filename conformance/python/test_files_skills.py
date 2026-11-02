"""Files and skills through the GA SDK, plus the beta shapes by header."""

import io
import json
import uuid

import anthropic
import pytest

MD = b"---\nname: sdk-skill\ndescription: A conformance skill.\n---\n# body\n"


def upload(client: anthropic.Anthropic, name: str = "a.txt", data: bytes = b"data", **kw):
    return client.files.upload(file=(name, io.BytesIO(data), "text/plain"), **kw)


def test_the_files_list_pages_with_next_page(client: anthropic.Anthropic) -> None:
    ids = {upload(client, f"p{i}.txt").id for i in range(3)}
    first = client.files.list(limit=1)
    assert first.next_page is not None
    assert ids <= {f.id for f in client.files.list(limit=1)}
    picked = client.files.list(ids=list(ids)[:2])
    assert {f.id for f in picked.data} == set(list(ids)[:2])


def test_an_upload_serves_the_ga_fields(client: anthropic.Anthropic) -> None:
    meta = upload(client, "dir/sub/nested.txt", expires_in_seconds=3600)
    assert (meta.filename, meta.downloadable) == ("nested.txt", False)
    assert meta.expires_at is not None
    empty = upload(client, "empty.txt", b"")
    assert empty.size_bytes == 0 and empty.expires_at is None
    with pytest.raises(anthropic.BadRequestError):
        upload(client, expires_in_seconds=10)


def test_an_upload_is_not_downloadable_and_errors_match_live(client: anthropic.Anthropic) -> None:
    meta = upload(client)
    with pytest.raises(anthropic.BadRequestError) as caught:
        client.files.download(meta.id)
    assert caught.value.body["error"]["details"] == {"error_code": "file_not_downloadable"}
    with pytest.raises(anthropic.NotFoundError) as caught:
        client.files.retrieve_metadata("file_011CZZZZZZZZZZZZZZZZZZZZ")
    assert caught.value.body["error"]["message"] == "File `file_011CZZZZZZZZZZZZZZZZZZZZ` not found."
    with pytest.raises(anthropic.BadRequestError) as caught:
        client.files.retrieve_metadata("not-a-file-id")
    assert caught.value.body["error"]["message"] == "File id must have `file_` prefix."


def test_the_beta_files_header_selects_the_beta_list_shape(client: anthropic.Anthropic) -> None:
    upload(client)
    raw = client.files.with_raw_response.list(limit=1, extra_headers={"anthropic-beta": "files-api-2025-04-14"})
    body = json.loads(raw.http_response.text)
    assert {"data", "first_id", "has_more", "last_id"} <= set(body) and "next_page" not in body
    assert "expires_at" not in body["data"][0]


def skill_files(directory: str):
    return [(f"{directory}/SKILL.md", io.BytesIO(MD), "text/markdown"), (f"{directory}/notes/x.md", io.BytesIO(b"x"), "text/markdown")]


def test_a_skill_lifecycle_in_ga_shapes(client: anthropic.Anthropic) -> None:
    label = f"sdk skill {uuid.uuid4().hex[:6]}"
    skill = client.skills.create(files=skill_files("sdk-skill"), display_name=label)
    assert (skill.display_name, skill.source.type) == (label, "custom")
    assert skill.latest_version_id.startswith("skver_")
    v2 = client.skills.versions.create(skill.id, files=skill_files("sdk-skill"))
    assert v2.id.startswith("skver_") and v2.name == "sdk-skill"
    assert client.skills.retrieve(skill.id).latest_version_id == v2.id
    assert client.skills.versions.retrieve("latest", skill_id=skill.id).id == v2.id
    assert [v.id for v in client.skills.versions.list(skill.id)] == [v2.id, skill.latest_version_id]
    gone = client.skills.versions.delete(v2.id, skill_id=skill.id)
    assert (gone.id, gone.type) == (v2.id, "skill_version_deleted")
    assert client.skills.retrieve(skill.id).latest_version_id == skill.latest_version_id
    with pytest.raises(anthropic.BadRequestError):
        client.skills.versions.delete(skill.latest_version_id, skill_id=skill.id)
    assert skill.id in {s.id for s in client.skills.list(source="custom")}
    deleted = client.skills.delete(skill.id)
    assert (deleted.id, deleted.type) == (skill.id, "skill_deleted")
    with pytest.raises(anthropic.NotFoundError):
        client.skills.retrieve(skill.id)


def test_a_root_skill_md_and_the_live_validation(client: anthropic.Anthropic) -> None:
    skill = client.skills.create(files=[("SKILL.md", io.BytesIO(MD), "text/markdown")])
    assert skill.display_name == "sdk-skill"
    with pytest.raises(anthropic.BadRequestError) as caught:
        client.skills.create(files=[("a/README.md", io.BytesIO(b"x"), "text/markdown")])
    assert caught.value.body["error"]["message"] == "SKILL.md not found in uploaded files"


def test_the_beta_skills_header_selects_the_beta_shapes(client: anthropic.Anthropic) -> None:
    beta = {"anthropic-beta": "skills-2025-10-02"}
    raw = client.skills.with_raw_response.create(files=skill_files("beta-skill"), extra_headers=beta)
    skill = json.loads(raw.http_response.text)
    assert skill["source"] == "custom" and skill["latest_version"].isdigit() and "display_title" in skill
    versions = client.skills.versions.with_raw_response.list(skill["id"], extra_headers=beta, extra_query={"beta": "true"})
    v = json.loads(versions.http_response.text)["data"][0]
    assert v["id"].startswith("skill_version_") and v["directory"] == "beta-skill"


def test_a_page_cursor_survives_deleting_its_anchor(client: anthropic.Anthropic) -> None:
    for i in range(3):
        upload(client, f"anchor{i}.txt")
    first = client.files.list(limit=1)
    client.files.delete(first.data[0].id)
    second = client.files.list(limit=1, page=first.next_page)
    assert second.data[0].id != first.data[0].id
    assert second.data[0].created_at <= first.data[0].created_at
