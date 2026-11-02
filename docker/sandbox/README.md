# Session sandbox image

Each session runs in one container of this image (`AGENTD_RUNTIME=docker`,
`AGENTD_SANDBOX_IMAGE=agentd-sandbox`). The agent loop runs in agentd; the
container's first process, `agentd-worker`, acknowledges the session's work
item, heartbeats its lease, and long-polls agentd
(`/v1/environments/{id}/work/{work_id}/ops`) for the commands and file
transfers the loop queues. The sandbox opens no port, so the same worker
runs on any runtime; it authenticates with a work token that reaches only its
own work item, and no model key or vault secret enters the container.

The worker reaches agentd at `AGENTD_SANDBOX_API_URL` (default
`http://host.docker.internal:<port>`).

```bash
docker build -f docker/sandbox/Dockerfile -t agentd-sandbox .
```

Layout inside the container:

- `/workspace`: the agent's working directory (writable); skills are
  unpacked under `/workspace/skills/<name>/`.
- `/mnt/session/uploads/`: session file resources, read-only.
- `/mnt/session/outputs/`: files written here come back as session-scoped
  files (`GET /v1/files?scope_id=<session id>`).
