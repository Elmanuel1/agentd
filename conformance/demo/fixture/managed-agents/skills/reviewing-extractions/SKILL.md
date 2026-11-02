---
name: reviewing-extractions
description: Adversarially reviews extraction output against source documents. Use when verifying whether extracted values are correct, supported by evidence, and free from hallucination.
---

# Reviewing Extractions

<task>
  Verify extraction output against the source document's `blocks.json`. Produce output conforming to the Reviewer schema in `.claude/schemas/reviewer.schema.json`. Follow the scoring model in `.claude/rules/pipeline-rules.md`.

  ## Two Review Modes

  Your output schema depends on which agent you are reviewing:

  **Field-keyed review** (for bid-extractor — including bonds/insurance/requirements — and addenda-processor):
  - Set `review_type: "field"`
  - Each review entry keyed by `field_name`
  - **When reviewing addenda-processor:** each entry MUST include `addendum_number` to form the compound key `(addendum_number, field_name)` — the same field can be changed by multiple addenda
  - Verify proposed values against source text

  **Item-keyed review** (for vendor-grouper):
  - Set `review_type: "item"`
  - Each review entry keyed by `group_name` + `item_id`
  - Verify two things: (1) does the item exist in the source? (2) is it in the right group?
  - Review the vendor group names and types for correctness
  - When disproved, provide `corrected_group` (the correct group name)

  ## Review Process

  For each extracted entry:
  1. Read the proposed value and its citations
  2. **If any citation has `block_id: "__unresolved__"`** — the pipeline could not find this snippet in the source document. Immediately mark the entry **disproved** with evidence `"Citation unresolved: text_snippet not found in any source block"`. Do not attempt further verification for this entry.
  3. For all other citations: find the `text_snippet` in the cited block's `content` field and verify the proposed value is correctly derived from it
  4. Check for contradicting information elsewhere in the document
  5. Assign a verdict: verified, disproved, or uncertain

  ## Verdict Standards

  **verified** — ALL of these must be true:
  - No citation has `block_id: "__unresolved__"`
  - The proposed value is a reasonable interpretation of the cited text
  - No contradicting information elsewhere in the document
  - (vendor-grouper only) The item belongs to the assigned group based on scope

  **disproved** — ANY of these is true:
  - Any citation has `block_id: "__unresolved__"` (pipeline could not locate the snippet)
  - The proposed value contradicts the cited text
  - A more authoritative section (addendum, amendment) provides a different value
  - The value is fabricated (not derivable from any source text)
  - (vendor-grouper only) The item is assigned to the wrong group

  **uncertain** — use when:
  - The cited text exists but is ambiguous
  - The value is plausible but the evidence is weak
  - You cannot find supporting OR contradicting evidence
  - (vendor-grouper only) The item could plausibly belong to multiple groups
  - **NOT allowed on retry reviews** — on a second pass, you must commit to verified or disproved

  ## Avoiding False Disproval

  - Do NOT disprove a value just because you would have extracted it differently
  - Timezone conversions, date format differences, and reasonable rounding are NOT grounds for disproval
  - If the extractor's interpretation is defensible, verdict is verified
  - Only disprove when the source clearly says something different

  ## Evidence

  Always provide the specific text you found (or didn't find) as evidence. For disproved entries, quote the contradicting source text. For uncertain entries, explain what you searched for and why it was inconclusive.
</task>

<rule>
  - If any citation has `block_id: "__unresolved__"`, immediately disprove — do not attempt to verify
  - uncertain is NOT valid on retry reviews — commit to verified or disproved
  - Do not disprove based on personal extraction preference — only disprove when the source clearly contradicts the value
  - Always include specific evidence text for every verdict
  - For addenda-processor reviews, include `addendum_number` in every field-keyed entry
</rule>
