#!/usr/bin/env python3
"""Run the extraction agents on a local harness (SIM_HARNESS: codex, pi or mini), laid out like the
Managed Agents session.

Each agent is a thread with its own home: its system prompt, its skills, its session. A follow-up
continues the same thread, and a thread on a long document starts fresh after two follow-ups. The pipeline is the coordinator's: extraction, the
citation check, result.json, then up to MAX_ROUNDS grader rounds on the rubric; then each later
upload. Cost is the OpenRouter key's usage before and after, plus each thread's tokens.

usage: driver.py <plan.json> [--dry-run]
"""
import contextlib
import json
import os
import re
import shutil
import subprocess
import sys
import threading
import time
import urllib.request
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

UP = Path("/mnt/session/uploads/extraction")
OUT = Path("/mnt/session/outputs")
SIM = Path("/sim")
HOMES = Path("/workspace/homes")
HARNESS = os.environ.get("SIM_HARNESS", "codex")
LOG = OUT / "sim-log.jsonl"
MAX_ROUNDS = 5
REVIEW_ROUNDS = 3
# The fact-checker answers a fixed question per value against its own quote: low effort is enough.
EFFORT = {"citation-verifier": "low"}
LONG_DOC_PAGES = 50
MAX_FOLLOW_UPS = 2
DRY = "--dry-run" in sys.argv

MODEL = os.environ.get("SIM_MODEL", "openai/gpt-6-luna")
BASE_URL = os.environ.get("SIM_BASE_URL", "https://openrouter.ai/api/v1")
CONFIG = f'''model_provider = "openrouter"
model = "{MODEL}"
approval_policy = "never"
sandbox_mode = "danger-full-access"
model_reasoning_summary = "detailed"

[model_providers.openrouter]
name = "OpenRouter"
base_url = "{BASE_URL}"
env_key = "OPENROUTER_API_KEY"
wire_api = "responses"
'''

GRADER_SCHEMA = {
    "type": "object", "additionalProperties": False, "required": ["result", "criteria"],
    "properties": {
        "result": {"type": "string", "enum": ["satisfied", "needs_revision"]},
        "criteria": {"type": "array", "items": {"type": "object", "additionalProperties": False,
            "required": ["criterion", "passed", "findings"],
            "properties": {"criterion": {"type": "string"}, "passed": {"type": "boolean"},
                           "findings": {"type": "array", "items": {"type": "object", "additionalProperties": False,
                               "required": ["path", "finding"],
                               "properties": {"path": {"type": "string"}, "finding": {"type": "string"}}}}}}},
    },
}


def log(event, **data):
    line = json.dumps({"t": round(time.time(), 1), "event": event, **data})
    print(line, flush=True)
    with LOG.open("a") as f:
        f.write(line + "\n")


class Hooks:
    """Points where a host (the AG-UI server) watches the run. The default does nothing."""

    def invocation_start(self, thread, prompt):
        return None

    def model_message(self, thread, message):
        pass

    def tool_results(self, thread, message, outputs):
        pass

    def invocation_end(self, thread, reply, rc):
        pass

    @contextlib.contextmanager
    def step(self, name):
        yield

    def changed(self, **state):
        pass

    def check_cancel(self):
        pass

    def inbox(self, thread):
        """Messages from the team for this agent thread, sent while the run is going; each is given once."""
        return []


HOOKS = Hooks()
SEQ_LOCK = threading.Lock()

# Which rubric criteria each agent's own output answers for. Shared rules (hard failures, review flags,
# citations) apply to every output.
SHARED_CRITERIA = (r"^5\.", r"^Review flags", r"^C\d")
OWNED_CRITERIA = {
    "bid-extractor": (r"^1\.\d", r"^2\.(1|2|3|5|6|7|8|9|10|11)\b"),
    "checklist-extractor": (r"^2\.4\b",),
    "vendor-grouper": (r"^3\.", r"^4\."),
}


def criteria_for(rubric, agent):
    """The rubric's `###` headings an agent's output is graded on."""
    headings = re.findall(r"^### (.+)$", Path(rubric).read_text(), flags=re.M)
    if agent not in OWNED_CRITERIA:
        return [h.strip() for h in headings]
    own = OWNED_CRITERIA[agent] + SHARED_CRITERIA
    return [h.strip() for h in headings if any(re.match(p, h) for p in own)]


CHECKLIST_LISTS = ("conditions", "obligations", "requirements")


def load_json(path):
    try:
        return json.loads(Path(path).read_text())
    except (OSError, json.JSONDecodeError):
        return None


def doc_of(path):
    m = re.search(r"([0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})\.json$", str(path))
    return m.group(1) if m else ""


def assemble(up, outputs, later, pages):
    """The coordinator's Phase 2, done as a copy (extraction-orchestrator.md): fields from the bid extractor of
    the longest document, falling back to another document's when null; conditions, obligations and requirements
    from the checklist extractor; every other list from the bid extractor; vendor groups from the vendor
    grouper; lists joined across documents. A later upload is its change detectors' items, changes and
    questions_answers, concatenated in document order."""
    files = [(p, load_json(p)) for p in outputs]
    files = [(p, d) for p, d in files if isinstance(d, dict)]
    if later:
        out = {"items": [], "changes": [], "questions_answers": []}
        for _, d in files:
            for k in out:
                out[k] += d.get(k) or []
        return out
    by_agent = {}
    for p, d in files:
        agent = Path(p).name[: -len(doc_of(p)) - 6] if doc_of(p) else Path(p).stem
        by_agent.setdefault(agent, []).append((doc_of(p), d))
    for docs in by_agent.values():
        docs.sort(key=lambda x: -pages.get(x[0], 0))
    out = {}
    for doc, d in by_agent.get("bid-extractor", []):
        for k, v in d.items():
            if k == "source_agent":
                continue
            if isinstance(v, dict) and "value" in v:
                if k not in out or out[k].get("value") in (None, "", [], {}):
                    out[k] = dict(v)
                elif v.get("value") not in (None, "", [], {}) and v.get("value") != out[k].get("value"):
                    # Another document gives a different value: kept beside it for the conflict callout, never dropped.
                    out[k].setdefault("competing_values", []).append({"value": v["value"], "citations": v.get("citations") or [],
                                                                      "document_id": doc})
                    out[k]["review_reason"] = out[k].get("review_reason") or "conflicting_values"
            elif isinstance(v, list) and k not in CHECKLIST_LISTS:
                kept = out.setdefault(k, [])
                for e in v:
                    if not (isinstance(e, dict) and any(same_thing_same_values(e, x) for x in kept)):
                        kept.append(e)
    flag_conflicting_amounts(out)
    for k in CHECKLIST_LISTS:
        out[k] = [e for _, d in by_agent.get("checklist-extractor", []) for e in d.get(k) or []]
    out["vendor_groups"] = [g for _, d in by_agent.get("vendor-grouper", []) for g in d.get("vendor_groups") or []]
    return out


