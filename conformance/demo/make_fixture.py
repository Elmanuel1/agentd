#!/usr/bin/env python3
"""Copy the slice of ~/tosspaper-eval-data the scripted demo run needs into
fixture/, with the same layout, so CI replays the demo without the full data.

The agents, skills, rubrics, plan, helpers, schemas, rules and grader prompt are
copied as they are; each document keeps its first three pages.

usage: make_fixture.py [--data ~/tosspaper-eval-data]
"""
import argparse
import json
import shutil
from pathlib import Path

HERE = Path(__file__).resolve().parent
PAGES = 3
COPY = [
    "managed-agents/agents", "managed-agents/skills", "managed-agents/bin", "managed-agents/schemas",
    "managed-agents/original/rules", "managed-agents/original/data", "managed-agents/environments",
    "managed-agents/rubric-u0.md", "managed-agents/rubric-later.md", "managed-agents/skill-ids.tsv",
    "managed-agents/session-2025-45.json", "codex-sim/plan-2025-45.json", "harness-sim/driver.py",
]


def keep(path: Path) -> bool:
    return ".before-" not in path.name and "__pycache__" not in path.parts


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--data", type=Path, default=Path.home() / "tosspaper-eval-data")
    args = ap.parse_args()
    out = HERE / "fixture"
    for rel in COPY:
        src, dst = args.data / rel, out / rel
        dst.parent.mkdir(parents=True, exist_ok=True)
        if src.is_dir():
            for f in src.rglob("*"):
                if f.is_file() and keep(f):
                    (dst / f.relative_to(src)).parent.mkdir(parents=True, exist_ok=True)
                    shutil.copy2(f, dst / f.relative_to(src))
        else:
            shutil.copy2(src, dst)
    for doc in (args.data / "inputs" / "2025-45" / "parsed").iterdir():
        blocks = json.loads((doc / "blocks.json").read_text())
        first = [b for b in blocks if b.get("bbox", {}).get("page", 1) <= PAGES]
        dst = out / "inputs" / "2025-45" / "parsed" / doc.name / "blocks.json"
        dst.parent.mkdir(parents=True, exist_ok=True)
        dst.write_text(json.dumps(first))


if __name__ == "__main__":
    main()
