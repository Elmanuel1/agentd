"""agentd's session harness: the agent loop of one session, run inside its sandbox.

It takes the session's work item, mounts the session's files and skills, and runs the agent with
harness-sim's mini-swe-agent loop. The agent's tools are the official SDK's `agent_toolset`
(bash, read, write, edit, glob, grep), run in a child process as `nobody`; each call is its own
`agent.tool_use` / `agent.tool_result` on the session's stream. Model calls go to an
OpenAI-compatible API through the egress proxy, with a placeholder key the proxy replaces.
"""
