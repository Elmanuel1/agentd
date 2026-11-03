# Session sandbox image

Each session runs in one container of this image (`AGENTD_RUNTIME=docker`,
`AGENTD_SANDBOX_IMAGE=agentd-sandbox`). The session's agent loop, its
harness, runs inside the container; agentd keeps the API, the event log and
the container's lifecycle. The container's first process is the harness
(`python3 -m agentd_harness`, from `harness/`): it acknowledges the session's
work item, keeps its lease, and runs the agent, its tools as `nobody`. The
sandbox opens no port; it authenticates with a work token that reaches only
its own session, and no model key or vault secret enters it.

```bash
docker build -f docker/sandbox/Dockerfile -t agentd-sandbox .
```

Layout inside the container:

- `/workspace`: the agent's working directory (writable); skills are
  unpacked under `/workspace/skills/<name>/`.
- `/mnt/session/uploads/`: session file resources, read-only.
- `/mnt/session/outputs/`: files written here come back as session-scoped
  files (`GET /v1/files?scope_id=<session id>`).
