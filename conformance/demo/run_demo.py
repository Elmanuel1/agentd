#!/usr/bin/env python3
"""Replay the tender-extraction demo on agentd through the official Anthropic SDK.

The demo (~/tosspaper-eval-data) ran these agents on Claude Managed Agents and in
local simulations. This script runs the same agents, skills, documents, plan and
rubrics against agentd, with only the SDK's base URL changed:

1. Upload the skills and create the agents (the orchestrator coordinates the
   five workers as session threads).
2. Upload the documents, helpers, schemas and rules as session file resources.
3. Start a session, send each upload of the plan as a `user.define_outcome`,
   and record the event stream.
4. Download the session's outputs.
5. Grade each upload's `result.json` with the demo's grader prompt and rubric,
   in a grader session of its own.

usage: run_demo.py --out DIR [--model ID] [--data ~/tosspaper-eval-data]
       [--max-iterations N] [--uploads 0,1,2] [--budget-usd 1.5]
Environment: AGENTD_BASE_URL, AGENTD_API_KEY; OPENROUTER_API_KEY (optional)
reads that key's spend before and after, as the earlier runs measured cost,
so use a key nothing else spends on during the run.
"""
import argparse
import ast
import io
import json
import mimetypes
import os
import re
import threading
import time
import urllib.request
from pathlib import Path

import anthropic
import yaml

BETA = ["managed-agents-2026-04-01"]
GRADE_PATH = "/mnt/session/outputs/grade.json"


def log(out: Path, event: str, **data) -> None:
    line = json.dumps({"t": round(time.time(), 1), "event": event, **data})
    print(line[:400], flush=True)
    with (out / "demo-log.jsonl").open("a") as f:
        f.write(line + "\n")


def frontmatter(path: Path) -> tuple[dict, str]:
    text = path.read_text()
    m = re.match(r"\A---\n(.*?)\n---\n(.*)\Z", text, flags=re.S)
    return yaml.safe_load(m.group(1)), m.group(2)


def upload_skills(client, ma: Path) -> dict[str, str]:
    """Each skill directory as a custom skill; old skill id -> new skill id."""
    ids = {}
    for line in (ma / "skill-ids.tsv").read_text().splitlines():
        name, old = line.split("\t")
        root = ma / "skills" / name
        files = [(f"{name}/{p.relative_to(root)}", io.BytesIO(p.read_bytes()), mimetypes.guess_type(p.name)[0] or "text/plain")
                 for p in sorted(root.rglob("*")) if p.is_file()]
        ids[old] = client.skills.create(files=files, display_name=name).id
    return ids


def create_agent(client, path: Path, model: str, skills: dict[str, str], roster: list | None = None):
    meta, body = frontmatter(path)
    params = dict(
        name=meta["name"], model=model, description=meta.get("description"), system=body,
        tools=meta.get("tools", []),
        skills=[{"type": "custom", "skill_id": skills[s["skill_id"]], "version": "latest"} for s in meta.get("skills", [])],
    )
    if roster:
        params["multiagent"] = {"type": "coordinator", "agents": [{"type": "agent", "id": a.id, "version": a.version} for a in roster]}
    return client.beta.agents.create(**params)


def source(data: Path, mount: str) -> Path:
    """Where a mounted file lives locally, as the local simulation mounts it."""
    ma, rel = data / "managed-agents", mount.removeprefix("/extraction/")
    kind, rest = rel.split("/", 1)
    return {"docs": data / "inputs" / "2025-45" / "parsed", "bin": ma / "bin", "schemas": ma / "schemas",
            "rules": ma / "original" / "rules", "data": ma / "original" / "data"}[kind] / rest


def upload(client, path: Path, name: str | None = None) -> str:
    return client.files.upload(file=(name or path.name, io.BytesIO(path.read_bytes()), "text/plain")).id


def key_usage() -> float | None:
    key = os.environ.get("OPENROUTER_API_KEY")
    if not key:
        return None
    req = urllib.request.Request("https://openrouter.ai/api/v1/key", headers={"Authorization": f"Bearer {key}"})
    with urllib.request.urlopen(req, timeout=30) as r:
        return json.load(r)["data"].get("usage")


def idles(client, session_id: str) -> int:
    return len(list(client.beta.sessions.events.list(session_id, limit=1000, types=["session.status_idle"])))


def record(client, session_id: str, out: Path) -> threading.Thread:
    """Stream the session's events into events.jsonl as they happen."""
    def run() -> None:
        with (out / "events.jsonl").open("a") as f, client.beta.sessions.events.stream(session_id) as stream:
            for event in stream:
                f.write(event.model_dump_json() + "\n")
                f.flush()
    t = threading.Thread(target=run, daemon=True)
    t.start()
    return t


