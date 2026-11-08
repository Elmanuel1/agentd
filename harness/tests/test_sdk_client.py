"""The harness's one client to agentd, the official SDK: the requests it sends, the client events
it hands on once, and the tool host's results without the SDK's private helpers."""

import asyncio
import contextlib
import json

import anthropic
import httpx2
from anthropic.lib.tools import ToolError

from agentd_harness import toolhost
from agentd_harness.api import Agentd
from agentd_harness.inbox import Inbox


def recording_agentd():
    seen = []

    def handler(request):
        seen.append((request.method, request.url.path, request.content))
        if request.url.path.endswith("/events"):
            sent = json.loads(request.content)["events"]
            return httpx2.Response(200, json={"data": [{**e, "id": f"sevt_{i}"} for i, e in enumerate(sent)]})
        if request.url.path.endswith("/content"):
            return httpx2.Response(200, content=b"bytes")
        if request.method == "DELETE" and request.url.path.endswith("file_gone"):
            return httpx2.Response(404, json={"type": "error", "error": {"type": "not_found_error", "message": "x"}})
        if request.method == "DELETE":
            return httpx2.Response(200, json={"id": "file_old", "type": "file_deleted"})
        return httpx2.Response(200, json={"id": "file_new", "type": "file", "filename": "out.json",
                                          "mime_type": "application/json", "size_bytes": 2,
                                          "created_at": "2026-11-08T00:00:00Z", "downloadable": True})

    client = anthropic.Anthropic(base_url="http://agentd", auth_token="sk-ant-work-t", max_retries=0,
                                 http_client=httpx2.Client(transport=httpx2.MockTransport(handler)))
    return Agentd(client, "sesn_1"), seen


def test_agent_events_outputs_and_mounts_go_through_the_public_api():
    agentd, seen = recording_agentd()
    (use,) = agentd.emit([{"type": "agent.tool_use", "name": "bash", "input": {}}], thread="sthr_2")
    assert use["id"] == "sevt_0" and use["type"] == "agent.tool_use"
    assert agentd.save_output("out.json", b"{}") == "file_new"
    agentd.drop_output("file_old")
    agentd.drop_output("file_gone")
    assert agentd.file("file_in") == b"bytes"
    method, path, body = seen[0]
    assert (method, path) == ("POST", "/v1/sessions/sesn_1/events")
    assert json.loads(body)["events"][0]["session_thread_id"] == "sthr_2"
    assert [(m, p) for m, p, _ in seen[1:]] == [
        ("POST", "/v1/files"), ("DELETE", "/v1/files/file_old"), ("DELETE", "/v1/files/file_gone"),
        ("GET", "/v1/files/file_in/content")]
    assert not any("/harness/" in p for _, p, _ in seen)


class History:
    def __init__(self, events):
        self.events = events

    def history(self):
        return iter(self.events)


def test_each_client_event_is_handed_on_once_and_an_earlier_run_s_are_not():
    earlier = {"id": "sevt_1", "type": "user.message", "processed_at": "2026-11-08T00:00:00Z"}
    queued = {"id": "sevt_2", "type": "user.message", "processed_at": None}
    inbox = Inbox(History([earlier, queued, {"id": "sevt_3", "type": "agent.message"}]))
    inbox.catch_up()
    inbox.offer({**queued, "processed_at": "2026-11-08T00:00:01Z"})
    later = {"id": "sevt_4", "type": "user.message", "processed_at": "2026-11-08T00:00:02Z"}
    inbox.agentd.events.append(later)
    inbox.catch_up()
    assert [e["id"] for e in inbox.work] == ["sevt_2", "sevt_4"]


def test_a_dropped_stream_reconnects_catches_up_and_hands_each_event_on_once(monkeypatch):
    import agentd_harness.inbox as inbox_module

    monkeypatch.setattr(inbox_module.time, "sleep", lambda _s: None)
    first = {"id": "sevt_1", "type": "user.message", "processed_at": None}
    second = {"id": "sevt_2", "type": "user.message", "processed_at": None}

    class Flaky:
        opened = 0
        events = [first]

        def history(self):
            return iter(list(self.events))

        @contextlib.contextmanager
        def stream(self):
            self.opened += 1
            yield self.frames()

        def frames(self):
            if self.opened == 1:
                yield first
                self.events.append(second)
                raise httpx2.ReadError("connection reset")
            yield second
            yield {"id": "sevt_3", "type": "session.deleted"}

    inbox = Inbox(Flaky())
    inbox._follow()
    assert [e["id"] for e in inbox.work] == ["sevt_1", "sevt_2"] and inbox.closed
    assert inbox.agentd.opened == 2


def test_tool_results_are_well_formed_without_the_sdk_s_private_helpers(tmp_path):
    assert toolhost.blocks("a\ud800b") == [{"type": "text", "text": "a�b"}]
    assert toolhost.error_content(ToolError([{"type": "text", "text": "no"}])) == [{"type": "text", "text": "no"}]
    assert toolhost.error_content(ValueError("x" * 5000)).startswith("ValueError(")
    assert len(toolhost.error_content(ValueError("x" * 5000))) == toolhost.FIELD_MAX
    out = []

    async def feed():
        yield json.dumps({"id": 1, "name": "bash", "input": {}})

    asyncio.run(toolhost.serve(feed(), out.append, workdir=tmp_path, roots=[tmp_path], read_only=[]))
    assert json.loads(out[0])["is_error"]
