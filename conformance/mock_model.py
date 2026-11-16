"""A scripted model for agentd's loop: each request gets the next reply of the script named by
the request's agent (`user` on a chat completion, `metadata.user_id` on an Anthropic message), or
of `default`.

Scripts are Anthropic-shaped replies (`content` blocks with `text` and `tool_use`, `stop`, and
`usage`). `POST …/chat/completions` answers them as chat completions with `tool_calls`; a reply's
`usage.cost` is its list cost in USD, as OpenRouter reports it (0 when left out; `null` answers
without one, as a model with no list price). `POST /v1/messages` answers them as Anthropic
messages. `GET …/models` lists `--model` ids with a `context_length`, as OpenRouter does, and
`GET /v1/models/{id}` answers `max_input_tokens`, as Anthropic does. Used by CI and the scripted
demo.

    python conformance/mock_model.py --port 18766 conformance/fixtures/mock-model.json
"""

import argparse
import json
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer


class Script:
    def __init__(self, replies):
        self.replies = replies
        self.next = {}
        self.lock = threading.Lock()

    def reply(self, name):
        with self.lock:
            key = name if name in self.replies else "default"
            replies = self.replies.get(key) or [{"content": [{"type": "text", "text": "ok"}]}]
            i = self.next.get(key, 0)
            self.next[key] = i + 1
            return replies[min(i, len(replies) - 1)] if key == "default" or i < len(replies) else self.default()

    def default(self):
        replies = self.replies.get("default") or [{"content": [{"type": "text", "text": "ok"}]}]
        return replies[-1]


def completion(reply):
    text = "\n".join(b["text"] for b in reply.get("content", []) if b.get("type") == "text")
    calls = [{"id": b["id"], "type": "function", "function": {"name": b["name"], "arguments": json.dumps(b.get("input", {}))}}
             for b in reply.get("content", []) if b.get("type") == "tool_use"]
    u = reply.get("usage") or {}
    cost = u.get("cost", 0)
    message = {"role": "assistant", "content": text or None, **({"tool_calls": calls} if calls else {})}
    return {"id": "chatcmpl-mock", "object": "chat.completion", "model": "mock",
            "choices": [{"index": 0, "message": message, "finish_reason": "tool_calls" if calls else "stop"}],
            "usage": {"prompt_tokens": u.get("input_tokens", 0) + u.get("cache_read_input_tokens", 0),
                      "completion_tokens": u.get("output_tokens", 0),
                      "prompt_tokens_details": {"cached_tokens": u.get("cache_read_input_tokens", 0)},
                      **({"cost": cost} if cost is not None else {})}}


def message(reply):
    content = [b for b in reply.get("content", []) if b.get("type") in ("text", "tool_use", "thinking")]
    calls = any(b["type"] == "tool_use" for b in content)
    u = reply.get("usage") or {}
    return {"id": "msg_mock", "type": "message", "role": "assistant", "model": "mock", "content": content,
            "stop_reason": reply.get("stop") or ("tool_use" if calls else "end_turn"), "stop_sequence": None,
            "usage": {k: u.get(k, 0) for k in ("input_tokens", "output_tokens", "cache_read_input_tokens",
                                               "cache_creation_input_tokens")}}


def handler(script, models, window):
    class Handler(BaseHTTPRequestHandler):
        def _send(self, body, status=200):
            data = json.dumps(body).encode()
            self.send_response(status)
            self.send_header("content-type", "application/json")
            self.send_header("content-length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)

        def do_GET(self):
            path = self.path.split("?")[0]
            if path.startswith("/v1/models/"):
                model = path.removeprefix("/v1/models/")
                return self._send({"type": "model", "id": model, "max_input_tokens": window})
            if path.endswith("/models"):
                return self._send({"data": [{"id": m, "context_length": window} for m in models]})
            self._send({"error": {"message": "not found"}}, 404)

        def do_POST(self):
            body = json.loads(self.rfile.read(int(self.headers.get("content-length", 0))) or b"{}")
            if self.path.split("?")[0].endswith("/v1/messages"):
                return self._send(message(script.reply((body.get("metadata") or {}).get("user_id", "default"))))
            if not self.path.endswith("/chat/completions"):
                return self._send({"error": {"message": "not found"}}, 404)
            self._send(completion(script.reply(body.get("user", "default"))))

        def log_message(self, *args):
            pass

    return Handler


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("script")
    ap.add_argument("--port", type=int, default=18766)
    ap.add_argument("--model", action="append", default=[], help="an id GET …/models lists (repeatable)")
    ap.add_argument("--window", type=int, default=200000, help="each model's context window in tokens")
    args = ap.parse_args()
    with open(args.script) as f:
        script = Script(json.load(f))
    ThreadingHTTPServer(("0.0.0.0", args.port), handler(script, args.model, args.window)).serve_forever()


if __name__ == "__main__":
    main()