class Budget:
    """Stops the run once the key has spent more than `usd` since `start`, as
    the earlier runs' driver did, by interrupting the session."""

    def __init__(self, usd: float, start: float | None):
        self.usd, self.start, self.checked, self.spent = usd, start, 0.0, 0.0

    def over(self) -> bool:
        if self.start is None or time.time() - self.checked < 30:
            return False
        self.checked = time.time()
        self.spent = (key_usage() or self.start) - self.start
        return self.spent > self.usd


BUDGET = Budget(float("inf"), None)


def run_until_settled(client, session_id: str, outcome_id: str | None, idle_before: int, timeout: float):
    """Wait until the session is idle again, with the outcome (if any) decided."""
    deadline = time.time() + timeout
    while time.time() < deadline:
        time.sleep(2)
        if BUDGET.over():
            client.beta.sessions.events.send(session_id, events=[{"type": "user.interrupt"}])
            raise SystemExit(f"budget ${BUDGET.usd} reached: ${BUDGET.spent:.2f} spent; session interrupted")
        s = client.beta.sessions.retrieve(session_id)
        if s.status != "idle" or idles(client, session_id) <= idle_before:
            continue
        last = list(client.beta.sessions.events.list(session_id, limit=1000, types=["session.status_idle"]))[-1]
        done = [e for e in s.outcome_evaluations if e.outcome_id == outcome_id and e.result != "needs_revision"]
        if last.stop_reason.type != "end_turn" or not outcome_id or done:
            return s, last.stop_reason.type
    raise TimeoutError(f"session {session_id} did not settle in {timeout}s")


def grader_prompt(data: Path) -> tuple[str, dict]:
    """The demo's own grader prompt and verdict schema, read from its driver."""
    tree = ast.parse((data / "harness-sim" / "driver.py").read_text())
    found = {t.id: n.value for n in tree.body if isinstance(n, ast.Assign) for t in n.targets if isinstance(t, ast.Name)}
    return ast.literal_eval(found["GRADER_PROMPT"]), ast.literal_eval(found["GRADER_SCHEMA"])


def grade(client, args, env_id, resources, upload_plan, result: bytes, model: str, out: Path) -> dict:
    """One grading of `result` against the upload's rubric, as the demo grades."""
    prompt, schema = grader_prompt(args.data)
    rubric = Path(upload_plan["rubric"].replace("/sim/rubrics", str(args.data / "managed-agents")))
    agent = client.beta.agents.create(name="rubric-grader", model=model, system=prompt,
                                      tools=[{"type": "agent_toolset_20260401", "configs": [
                                          {"name": "web_fetch", "enabled": False}, {"name": "web_search", "enabled": False}]}])
    files = resources + [
        {"type": "file", "file_id": upload(client, rubric), "mount_path": f"/rubrics/{rubric.name}"},
        {"type": "file", "file_id": client.files.upload(file=("result.json", io.BytesIO(result), "application/json")).id,
         "mount_path": "/graded/result.json"},
    ]
    session = client.beta.sessions.create(agent=agent.id, environment_id=env_id, resources=files, title="demo rubric grade")
    ask = (f"{upload_plan['description']}\n\nThe rubric is `/mnt/session/uploads/rubrics/{rubric.name}`. Grade "
           f"`/mnt/session/uploads/graded/result.json` against it, one criterion per rubric heading, as your instructions say."
           f"\n\nWrite your verdict to `{GRADE_PATH}` as JSON matching this schema, then reply with its `result`:\n{json.dumps(schema)}")
    client.beta.sessions.events.send(session.id, events=[{"type": "user.message", "content": [{"type": "text", "text": ask}]}])
    run_until_settled(client, session.id, None, 0, args.timeout)
    for f in client.beta.files.list(scope_id=session.id, betas=BETA):
        if f.filename == "grade.json":
            return json.loads(client.beta.files.download(f.id).read())
    return {"result": "needs_revision", "criteria": []}