NO_DROP = ("Never delete a value, entry or line to clear a note: fix its quote, or keep it and say in review_reason why. "
           "Two entries or lines that state the same thing with the same values may be merged into one that keeps both citations. "
           "The same thing with a different amount, limit, date or quantity is a conflict: keep both, each with its own citation. ")


def holdings(doc):
    """What a file holds that must not shrink in a fix: each set field by name, and the number of entries in each
    list and of schedule lines. Counts, not text, so a corrected description is not mistaken for a removal."""
    held = {}
    if not isinstance(doc, dict):
        return held
    for k, v in doc.items():
        if isinstance(v, dict) and "value" in v and v.get("value") not in (None, "", [], {}):
            held[f"field {k}"] = 1
        elif isinstance(v, list) and k != "vendor_groups":
            held[f"{k} entries"] = len(v)
    lines = sum(len(g.get("items") or []) for g in doc.get("vendor_groups") or [])
    if lines:
        held["schedule lines"] = lines
    return held


def _entry_text(e):
    return " ".join(str(e.get(k) or "") for k in ("type", "topic", "category", "description", "ref")).lower()


def _near(a, b, at=0.8):
    import difflib
    return difflib.SequenceMatcher(None, a, b).ratio() >= at


VALUE_KEYS = ("type", "topic", "category", "amount", "percentage", "currency", "minimum_coverage", "date", "mandatory",
              "quantity", "unit", "applies_at", "unit_price", "value")


def _values(e):
    return {k: e.get(k) for k in VALUE_KEYS if k in e}


def same_thing_same_values(a, b):
    """A true repeat, which may be merged: the same thing (near-identical wording) with every value equal. The same
    thing with a different amount, limit, date or quantity is a conflict, kept twice for the conflict callout."""
    return _values(a) == _values(b) and _near(_entry_text(a), _entry_text(b))


def shrunk(before, after):
    """What a fix removed: fields it emptied, and list entries or schedule lines that are gone without a near
    duplicate left in their place. Merging two entries that state the same thing is allowed; losing one is not."""
    after = after if isinstance(after, dict) else {}
    gone = []
    for k, v in (before or {}).items():
        if isinstance(v, dict) and "value" in v and v.get("value") not in (None, "", [], {}):
            w = after.get(k)
            if not (isinstance(w, dict) and w.get("value") not in (None, "", [], {})):
                gone.append(f"field {k}")
        elif isinstance(v, list) and k != "vendor_groups":
            now = [e for e in after.get(k) or [] if isinstance(e, dict)]
            if len(now) >= len(v):
                continue
            for e in v:
                if not isinstance(e, dict) or e in now:
                    continue
                # Gone unless an entry left stands for it: a true repeat, or the same entry with corrected wording.
                if not any(same_thing_same_values(e, x) or (_values(e) == _values(x) and _near(_entry_text(e), _entry_text(x), 0.6))
                           for x in now):
                    gone.append(f"{k}: {(e.get('description') or e.get('type') or '')[:100]}")
    lines_before = [i for g in (before or {}).get("vendor_groups") or [] for i in g.get("items") or []]
    lines_after = [i for g in after.get("vendor_groups") or [] for i in g.get("items") or []]
    if len(lines_after) < len(lines_before):
        for i in lines_before:
            iid = str(i.get("item_id") or "").strip()
            # A line merged into another is a true repeat: same item number (or wording), same quantity and unit.
            if any((iid and str(j.get("item_id") or "").strip() == iid or not iid and _near(str(i.get("description") or "").lower(),
                    str(j.get("description") or "").lower())) and j.get("quantity") == i.get("quantity") and j.get("unit") == i.get("unit")
                   for j in lines_after):
                continue
            gone.append(f"line {iid or i.get('description')}")
    return gone


def flag_conflicting_amounts(out):
    """conflict.rs flag_conflicting_amounts: a bond or policy of one type required with two different amounts keeps
    both entries, each with its own citation, and both go to review."""
    def differ(a, b):
        return isinstance(a, (int, float)) and isinstance(b, (int, float)) and abs(a - b) > 1e-6 * max(abs(a), 1.0)
    for key, amounts in (("insurance", ("minimum_coverage",)), ("bonds", ("percentage", "amount"))):
        entries = [e for e in out.get(key) or [] if isinstance(e, dict)]
        for i, e in enumerate(entries):
            if any(j != i and f.get("type") == e.get("type") and any(differ(e.get(a), f.get(a)) for a in amounts)
                   for j, f in enumerate(entries)):
                e["review_reason"] = e.get("review_reason") or "conflicting_amounts"


def json_path_node(doc, path):
    node = doc
    for part in re.findall(r"[^.\[\]]+|\[\d+\]", path):
        node = node[int(part[1:-1])] if part.startswith("[") else node[part]
    return node


def flag_unresolved(out, failed):
    """A reviewer note the agent would not act on is kept on the value as its review_reason, for a person
    to decide. The value itself is never changed or blanked."""
    doc = json.loads(Path(out).read_text())
    flagged = 0
    for c in failed:
        for f in c.get("findings", []):
            path = re.split(r"\.(value|citations)\b", f.get("path", ""))[0]
            try:
                node = json_path_node(doc, path)
            except (KeyError, IndexError, TypeError, ValueError):
                continue
            if isinstance(node, dict):
                node["review_reason"] = f"Reviewer note not resolved: {f.get('finding', '')}"[:600]
                flagged += 1
    Path(out).write_text(json.dumps(doc, indent=1))
    return flagged


