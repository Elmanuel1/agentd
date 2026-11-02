# Output Schema Rules

These schemas are mandatory. Every agent MUST produce output conforming exactly to its schema.
Do NOT add extra fields. Do NOT omit required fields. Do NOT change field names.

## Shared Citation Type

Every citation across all agents uses this exact shape:

{
  "block_id": "string (uuid — block_id from blocks.json)",
  "page_number": integer,
  "text_snippet": "string (exact substring from block's content field)",
  "bounding_box": { "left": float, "top": float, "width": float, "height": float }
}

## bid-extractor Output

{
  "source_agent": "bid-extractor",
  "bid_information": [
    {
      "field_name": "string (from allowed enum)",
      "field_type": "string",
      "proposed_value": any | null,
      "explanation": "string (why this value was extracted)",
      "citations": [ Citation ]
    }
  ],
  "bonds": [
    {
      "type": "string (bid_bond | performance_bond | labour_material_bond | maintenance_bond)",
      "percentage": number | null,
      "amount": number | null,
      "description": "string",
      "conditions": ["string"],
      "explanation": "string (source reference and context)",
      "citations": [ Citation ]
    }
  ],
  "insurance": [
    {
      "type": "string (cgl | auto_liability | non_owned_auto | professional_liability | errors_omissions | pollution_liability | builders_risk | installation_floater | equipment_floater | workers_comp | umbrella)",
      "minimum_coverage": number | null,
      "currency": "string | null",
      "description": "string",
      "certificate_required": boolean,
      "conditions": ["string"],
      "explanation": "string (source reference and context)",
      "citations": [ Citation ]
    }
  ],
  "requirements": [
    {
      "category": "string (warranty | safety | compliance | holdback | payment_terms)",
      "description": "string",
      "explanation": "string (source reference and context)",
      "citations": [ Citation ]
    }
  ]
}

Allowed field_name values: name, reference_number, location, scope_of_work,
delivery_method, currency, timezone, closing_date, events, start_date, completion_date,
inquiry_deadline, submission_method, submission_url, conditions, parties,
liquidated_damages.

Note: bonds, insurance, and requirements are extracted by bid-extractor (not a separate agent).

## vendor-grouper Output

{
  "source_agent": "vendor-grouper",
  "vendor_groups": [
    {
      "group_name": "string (from predefined groups or 'ungrouped')",
      "group_type": "subcontractor" | "supplier" | null,
      "explanation": "string (why these items belong to this group)",
      "items": [
        {
          "item_id": "string (the number the row prints, exactly, e.g. '4.1', 'B-12', '6)'; nothing added. Empty string when the row prints none: code numbers those rows by position)",
          "description": "string",
          "quantity": number | null,
          "unit": "string | null",
          "explanation": "string (source reference for this item)",
          "citations": [ Citation ]
        }
      ]
    }
  ]
}

## addenda-processor Output

{
  "source_agent": "addenda-processor",
  "addenda": [
    {
      "addendum_number": integer,
      "addendum_date": "string (ISO 8601 date)",
      "changes": [
        {
          "field_name": "string",
          "field_type": "string (discriminator - determines merge target)",
          "original_value": any | null,
          "revised_value": any,
          "explanation": "string",
          "citations": [ Citation ]
        }
      ]
    }
  ]
}

field_type is a discriminator that routes each change to the correct agent's schema during merge:
- Standard field types (date_time, string, number, boolean, enum, object, array):
  -> routes to bid-extractor. field_name must be a TenderFieldName enum value.
- "line_item": -> routes to vendor-grouper. field_name is the item_id (e.g. "4.1", "B-12").
- "bond": -> routes to bid-extractor bonds[]. field_name is the bond type.
- "insurance": -> routes to bid-extractor insurance[]. field_name is the insurance type.
- "requirement": -> routes to bid-extractor requirements[]. field_name is the category.

## Reviewer Output (Variant 1: field-keyed)

For bid-extractor (including bonds/insurance/requirements) and addenda-processor reviews:

{
  "source_agent": "extraction-reviewer",
  "reviewed_agent": "string",
  "review_type": "field",
  "reviews": [
    {
      "field_name": "string",
      "addendum_number": integer | null,
      "verdict": "verified" | "disproved" | "uncertain",
      "reason": "string (only for disproved/uncertain)",
      "evidence": { "page_number": int, "text_snippet": "string" } | null,
      "corrected_value": any | null,
      "score_extractor": integer,
      "score_reviewer": integer
    }
  ],
  "summary": {
    "total_fields": integer,
    "verified": integer,
    "disproved": integer,
    "uncertain": integer,
    "extractor_score": integer,
    "reviewer_score": integer
  }
}

## Reviewer Output (Variant 2: item-keyed)

For vendor-grouper reviews:

{
  "source_agent": "extraction-reviewer",
  "reviewed_agent": "vendor-grouper",
  "review_type": "item",
  "reviews": [
    {
      "group_name": "string",
      "item_id": "string",
      "verdict": "verified" | "disproved" | "uncertain",
      "reason": "string (only for disproved/uncertain)",
      "evidence": { "page_number": int, "text_snippet": "string" } | null,
      "corrected_group": "string | null",
      "score_extractor": integer,
      "score_reviewer": integer
    }
  ],
  "summary": {
    "total_items": integer,
    "verified": integer,
    "disproved": integer,
    "uncertain": integer,
    "extractor_score": integer,
    "reviewer_score": integer
  }
}

## Re-extraction Output (retry)

Retry output is a single entry. Retry reviewer verdict must be verified or disproved only.

### bid-extractor retry (bid field):
{ "field_name": "string", "proposed_value": any | null, "explanation": "string", "citations": [ Citation ] }

### bid-extractor retry (bond/insurance/requirement):
{ "entry_type": "bond"|"insurance"|"requirement", "type": "string", ... remaining fields match Phase 1 shape }

### addenda-processor retry:
{ "addendum_number": integer, "field_name": "string", "field_type": "string", "original_value": any|null, "revised_value": any|null, "explanation": "string", "citations": [ Citation ] }

## Invariants

- Every output MUST include `source_agent` with the agent's name
- Every entry MUST have an `explanation` field
- Every non-null `proposed_value` MUST have at least one citation
- `null` proposed_value = "not extractable" — citations array must be empty
- Do NOT set `confidence` — computed by deterministic Rust code after the pipeline
- Do NOT invent field names — use only the allowed enum values