def which_upload(result: dict, plan: dict) -> int | None:
    """Several outputs are named result.json; the content says whose it is."""
    text = json.dumps(result)
    for u in plan["uploads"][1:]:
        if any(d["doc_id"] in text for d in u["docs"]):
            return u["upload"]
    later = ("changes" in result) or ("questions_answers" in result)
    return None if later else 0


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", type=Path, required=True)
    ap.add_argument("--model", default="openai/gpt-6-luna")
    ap.add_argument("--data", type=Path, default=Path.home() / "tosspaper-eval-data")
    ap.add_argument("--max-iterations", type=int, default=5)
    ap.add_argument("--uploads", default="0,1,2")
    ap.add_argument("--timeout", type=float, default=3 * 3600)
    ap.add_argument("--budget-usd", type=float, default=1.5, help="interrupt the run past this spend on OPENROUTER_API_KEY")
    args = ap.parse_args()
    args.out.mkdir(parents=True, exist_ok=True)
    client = anthropic.Anthropic(base_url=os.environ.get("AGENTD_BASE_URL", "http://127.0.0.1:8080"),
                                 api_key=os.environ["AGENTD_API_KEY"], max_retries=5)
    ma = args.data / "managed-agents"
    plan = json.loads((args.data / "codex-sim" / "plan-2025-45.json").read_text())
    wanted = {int(u) for u in args.uploads.split(",")}
    global BUDGET
    started, spend_before = time.time(), key_usage()
    BUDGET = Budget(args.budget_usd, spend_before)
    log(args.out, "start", model=args.model, base_url=str(client.base_url), key_usage_before=spend_before)

    skills = upload_skills(client, ma)
    orch_meta, _ = frontmatter(ma / "agents" / "extraction-orchestrator.md")
    roster = [create_agent(client, (ma / "agents" / p).resolve(), args.model, skills) for p in orch_meta["multiagent"]["agents"]]
    orchestrator = create_agent(client, ma / "agents" / "extraction-orchestrator.md", args.model, skills, roster)
    env_cfg = yaml.safe_load((ma / "environments" / "extraction-agents.yaml").read_text())
    env = client.beta.environments.create(name=f"{env_cfg['name']}-{int(started)}", config=env_cfg["config"])
    # The mounts of the Managed Agents session this demo ran as.
    mounts = [r["mount_path"] for r in json.loads((ma / "session-2025-45.json").read_text())["resources"]]
    resources = [{"type": "file", "file_id": upload(client, source(args.data, m)), "mount_path": m} for m in mounts]
    session = client.beta.sessions.create(agent=orchestrator.id, environment_id=env.id, resources=resources,
                                          title="Extraction agents: 2025-45 on agentd")
    log(args.out, "session", id=session.id, agents=[a.name for a in roster], files=len(resources))
    record(client, session.id, args.out)

    outcomes = {}
    for u in plan["uploads"]:
        if u["upload"] not in wanted:
            continue
        rubric = Path(u["rubric"].replace("/sim/rubrics", str(ma)))
        before = idles(client, session.id)
        sent = client.beta.sessions.events.send(session.id, events=[{
            "type": "user.define_outcome", "description": u["description"],
            "rubric": {"type": "file", "file_id": upload(client, rubric)}, "max_iterations": args.max_iterations}])
        oid = sent.data[0].outcome_id
        t0 = time.time()
        s, stop = run_until_settled(client, session.id, oid, before, args.timeout)
        ev = next((e for e in s.outcome_evaluations if e.outcome_id == oid), None)
        outcomes[u["upload"]] = {"result": ev.result if ev else None, "rounds": (ev.iteration + 1) if ev else 0,
                                 "seconds": round(time.time() - t0), "stop": stop}
        log(args.out, "outcome", upload=f"u{u['upload']}", **outcomes[u["upload"]])

    outputs = args.out / "outputs"
    outputs.mkdir(exist_ok=True)
    results = {}
    for f in client.beta.files.list(scope_id=session.id, betas=BETA, limit=1000):
        body = client.beta.files.download(f.id).read()
        (outputs / f"{f.id}-{f.filename}").write_bytes(body)
        if f.filename == "result.json":
            try:
                n = which_upload(json.loads(body), plan)
            except json.JSONDecodeError:
                n = None
            if n is not None:
                results[n] = body
                (args.out / f"u{n}-result.json").write_bytes(body)
    session = client.beta.sessions.retrieve(session.id)
    spend_after = key_usage()
    # The full log as the API keeps it, whatever the live stream caught.
    with (args.out / "events-final.jsonl").open("w") as f:
        for event in client.beta.sessions.events.list(session.id, limit=1000):
            f.write(event.model_dump_json() + "\n")
    grades = {}
    for u in plan["uploads"]:
        n = u["upload"]
        if n in results:
            g = grade(client, args, env.id, resources, u, results[n], args.model, args.out)
            (args.out / f"u{n}-grade.json").write_text(json.dumps(g, indent=1))
            crit = g.get("criteria", [])
            grades[n] = {"result": g.get("result"), "passed": sum(1 for c in crit if c.get("passed")), "criteria": len(crit)}
    summary = {
        "model": args.model, "session": session.id, "wall_seconds": round(time.time() - started), "outcomes": outcomes,
        "grades": grades, "usage": session.usage.model_dump(),
        "cost_usd": round(spend_after - spend_before, 4) if spend_before is not None and spend_after is not None else None,
    }
    (args.out / "summary.json").write_text(json.dumps(summary, indent=1))
    log(args.out, "done", **summary)


if __name__ == "__main__":
    main()
