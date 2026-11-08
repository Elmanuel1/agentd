#!/usr/bin/env python3
"""Resolve citations to Textract blocks the way the pipeline does (citation_locator.rs).

A quote is located as the shortest run of consecutive `line` blocks containing it (at most 8 lines;
40 for an exact quote, also matched against table rows joined " | "), on the cited page first and
then on any page of the cited document, ignoring case, spacing and punctuation. The block id is the
run's first line; the box covers every line in it. A quote of a table row, cells joined " | ", is
also matched against the page's rows, and against the rows of Textract table blocks (which keep a
wrapped cell whole). A quote that is not found is `__unresolved__`.

usage:
  cite.py locate <doc_id> <page> "<text_snippet>" [--exact]
  cite.py check <output.json> [--doc <doc_id>] [--exact] [--write]
      every citation in the file (an object with `text_snippet`): resolved, page corrected, or
      reported unresolved. --write fills block_id, page_number and bounding_box in the file.
"""
import argparse
import json
import sys
from pathlib import Path

import quotes
from blocks import DOCS, load_blocks, normalise, page_of, row_text, rows
from runs import Run, shortest_window

MAX_WINDOW = 8
MAX_EXACT_WINDOW = 40
MIN_PARTIAL_LINE_LEN = 4
UNRESOLVED = "__unresolved__"


class Locator:
    def __init__(self, doc_id):
        blocks = load_blocks(doc_id)
        self.runs, self.row_runs = [], []
        for b in blocks:
            if b.get("type") != "line" or not b.get("bbox") or b.get("content") is None:
                continue
            page, text = page_of(b), normalise(b["content"])
            if page is None or not text:
                continue
            line = {"cells": [b], "page": page, "text": text}
            if self.runs and self.runs[-1][-1]["page"] == page:
                self.runs[-1].append(line)
            else:
                self.runs.append([line])
        for b in blocks:
            if b.get("type") not in ("table", "layout_table") or "|" not in (b.get("content") or "") or not b.get("bbox"):
                continue
            page = page_of(b)
            run = []
            for raw in b["content"].split("\n"):
                text = normalise(raw)
                if text and set(raw.replace("|", "").strip()) - set("-: "):
                    run.append({"cells": [b], "page": page, "text": text})
            if page is not None and run:
                self.row_runs.append(run)
        for row in rows(blocks):
            text = row_text(row)
            line = {"cells": row["cells"], "page": row["page"], "text": normalise(text)}
            if self.row_runs and self.row_runs[-1][-1]["page"] == row["page"]:
                self.row_runs[-1].append(line)
            else:
                self.row_runs.append([line])
        self.runs = [Run(r) for r in self.runs]
        self.row_runs = [Run(r) for r in self.row_runs]

    def locate(self, page, snippet, exact=False):
        needle = normalise(snippet)
        if not needle:
            return None
        for runs in (self.on(self.runs, page), self.runs):
            found = shortest_window(runs, needle, MAX_EXACT_WINDOW if exact else MAX_WINDOW)
            if not found and not exact:
                found = partial_span(runs, needle)
            if found:
                return location(found)
        for runs in (self.on(self.row_runs, page), self.row_runs):
            found = shortest_window(runs, needle, MAX_EXACT_WINDOW if exact else MAX_WINDOW)
            if found:
                return location(found)
        return None

    @staticmethod
    def on(runs, page):
        return [r for r in runs if r.page == page]


def partial_span(runs, needle):
    best = None
    for run in (r.lines for r in runs):
        start = 0
        for end in range(len(run) + 1):
            inside = end < len(run) and len(run[end]["text"]) >= MIN_PARTIAL_LINE_LEN and run[end]["text"] in needle
            if inside:
                continue
            span = run[start:end]
            if len(span) > (len(best) if best else 0):
                best = span
            start = end + 1
    if best and sum(len(l["text"]) for l in best) * 2 >= len(needle):
        return best
    return None


def location(lines):
    boxes = []
    for l in lines:
        for c in l["cells"]:
            b = c["bbox"]
            left, top = b.get("left") or 0.0, b.get("top") or 0.0
            boxes.append((left, top, left + (b.get("width") or 0.0), top + (b.get("height") or 0.0)))
    left, top = min(b[0] for b in boxes), min(b[1] for b in boxes)
    right, bottom = max(b[2] for b in boxes), max(b[3] for b in boxes)
    return {"block_id": lines[0]["cells"][0]["block_id"], "page_number": lines[0]["page"],
            "bounding_box": {"left": left, "top": top, "width": right - left, "height": bottom - top}}


