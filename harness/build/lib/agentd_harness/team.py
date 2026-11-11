"""A coordinator's roster: `list_agents` and `send_to_agent`, each delegation a session thread of
its own with Anthropic's thread events. The coordinator decides what to delegate. Delegation does
not block: the thread works while the coordinator goes on, and its reply comes back as a message
that starts the coordinator's next turn. The session settles once every thread is idle."""

import json
import queue
import threading

from .budget import BudgetReached
from .prompt import system_prompt
from .thread import Thread, thread_id
from .tools import Policy

LIST_AGENTS = {"type": "function", "function": {
    "name": "list_agents", "description": "List the agents you can delegate to.",
    "parameters": {"type": "object", "properties": {}}}}
SEND_TO_AGENT = {"type": "function", "function": {
    "name": "send_to_agent",
    "description": "Send a task to a roster agent (a new thread) or a follow-up to a thread you started. Its reply arrives later as a message from that thread.",
    "parameters": {"type": "object", "properties": {
        "agent_name": {"type": "string", "description": "A roster agent, for a new thread."},
        "session_thread_id": {"type": "string", "description": "A thread you started, for a follow-up."},
        "message": {"type": "string"}}, "required": ["message"]}}}
FUNCTIONS = [LIST_AGENTS, SEND_TO_AGENT]


def roster(agent):
    multi = agent.get("multiagent") or {}
    if multi.get("type") != "coordinator":
        return []
    return [a for a in multi.get("agents") or [] if isinstance(a, dict) and a.get("name")]


class Team:
    def __init__(self, run, coordinator, members):
        self.run, self.coordinator, self.members = run, coordinator, {m["name"]: m for m in members}
        self.threads = {}
        self.primary_id = "sthr_" + run.agentd.session_id.removeprefix("sesn_")
        self.replies = queue.Queue()
        self.running = 0
        # Set when the budget stopped a thread: the turn ends budget_reached, not end_turn.
        self.over_budget = False
        self.lock = threading.Lock()

    def bind(self, thread):
        thread.handlers["list_agents"] = self.list_agents
        thread.handlers["send_to_agent"] = self.send_to_agent

    def list_agents(self, _input):
        lines = [f"- {name}: {m.get('description') or ''}" for name, m in self.members.items()]
        return [{"type": "text", "text": "\n".join(lines) or "No agents."}], False

    def open(self, name):
        member = self.members[name]
        tid = thread_id()
        system = system_prompt(member, self.run.skills.get(name, []), [], self.run.credentials)
        policy = Policy(member)
        thread = Thread(self.run, member, system, policy.functions(), policy, tid=tid)
        self.threads[tid] = thread
        self.run.agentd.emit([{"type": "session.thread_created", "agent_name": name}], thread=tid)
        return thread

    def send_to_agent(self, tool_input):
        message = tool_input.get("message") or ""
        tid, name = tool_input.get("session_thread_id"), tool_input.get("agent_name")
        if tid:
            thread = self.threads.get(tid)
            if thread is None:
                return [{"type": "text", "text": f"send_to_agent: no thread {tid}"}], True
        elif name in self.members:
            thread = self.open(name)
        else:
            return [{"type": "text", "text": f"send_to_agent: no agent named {name}; see list_agents"}], True
        content = [{"type": "text", "text": message}]
        self.run.agentd.emit([{"type": "agent.thread_message_sent", "content": content,
                               "to_agent_name": thread.name, "to_session_thread_id": thread.tid}])
        with self.lock:
            self.running += 1
        threading.Thread(target=self._work, args=(thread, message), daemon=True).start()
        return [{"type": "text", "text": f"Sent to thread {thread.tid}. The reply arrives as a message from that thread."}], False

    def _work(self, thread, message):
        """Run `thread` on `message`, then hand its reply to the coordinator."""
        try:
            self.run.agentd.emit([
                {"type": "session.thread_status_running", "agent_name": thread.name},
                {"type": "agent.thread_message_received", "content": [{"type": "text", "text": message}],
                 "from_agent_name": self.coordinator["name"], "from_session_thread_id": self.primary_id},
            ], thread=thread.tid)
            try:
                reply = thread.send(message)
            except BudgetReached:
                self.run.agentd.emit([{"type": "session.thread_status_idle", "agent_name": thread.name,
                                       "stop_reason": {"type": "budget_reached"}, "stop_details": None}], thread=thread.tid)
                self.over_budget = True
                return
            except Exception as e:  # noqa: BLE001 - the coordinator hears of a thread that failed
                reply = f"The thread failed: {str(e)[-500:]}"
            content = [{"type": "text", "text": reply}]
            self.run.agentd.emit([
                {"type": "agent.thread_message_sent", "content": content, "to_session_thread_id": self.primary_id},
                {"type": "session.thread_status_idle", "agent_name": thread.name,
                 "stop_reason": {"type": "end_turn"}, "stop_details": None},
            ], thread=thread.tid)
            self.run.agentd.emit([{"type": "agent.thread_message_received", "content": content,
                                   "from_agent_name": thread.name, "from_session_thread_id": thread.tid}])
            self.replies.put(f"Message from {thread.name} (thread {thread.tid}):\n{reply}")
        finally:
            with self.lock:
                self.running -= 1
            self.replies.put(None)

    def settle(self, primary):
        """Give the coordinator (None: one the budget stopped) each thread's reply as it comes, until
        every thread is idle. Once the budget stops it, the other threads stop at their next request."""
        stopped = None
        while True:
            with self.lock:
                if self.running == 0 and self.replies.empty():
                    break
            try:
                note = self.replies.get(timeout=1)
            except queue.Empty:
                self.run.inbox.check()
                continue
            if note is None or primary is None:
                continue
            try:
                primary.send(note)
            except BudgetReached as e:
                primary, stopped = None, e
        over, self.over_budget = self.over_budget, False
        if stopped or over:
            raise stopped or BudgetReached()


def describe(members):
    return json.dumps([{"name": m["name"], "description": m.get("description")} for m in members])
