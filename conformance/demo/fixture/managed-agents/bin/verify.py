#!/usr/bin/env python3
"""Check that each value's citations state it, the way the pipeline's field review does
(field_review.rs): a value its own quote plainly contains passes; a value with no citation is
`no_citation`; every other value goes to the citation-verifier agent with the text Jev reads,
the quote marked inside the page around it.

usage:
  verify.py prepare <output.json> --doc <doc_id> --out <work.json>
      writes the values to check, each with `field`, `field_meaning`, `value` and `source_text`
  verify.py apply <output.json> <verdicts.json> --work <work.json> [--write]
      records each failed verdict as the value's `review_reason` ("verifier: <verdict>: <reason>")
      and prints the failures as JSON lines, for the agent that produced the value to fix
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
SIGNALS = json.loads((Path(__file__).resolve().parent / "signals.json").read_text())
PAGE_SPAN = 1400
MAX_SOURCE_CHARS = 3000
MAX_CITATIONS = 3
KEY_CHARS = 40
LINE_MEANING = "a line of the price schedule: its item number, description, quantity, unit and who prices it"
BOOKKEEPING = {"citations", "pipeline_outcome", "review_reason", "confidence", "source_agent", "change_history", "explanation",
               "block_id", "bounding_box", "document_id"}
TYPE_KEYS = ("type", "category", "role", "kind")
PREFIX = "Citation check: "
VERDICT_TEXT = {
    "not_in_source": "the cited text does not state this value",
    "wrong_value": "the cited text gives a different value",
    "wrong_kind": "the value is a different kind of thing than the field asks for",
    "incomplete": "the value leaves out part of what the cited text states",
    "weak_citation": "the quote itself does not state the value; only the text around it does",
    "unit_mismatch": "the row shows a different unit",
    "quantity_mismatch": "the row shows a different quantity",
    "description_mismatch": "the row describes different work",
    "no_citation": "the value has no citation",
    "unchecked": "the citation check gave no verdict",
    "other": "the value does not match its cited text",
}
VALUE_VERDICTS = {"correct", "not_in_source", "wrong_kind", "wrong_value", "incomplete", "other"}
ROW_VERDICTS = {"correct", "unit_mismatch", "quantity_mismatch", "description_mismatch", "not_in_source"}


def norm(text):
    return " ".join("".join(c.lower() if c.isalnum() else " " for c in str(text)).split())


def page_texts(doc_id):
    """Row-aware page text, as the pipeline gives Jev: the line blocks that share a row joined by
    " | " (cite.py rows), so a table row reads whole and its section heading sits above it."""
    blocks = cite.load_blocks(doc_id)
    pages = {}
    for row in cite.rows(blocks):
        text = " | ".join(c["content"].strip() for c in row["cells"] if (c.get("content") or "").strip())
        if text:
            pages.setdefault(row["page"], []).append(text)
    return {p: "\n".join(lines) for p, lines in pages.items()}


def source_text(pages, page, quote):
    text = pages.get(page, "")
    flat = text.replace("\n", " ").lower()
    key = " ".join(quote.split()).lower()[:KEY_CHARS]
    at = flat.find(key) if key else -1
    if at >= 0:
        start = max(0, at - PAGE_SPAN // 3)
        window = text[start:start + PAGE_SPAN]
    else:
        window = text[:MAX_SOURCE_CHARS]
    return f"[p{page}] quoted: {quote}\n[p{page}] page: {window}"


def entry_value(node):
    return {k: v for k, v in node.items() if k not in BOOKKEEPING}


def claimed_type(list_key, entry):
    for k in TYPE_KEYS:
        if isinstance(entry.get(k), str):
            return SIGNALS["entry_types"].get(f"{list_key}.{entry[k]}")
    return None


def contained(value, quotes):
    """A scalar value its own quote states as written (numbers compared by digits)."""
    if isinstance(value, bool) or value is None or isinstance(value, (dict, list)):
        return False
    joined = norm(" ".join(quotes))
    if isinstance(value, (int, float)):
        digits = re.sub(r"\D", "", " ".join(quotes))
        whole = str(int(value)) if float(value).is_integer() else str(value).replace(".", "")
        return bool(whole) and whole in digits
    v = norm(value)
    return len(v) >= 3 and v in joined


def row_contained(item, quotes):
    joined = norm(" ".join(quotes))
    item_id, qty = norm(item.get("item_id") or ""), item.get("quantity")
    words = [w for w in norm(item.get("description") or "").split() if len(w) > 2]
    if not item_id or item_id not in joined or not words:
        return False
    if qty is not None and not contained(qty, quotes):
        return False
    covered = sum(w in joined.split() for w in words) / len(words)
    return covered >= 0.75


def targets(output):
    """Every checkable value: (path, field, field_meaning, claimed_type, kind, node, value, citations)."""
    fields, lists = SIGNALS["fields"], SIGNALS["lists"]
    for key, node in output.items():
        if key in fields and isinstance(node, dict) and "value" in node:
            if node.get("value") not in (None, "", [], {}):
                yield key, key, fields[key], None, "field", node, node["value"], node.get("citations") or []
        elif key in lists and isinstance(node, list):
            for i, entry in enumerate(node):
                if isinstance(entry, dict):
                    yield f"{key}[{i}]", key, lists[key], claimed_type(key, entry), "entry", entry, entry_value(entry), entry.get("citations") or []
    for g, group in enumerate(output.get("vendor_groups") or []):
        for i, item in enumerate(group.get("items") or []):
            yield f"vendor_groups[{g}].items[{i}]", "lines", LINE_MEANING, None, "row", item, entry_value(item), item.get("citations") or []
    for i, change in enumerate(output.get("changes") or []):
        cites = change.get("citations") or ([change["citation"]] if change.get("citation") else []) or \
            ([{"page_number": change.get("page"), "text_snippet": change.get("quote")}] if change.get("quote") else [])
        field = change.get("field") or ""
        meaning = fields.get(field) or lists.get(field) or (LINE_MEANING if field == "vendor_groups" else field.replace("_", " "))
        value = {"op": change.get("op"), "value": change.get("value"), "existing": change.get("existing")}
        yield f"changes[{i}]", field, meaning, None, "change", change, value, cites


def prepare(args):
    output = json.loads(Path(args.output).read_text())
    pages = page_texts(args.doc)
    items, contained_n, uncited = [], 0, []
    for path, field, meaning, claimed, kind, node, value, cites in targets(output):
        quotes = [c.get("text_snippet") or "" for c in cites if c.get("text_snippet")]
        if not quotes:
            uncited.append(path)
            continue
        plain = row_contained(value, quotes) if kind == "row" else kind == "field" and contained(value, quotes)
        if plain:
            contained_n += 1
            continue
        parts = [source_text(pages, c.get("page_number") or 0, c["text_snippet"]) for c in cites[:MAX_CITATIONS] if c.get("text_snippet")]
        item = {"id": f"v{len(items) + 1}", "path": path, "kind": kind, "field": field, "field_meaning": meaning,
                "value": value, "source_text": "\n…\n".join(parts)[:MAX_SOURCE_CHARS]}
        if claimed:
            item["claimed_type"] = claimed
        note = node.get("review_reason")
        if note and not str(note).startswith(PREFIX):
            item["review_reason"] = note
        items.append(item)
    work = {"output": str(Path(args.output)), "doc_id": args.doc, "items": items, "uncited": uncited}
    Path(args.out).parent.mkdir(parents=True, exist_ok=True)
    Path(args.out).write_text(json.dumps(work, indent=1))
    print(json.dumps({"to_verify": len(items), "contained": contained_n, "no_citation": len(uncited), "work": args.out}))


def resolve(output, path):
    node = output
    for part in re.findall(r"[^.\[\]]+|\[\d+\]", path):
        node = node[int(part[1:-1])] if part.startswith("[") else node[part]
    return node


def apply(args):
    output = json.loads(Path(args.output).read_text())
    work = json.loads(Path(args.work).read_text())
    verdicts = {v.get("id"): v for v in json.loads(Path(args.verdicts).read_text()).get("verdicts", [])}
    failures = []
    for item in work["items"]:
        node = resolve(output, item["path"])
        v = verdicts.get(item["id"])
        if v is None:
            failures.append({"path": item["path"], "verdict": "unchecked", "reason": "the verifier gave no verdict", "source_text": item["source_text"]})
            continue
        allowed = ROW_VERDICTS if item["kind"] == "row" else VALUE_VERDICTS
        verdict = v.get("verdict") if v.get("verdict") in allowed else "other"
        if verdict == "correct" and v.get("quote_states", True) is False:
            verdict = "weak_citation"
        if verdict == "correct":
            if str(node.get("review_reason") or "").startswith(PREFIX):
                node.pop("review_reason", None)
            continue
        reason = (v.get("reason") or "").strip()
        node["review_reason"] = f"{PREFIX}{VERDICT_TEXT.get(verdict, verdict)}." + (f" {reason}" if reason else "")
        failures.append({"path": item["path"], "verdict": verdict, "reason": v.get("reason"), "source_text": item["source_text"]})
    for path in work.get("uncited", []):
        resolve(output, path)["review_reason"] = f"{PREFIX}{VERDICT_TEXT['no_citation']}."
        failures.append({"path": path, "verdict": "no_citation", "reason": "the value has no citation"})
    if args.write:
        Path(args.output).write_text(json.dumps(output, indent=1))
    print(json.dumps({"checked": len(work["items"]), "failed": len(failures)}))
    for f in failures:
        print(json.dumps(f))
    return 0


def main():
    ap = argparse.ArgumentParser()
    sub = ap.add_subparsers(dest="cmd", required=True)
    p = sub.add_parser("prepare")
    p.add_argument("output")
    p.add_argument("--doc", required=True)
    p.add_argument("--out", required=True)
    a = sub.add_parser("apply")
    a.add_argument("output")
    a.add_argument("verdicts")
    a.add_argument("--work", required=True)
    a.add_argument("--write", action="store_true")
    args = ap.parse_args()
    return prepare(args) if args.cmd == "prepare" else apply(args)


if __name__ == "__main__":
    sys.exit(main())