def key_usage():
    key = os.environ.get("OPENROUTER_API_KEY")
    if DRY or not key or "openrouter.ai" not in BASE_URL:
        return None
    req = urllib.request.Request("https://openrouter.ai/api/v1/key", headers={"Authorization": f"Bearer {key}"})
    with urllib.request.urlopen(req, timeout=30) as r:
        return json.load(r)["data"].get("usage")


BUDGET = float(os.environ.get("SIM_BUDGET_USD", "2"))
START_USAGE = None


def guard():
    """Stop the run once the key has spent more than the budget since it started."""
    if START_USAGE is None:
        return
    spent = key_usage() - START_USAGE
    if spent > BUDGET:
        log("budget_reached", spent_usd=round(spent, 4), budget_usd=BUDGET)
        raise SystemExit(f"budget ${BUDGET} reached: ${spent:.2f} spent")


def strip_frontmatter(text):
    return re.sub(r"\A---\n.*?\n---\n", "", text, flags=re.S)


def skill_names(agent_md):
    ids = dict(l.split("\t")[::-1] for l in (SIM / "skill-ids.tsv").read_text().splitlines() if "\t" in l)
    return [ids[i] for i in re.findall(r"skill_id:\s*(skill_[A-Za-z0-9]+)", agent_md) if i in ids]


class BaseThread:
    """One agent conversation in its own home directory."""
    seq = 0

    def __init__(self, agent, label):
        with SEQ_LOCK:
            BaseThread.seq += 1
            seq = BaseThread.seq
        self.agent, self.label, self.session, self.follow_ups = agent, label, None, 0
        self.home = HOMES / f"{seq:02d}-{agent}-{label}"
        self.cwd = self.home / "work"
        self.cwd.mkdir(parents=True, exist_ok=True)
        md = (SIM / "agents" / f"{agent}.md").read_text() if (SIM / "agents" / f"{agent}.md").exists() else ""
        self.system = strip_frontmatter(md) if md else GRADER_PROMPT
        self.skills = skill_names(md)
        self.tokens = {"input": 0, "cached": 0, "output": 0, "cost": 0.0, "requests": 0}

    def logged(self, started, reply, rc=0, stderr=""):
        log("turn", thread=self.home.name, rc=rc, seconds=round(time.time() - started), session=self.session,
            tokens=self.tokens, reply=reply[:400], stderr=stderr[-600:] if rc else "")
        return reply


class CodexThread(BaseThread):
    """A Codex session in its own CODEX_HOME: AGENTS.md as its system prompt, its skills copied in."""

    def __init__(self, agent, label):
        super().__init__(agent, label)
        (self.home / "config.toml").write_text(CONFIG)
        (self.cwd / "AGENTS.md").write_text(self.system)
        for name in self.skills:
            shutil.copytree(SIM / "skills" / name, self.home / "skills" / name, dirs_exist_ok=True)

    def send(self, prompt, schema=None):
        last = self.cwd / f"last-{int(time.time() * 1000)}.txt"
        opts = ["--json", "--skip-git-repo-check", "--dangerously-bypass-approvals-and-sandbox", "-C", str(self.cwd), "-o", str(last)]
        if schema:
            sf = self.cwd / "schema.json"
            sf.write_text(json.dumps(schema))
            opts += ["--output-schema", str(sf)]
        # `codex exec` takes its options before the `resume` subcommand.
        cmd = ["codex", "exec", *opts] + (["resume", self.session] if self.session else []) + [prompt]
        env = dict(os.environ, CODEX_HOME=str(self.home))
        started = time.time()
        p = subprocess.run(cmd, env=env, capture_output=True, text=True, timeout=3600, stdin=subprocess.DEVNULL)
        guard()
        for line in p.stdout.splitlines():
            try:
                e = json.loads(line)
            except json.JSONDecodeError:
                continue
            if e.get("type") == "thread.started" and e.get("thread_id"):
                self.session = e["thread_id"]
            u = e.get("usage") if isinstance(e.get("usage"), dict) else None
            if e.get("type") == "turn.completed" and u:
                self.tokens["input"] += u.get("input_tokens", 0)
                self.tokens["cached"] += u.get("cached_input_tokens", 0)
                self.tokens["output"] += u.get("output_tokens", 0)
        reply = last.read_text() if last.exists() else ""
        return self.logged(started, reply, p.returncode, p.stderr)


PI_MODELS = {"providers": {"openrouter": {"baseUrl": BASE_URL, "api": "openai-completions", "apiKey": "OPENROUTER_API_KEY",
    "models": [{"id": MODEL, "name": MODEL, "reasoning": True, "input": ["text"], "contextWindow": 1050000, "maxTokens": 128000,
                "cost": {"input": 0.10, "output": 0.50, "cacheRead": 0.01, "cacheWrite": 0.125}}]}}}


class PiThread(BaseThread):
    """A Pi session: the agent's prompt replaces Pi's default system prompt; skills load by --skill
    (names and descriptions in the prompt, SKILL.md read on demand); a follow-up is --continue."""

    def __init__(self, agent, label):
        super().__init__(agent, label)
        self.agent_dir = self.home / "pi"
        self.agent_dir.mkdir(parents=True, exist_ok=True)
        (self.agent_dir / "models.json").write_text(json.dumps(PI_MODELS))
        (self.agent_dir / "settings.json").write_text(json.dumps({"quietStartup": True}))
        (self.home / "system.md").write_text(self.system)
        self.seen = 0

    def send(self, prompt, schema=None):
        # Text mode: --mode json re-prints the whole message on every streamed delta, which ran Pi out
        # of heap in a 1 GB container. The reply is stdout; usage comes from the session file.
        cmd = ["pi", "-p", "--provider", "openrouter", "--model", MODEL, "--thinking", "medium",
               "--session-dir", str(self.home / "sessions"), "--system-prompt", self.system,
               "--no-context-files", "--no-extensions", "--no-skills", "--no-prompt-templates", "--no-themes",
               "--tools", "read,bash,edit,write"]
        for name in self.skills:
            cmd += ["--skill", str(SIM / "skills" / name)]
        if self.session:
            cmd.append("--continue")
        cmd.append(prompt)
        env = dict(os.environ, PI_CODING_AGENT_DIR=str(self.agent_dir), PI_OFFLINE="1", PI_TELEMETRY="0")
        started = time.time()
        p = subprocess.run(cmd, env=env, cwd=self.cwd, capture_output=True, text=True, timeout=3600, stdin=subprocess.DEVNULL)
        guard()
        files = sorted((self.home / "sessions").rglob("*.jsonl"), key=lambda f: f.stat().st_mtime)
        if files:
            lines = files[-1].read_text().splitlines()
            for line in lines[self.seen:]:
                try:
                    e = json.loads(line)
                except json.JSONDecodeError:
                    continue
                if e.get("type") == "session":
                    self.session = e.get("id")
                m = e.get("message") or {}
                if m.get("role") == "assistant":
                    u = m.get("usage") or {}
                    self.tokens["requests"] += 1
                    self.tokens["input"] += u.get("input", 0) + u.get("cacheRead", 0)
                    self.tokens["cached"] += u.get("cacheRead", 0)
                    self.tokens["output"] += u.get("output", 0)
                    self.tokens["cost"] += (u.get("cost") or {}).get("total", 0)
            self.seen = len(lines)
        return self.logged(started, p.stdout.strip(), p.returncode, p.stderr)


