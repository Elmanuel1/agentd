#!/usr/bin/env python3
"""A document's text as the pipeline gives it to an agent: the filtered blocks (filter.rs), each
page headed `--- Page N ---` and each block `[b:<block_id>] <content>` (pipeline.rs blocks_to_text).

usage:
  pages.py <doc_id> [--pages 1-5,9]     the text of those pages (all pages when omitted)
  pages.py <doc_id> --grep <regex>      the blocks matching the regex, case-insensitive, with page
  pages.py <doc_id> --outline           per page: block count and section headers
  pages.py <doc_id> --rows [--pages N-M] each page's table rows, cells joined by " | " left to
                                        right, as a schedule row is quoted in a citation

Each call prints at most --max-chars (default 20000) characters, stopping at a page boundary, and
says how to read the rest: read a long document in parts, not in one call.
"""
import argparse
import json
import os
import re
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import cite  # noqa: E402

DOCS = Path(os.environ.get("EXTRACTION_DOCS", "/mnt/session/uploads/extraction/docs"))
MIN_CONTENT_LENGTH = 5
TEXT_BLOCK_TYPES = {
    "line", "word", "cell", "title", "section_header", "paragraph", "list_item", "header", "footer",
    "key_value", "table", "table_cell", "table_row", "page_header", "page_footer", "page_number",
    "layout_text", "layout_section_header", "layout_header", "layout_footer", "layout_title", "layout_list",
    "layout_key_value", "layout_page_number", "table_title", "table_footer", "merged_cell", "layout_table",
}


def keep(block):
    if block.get("type") == "word" or block.get("type") not in TEXT_BLOCK_TYPES:
        return False
    content = (block.get("content") or "").strip()
    return len(content) >= MIN_CONTENT_LENGTH and sum(c.isalpha() for c in content) >= 2


def filtered(doc_id):
    blocks = cite.load_blocks(doc_id)
    absorbing = {b["block_id"] for b in blocks
                 if b.get("type") in ("table", "merged_cell") and (b.get("content") or "").strip()}
    return [b for b in blocks
            if not (b.get("type") == "cell" and b.get("parent") in absorbing) and keep(b)]


def page_of(block):
    bbox = block.get("bbox") or {}
    return bbox.get("page") or bbox.get("original_page")


def page_set(spec):
    pages = set()
    for part in spec.split(","):
        a, _, b = part.strip().partition("-")
        pages.update(range(int(a), int(b or a) + 1))
    return pages


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("doc_id")
    ap.add_argument("--pages")
    ap.add_argument("--grep")
    ap.add_argument("--outline", action="store_true")
    ap.add_argument("--rows", action="store_true")
    ap.add_argument("--max-chars", type=int, default=20000)
    args = ap.parse_args()
    blocks = filtered(args.doc_id)

    if args.grep:
        pattern = re.compile(args.grep, re.I)
        used, shown, hits = 0, 0, [b for b in blocks if pattern.search(b["content"])]
        for b in hits:
            line = f"p{page_of(b)} [b:{b['block_id']}] {b['content'].strip()}"
            if used + len(line) > args.max_chars:
                print(f"... {len(hits) - shown} more matches not shown; narrow the regex or add --pages.")
                break
            print(line)
            used += len(line) + 1
            shown += 1
        return

    if args.outline:
        pages = {}
        for b in blocks:
            entry = pages.setdefault(page_of(b), {"blocks": 0, "headers": []})
            entry["blocks"] += 1
            if b["type"] in ("layout_section_header", "layout_title", "section_header", "title") and len(entry["headers"]) < 6:
                entry["headers"].append(b["content"].strip()[:80])
        for p in sorted(k for k in pages if k is not None):
            print(f"p{p} ({pages[p]['blocks']} blocks): " + " | ".join(pages[p]["headers"]))
        return

    wanted = page_set(args.pages) if args.pages else None
    by_page = {}
    if args.rows:
        raw = cite.load_blocks(args.doc_id)
        for row in cite.rows(raw):
            if wanted is not None and row["page"] not in wanted:
                continue
            text = cite.row_text(row)
            if text:
                by_page.setdefault(row["page"], []).append(text)
        blocks = []
    for b in blocks:
        page = page_of(b)
        if page is None or (wanted is not None and page not in wanted):
            continue
        by_page.setdefault(page, []).append(f"[b:{b['block_id']}] {b['content'].strip()}")
    pages = sorted(by_page)
    out, used = [], 0
    for i, page in enumerate(pages):
        chunk = f"\n--- Page {page} ---\n" + "\n".join(by_page[page])
        if out and used + len(chunk) > args.max_chars:
            rest = pages[i:]
            print("".join(out))
            print(f"\n... stopped at {args.max_chars} characters before page {page}. Next: --pages {page}-{rest[-1]}")
            return
        out.append(chunk)
        used += len(chunk)
    print("".join(out))


if __name__ == "__main__":
    main()
