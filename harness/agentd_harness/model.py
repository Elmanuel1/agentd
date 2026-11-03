"""The model as harness-sim's mini-swe-agent loop calls it: OpenAI-compatible chat completions
(OpenRouter by default), with the agent's tools as functions instead of mini-swe-agent's single
bash tool. The key is a placeholder; the egress proxy puts the real one in."""

import json
import os

import requests
from minisweagent.models.openrouter_model import OpenRouterModel


class Model(OpenRouterModel):
    def __init__(self, *, functions, base_url, user, **kwargs):
        kwargs.setdefault("cost_tracking", "ignore_errors")
        super().__init__(**kwargs)
        self.functions = functions
        # The thread's name, which a scripted model picks its replies by.
        self.user = user
        self._api_url = f"{base_url.rstrip('/')}/chat/completions"
        self._api_key = os.environ.get("OPENROUTER_API_KEY", "")

    def _query(self, messages, **kwargs):
        payload = {"model": self.config.model_name, "messages": messages, "usage": {"include": True}, "user": self.user,
                   **({"tools": self.functions} if self.functions else {}), **(self.config.model_kwargs | kwargs)}
        r = requests.post(self._api_url, data=json.dumps(payload), timeout=900,
                          headers={"Authorization": f"Bearer {self._api_key}", "Content-Type": "application/json"})
        if r.status_code >= 400:
            raise RuntimeError(f"model: HTTP {r.status_code}: {r.text[:500]}")
        return r.json()

    def _prepare_messages_for_api(self, messages):
        """The conversation as the API takes it: no `exit` entries, and a result for every tool call."""
        out, answered = [], {m.get("tool_call_id") for m in messages if m.get("role") == "tool"}
        for m in messages:
            if m.get("role") == "exit":
                continue
            out.append({k: v for k, v in m.items() if k != "extra"})
            for call in (m.get("tool_calls") or []) if m.get("role") == "assistant" else []:
                if call.get("id") not in answered:
                    out.append({"role": "tool", "tool_call_id": call["id"], "content": "Not run."})
        return out

    def _parse_actions(self, response):
        """Each tool call as `{tool, input, tool_call_id}`; none means the reply is the answer."""
        actions = []
        for call in response["choices"][0]["message"].get("tool_calls") or []:
            fn = call.get("function") or {}
            try:
                args = json.loads(fn.get("arguments") or "{}")
            except json.JSONDecodeError:
                args = {"_unparsed": fn.get("arguments")}
            actions.append({"tool": fn.get("name"), "input": args if isinstance(args, dict) else {}, "tool_call_id": call.get("id")})
        return actions

    def format_observation_messages(self, message, outputs, template_vars=None):
        return [{"role": "tool", "tool_call_id": a["tool_call_id"], "content": text(o)}
                for a, o in zip(message.get("extra", {}).get("actions", []), outputs)]


def text(output):
    """A tool result's content blocks as the text the model reads."""
    parts = [b.get("text", "") if b.get("type") == "text" else f"[{b.get('type')}]" for b in output["content"]]
    body = "\n".join(parts)
    return f"Error: {body}" if output["is_error"] else body


def usage(response):
    """OpenRouter usage as Anthropic `model_usage`: input tokens exclude cache reads."""
    u = response.get("usage") or {}
    cached = (u.get("prompt_tokens_details") or {}).get("cached_tokens", 0) or 0
    return {"input_tokens": max((u.get("prompt_tokens") or 0) - cached, 0), "cache_read_input_tokens": cached,
            "cache_creation_input_tokens": 0, "output_tokens": u.get("completion_tokens") or 0}
