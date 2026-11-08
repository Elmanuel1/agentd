"""Client events, as the harness receives them: it follows the session's stream, catches up from
the session's events each time the stream opens, and hands each client event, once, to whoever
waits for it: new work for the loop, an answer for a tool call that waits, or an interrupt.
agentd stamps a client event processed when the harness next reports an event."""

import threading
import time

import anthropic
import httpx2

WORK = ("user.message", "user.define_outcome", "system.message")


class Interrupted(Exception):
    """The client interrupted the turn."""


def is_client(event):
    return event["type"].startswith(("user.", "system."))


class Inbox:
    def __init__(self, agentd):
        self.agentd = agentd
        self.cond = threading.Condition()
        self.work = []
        self.answers = {}
        self.interrupted = False
        self.closed = False
        self.seen = set()
        # Client events already processed when the harness started: an earlier run's.
        self.earlier = None

    def start(self):
        threading.Thread(target=self._follow, daemon=True).start()

    def _follow(self):
        """Follow the stream, reconnecting with a backoff; only the session's end stops it."""
        wait = 1
        try:
            while True:
                try:
                    # Open first, then catch up: an event sent in between is in one or the other.
                    with self.agentd.stream() as events:
                        self.catch_up()
                        for event in events:
                            wait = 1
                            if event["type"] == "session.deleted":
                                return
                            self.offer(event)
                except anthropic.NotFoundError:
                    return
                except (anthropic.APIError, httpx2.HTTPError):
                    pass
                time.sleep(wait)
                wait = min(wait * 2, 30)
        finally:
            with self.cond:
                self.closed = True
                self.cond.notify_all()

    def catch_up(self):
        history = [e for e in self.agentd.history() if is_client(e)]
        if self.earlier is None:
            self.earlier = {e["id"] for e in history if e.get("processed_at")}
        for event in history:
            self.offer(event)

    def offer(self, event):
        """Deliver a client event the harness has not had yet."""
        if not is_client(event) or event["id"] in self.seen or event["id"] in self.earlier:
            return
        self.seen.add(event["id"])
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
