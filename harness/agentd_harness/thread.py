"""One agent thread: harness-sim's mini-swe-agent loop, with each model request, message and tool
call recorded on the session as Anthropic's events. A primary thread writes to the session's own
stream; a roster agent's thread to its `session_thread_id`."""

import secrets
import string

from minisweagent.agents.default import DefaultAgent

from .inbox import Interrupted
from .model import Model, text, usage

STEP_LIMIT = 250
ALLOW = {"type": "auto", "evaluated_permission": {"type": "allow"}}
_B62 = string.digits + string.ascii_letters


def thread_id():
    return "sthr_01" + "".join(secrets.choice(_B62) for _ in range(22))


class _NoEnv:
    """mini-swe-agent's environment slot: every action runs through the thread's own `act`."""

    def get_template_vars(self, **kwargs):
        return {}

    def serialize(self):
        return {}


class Thread:
    def __init__(self, run, agent, system, functions, policy, tid=None, record=True):
        self.run, self.agent, self.policy, self.tid, self.record = run, agent, policy, tid, record
        self.name = agent["name"]
        model_id = (agent.get("model") or {}).get("id") or agent.get("model")
        self.model = Model(model_name=model_id, functions=functions, base_url=run.model_base_url, user=self.name)
        self.loop = DefaultAgent(self.model, _NoEnv(), system_template="", instance_template="",
                                 step_limit=0, cost_limit=0)
        self.system = system
        self.handlers = {}
        # Set by a handler whose call ends the thread's work (the grader's verdict).
        self.finished = False
        self.usage = {"input_tokens": 0, "cache_read_input_tokens": 0, "cache_creation_input_tokens": 0, "output_tokens": 0}

    def emit(self, *events):
        if not self.record:
            return [None] * len(events)
        return self.run.agentd.emit(list(events), thread=self.tid)

    def send(self, prompt):
        """Run the agent on `prompt` until it answers; its reply."""
        a = self.loop
        if not a.messages:
            a.add_messages({"role": "system", "content": self.system})
        a.add_messages({"role": "user", "content": prompt})
        reply = ""
        for _ in range(STEP_LIMIT):
            self.run.inbox.check()
            (start,) = self.emit({"type": "span.model_request_start"})
            message = a.query()
            used = usage(message["extra"]["response"])
            self.usage = {k: self.usage[k] + used[k] for k in self.usage}
            end = {"type": "span.model_request_end", "is_error": False, "model_usage": used}
            if start:
                end["model_request_start_id"] = start["id"]
            self.emit(end)
            said = message.get("content")
            if isinstance(said, str) and said.strip():
                reply = said
                self.emit({"type": "agent.message", "content": [{"type": "text", "text": said}]})
            actions = message["extra"].get("actions") or []
            if not actions:
                return reply
            outputs = [self.act(action) for action in actions]
            a.add_messages(*self.model.format_observation_messages(message, outputs))
            if self.finished:
                return reply
        return reply

    def act(self, action):
        """Run one tool call, recorded as its `agent.tool_use` and `agent.tool_result`."""
        name, tool_input = action["tool"], action["input"]
        if name in self.handlers:
            content, is_error = self.handlers[name](tool_input)
            return {"content": content, "is_error": is_error}
        if name in self.policy.custom:
            return self.custom(name, tool_input)
        policy = self.policy.allowed.get(name)
        if policy is None:
            return {"content": [{"type": "text", "text": f"no tool named {name}"}], "is_error": True}
        if policy == "always_ask":
            (use,) = self.emit({"type": "agent.tool_use", "name": name, "input": tool_input, "evaluated_permission": "ask"})
            answer = self.run.wait(use["id"]) if use else {"result": "allow"}
            if answer.get("result") != "allow":
                content = [{"type": "text", "text": answer.get("deny_message") or "The user denied this tool call."}]
                self.emit({"type": "agent.tool_result", "tool_use_id": use["id"], "content": content, "is_error": True})
                return {"content": content, "is_error": True}
        else:
            (use,) = self.emit({"type": "agent.tool_use", "name": name, "input": tool_input,
                                "evaluated_permission": "allow", "evaluation": ALLOW})
        content, is_error = self.run.tools.run(name, tool_input)
        if use:
            self.emit({"type": "agent.tool_result", "tool_use_id": use["id"], "content": content, "is_error": is_error})
        return {"content": content, "is_error": is_error}

    def custom(self, name, tool_input):
        """A custom tool: the client runs it and answers with `user.custom_tool_result`."""
        (use,) = self.emit({"type": "agent.custom_tool_use", "name": name, "input": tool_input})
        answer = self.run.wait(use["id"])
        content = answer.get("content") or []
        return {"content": content, "is_error": bool(answer.get("is_error"))}


__all__ = ["Thread", "Interrupted", "thread_id", "text"]
