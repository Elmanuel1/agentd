"""A coordinator's roster: `list_agents` and `send_to_agent`, each delegation a session thread of
its own with Anthropic's thread events. The coordinator decides what to delegate."""

import json

from .prompt import system_prompt
from .thread import Thread, thread_id
from .tools import Policy

LIST_AGENTS = {"type": "function", "function": {
    "name": "list_agents", "description": "List the agents you can delegate to.",
    "parameters": {"type": "object", "properties": {}}}}
SEND_TO_AGENT = {"type": "function", "function": {
    "name": "send_to_agent",
    "description": "Send a task to a roster agent (a new thread) or a follow-up to a thread you started; returns its reply.",
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
        self.run.agentd.emit([{"type": "session.thread_status_running", "agent_name": thread.name}], thread=thread.tid)
        reply = thread.send(message)
        self.run.agentd.emit([{"type": "session.thread_status_idle", "agent_name": thread.name,
                               "stop_reason": {"type": "end_turn"}, "stop_details": None}], thread=thread.tid)
        answer = [{"type": "text", "text": reply}]
        self.run.agentd.emit([{"type": "agent.thread_message_received", "content": answer,
                               "from_agent_name": thread.name, "from_session_thread_id": thread.tid}])
        return [{"type": "text", "text": f"Message from {thread.name} (thread {thread.tid}):\n{reply}"}], False


def describe(members):
    return json.dumps([{"name": m["name"], "description": m.get("description")} for m in members])
