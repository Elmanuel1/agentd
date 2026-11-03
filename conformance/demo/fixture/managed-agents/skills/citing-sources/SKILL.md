---
name: citing-sources
description: Resolves extracted text snippets to source locations in parsed documents. Use when producing citations for extracted values — looks up blocks.json to find page numbers, bounding boxes, and block references for any text snippet.
---

# Citing Sources

<task>
  Produce citations for extracted values. Output must conform to the Shared Citation Type defined in `.claude/schemas/citation.schema.json`.

  ## What the Pipeline Does for You

  The pipeline resolves `block_id` and `bounding_box` from your `text_snippet` after you finish — **do not output `block_id`**. Your only job is to write an accurate `text_snippet`. If your `text_snippet` does not appear verbatim in any block, the citation will be marked `__unresolved__` and the reviewer will disprove the field.

  ## Your Only Job: Write an Accurate text_snippet

  1. Find the source text in the document that supports the extracted value
  2. Copy a short, exact substring of that text — the pipeline uses this to locate the block
  3. The snippet must be verbatim — if it is not an exact substring of the block's content, the citation will fail to resolve

  ## Matching Rules

  - `text_snippet` MUST be an exact substring of the source block's `content` — never paraphrase
  - Choose the shortest snippet that unambiguously identifies the source
  - If text spans multiple blocks, use the block with the most relevant portion
  - Prefer `line` or `layout_*` blocks over `word` blocks (more context, more likely to resolve)
  - For body content vs TOC: always cite the body content pages, not the table of contents
  - Use `parent` to navigate the hierarchy when needed (e.g., find the table a cell belongs to)

  ## Citations Are Required for Every Field

  Citations are not optional and are not limited to bonds and requirements. **Every extracted field must be cited** — including scalar fields like `name`, `closing_date`, `scope_of_work`, `currency`, `reference_number`, `timezone`, `start_date`, `completion_date`, `inquiry_deadline`, `submission_method`, `submission_url`, `delivery_method`, `location`, and `liquidated_damages`.

  All top-level fields in the bid-extractor output use the `{"value": ..., "citations": [...]}` wrapper. The `citations` array must contain at least one citation. Use `{"value": null, "citations": []}` only when the field is genuinely absent from the document.

  ### Example — Citing a Scalar Field

  Suppose you extracted `closing_date` as `"2025-03-15T14:00:00"` from:
  > "Bids must be submitted no later than 2:00 PM local time on March 15, 2025."

  ```json
  "closing_date": {
    "value": "2025-03-15T14:00:00",
    "citations": [
      {
        "document_id": "a1b2c3d4-e5f6-7890-abcd-ef1234567890",
        "page_number": 3,
        "text_snippet": "Bids must be submitted no later than 2:00 PM local time on March 15, 2025"
      }
    ]
  }
  ```

  The pipeline locates the block from `text_snippet` automatically.
</task>

<rule>
  - `text_snippet` must be verbatim — an exact substring of the source block's content, never paraphrased
  - Always include `document_id` (the UUID directory name) in every citation
  - Do NOT output `block_id` — the pipeline sets it from `text_snippet`
  - Citations are required for every extracted field — including scalar fields, not just bonds and requirements
  - Use `{"value": null, "citations": []}` only when the field is genuinely not present in the document
  - Always cite body content pages, not table of contents entries
</rule>