LOCATORS = {}


def locator(doc_id):
    if doc_id not in LOCATORS:
        LOCATORS[doc_id] = Locator(doc_id)
    return LOCATORS[doc_id]


def walk(node, path, found):
    if isinstance(node, dict):
        if "text_snippet" in node:
            found.append((path, node))
        elif isinstance(node.get("quote"), str) and "page" in node:
            found.append((path, node))
        for k, v in node.items():
            walk(v, f"{path}.{k}" if path else k, found)
    elif isinstance(node, list):
        for i, v in enumerate(node):
            walk(v, f"{path}[{i}]", found)


def resolve_change(c, doc, where, unresolved):
    """A change's `quote` and `page`: located exactly, as the pipeline does for changes."""
    if not doc or not (DOCS / doc / "blocks.json").exists():
        unresolved.append((where, {"page_number": c.get("page"), "text_snippet": c["quote"]}, "pass --doc"))
        c["block_id"] = UNRESOLVED
        return 0
    loc = locator(doc).locate(c.get("page") or 0, c["quote"], exact=True)
    if loc is None:
        unresolved.append((where, {"page_number": c.get("page"), "text_snippet": c["quote"]}, "quote not found exactly"))
        c["block_id"] = UNRESOLVED
        return 0
    moved = int(loc["page_number"] != c.get("page"))
    c.update(block_id=loc["block_id"], page=loc["page_number"], bounding_box=loc["bounding_box"])
    return moved


def check(args):
    path = Path(args.file)
    data = json.loads(path.read_text())
    citations = []
    walk(data, "", citations)
    unresolved, moved = [], 0
    for where, c in citations:
        if "text_snippet" not in c:
            moved += resolve_change(c, args.doc, where, unresolved)
            continue
        doc = c.get("document_id") or args.doc
        if not doc or not (DOCS / doc / "blocks.json").exists():
            unresolved.append((where, c, "unknown document_id"))
            c["block_id"] = UNRESOLVED
            continue
        loc = locator(doc).locate(c.get("page_number") or 0, c.get("text_snippet") or "", args.exact)
        if loc is None:
            unresolved.append((where, c, "text_snippet not found in the document"))
            c["block_id"] = UNRESOLVED
            continue
        if loc["page_number"] != c.get("page_number"):
            moved += 1
        c.update(loc)
        c["document_id"] = doc
    rows_bad = quotes.row_quote_problems(data) + quotes.off_page_quotes(citations, args.doc)
    if args.write:
        path.write_text(json.dumps(data, indent=1))
    print(json.dumps({"citations": len(citations), "resolved": len(citations) - len(unresolved),
                      "page_corrected": moved, "unresolved": len(unresolved)}))
    for where, c, why in unresolved:
        print(f"UNRESOLVED {where}: p{c.get('page_number')} \"{(c.get('text_snippet') or '')[:120]}\" ({why})")
    for where, snippet, why in rows_bad:
        print(f"ROW_QUOTE {where}: \"{snippet[:120]}\" ({why})")
    if unresolved or rows_bad:
        print("Fix each quote; never delete the value. A quote you cannot find stays, with a review_reason saying so.")
    return 1 if unresolved or rows_bad else 0


def main():
    ap = argparse.ArgumentParser()
    sub = ap.add_subparsers(dest="cmd", required=True)
    lo = sub.add_parser("locate")
    lo.add_argument("doc_id")
    lo.add_argument("page", type=int)
    lo.add_argument("snippet")
    lo.add_argument("--exact", action="store_true")
    ch = sub.add_parser("check")
    ch.add_argument("file")
    ch.add_argument("--doc")
    ch.add_argument("--exact", action="store_true")
    ch.add_argument("--write", action="store_true")
    args = ap.parse_args()
    if args.cmd == "locate":
        loc = locator(args.doc_id).locate(args.page, args.snippet, args.exact)
        print(json.dumps(loc or {"block_id": UNRESOLVED}))
        return 0 if loc else 1
    return check(args)


if __name__ == "__main__":
    sys.exit(main())
