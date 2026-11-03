"""The agent's tools as the harness offers them: the `agent_toolset` tools the agent's config
enables, each with its permission policy, run in the tool host; and the session's custom tools,
which the client answers."""

import itertools
import json
import os
import subprocess
import sys
import threading

from anthropic.lib.tools import agent_toolset as toolset

TOOLSET = "agent_toolset_20260401"
NOBODY = 65534
# What a tool's process may see: never the harness's own settings or its work token.
PASSED = ("PATH", "LANG", "LC_ALL", "TZ", "HTTP_PROXY", "HTTPS_PROXY", "http_proxy", "https_proxy",
          "ALL_PROXY", "NO_PROXY", "no_proxy", "SSL_CERT_FILE", "REQUESTS_CA_BUNDLE", "CURL_CA_BUNDLE",
          "NODE_EXTRA_CA_CERTS")


def specs():
    """Name, description and input schema of every toolset tool, from the SDK itself."""
    ctx = toolset.AgentToolContext(workdir="/tmp")
    return {t.name: t.to_dict() for t in toolset.beta_agent_toolset_20260401(ctx)}


class Policy:
    """Which tools an agent has and how each may run: `always_allow` or `always_ask`."""

    def __init__(self, agent):
        self.allowed = {}
        self.custom = {}
        for tool in agent.get("tools") or []:
            if tool.get("type") == TOOLSET:
                default = tool.get("default_config") or {}
                per = {c["name"]: c for c in tool.get("configs") or []}
                for name in specs():
                    config = {**default, **per.get(name, {})}
                    if config.get("enabled", True):
                        policy = (config.get("permission_policy") or {}).get("type", "always_allow")
                        self.allowed[name] = policy
            elif tool.get("type") == "custom":
                self.custom[tool["name"]] = tool

    def functions(self, extra=()):
        """The tools as OpenAI-compatible function definitions, for the model."""
        known = specs()
        out = []
        for name in self.allowed:
            spec = known[name]
            out.append({"type": "function", "function": {
                "name": name, "description": spec["description"], "parameters": spec["input_schema"]}})
        for name, tool in self.custom.items():
            out.append({"type": "function", "function": {
                "name": name, "description": tool.get("description", ""),
                "parameters": tool.get("input_schema") or {"type": "object", "properties": {}}}})
        return out + list(extra)


class ToolHost:
    """The tool host process, started as `nobody` with a clean environment."""

    def __init__(self, env=None, user=NOBODY):
        env = env if env is not None else os.environ
        clean = {k: env[k] for k in PASSED if k in env}
        clean["HOME"] = "/workspace"
        # The agent's credential placeholders, which are its to use.
        clean.update({k: v for k, v in env.items() if v.startswith("agentd-ph-")})
        privileged = os.geteuid() == 0
        self.proc = subprocess.Popen(
            [sys.executable, "-m", "agentd_harness.toolhost"],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True, env=clean, cwd="/",
            user=user if privileged else None, group=user if privileged else None,
            extra_groups=[] if privileged else None)
        self.ids = itertools.count(1)
        self.lock = threading.Lock()

    def run(self, name, tool_input):
        """`(content blocks, is_error)` of running tool `name` on `tool_input`."""
        with self.lock:
            request = {"id": next(self.ids), "name": name, "input": tool_input}
            self.proc.stdin.write(json.dumps(request) + "\n")
            self.proc.stdin.flush()
            line = self.proc.stdout.readline()
        if not line:
            return [{"type": "text", "text": "the tool host stopped"}], True
        reply = json.loads(line)
        return reply["content"], reply["is_error"]

    def close(self):
        self.proc.kill()


class ToolHosts:
    """A tool host per agent thread: threads run their tools at the same time, each in its own
    process, so one thread's long command never holds up another's."""

    def __init__(self, env):
        self.env, self.hosts, self.lock = env, {}, threading.Lock()

    def run(self, name, tool_input, owner=None):
        with self.lock:
            host = self.hosts.get(owner)
            if host is None:
                host = self.hosts[owner] = ToolHost(self.env)
        return host.run(name, tool_input)

    def close(self):
        with self.lock:
            for host in self.hosts.values():
                host.close()
