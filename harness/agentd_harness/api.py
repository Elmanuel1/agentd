"""agentd, as the harness reaches it: the official SDK's public methods, with the session's work token."""

import contextlib

import anthropic


class Agentd:
    def __init__(self, client, session_id):
        self.client = client
        self.session_id = session_id

    def session(self):
        return self.client.beta.sessions.retrieve(self.session_id).model_dump(mode="json")

    @contextlib.contextmanager
    def stream(self):
        """The session's live events, as dicts; the stream is open once this enters."""
        with self.client.beta.sessions.events.stream(self.session_id) as events:
            yield (event.model_dump(mode="json") for event in events)

    def history(self):
        """Every event of the session so far, oldest first, as dicts."""
        for event in self.client.beta.sessions.events.list(self.session_id, limit=1000):
            yield event.model_dump(mode="json")

    def emit(self, events, thread=None):
        """Record `events` for `thread` (the primary thread when None); the recorded events, with ids."""
        sent = [{**e, "session_thread_id": thread} if thread else e for e in events]
        recorded = self.client.beta.sessions.events.send(self.session_id, events=sent)
        return [event.model_dump(mode="json") for event in recorded.data]

    def save_output(self, name, data):
        """Save `data` as the session's output `name`; its file id."""
        return self.client.beta.files.upload(file=(name, data)).id

    def drop_output(self, file_id):
        """Remove an output's earlier version; one already gone, a client deleted it say, is removed."""
        with contextlib.suppress(anthropic.NotFoundError):
            self.client.beta.files.delete(file_id)

    def file(self, file_id):
        """A file the session mounts or a rubric it names: a client's uploads are not downloadable."""
        return self.client.beta.files.download(file_id).read()

    def skill(self, skill_id, version):
        """A skill version's archive, as the SDK addresses it: the latest by its version id."""
        if version in (None, "", "latest"):
            version = self.client.beta.skills.retrieve(skill_id).latest_version_id
        return self.client.beta.skills.versions.download(str(version), skill_id=skill_id).read()
