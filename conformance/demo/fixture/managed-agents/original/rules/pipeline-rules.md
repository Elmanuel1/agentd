# Pipeline Rules

Rules governing the extract -> review -> retry pipeline.

## Scoring Model (prompt heuristic - not enforced by code)

- Extractor: +10 per verified entry, -25 per disproved entry
- Reviewer: +15 per successful disproval
- Net incentive: extractors are conservative (penalty >> reward), reviewers are thorough
- Scores are self-reported by agents in their output — pipeline does NOT consume them
- Only the verdict (verified/disproved/uncertain) drives pipeline behavior

## Retry Rules

- Max 1 retry per disproved/uncertain field — no infinite loops
- Extractor can return null to say "not extractable" — no penalty for honest nulls
- Retried fields get the reviewer's reason + evidence as context
- If disproved a second time: value still proposed but carries lowest confidence
- Retry reviewer must return verified or disproved only — uncertain is not valid on retry
- Vendor-grouper is excluded from retry entirely (reviewer verdicts are informational)
- Bond/insurance/requirement entries follow the same retry rules as bid fields (via bid-extractor)

## Verdict Definitions (internal - never surface to user)

- verified: evidence found in source, entry stands
- disproved: contradicted by source or not found -> enters retry loop
- uncertain: insufficient evidence -> enters retry loop

All fields are proposed values with a confidence score by the time they reach the user.

## Pipeline Outcome Labels (set during merge)

- verified: passed review on first attempt
- uncertain: reviewer found insufficient evidence, no retry (vendor-grouper items only)
- disproved: reviewer contradicted value, no retry (vendor-grouper items only)
- uncertain_retry_verified: was uncertain, retried, passed review
- uncertain_retry_disproved: was uncertain, retried, reviewer disproved the retry
- disproved_retry_verified: was disproved, retried, passed review
- disproved_retry_disproved: was disproved, retried, still disproved
- not_found: extractor returned null (not extractable)

## Merge Rules (Rust - src/merge.rs)

1. Deserialize all 4 agent outputs + 4 review outputs
2. Attach pipeline_outcome to each field/item based on review verdicts
3. Apply addenda overrides using field_type routing
4. Concatenate all entries into merged output
5. null returns marked as not_found
6. Merge does NOT compute confidence or detect conflicts

## Conflict Detection (Rust - src/conflict.rs)

- Same field with different values from multiple agents -> competing_values[]
- Non-conflicting fields pass through as-is
- Pure value comparison — no AI judgment

## Validation (Rust - src/validate.rs)

- Validate merged.json schema before confidence computation
- Check: required fields present, citation arrays non-empty for non-null values, pipeline_outcome is valid enum
- On failure: write status "error" and exit — do not proceed to confidence
