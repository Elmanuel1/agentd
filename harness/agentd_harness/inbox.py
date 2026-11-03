"""Client events, as the harness receives them: it follows the session's stream, takes each
queued client event from agentd (which stamps it processed), and hands it to whoever waits for
it: new work for the loop, an answer for a tool call that waits, or an interrupt."""

import threading
import time

import anthropic
import httpx

WORK = ("user.message", "user.define_outcome", "system.message")


class Interrupted(Exception):
    """The client interrupted the turn."""


class Inbox:
    def __init__(self, agentd):
        self.agentd = agentd
        self.cond = threading.Condition()
        self.work = []
        self.answers = {}
        self.interrupted = False
        self.closed = False

    def start(self):
        threading.Thread(target=self._follow, daemon=True).start()

    def _follow(self):
        """Follow the stream, reconnecting with a backoff; only the session's end stops it."""
        wait = 1
        try:
            while True:
                try:
                    # A client event sent while no stream was open is taken here.
                    self.take()
                    for event in self.agentd.stream():
                        wait = 1
                        if event["type"] == "session.deleted":
                            return
                        if event["type"].startswith(("user.", "system.")) and not event.get("processed_at"):
                            self.take()
                except anthropic.NotFoundError:
                    return
                except (anthropic.APIError, httpx.HTTPError):
                    pass
                time.sleep(wait)
                wait = min(wait * 2, 30)
        finally:
            with self.cond:
                self.closed = True
                self.cond.notify_all()

    def take(self):
        for event in self.agentd.take():
            self.deliver(event)

    def deliver(self, event):
        with self.cond:
            kind = event["type"]
            if kind == "user.interrupt":
                self.interrupted = True
            elif kind == "user.tool_confirmation":
                self.answers[event["tool_use_id"]] = event
            elif kind == "user.custom_tool_result":
                self.answers[event["custom_tool_use_id"]] = event
            elif kind in WORK:
                self.work.append(event)
            self.cond.notify_all()

    def next_work(self):
        """The client events to act on next, or None once the session is gone."""
        with self.cond:
            self.cond.wait_for(lambda: self.work or self.closed)
            work, self.work = self.work, []
            self.interrupted = False
            return work or None

    def check(self):
        if self.interrupted:
            raise Interrupted()

    def answer(self, event_id):
        """The client's answer to event `event_id`; raises Interrupted if the turn is interrupted."""
        with self.cond:
            self.cond.wait_for(lambda: event_id in self.answers or self.interrupted or self.closed)
            if event_id in self.answers:
                return self.answers.pop(event_id)
            raise Interrupted()
