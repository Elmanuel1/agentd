#!/usr/bin/env python3
"""A document's Textract blocks as the citation tools read them: its printed rows, each row's cells
left to right joined " | ", and the page each falls on."""
import functools
import json
import os
import tempfile
from pathlib import Path

DOCS = Path(os.environ.get("EXTRACTION_DOCS", "/mnt/session/uploads/extraction/docs"))
ROW_TOLERANCE = 0.5
DEFAULT_LINE_HEIGHT = 0.01


def normalise(s):
    return " ".join("".join(c.lower() if c.isalnum() else " " for c in str(s)).split())


def row_text(row):
    """A printed row as the pipeline reads it: its cells left to right, joined " | "."""
    return " | ".join(c["content"].strip() for c in row["cells"] if (c.get("content") or "").strip())


def page_of(block):
    bbox = block.get("bbox") or {}
    return bbox.get("page") or bbox.get("original_page")


def load_blocks(doc_id):
    """The document's blocks without Textract's single-word blocks, which no tool reads, kept in a cache
    file under the temp directory that is named by the document's size and modification time."""
    source = DOCS / doc_id / "blocks.json"
    stat = source.stat()
    cache = Path(tempfile.gettempdir()) / "extraction-blocks" / f"{doc_id}-{stat.st_size}-{int(stat.st_mtime)}.json"
    try:
        return json.loads(cache.read_text())
    except FileNotFoundError:
        pass
    blocks = [b for b in json.loads(source.read_text()) if b.get("type") != "word"]
    cache.parent.mkdir(parents=True, exist_ok=True)
    part = cache.with_suffix(f".{os.getpid()}.part")
    part.write_text(json.dumps(blocks))
    part.replace(cache)
    return blocks


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
            continue
        built.append({"page": page, "top": top, "height": max(height, 1e-12), "cells": [(left, b)]})
    return [{"page": r["page"], "cells": [b for _, b in sorted(r["cells"], key=lambda x: x[0])]} for r in built]


@functools.cache
def page_lines(doc_id):
    """Each page as the pipeline reads it: one line per printed row, cells joined " | "."""
    pages = {}
    for row in rows(load_blocks(doc_id)):
        text = row_text(row)
        if text:
            pages.setdefault(row["page"], []).append(text)
    return pages