MINI_RUNTIME = """

# Harness

You work through one tool, `bash`. Each call runs in a fresh subshell in your working directory; a `cd` or
variable does not carry over. Write files with a heredoc (`cat <<'EOF' > path`). Finish only once every file your
task asks for is written, with one last bash call and nothing else in it, which prints the marker line and then your
reply:
```
cat <<'EOF'
COMPLETE_TASK_AND_SUBMIT_FINAL_OUTPUT
<your reply>
EOF
```
"""


def skill_index(names):
    """Skills as Managed Agents and Pi give them: each name and description, the SKILL.md read on demand."""
    if not names:
        return ""
    lines = ["", "# Skills", "", "Before doing work a skill covers, read its SKILL.md with `cat`; its scripts and references sit beside it.", ""]
    for name in names:
        text = (SIM / "skills" / name / "SKILL.md").read_text()
        m = re.search(r"^description:\s*(.+)$", text, flags=re.M)
        desc = m.group(1).strip().strip('"') if m else ""
        lines.append(f"- `{name}` ({SIM / 'skills' / name / 'SKILL.md'}): {desc}")
    return "\n".join(lines) + "\n"


# Compaction, as Codex does it: once a thread's prompt passes 90% of the model's window, the agent writes its own
# handoff memory and the conversation is rebuilt from the system prompt, the people's own messages word for word
# (newest first, up to 20k tokens) and that memory. The full trajectory stays on disk.
COMPACT_SHARE = 0.90
KEEP_USER_TOKENS = 20_000
_WINDOWS = {}

HANDOFF_PREFIX = ("Another language model started this task and wrote the working memory below before its context was "
                  "cleared. You also have the files and tools it used. Build on its work and do not redo what it lists "
                  "as done; re-read a file only when you need its exact contents.\n\n")

MEMORY_PROMPT = """Your context is nearly full. Before your next step, write your working memory to `{path}` with one bash \
call (a heredoc), then carry on with your task. Only the system prompt, the messages people sent you (word for word) \
and this file will remain; your own steps, tool calls and their output will be cleared. It is a handoff: write it so \
another agent could finish the work from it. Keep what you still need, drop what you do not. Use these sections:

## Task
What you were asked, by whom, and what a finished answer looks like.
## Done
What is finished, with the files you wrote or read (full paths).
## Key facts
Values you still need, each with its source (file, page). Nothing you can simply re-read later.
## Decisions
What was decided and why, including any instruction from the team you are following.
## Open
Questions still unanswered, anything you were unsure of, and your exact next step.
"""


def context_window(model):
    """The model's context window in tokens, from SIM_CONTEXT_WINDOW or OpenRouter's model list; None if unknown."""
    if model in _WINDOWS:
        return _WINDOWS[model]
    window = int(os.environ["SIM_CONTEXT_WINDOW"]) if os.environ.get("SIM_CONTEXT_WINDOW") else None
    if window is None:
        try:
            with urllib.request.urlopen("https://openrouter.ai/api/v1/models", timeout=20) as r:
                models = json.loads(r.read())["data"]
            window = next((m.get("context_length") for m in models if m.get("id") == model), None)
        except Exception as e:
            log("context_window_unknown", model=model, error=str(e)[-300:])
    if window is None:
        log("context_window_unknown", model=model, error="not in the model list; compaction is off for it")
    _WINDOWS[model] = window
    return window


def compact_at(window):
    return int(window * COMPACT_SHARE)


def kept_user_messages(messages, budget=KEEP_USER_TOKENS):
    """The people's messages to keep word for word: newest first until the budget is spent, the one that does not
    fit cut to what is left, returned oldest first."""
    kept, left = [], budget
    for text in reversed(messages):
        if left <= 0:
            break
        tokens = len(text) // 4 + 1
        kept.append(text if tokens <= left else text[: left * 4])
        left -= tokens
    return list(reversed(kept))


