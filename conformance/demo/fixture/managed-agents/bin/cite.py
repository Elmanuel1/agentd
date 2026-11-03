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
import os
import re
import sys
import tempfile
from bisect import bisect_left, bisect_right
from pathlib import Path

DOCS = Path(os.environ.get("EXTRACTION_DOCS", "/mnt/session/uploads/extraction/docs"))
MAX_WINDOW = 8
MAX_EXACT_WINDOW = 40
MIN_PARTIAL_LINE_LEN = 4
ROW_TOLERANCE = 0.5
DEFAULT_LINE_HEIGHT = 0.01
UNRESOLVED = "__unresolved__"


def normalise(s):
    return " ".join("".join(c.lower() if c.isalnum() else " " for c in s).split())


def load_blocks(doc_id):
    """The document's blocks without Textract's single-word blocks, which nothing here reads: four
    fifths of a large document. The first load keeps them in a cache file, so later calls skip them."""
    source = DOCS / doc_id / "blocks.json"
    stat = source.stat()
    cache = Path(tempfile.gettempdir()) / "extraction-blocks" / f"{doc_id}-{stat.st_size}-{int(stat.st_mtime)}.json"
    try:
        return json.loads(cache.read_text())
    except (OSError, ValueError):
        pass
    blocks = [b for b in json.loads(source.read_text()) if b.get("type") != "word"]
    try:
        cache.parent.mkdir(parents=True, exist_ok=True)
        part = cache.with_suffix(f".{os.getpid()}.part")
        part.write_text(json.dumps(blocks))
        part.replace(cache)
    except OSError:
        pass
    return blocks


class Run:
    """Consecutive lines with their text joined by single spaces, so a quote is searched once per
    run rather than once per window of lines."""

    def __init__(self, lines):
        self.lines, self.starts, parts, at = lines, [], [], 0
        for line in lines:
            self.starts.append(at)
            parts.append(line["text"])
            at += len(line["text"]) + 1
        self.text = " ".join(parts)
        self.page = lines[0]["page"]


def page_of(block):
    bbox = block.get("bbox") or {}
    return bbox.get("page") or bbox.get("original_page")


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
            text = " | ".join(c["content"].strip() for c in row["cells"] if (c.get("content") or "").strip())
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


def rows(blocks):
    placed = []
    for b in blocks:
        if b.get("type") != "line" or not (b.get("content") or "").strip() or not b.get("bbox"):
            continue
        bbox, page = b["bbox"], page_of(b)
        if page is None:
            continue
        placed.append((page, bbox.get("top") or 0.0, bbox.get("left") or 0.0, bbox.get("height") or DEFAULT_LINE_HEIGHT, b))
    placed.sort(key=lambda x: (x[0], x[1]))
    built = []
    for page, top, left, height, b in placed:
        if built and built[-1]["page"] == page and top - built[-1]["top"] <= ROW_TOLERANCE * built[-1]["height"]:
            built[-1]["cells"].append((left, b))
        else:
            built.append({"page": page, "top": top, "height": max(height, 1e-12), "cells": [(left, b)]})
    return [{"page": r["page"], "cells": [b for _, b in sorted(r["cells"], key=lambda x: x[0])]} for r in built]


def shortest_window(runs, needle, max_window):
    """The fewest consecutive lines of one run holding `needle`, the first run's on a tie. Matches
    may overlap: a later one can sit on fewer lines."""
    best = None
    for run in runs:
        at = run.text.find(needle)
        while at != -1:
            first = bisect_right(run.starts, at) - 1
            last = bisect_left(run.starts, at + len(needle)) - 1
            if last - first < max_window and (best is None or last - first + 1 < len(best)):
                best = run.lines[first:last + 1]
            if best is not None and len(best) == 1:
                return best
            at = run.text.find(needle, at + 1)
    return best


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
    rows_bad = row_quote_problems(data) + off_page_quotes(citations, args.doc)
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


QUOTE_WINDOW = 8
_PAGE_LINES = {}


def quote_words(text):
    return [w for w in re.split(r"[^0-9a-zA-Z\u00C0-\uFFFF]+", text.lower()) if w]


def page_lines(doc_id):
    """Each page as the pipeline's checks read it: one line per printed row, cells joined " | "."""
    if doc_id not in _PAGE_LINES:
        blocks = load_blocks(doc_id)
        pages = {}
        for row in rows(blocks):
            text = " | ".join(c["content"].strip() for c in row["cells"] if (c.get("content") or "").strip())
            if text:
                pages.setdefault(row["page"], []).append(text)
        _PAGE_LINES[doc_id] = pages
    return _PAGE_LINES[doc_id]


def quote_on_page(lines, quote):
    """The pipeline's rule (score.rs quote_on_page): the quote's words in order within a few consecutive
    lines; a quote of a table row (with " | ") holds when all its words fall within those lines."""
    words = quote_words(quote)
    if not words:
        return False
    if len(words) < 3:
        flat = " ".join(" ".join(lines).replace("|", " ").split()).lower()
        return " ".join(quote.replace("|", " ").split()).lower() in flat
    line_words = [quote_words(l) for l in lines]
    for start in range(len(line_words)):
        nxt = 0
        for lw in line_words[start:start + QUOTE_WINDOW]:
            for w in lw:
                if nxt < len(words) and w == words[nxt]:
                    nxt += 1
            if nxt == len(words):
                return True
    if "|" in quote:
        for start in range(len(line_words)):
            window = {w for lw in line_words[start:start + QUOTE_WINDOW] for w in lw}
            if all(w in window for w in words):
                return True
    return False


def off_page_quotes(citations, doc):
    """Citations the pipeline's check would not find on their page: reworded, reordered, or rebuilt from
    several printed lines."""
    bad = []
    for where, c in citations:
        d = c.get("document_id") or doc
        snippet = c.get("text_snippet")
        if not snippet or not d or not (DOCS / d / "blocks.json").exists():
            continue
        lines = page_lines(d).get(c.get("page_number"), [])
        if not quote_on_page(lines, snippet):
            bad.append((where, snippet, "not on its page as printed: copy the words in the page's order; quote a table "
                                        "row as one line of pages.py --rows"))
    return bad


def row_quote_problems(data):
    """A schedule line is cited by its whole printed row: its own mark (the item number, or a
    sub-row's first cell) and the row's cells joined by " | ", as pages.py --rows prints it."""
    bad = []
    for g, group in enumerate(data.get("vendor_groups") or []):
        for i, item in enumerate(group.get("items") or []):
            cites = item.get("citations") or []
            snippet = (cites[0].get("text_snippet") or "") if cites else ""
            where = f"vendor_groups[{g}].items[{i}]"
            item_id = (item.get("item_id") or "").strip()
            if not cites or not item_id:
                continue
            own = item_id.split(" ", 1)[1] if " " in item_id else item_id
            if snippet.count("|") < 2:
                bad.append((where, snippet, "quote the whole row from pages.py --rows, cells joined by ' | '"))
            elif normalise(own) not in normalise(snippet) and normalise(item_id) not in normalise(snippet):
                bad.append((where, snippet, f"the row quote does not include the line's own mark {own!r}"))
    return bad


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
