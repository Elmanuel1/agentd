"""One session's run: set the sandbox up, then act on each batch of client events, with the
session's status moving between running and idle as Anthropic's does."""

import json

from . import mounts
from .inbox import Inbox, Interrupted
from .outcome import run_outcome
from .prompt import system_prompt
from .team import FUNCTIONS, Team, roster
from .thread import Thread
from .tools import Policy, ToolHost

MODEL_KEY = "OPENROUTER_API_KEY"


def text_of(content):
    return "\n".join(b.get("text", "") for b in content or [] if b.get("type") == "text")


class SessionRun:
    def __init__(self, agentd, env):
        self.agentd = agentd
        self.model_base_url = env.get("AGENTD_MODEL_BASE_URL", "https://openrouter.ai/api/v1")
        self.credentials = json.loads(env.get("AGENTD_CREDENTIALS") or "[]")
        session = agentd.session()
        agent = session["agent"]
        members = roster(agent)
        mounts.prepare_dirs()
        mounts.mount_files(agentd, session)
        self.skills = mounts.mount_skills(agentd, [agent, *members])
        # The agent's tools never see the model key, not even as its placeholder.
        self.tools = ToolHost({k: v for k, v in env.items() if k != MODEL_KEY})
        self.inbox = Inbox(agentd)
        policy = Policy(agent)
        functions = policy.functions(FUNCTIONS if members else ())
        system = system_prompt(agent, self.skills.get(agent["name"], []), members, self.credentials)
        self.primary = Thread(self, agent, system, functions, policy)
        if members:
            Team(self, agent, members).bind(self.primary)

    def status(self, event):
        self.agentd.emit([event])

    def wait(self, event_id):
        """Hold the turn until the client answers event `event_id`."""
        self.status({"type": "session.status_idle", "stop_reason": {"type": "requires_action", "event_ids": [event_id]}})
        answer = self.inbox.answer(event_id)
        self.status({"type": "session.status_running"})
        return answer

    def handle(self, event):
        kind = event["type"]
        if kind == "user.message":
            self.primary.send(text_of(event.get("content")))
        elif kind == "user.define_outcome":
            run_outcome(self, self.primary, event)
        elif kind == "system.message":
            self.primary.loop.add_messages({"role": "system", "content": text_of(event.get("content"))})

    def serve(self):
        self.inbox.start()
        while (work := self.inbox.next_work()) is not None:
            self.status({"type": "session.status_running"})
            stop = {"type": "end_turn"}
            try:
                for event in work:
                    self.handle(event)
            except Interrupted:
                pass
            except Exception as e:  # noqa: BLE001 - a failed turn is reported on the session, not raised
                self.status({"type": "session.error", "error": {"type": "harness_error", "message": str(e)[-1000:]}})
                stop = {"type": "retries_exhausted"}
            try:
                mounts.sync_outputs(self.agentd)
            except Exception as e:  # noqa: BLE001 - the turn still ends; the failure is reported
                self.status({"type": "session.error", "error": {"type": "outputs_error", "message": str(e)[-1000:]}})
            self.status({"type": "session.status_idle", "stop_reason": stop})
        self.tools.close()