class MiniThread(BaseThread):
    """A mini-swe-agent conversation: one bash tool on OpenRouter chat completions, kept in memory
    between follow-ups."""

    def __init__(self, agent, label):
        super().__init__(agent, label)
        from minisweagent.agents.default import DefaultAgent
        from minisweagent.environments.local import LocalEnvironment
        self.agent_obj = DefaultAgent(
            MiniModel(model_name=MODEL, model_kwargs={"reasoning": {"effort": EFFORT.get(agent, "medium"), "summary": "auto"}},
                      observation_template=MINI_OBSERVATION, format_error_template=MINI_FORMAT_ERROR),
            LocalEnvironment(cwd=str(self.cwd), timeout=600, env={"PAGER": "cat"}),
            system_template="", instance_template="", step_limit=0, cost_limit=0,
            output_path=self.home / "trajectory.json")
        self.prompt = self.system + skill_index(self.skills) + MINI_RUNTIME
        self.invocation = None
        # Only threads that run open-ended conversations (the follow-up coordinator) compact themselves.
        self.compact = False
        self.memory_asked_at = None
        # Every message a person gave this thread (tasks and team messages), kept apart so compaction never loses them.
        self.user_messages = []
        query, execute = self.agent_obj.query, self.agent_obj.execute_actions

        def watched_query():
            HOOKS.check_cancel()
            # A team message sent mid-run joins the conversation before the agent's next step.
            for note in HOOKS.inbox(self):
                self.agent_obj.add_messages({"role": "user", "content": note})
                self.user_messages.append(note)
            message = query()
            HOOKS.model_message(self, message)
            return message

        def watched_execute(message):
            from minisweagent.exceptions import Submitted
            try:
                outputs = execute(message)
            except Submitted:
                HOOKS.tool_results(self, message, None)
                raise
            HOOKS.tool_results(self, message, outputs)
            return outputs

        self.agent_obj.query, self.agent_obj.execute_actions = watched_query, watched_execute

    def send(self, prompt, schema=None):
        from minisweagent.exceptions import FormatError, InterruptAgentFlow
        a = self.agent_obj
        started = time.time()
        before = len(a.messages)
        self.invocation = HOOKS.invocation_start(self, prompt)
        if not a.messages:
            a.add_messages({"role": "system", "content": self.prompt})
            self.session = f"mini-{self.home.name}"
        a.add_messages({"role": "user", "content": prompt})
        self.user_messages.append(prompt)
        steps, rc = 0, 0
        while True:
            steps += 1
            if steps > MINI_STEP_LIMIT:
                rc = 3
                break
            try:
                a.step()
                a.n_consecutive_format_errors = 0
            except FormatError as e:
                said = (((e.messages[0].get("extra") or {}).get("response") or {}).get("choices") or [{}])[0].get("message") or {}
                if isinstance(said.get("content"), str) and said["content"].strip() and not said.get("tool_calls"):
                    # A plain answer with no tool call is the agent's reply: take it and finish.
                    a.add_messages({"role": "assistant", "content": said["content"], "extra": {"response": e.messages[0]["extra"]["response"]}})
                    a.add_messages({"role": "exit", "content": said["content"], "extra": {"exit_status": "Submitted", "submission": said["content"]}})
                    break
                a.n_consecutive_format_errors += 1
                a.add_messages(*e.messages)
                if a.n_consecutive_format_errors >= 3:
                    rc = 2
                    break
            except InterruptAgentFlow as e:
                a.add_messages(*e.messages)
            except Exception as e:
                log("mini_error", thread=self.home.name, error=str(e)[-500:])
                rc = 1
                break
            finally:
                a.save(a.config.output_path)
            if a.messages[-1].get("role") == "exit":
                break
            if self.compact:
                self.maybe_compact()
        reply = ""
        for m in a.messages[before:]:
            if m.get("role") == "assistant" and isinstance(m.get("content"), str) and m["content"].strip():
                reply = m["content"]
            if m.get("role") == "exit" and str((m.get("extra") or {}).get("submission") or "").strip():
                reply = m["extra"]["submission"].strip()
            u = (m.get("extra") or {}).get("response", {}).get("usage") if m.get("role") == "assistant" else None
            if u:
                self.tokens["requests"] += 1
                self.tokens["input"] += u.get("prompt_tokens", 0)
                self.tokens["cached"] += (u.get("prompt_tokens_details") or {}).get("cached_tokens", 0)
                self.tokens["output"] += u.get("completion_tokens", 0)
                self.tokens["cost"] += u.get("cost", 0) or 0
        HOOKS.invocation_end(self, reply, rc)
        guard()
        return self.logged(started, reply, rc)

    def prompt_tokens(self):
        for m in reversed(self.agent_obj.messages):
            if m.get("role") == "assistant":
                return ((m.get("extra") or {}).get("response") or {}).get("usage", {}).get("prompt_tokens", 0)
        return 0

    def maybe_compact(self):
        """Asks the agent for its handoff memory once the prompt is past the threshold; once it is written, rebuilds
        the conversation from the system prompt, the people's messages and the memory, in that order."""
        a = self.agent_obj
        memory = self.home / "memory.md"
        if self.memory_asked_at is not None:
            if memory.exists() and memory.stat().st_mtime >= self.memory_asked_at and memory.read_text().strip():
                before = len(a.messages)
                kept = kept_user_messages(self.user_messages)
                a.messages = [a.messages[0], *({"role": "user", "content": m} for m in kept),
                              {"role": "user", "content": HANDOFF_PREFIX + memory.read_text()}]
                self.memory_asked_at = None
                log("compacted", thread=self.home.name, messages_before=before, kept_user_messages=len(kept),
                    memory_chars=len(memory.read_text()))
            return
        window = context_window(MODEL)
        if window is None:
            return
        used = self.prompt_tokens()
        if used >= compact_at(window):
            self.memory_asked_at = time.time()
            a.add_messages({"role": "user", "content": MEMORY_PROMPT.format(path=memory)})
            log("compact_asked", thread=self.home.name, prompt_tokens=used, window=window)


MINI_STEP_LIMIT = 250
MINI_OBSERVATION = ("<returncode>{{output.returncode}}</returncode>\n"
                    "{% if output.exception_info %}<exception>{{output.exception_info}}</exception>\n{% endif %}"
                    "{% if output.output | length < 40000 %}<output>\n{{output.output}}</output>"
                    "{% else %}<output_head>\n{{output.output[:20000]}}</output_head>\n<output_tail>\n{{output.output[-20000:]}}</output_tail>\n"
                    "<warning>Output too long: {{output.output | length - 40000}} characters left out. Print less at a time.</warning>{% endif %}")
MINI_FORMAT_ERROR = ("{{error}}\nEvery response must make at least one `bash` tool call. To finish, make a last response with your reply "
                     "as its text and one bash call, `echo COMPLETE_TASK_AND_SUBMIT_FINAL_OUTPUT`.")


