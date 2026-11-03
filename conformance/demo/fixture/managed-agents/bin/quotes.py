#!/usr/bin/env python3
"""The pipeline's checks of a quote as printed (score.rs): the quote's words in order on its own
page, and a schedule line cited by its whole printed row. `cite.py check` reports both."""
import json
import re

from cite import DOCS, normalise, row_text, rows

QUOTE_WINDOW = 8
_PAGE_LINES = {}


def quote_words(text):
    return [w for w in re.split(r"[^0-9a-zA-Z\u00C0-\uFFFF]+", text.lower()) if w]


def page_lines(doc_id):
    """Each page as the pipeline's checks read it: one line per printed row, cells joined " | "."""
    if doc_id not in _PAGE_LINES:
        blocks = json.loads((DOCS / doc_id / "blocks.json").read_text())
        pages = {}
        for row in rows(blocks):
            text = row_text(row)
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
