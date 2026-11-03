"""agentd, as the harness reaches it with its session's work token: the official SDK for what the
public API serves, and agentd's harness routes for what a harness writes back."""

import httpx
import anthropic

BETA = "managed-agents-2026-04-01"


class Agentd:
    def __init__(self, base_url, token, session_id):
        self.session_id = session_id
        self.client = anthropic.Anthropic(base_url=base_url, auth_token=token, max_retries=5)
        self.http = httpx.Client(
            base_url=base_url.rstrip("/"), timeout=120,
            headers={"Authorization": f"Bearer {token}", "anthropic-version": "2023-06-01", "anthropic-beta": BETA})

    def session(self):
        return self.client.beta.sessions.retrieve(self.session_id).model_dump(mode="json")

    def stream(self):
        """The session's live events, as dicts."""
        with self.client.beta.sessions.events.stream(self.session_id) as events:
            for event in events:
                yield event.model_dump(mode="json")

    def emit(self, events, thread=None):
        """Record `events` for `thread` (the primary thread when None); the recorded events, with ids."""
        body = {"events": [{**e, "session_thread_id": thread} if thread else e for e in events]}
        r = self.http.post(f"/v1/sessions/{self.session_id}/harness/events", json=body)
        r.raise_for_status()
        return r.json()["data"]

    def take(self):
        """The client events waiting for the agent, stamped processed."""
        r = self.http.post(f"/v1/sessions/{self.session_id}/harness/take")
        r.raise_for_status()
        return r.json()["data"]

    def save_output(self, path, data, previous=None):
        params = {"path": path, **({"previous": previous} if previous else {})}
        r = self.http.put(f"/v1/sessions/{self.session_id}/harness/outputs", params=params, content=data)
        r.raise_for_status()
        return r.json()["id"]

    def file(self, file_id):
        """A file the session mounts or a rubric it names: a client's uploads are not downloadable."""
        r = self.http.get(f"/v1/sessions/{self.session_id}/harness/files/{file_id}")
        r.raise_for_status()
        return r.content

    def skill(self, skill_id, version):
        if version in (None, "", "latest"):
            version = self.client.beta.skills.retrieve(skill_id).latest_version
        return self.client.beta.skills.versions.download(str(version), skill_id=skill_id).read()