def mini_model_class():
    from minisweagent.models.openrouter_model import OpenRouterModel

    class Model(OpenRouterModel):
        """OpenRouter chat completions with a longer request timeout, sending the conversation as the
        API takes it: no `exit` entries, and a result for every tool call (the submit call has none)."""

        def _query(self, messages, **kwargs):
            import requests
            payload = {"model": self.config.model_name, "messages": messages, "tools": [MINI_BASH_TOOL],
                       "usage": {"include": True}, **(self.config.model_kwargs | kwargs)}
            r = requests.post(f"{BASE_URL}/chat/completions", headers={"Authorization": f"Bearer {self._api_key}", "Content-Type": "application/json"},
                              data=json.dumps(payload), timeout=900)
            if r.status_code >= 400:
                raise RuntimeError(f"HTTP {r.status_code}: {r.text[:500]}")
            return r.json()

        def _prepare_messages_for_api(self, messages):
            out, answered = [], {m.get("tool_call_id") for m in messages if m.get("role") == "tool"}
            for m in messages:
                if m.get("role") == "exit":
                    continue
                out.append({k: v for k, v in m.items() if k != "extra"})
                for tc in m.get("tool_calls") or [] if m.get("role") == "assistant" else []:
                    if tc.get("id") not in answered:
                        out.append({"role": "tool", "tool_call_id": tc["id"], "content": "Done."})
            return out

    return Model


MINI_BASH_TOOL = {"type": "function", "function": {"name": "bash", "description": "Run a bash command in a fresh subshell and return its output.",
                  "parameters": {"type": "object", "properties": {"command": {"type": "string"}}, "required": ["command"]}}}


def MiniModel(**kwargs):
    return mini_model_class()(**kwargs)


THREADS = {"codex": CodexThread, "pi": PiThread, "mini": MiniThread}


def Thread(agent, label):
    return THREADS[HARNESS](agent, label)


