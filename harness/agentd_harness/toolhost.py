"""The tool host: runs the agent's tools for the harness, as `nobody`.

It reads one JSON request per line on stdin, `{"id", "name", "input"}`, runs the named tool of the
SDK's `agent_toolset_20260401`, and writes `{"id", "content", "is_error"}` on stdout. It runs as
an unprivileged user with a clean environment, so a tool never sees the harness's work token.
"""

import asyncio
import json
import sys
from pathlib import Path

from anthropic.lib.tools import agent_toolset as toolset
from anthropic.lib.tools._tool_dispatch import run_runnable_tool, tool_error_content, tool_result_content

WORKDIR = Path("/workspace")
ROOTS = [Path("/workspace"), Path("/mnt/session"), Path("/tmp")]
READ_ONLY = [Path("/mnt/session/uploads")]


def blocks(content):
    """Tool output as content blocks, the form `agent.tool_result` carries."""
    if content is None:
        return []
    if isinstance(content, str):
        return [{"type": "text", "text": content}]
    out = []
    for block in content:
        out.append(block if isinstance(block, dict) else block.model_dump(exclude_none=True))
    return out


async def serve(reader, write, workdir=WORKDIR, roots=ROOTS, read_only=READ_ONLY):
    ctx = toolset.AgentToolContext(workdir=workdir, allowed_roots=list(roots), read_only_roots=list(read_only))
    await ctx.__aenter__()
    try:
        tools = {t.name: t for t in toolset.beta_agent_toolset_20260401(ctx)}
        async for line in reader:
            request = json.loads(line)
            tool = tools.get(request["name"])
            try:
                if tool is None:
                    raise ValueError(f"no tool named {request['name']}")
                content, is_error = tool_result_content(await run_runnable_tool(tool, request["input"])), False
            except Exception as e:  # noqa: BLE001 - every failure goes back to the agent as its result
                content, is_error = tool_error_content(e), True
            write(json.dumps({"id": request["id"], "content": blocks(content), "is_error": is_error}))
    finally:
        await ctx.__aexit__(None, None, None)


async def _stdin_lines():
    loop = asyncio.get_running_loop()
    reader = asyncio.StreamReader(limit=64 * 1024 * 1024)
    await loop.connect_read_pipe(lambda: asyncio.StreamReaderProtocol(reader), sys.stdin)
    while line := await reader.readline():
        yield line


def _write(text):
    sys.stdout.write(text + "\n")
    sys.stdout.flush()


if __name__ == "__main__":
    asyncio.run(serve(_stdin_lines(), _write))
