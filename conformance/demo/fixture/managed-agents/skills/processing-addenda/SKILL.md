---
name: processing-addenda
description: Extracts changes from construction tender addenda documents. Use when identifying what an addendum modifies — dates, bonds, insurance, quantities, scope, or any other aspect of the original tender. Produces structured overrides that supersede the main document.
---

# Processing Addenda

Extract all changes introduced by addenda in `/workspace/blocks.json`. Produce output conforming to the addenda-processor schema in `.claude/rules/output-schemas.md`.

## What Are Addenda

Addenda are formal amendments issued after the original tender document. They legally supersede the original. An addendum can change anything — closing dates, bond requirements, insurance minimums, line item quantities, scope of work, submission methods, or contractual conditions.

## How to Find Addenda

Look for sections headed:
- "Addendum No. 1", "Addendum #2", "Addenda"
- "Amendment", "Revision", "Errata", "Corrigendum"
- Usually at the end of the document or as separate attached pages
- May reference the original section/clause being changed

## For Each Addendum

1. Record the addendum number and date
2. For each change in the addendum:
   - Record the original value (if stated) and the revised value
   - Cite the addendum text, not the original document
3. If the addendum introduces entirely new requirements not in the original, those are still changes (original_value = null)

## field_type Routing (CRITICAL)

The `field_type` determines where each change is routed during merge. Use the correct type:

| Change affects | field_type | field_name |
|---|---|---|
| Bid dates, submission, parties, etc. | `"date_time"`, `"string"`, `"number"`, etc. | Must be a valid TenderFieldName: name, reference_number, location, scope_of_work, delivery_method, currency, timezone, closing_date, events, start_date, completion_date, inquiry_deadline, submission_method, submission_url, conditions, parties, liquidated_damages |
| Schedule of quantities line items | `"line_item"` | The ORIGINAL item_id as it appears in the schedule of quantities (e.g., "RC-1.5", "RS-4.6"). Must match exactly — use the full prefixed ID, not a shortened version. If the addendum references "Item 4.6", look up the full ID from the schedule (e.g., "RC-4.6"). |
| Bond requirements | `"bond"` | Bond type: bid_bond, performance_bond, labour_material_bond, maintenance_bond |
| Insurance requirements | `"insurance"` | Insurance type: cgl, auto_liability, non_owned_auto, umbrella, wrap_up_liability, professional_liability, errors_omissions, pollution_liability, builders_risk, installation_floater, equipment_floater, boiler_machinery, aircraft_watercraft, unmanned_aircraft, crane_liability, railway_protective, owners_contractors_protective, property, cyber_liability, workers_comp, other |
| Other requirements | `"requirement"` | Category: warranty, safety, compliance, holdback, payment_terms |

**Item ID matching (CRITICAL for line_item changes):**
- The `field_name` for `line_item` changes MUST use the exact item_id from the schedule of quantities
- Schedules often prefix items with section codes: "RC-1.5" (Richard Crescent item 1.5), "RS-4.6" (River Street item 4.6)
- If the addendum says "Tender Item 4.6", you must look up the full ID in the schedule — it could be "RC-4.6" or "RS-4.6"
- NEVER use shortened IDs like "4.6" or invented IDs like "EZ-storm-SWM-vents" — always use the original schedule item_id
- If a change cannot be matched to a specific schedule item, use `field_type: "requirement"` with a valid category instead

**Common mistakes to avoid:**
- Specification changes to quantities/materials (e.g., "change watermain from 200mm to 150mm") → `field_type: "line_item"`, NOT `"number"` or `"requirement"`
- New technical requirements (e.g., "add elbow restraints") → `field_type: "line_item"` if it modifies a schedule item, or `field_type: "requirement"` with a valid category if it's a general requirement
- Do NOT invent field_names outside the allowed enums — if a change doesn't fit any routing target, use `field_type: "requirement"` with a valid category

## Rules

- Extract ALL changes — do not skip minor ones
- Later addenda supersede earlier ones (Addendum 3 > Addendum 2 > Addendum 1)
- If no addenda exist in the document, return an empty addenda array
- Every change needs a citation via the citing-sources skill

## Before you answer: audit what you missed

Go back over each addendum and check that every numbered change, clarification and answer to a bidder question that changes a tender value (a date, bond, insurance, quantity, scope, submission rule) is an override. Add what you missed, then answer.
