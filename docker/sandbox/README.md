# Session sandbox image

Each session runs in one container of this image (`AGENTD_RUNTIME=docker`,
`AGENTD_SANDBOX_IMAGE=agentd-sandbox`). The session's agent loop, its
harness, runs inside the container; agentd keeps the API, the event log and
the container's lifecycle. The container's first process is the harness
(`python3 -m agentd_harness`, from `harness/`): it runs the work item agentd
claimed for the session, as the official SDK worker's `handle_item` does from
the `ANTHROPIC_*` variables, keeps its lease, and runs the agent, its tools as
`nobody`. The sandbox opens no port; it authenticates with the work item's
sessions token, which reaches only its own session, and no environment key,
model key or vault secret enters it.

```bash
docker build -f docker/sandbox/Dockerfile -t agentd-sandbox .
```

Layout inside the container:

- `/workspace`: the agent's working directory (writable); skills are
  unpacked under `/workspace/skills/<name>/`.
- `/mnt/session/uploads/`: session file resources, read-only.
- `/mnt/session/outputs/`: files written here come back as session-scoped
  files (`GET /v1/files?scope_id=<session id>`). A stopping sandbox saves
  the files that changed since its last turn before it exits.