class Coordinator:
    def __init__(self, plan):
        self.plan, self.threads, self.pages, self.grades, self.reviews = plan, {}, {}, 0, {}

    def own(self, key, agent, label):
        if key not in self.threads:
            self.threads[key] = Thread(agent, label)
        return self.threads[key]

    def thread(self, agent, doc, fresh=False):
        key = (agent, doc)
        t = self.threads.get(key)
        long_doc = self.pages.get(doc, 0) > LONG_DOC_PAGES
        if t is None or fresh or (long_doc and t.follow_ups >= MAX_FOLLOW_UPS):
            t = Thread(agent, doc[:8])
            self.threads[key] = t
        return t

    def follow_up(self, agent, doc, prompt, out=None):
        t = self.thread(agent, doc)
        if t.session:
            t.follow_ups += 1
        before = load_json(out) if out else None
        reply = t.send(prompt)
        if before is not None:
            gone = shrunk(before, load_json(out))
            if gone:
                keep = Path(out).with_suffix(".before-fix.json")
                keep.write_text(json.dumps(before, indent=1))
                log("dropped_in_fix", agent=agent, file=str(out), dropped=gone[:20], count=len(gone))
                t.send(f"Your fix removed {len(gone)} item(s) from `{out}` without a duplicate left in their place: " + "; ".join(gone[:20]) + f". The file before the fix is "
                       f"`{keep}`. Put each one back as it was there, then fix its quote or keep it with a review_reason saying why. "
                       + NO_DROP + "Run cite.py check again.")
        return reply

    def extract(self, up, doc, review=None):
        name, pages = doc["file_name"], doc["pages"]
        self.pages[doc["doc_id"]] = pages
        def task(agent):
            out = OUT / up / f"{agent}-{doc['doc_id']}.json"
            msg = (f'Document `{doc["doc_id"]}` ("{name}", {pages} pages). Extract per your instructions. '
                   f"Write the JSON to `{out}`.")
            self.thread(agent, doc["doc_id"]).send(msg)
            # Each output's citation check starts as soon as its agent is done; the three run side by side.
            if out.exists():
                self.verify(up, agent, doc["doc_id"], out)
                if review:
                    self.reviews[(up, agent)] = self.review_output(up, agent, doc["doc_id"], out, *review)
            return agent, out
        with HOOKS.step(f"{up}: extraction, {name}"), ThreadPoolExecutor(max_workers=3) as pool:
            return dict(pool.map(task, ["bid-extractor", "checklist-extractor", "vendor-grouper"]))

    def verify(self, up, agent, doc_id, out):
        with HOOKS.step(f"{up}: citation check, {agent}"):
            self.verify_steps(up, agent, doc_id, out)

    def verify_steps(self, up, agent, doc_id, out):
        v = OUT / up / "verify"
        v.mkdir(parents=True, exist_ok=True)
        for npass in (1, 2):
            work, verdicts = v / f"{agent}-{doc_id}.work{npass}.json", v / f"{agent}-{doc_id}.verdicts{npass}.json"
            prep = subprocess.run(["python3", str(UP / "bin/verify.py"), "prepare", str(out), "--doc", doc_id, "--out", str(work)],
                                  capture_output=True, text=True)
            if prep.returncode or not work.exists():
                log("verify_prepare_failed", agent=agent, stderr=prep.stderr[-400:])
                return
            n = json.loads(prep.stdout.splitlines()[0])["to_verify"]
            if n:
                self.follow_up("citation-verifier", f"{agent}-{doc_id}",
                               f"Verify citations. Work file: `{work}`. Write the verdicts to `{verdicts}`.")
            else:
                verdicts.write_text('{"verdicts": []}')
            if not verdicts.exists():
                log("verify_no_verdicts", agent=agent, npass=npass)
                return
            app = subprocess.run(["python3", str(UP / "bin/verify.py"), "apply", str(out), str(verdicts), "--work", str(work), "--write"],
                                 capture_output=True, text=True)
            lines = app.stdout.splitlines()
            failures = lines[1:] if lines else []
            log("verify", agent=agent, npass=npass, summary=lines[0] if lines else app.stderr[-300:])
            if npass == 2 or not failures:
                return
            self.follow_up(agent, doc_id, f"Citation check failures for `{out}`. Each is recorded as review_reason. "
                           f"Fix each in place, or keep a value you can show is right and say why. " + NO_DROP
                           + "Then run cite.py check again:\n" + "\n".join(failures), out=out)

    def write_result(self, up, outputs, later=False):
        """Assemble the upload's result.json from the agents' files. A copy, never a model call: a model asked to
        copy works from what it remembers and can bring back values an agent has since corrected."""
        with HOOKS.step(f"{up}: result"):
            result = assemble(up, outputs, later, self.pages)
            (OUT / up / "result.json").write_text(json.dumps(result, indent=1))

    def grade(self, up, rubric, description, rnd, recheck=None):
        """A fresh grader each round, as an outcome grader is: a grader that keeps its context reuses its
        last verdict instead of reading the corrected file. After round 1 it grades only the criteria that
        failed; the others keep their pass."""
        g = Thread("grader", f"{up}-r{rnd}")
        self.threads[("grader", up, rnd)] = g
        scope = ("one criterion per rubric heading" if not recheck else
                 "only these criteria, each against the file as it is now (other criteria passed earlier): "
                 + "; ".join(f"`{c}`" for c in recheck))
        msg = f"{description}\n\nThe rubric is `{rubric}`. Grade `{OUT / up / 'result.json'}` against it, {scope}, as your instructions say."
        if HARNESS == "codex":
            reply = g.send(msg, schema=GRADER_SCHEMA)
        else:
            path = OUT / up / "grades" / f"grade-{rnd}.json"
            path.parent.mkdir(parents=True, exist_ok=True)
            ask = (f"{msg}\n\nWrite your verdict to `{path}` as JSON matching this schema, then reply with its `result`:\n"
                   f"{json.dumps(GRADER_SCHEMA)}")
            g.send(ask)
            if not path.exists():
                g.send(f"You have not written `{path}`. Grade as asked and write the verdict there before you finish.")
            reply = path.read_text() if path.exists() else ""
        try:
            return json.loads(reply)
        except json.JSONDecodeError:
            log("grader_unparsed", reply=reply[:500])
            return {"result": "needs_revision", "criteria": []}

    def review_output(self, up, agent, doc_id, out, rubric, description):
        """Review one agent's output on the criteria it owns, as soon as its fact-check is done. Each round is a
        fresh reviewer; findings go straight back to the agent; a round with no verdict ends the loop."""
        criteria = criteria_for(rubric, agent)
        recheck, no_verdict, answer, last_failed = None, 0, None, None
        for rnd in range(1, REVIEW_ROUNDS + 1):
            scope = recheck or criteria
            g = Thread("grader", f"{agent[:12]}-r{rnd}")
            self.threads[("grader", up, agent, rnd)] = g
            path = OUT / up / "grades" / f"{agent}-{doc_id}-{rnd}.json"
            path.parent.mkdir(parents=True, exist_ok=True)
            ask = (f"{description}\n\nThis is one part of the result: `{out}`, written by the {agent} agent. Grade that file against "
                   f"the rubric `{rubric}`, only these criteria, each against the file as it is now: "
                   + "; ".join(f"`{c}`" for c in scope)
                   + f". Paths in your findings are JSON paths inside that file."
                   + (f"\n\nThe agent's answer to the last review, word for word:\n{answer}\nWhere it kept a value and gives page text and "
                      "a rule that support it, judge that evidence: pass the criterion when it holds, and do not repeat a note it has "
                      "answered unless you can show from the page why the answer is wrong." if answer else "")
                   + f"\n\nWrite your verdict to `{path}` as JSON matching "
                   f"this schema, then reply with its `result`:\n{json.dumps(GRADER_SCHEMA)}")
            with HOOKS.step(f"{up}: review, {agent}, round {rnd}"):
                if HARNESS == "codex":
                    reply = g.send(ask, schema=GRADER_SCHEMA)
                else:
                    g.send(ask)
                    if not path.exists():
                        g.send(f"You have not written `{path}`. Grade as asked and write the verdict there before you finish.")
                    reply = path.read_text() if path.exists() else ""
            try:
                graded = json.loads(reply).get("criteria", [])
            except json.JSONDecodeError:
                graded = []
            failed = [c for c in graded if not c.get("passed")]
            result = "satisfied" if graded and not failed else "needs_revision"
            log("review", upload=up, agent=agent, round=rnd, result=result, graded=len(graded), failed=len(failed))
            HOOKS.changed(grade={"upload": up, "agent": agent, "round": rnd, "result": result, "graded": len(graded),
                                 "failed": [c["criterion"] for c in failed]})
            if result == "satisfied":
                return "satisfied", rnd
            if not graded:
                no_verdict += 1
                if no_verdict >= 2:
                    return "not_reviewed", rnd
                continue
            this_failed = sorted((c["criterion"], f.get("path", "")) for c in failed for f in c.get("findings", []))
            if rnd == REVIEW_ROUNDS or this_failed == last_failed:
                # Out of rounds, or the same notes on the same values as last round: the agent and the reviewer
                # disagree. Stop and leave each note on its value for a person to decide.
                n = flag_unresolved(out, failed)
                log("review_unresolved", upload=up, agent=agent, round=rnd, flagged=n, repeated=this_failed == last_failed)
                return ("unresolved", rnd)
            last_failed = this_failed
            recheck = [c["criterion"] for c in failed]
            findings = [f'{c["criterion"]}: {f["path"]}: {f["finding"]}' for c in failed for f in c.get("findings", [])]
            before = Path(out).read_bytes()
            answer = self.follow_up(agent, doc_id,
                           f"Reviewer findings for `{out}`, word for word. Correct those values in that file in place, reading only the "
                           "pages they name, or keep a value your skill and the page support and say why. Edit no other file. "
                           + NO_DROP + "Then run cite.py check on it again:\n" + "\n".join(f"- {x}" for x in findings), out=out)
            if Path(out).read_bytes() == before:
                # The agent changed nothing: it stands by every value. The next reviewer weighs its answer once.
                log("review_kept", upload=up, agent=agent, round=rnd)
        return "unresolved", REVIEW_ROUNDS

    def route(self, path, later):
        if later:
            return "change-detector"
        if path.startswith("vendor_groups"):
            return "vendor-grouper"
        if path.split("[")[0].split(".")[0] in ("conditions", "obligations", "requirements"):
            return "checklist-extractor"
        return "bid-extractor"

    def outcome(self, up, docs, rubric, description, later=False, outputs=None):
        passed, recheck = {}, None
        for rnd in range(1, MAX_ROUNDS + 1):
            self.write_result(up, outputs, later)
            with HOOKS.step(f"{up}: grading, round {rnd}"):
                verdict = self.grade(up, rubric, description, rnd, recheck)
            graded = verdict.get("criteria", [])
            passed.update({c["criterion"]: c for c in graded if c.get("passed")})
            failed = [c for c in graded if not c.get("passed")]
            if graded and not failed:
                verdict["result"] = "satisfied"
            elif failed:
                verdict["result"] = "needs_revision"
            recheck = [c["criterion"] for c in failed] or recheck
            log("grade", upload=up, round=rnd, result=verdict.get("result"), failed=len(failed),
                criteria=len(verdict.get("criteria", [])), names=[c["criterion"][:60] for c in failed])
            HOOKS.changed(grade={"upload": up, "round": rnd, "result": verdict.get("result"), "graded": len(graded),
                                 "failed": [c["criterion"] for c in failed]})
            if verdict.get("result") == "satisfied":
                return "satisfied", rnd
            if rnd == MAX_ROUNDS:
                return "max_iterations_reached", rnd
            by_agent = {}
            for c in failed:
                for f in c.get("findings", []):
                    by_agent.setdefault(self.route(f.get("path", ""), later), []).append(f'{c["criterion"]}: {f["path"]}: {f["finding"]}')
            doc_id = docs[0]["doc_id"]
            main = max(docs, key=lambda d: d["pages"])["doc_id"]
            for agent, findings in by_agent.items():
                target = main if not later else doc_id
                own = OUT / up / f"{agent}-{target}.json"
                self.follow_up(agent, target,
                               f"Grader findings for `{own}`, word for word. Correct those values in that file in place, reading only the "
                               "pages they name, or keep a value your skill and the page support and say why. Edit no other file. "
                               "Then run cite.py check on it again:\n" + "\n".join(f"- {x}" for x in findings))

    def run(self):
        global START_USAGE
        before = key_usage()
        START_USAGE = before
        log("start", harness=HARNESS, model=MODEL, key_usage_before=before, budget_usd=BUDGET, plan=self.plan["bid"])
        first, laters = None, []
        for u in self.plan["uploads"]:
            up = f"u{u['upload']}"
            (OUT / up).mkdir(parents=True, exist_ok=True)
            outputs = []
            review = (u["rubric"], u["description"]) if u["kind"] == "first" else None
            for doc in u["docs"]:
                outs = self.extract(up, doc, review)
                outputs += [str(p) for p in outs.values()]
            if u["kind"] == "first":
                self.write_result(up, outputs)
                results = [v for (x, _), v in self.reviews.items() if x == up]
                ok = results and all(r == "satisfied" for r, _ in results)
                result = ("satisfied" if ok else "unresolved", max((n for _, n in results), default=0))
            else:
                changes = []
                for doc in u["docs"]:
                    out = OUT / up / f"change-detector-{doc['doc_id']}.json"
                    extracted = "\n".join(f"- `{p}`" for p in outputs if doc["doc_id"] in p)
                    with HOOKS.step(f"{up}: change detection, {doc['file_name']}"):
                      self.thread("change-detector", doc["doc_id"]).send(
                        f'Document `{doc["doc_id"]}` ("{doc["file_name"]}", {doc["pages"]} pages), a later upload. Current tender: '
                        f"`{OUT / first / 'result.json'}`, with the changes already made by "
                        + (", ".join(f"`{OUT / x / 'result.json'}`" for x in laters) or "no earlier upload")
                        + " (ignore their citations and review_reasons). What was extracted from this "
                        f"document:\n{extracted}\nWrite the JSON to `{out}`, then run cite.py check on it with --doc {doc['doc_id']} --write.")
                    if out.exists():
                        self.verify(up, "change-detector", doc["doc_id"], out)
                        self.reviews[(up, doc["doc_id"])] = self.review_output(up, "change-detector", doc["doc_id"], out, u["rubric"], u["description"])
                    changes.append(str(out))
                self.write_result(up, changes, later=True)
                results = [v for (x, _), v in self.reviews.items() if x == up]
                ok = results and all(r == "satisfied" for r, _ in results)
                result = ("satisfied" if ok else "unresolved", max((n for _, n in results), default=0))
            log("outcome", upload=up, result=result[0], rounds=result[1])
            HOOKS.changed(outcome={"upload": up, "result": result[0], "rounds": result[1], "path": str(OUT / up / "result.json")})
            if u["kind"] == "first":
                first = up
            else:
                laters.append(up)
        after = key_usage()
        tokens = {t.home.name: t.tokens for t in self.threads.values()}
        cost = round(after - before, 4) if isinstance(before, (int, float)) and isinstance(after, (int, float)) else None
        log("done", cost_usd=cost, key_usage_after=after, threads=len(self.threads), tokens=tokens)


