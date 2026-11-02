#!/usr/bin/env python3
"""Set agentd demo runs beside the earlier harness-sim runs: each upload's
outcome and rounds, the rubric criteria its last grading passed, the cost and
the wall time.

usage: compare.py AGENTD_RUN_DIR... [--data ~/tosspaper-eval-data]
"""
import argparse
import json
import re
from pathlib import Path

EARLIER = ["mini-2025-45-r1", "mini-2025-45-r2", "pi-2025-45-r1", "pi-2025-45-r2"]


def last_grade(upload_dir: Path) -> str:
    """Criteria passed by the end, as the driver counts them: later rounds
    re-grade only what failed, so a criterion passes once any round passes it."""
    grades = sorted(upload_dir.glob("grade-*.json"), key=lambda p: int(re.findall(r"\d+", p.stem)[-1]))
    if not grades:
        return "-"
    seen, passed = set(), set()
    for g in grades:
        for c in json.loads(g.read_text()).get("criteria", []):
            seen.add(c["criterion"])
            if c.get("passed"):
                passed.add(c["criterion"])
    return f"{len(passed)}/{len(seen)}"


def earlier(run: Path) -> dict:
    events = [json.loads(line) for line in (run / "sim-log.jsonl").read_text().splitlines() if line.strip()]
    start = next(e for e in events if e["event"] == "start")
    done = next((e for e in events if e["event"] == "done"), None)
    outcomes = {e["upload"]: f"{e['result']} ({e['rounds']})" for e in events if e["event"] == "outcome"}
    end = done["t"] if done else events[-1]["t"]
    return {
        "run": run.name, "model": start.get("model"), "cost": done.get("cost_usd") if done else None,
        "wall_min": round((end - start["t"]) / 60, 1), "outcomes": outcomes,
        "rubric": {u: last_grade(run / u) for u in ("u0", "u1", "u2")},
    }


def agentd(run: Path) -> dict:
    s = json.loads((run / "summary.json").read_text())
    return {
        "run": run.name, "model": s["model"], "cost": s.get("cost_usd"), "wall_min": round(s["wall_seconds"] / 60, 1),
        "outcomes": {f"u{u}": f"{o['result']} ({o['rounds']})" for u, o in s["outcomes"].items()},
        "rubric": {f"u{u}": f"{g['passed']}/{g['criteria']}" for u, g in s["grades"].items()},
    }


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("runs", nargs="+", type=Path)
    ap.add_argument("--data", type=Path, default=Path.home() / "tosspaper-eval-data")
    args = ap.parse_args()
    rows = [earlier(args.data / "harness-sim" / "runs" / r) for r in EARLIER if (args.data / "harness-sim" / "runs" / r).exists()]
    rows += [agentd(r) for r in args.runs]
    print("| run | model | u0 outcome | u0 rubric | u1 outcome | u1 rubric | u2 outcome | u2 rubric | cost $ | wall min |")
    print("|---|---|---|---|---|---|---|---|---|---|")
    for r in rows:
        cells = [r["run"], r["model"]]
        for u in ("u0", "u1", "u2"):
            cells += [r["outcomes"].get(u, "-"), r["rubric"].get(u, "-")]
        cells += [str(r["cost"]), str(r["wall_min"])]
        print("| " + " | ".join(cells) + " |")


if __name__ == "__main__":
    main()
