#!/usr/bin/env python3
"""Join an upload's per-agent, per-document outputs into one result.json, the shape the launcher
writes and score-extraction reads: values and citations as the agents wrote them, each with its
pipeline_outcome from the reviews.

usage:
  merge.py tender <upload_dir>     writes <upload_dir>/result.json from {agent}-{doc}.json,
                                   review-{agent}-{doc}.json and review-retry-{agent}-{doc}.json
  merge.py current <result.json>   prints the tender as the change-detector reads it: values only,
                                   no ids, citations or outcomes
"""
import json
import os
import re
import sys
from pathlib import Path

DOCS = Path(os.environ.get("EXTRACTION_DOCS", "/mnt/session/uploads/extraction/docs"))
SCHEMAS = Path(os.environ.get("EXTRACTION_SCHEMAS", "/mnt/session/uploads/extraction/schemas"))
CHECKLIST_LISTS = ("conditions", "obligations", "requirements")
DROP = ("citations", "pipeline_outcome", "block_id", "bounding_box", "explanation", "id")


def load(path):
    try:
        return json.loads(path.read_text())
    except (OSError, json.JSONDecodeError):
        return None


def page_count(doc):
    blocks = json.loads((DOCS / doc / "blocks.json").read_text())
    return max(((b.get("bbox") or {}).get("page") or 0) for b in blocks)


def verdicts(upload, agent, doc):
    """field_name (or group|item_id) -> pipeline outcome, from the review and the retry review."""
    first = load(upload / f"review-{agent}-{doc}.json") or {}
    retry = load(upload / f"review-retry-{agent}-{doc}.json") or {}

    def keyed(review):
        out = {}
        for r in review.get("reviews", []):
            key = r.get("field_name") or f'{r.get("group_name")}|{r.get("item_id")}'
            out[key] = r.get("verdict")
        return out

    first, retry = keyed(first), keyed(retry)
    outcomes = {}
    for key, verdict in first.items():
        if verdict in ("disproved", "uncertain") and key in retry:
            outcomes[key] = f'{verdict}_retry_{"verified" if retry[key] == "verified" else "disproved"}'
        else:
            outcomes[key] = verdict
    return outcomes


def labelled(value, outcome):
    if outcome == "verified" and value is None:
        return "not_found"
    return outcome or "unreviewed"


def tender(upload):
    """result.json as the scorer reads it: the bid-extractor keys at the top level, the checklists
    from checklist-extractor, and vendor_groups; every value carries its pipeline_outcome."""
    outputs = {}
    for f in upload.glob("*.json"):
        m = re.match(r"^(bid-extractor|checklist-extractor|vendor-grouper)-([0-9a-f-]{36})\.json$", f.name)
        if m:
            outputs[(m.group(1), m.group(2))] = load(f) or {}
    docs = sorted({d for _, d in outputs}, key=lambda d: -page_count(d))
    bid_props = json.loads((SCHEMAS / "bid-extractor.schema.json").read_text())["properties"]
    list_keys = [k for k, v in bid_props.items() if v.get("type") == "array"]
    result = {k: [] for k in list_keys}
    result["vendor_groups"] = []
    for doc in docs:
        bid = outputs.get(("bid-extractor", doc), {})
        outcomes = verdicts(upload, "bid-extractor", doc)
        for key in bid_props:
            if key in list_keys or key == "source_agent" or key not in bid:
                continue
            node = bid[key] if isinstance(bid[key], dict) and "value" in bid[key] else {"value": bid[key], "citations": []}
            if result.get(key) is None or result[key].get("value") is None:
                result[key] = dict(node, pipeline_outcome=labelled(node.get("value"), outcomes.get(key)))
        for key in list_keys:
            agent = "checklist-extractor" if key in CHECKLIST_LISTS else "bid-extractor"
            outcomes = verdicts(upload, agent, doc)
            for i, entry in enumerate(outputs.get((agent, doc), {}).get(key) or []):
                if isinstance(entry, dict):
                    result[key].append(dict(entry, pipeline_outcome=labelled(entry, outcomes.get(f"{key}[{i}]"))))
        group_outcomes = verdicts(upload, "vendor-grouper", doc)
        for group in outputs.get(("vendor-grouper", doc), {}).get("vendor_groups") or []:
            items = [dict(item, pipeline_outcome=labelled(item, group_outcomes.get(f'{group.get("group_name")}|{item.get("item_id")}')))
                     for item in group.get("items") or []]
            result["vendor_groups"].append(dict(group, items=items))
    (upload / "result.json").write_text(json.dumps(result, indent=1))
    counts = {"fields": sum(1 for k, v in result.items() if isinstance(v, dict) and v.get("value") is not None),
              "list_entries": sum(len(v) for k, v in result.items() if isinstance(v, list) and k != "vendor_groups"),
              "vendor_groups": len(result["vendor_groups"]),
              "items": sum(len(g["items"]) for g in result["vendor_groups"])}
    print(json.dumps({"wrote": str(upload / "result.json"), **counts}))


def bare(node):
    if isinstance(node, dict):
        if "value" in node and "citations" in node:
            return bare(node["value"])
        return {k: bare(v) for k, v in node.items() if k not in DROP and k != "document_id"}
    if isinstance(node, list):
        return [bare(x) for x in node]
    return node


def main():
    if len(sys.argv) != 3 or sys.argv[1] not in ("tender", "current"):
        sys.exit(__doc__)
    if sys.argv[1] == "tender":
        tender(Path(sys.argv[2]))
    else:
        print(json.dumps(bare(json.loads(Path(sys.argv[2]).read_text())), indent=1))


if __name__ == "__main__":
    main()