GRADER_PROMPT = """# Outcome grader

You grade a tender extraction artifact against a rubric, independently: you did not produce it, and you check it against the
source documents, not against what a tender usually says. Read the rubric file named in the message. Treat each `###`
heading as one criterion. For each: reason, check the values against the source text with
`python3 /mnt/session/uploads/extraction/bin/pages.py {document_id} --pages N` (or `--rows` for tables, `--grep` to search),
and decide passed true or false. For each failure give the JSON path of the value in the artifact (`closing_date`,
`insurance[2]`, `vendor_groups[1].items[4]`, `changes[0]`) and the finding: what is wrong and the page that shows it.
`result` is `satisfied` only when every criterion passes. Grade from the files as they are now; do not read earlier grade
files. Do not modify any file except the verdict file the message names.
"""


def main():
    plan = json.loads(Path(sys.argv[1]).read_text())
    LOG.parent.mkdir(parents=True, exist_ok=True)
    if DRY:
        log("dry_run", harness=HARNESS, note="a thread and one follow-up against SIM_BASE_URL (a mock server); no paid model calls")
        doc = plan["uploads"][0]["docs"][0]["doc_id"]
        t = Thread("vendor-grouper", "dryrun")
        t.send(f"Document `{doc}`. Print its outline, then reply ready.")
        t.send("Follow-up: reply ready again.")
        log("dry_done", session=t.session, tokens=t.tokens, skills=t.skills)
        return
    Coordinator(plan).run()


if __name__ == "__main__":
    main()
