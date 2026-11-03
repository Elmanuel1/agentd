---
name: extracting-bid-information
description: Extracts structured bid information from construction tender documents. Use when reading tender/bid documents to identify project name, dates, submission requirements, and other bid-level fields.
---

# Extracting Bid Information

<task>
  Extract structured fields from the main tender document. Read the `blocks.json` file at the path specified in your prompt and produce output conforming to the bid-extractor schema in `.claude/schemas/bid-extractor.schema.json`.

  ## Where to Look

  Tender documents follow common structures. Search these sections first:

  - **Instructions to Bidders / Invitation to Bid** — closing date, submission method, submission URL
  - **General Conditions / Supplementary Conditions** — liquidated damages, conditions
  - **Form of Tender / Bid Form** — reference number, project name, delivery method
  - **Notice to Contractors** — location, scope, start/completion dates, tender meeting

  ## Field-Specific Guidance

  **Name** — the name of the *project*, not of the document you are reading.

  A document's own heading is a document type, never a project name. `SCHEDULE OF APPROXIMATE QUANTITIES`, `FORM OF TENDER`, `ADDENDUM NO. 2`, `INSTRUCTIONS TO BIDDERS` describe what the page is, the same way "Invoice" is not a company name. Never return one.

  In order of preference:
  1. **A stated project or contract title** — `Renovation of City Hall Entrance`, `Contract 2024-17 — Lapointe Drive Extension`. Usually on a cover page, Form of Tender or Notice to Contractors.
  2. **The site address**, when no title is stated — `4499 31st Street, Vernon`. Civil projects are routinely identified by address alone, and a schedule of quantities often carries the address and nothing else. A tender's cover page and Form of Tender almost always state a title (`CONSTRUCTION OF 1200MM SANITARY SEWER AT …`): read them before settling for the address.
  3. **`null`** — when the document states neither.

  Do not compose a name from parts the document does not present together: no year prefixes, no engineer's name, no "<address> Phase 1" unless the document says so. The citation must point at a block that actually contains the name you return, and a composed string has no such block.

  A tender holding only a schedule of quantities usually has no project name in it. Returning `null` is correct there — the user is shown the field and asked to confirm it. A wrong name is worse than an empty one, because it becomes the subject line of RFQs sent to subcontractors.

  **Timezone** — extract the IANA timezone name (e.g., `America/Toronto`) based on the project location. This is a separate field — do NOT append timezone offsets to date values. Common Canadian timezones:
  - Ontario/Quebec → `America/Toronto`
  - Manitoba → `America/Winnipeg`
  - Saskatchewan → `America/Regina`
  - Alberta → `America/Edmonton`
  - British Columbia → `America/Vancouver`
  - Atlantic provinces → `America/Halifax`

  When the timezone is not stated explicitly (document says "local time" only), infer it from the project city/province and **cite the block that contains the city or province** — not a date or time block. Do NOT cite an unrelated block hoping it mentions a timezone.

  **Dates** — use ISO 8601 format WITHOUT timezone offset (timezone is a separate field). Watch for:
  - "2:00 PM local time" → `2026-01-20T14:00:00` (no offset, timezone field handles it)
  - "Fall 2025" or "TBD" → return `null`, not a guess
  - A calendar date the document states in words is still a date, returned in ISO 8601: "April 3, 2025 at 2:00 PM" → `2025-04-03T14:00:00`; "on or before October 3, 2025" → `2025-10-03`. Words around a stated date ("on or before", "no later than") do not make it relative.
  - Relative dates like "Within 10 days of notice to proceed" or "60 calendar days from award" → return as a string, do not convert to null. These are valid start_date/completion_date values: only a date stated relative to an event, with no calendar date, is returned as words.
  - A deadline stated relative to another ("at least four working days prior to closing") → return the document's own wording. Never work out a date from it, and never add a time the document does not state.

  **Delivery method** — decide from the price schedule, not from one sentence. The schedule's rows show how the contractor is paid: every row a lump sum (LS) → `lump_sum`; every row a unit rate times a quantity (m, m³, each, tonnes, hours) → `unit_price`; some rows lump sum and others unit rates → `mixed`, the usual case for a civil schedule of quantities. Instructions saying items "supplied on a unit price basis" are measured, and a bond "for 50% of the total lump sum price", do not decide it on their own. Cite the schedule rows that show it.

  **Events** — tender meetings, site visits, question deadlines. Any meeting held for bidders before close, on site or not ("site meeting", "pre-tender meeting", "bidders' tour"), is `pre_bid_meeting`; `site_visit` is only an unaccompanied or self-guided visit of the site. "No site meeting will be held" is no event. Each has a name, date, location, and whether mandatory, and `citations` quoting the text that announces it.

  **Currency** — the ISO code of the money bids are priced in.
  - When a page states it ("Canadian funds", "CAD", "US dollars"), use that code and cite those words.
  - When the documents print only "$" but tie the tender to one country's money, use that country's code: its sales tax (HST, GST, PST), a provincial or state law, licence or workers' compensation board, or a bond or deposit stated in that money ("lawful money of Canada"). Cite the words that tie it, and set `review_reason` to say it is inferred and from what.
  - Otherwise return `null`. A "$" alone, or the project's country alone, is not enough.
  - The same currency goes on every bond, insurance and liquidated-damages amount, by the same rule.

  **Submission URL** — always include the full URL with protocol (https://). If the source only says "www.example.com", prepend "https://".

  **Scope of work** — summarize in 1-2 sentences. Do not copy entire specification sections.

  **Liquidated damages** — return as an object with exactly these fields: `amount` (number), `currency` (string, e.g. "CAD"), `unit` (one of `"per_day"` / `"per_week"` / `"per_month"`), `description` (string). Include the rate, currency, unit, and any conditions or caps mentioned.
    - ❌ NEVER invent field names like `amount_per_day`, `daily_rate`, `penalty_per_day` — use `amount` + `unit: "per_day"`

  **Parties** — return as an array of objects: `[{"role": "owner", "name": "...", "address": "..."}, {"role": "consultant", "name": "...", "address": "..."}]`. Include owner, engineer/architect, and any other named parties. Every tender has an owner — the agency, municipality or company issuing it, named on the cover page — so an empty list is almost always wrong. Add the engineer or consultant whenever the document names one: the engineering firm or person the document calls the Consultant, Engineer or Contract Administrator. Never list the owner a second time under another role. A party's `name` is the organization's or person's own name ("Regional Municipality of Niagara", "EXP Services Inc."), never a role word: "Contract Administrator", "Prime Contractor", "the Owner" and "the Consultant" name no one, so they are not parties. Each party has `citations` quoting the text that names it.

  **Written for review** — every `description` is read by a person reviewing the tender, so write it as one plain sentence in sentence case: who must do what, and by when. Do not paste the page: no ALL CAPS, clause numbers, "see section" references or cut-off text. The citation's `text_snippet` keeps the exact words from the page.
  - Page: `CP PERMIT REQUIRED BEFORE BEGINNING ANY WORK WITHIN CP'S RIGHT-OF-WAY.` → description: "Obtain a CP Rail permit before starting any work within CP's right-of-way."

  **Conditions, obligations, requirements** — see the extracting-tender-checklists skill.

  **Pricing terms** — what changes a bidder's price or risk. Each is cited like any other value; return `null` or an empty list when the document does not state it.
  - `bid_validity_days` — how many days the bid stays irrevocable after closing, as a number ("irrevocable for 90 days" → 90). The condition itself also goes in `conditions` at `bidding_period`.
  - `taxes` — whether bid prices include sales taxes: `included`, `excluded` or `partly_included`, the taxes named (GST, HST, PST, sales tax), and the sentence that says so. The treatment is about the sales taxes named, not duties or other charges: "inclusive of all duties and taxes except HST, which is itemized separately" is `excluded`, taxes HST. `partly_included` is for one sales tax included and another excluded.
  - `contract_form` — the standard contract or general conditions, with edition: "CCDC 2 2020", "MMCD 2009 Platinum Edition", "AIA A101-2017", "OPSS.MUNI 100 November 2018". The edition is often on another page than the one naming the form (the general conditions clause): find it and include it. Say when supplementary conditions amend it.
  - `prevailing_wage` — a wage schedule the contractor must pay (prevailing wage, Davis-Bacon, fair wage policy).
  - `substitution_deadline` — when requests to use a substitute or equal product must be in.
  - `alternates` — prices asked for beside the base bid: `alternate` (an add or deduct option the owner may accept) or `separate_price` (a price for part of the work asked separately). Not a base-bid line of the schedule, and not an owner allowance or provisional sum.
  - `price_adjustments` — clauses that adjust the price with a cost index, one per material (fuel, asphalt cement, steel).
  - `separate_contracts` — separate contracts, lots or parts bid or awarded on their own. Empty for a single contract.
  - `participation_goals` — DBE, MBE, WBE, local hiring, Indigenous or social procurement goals, with the percentage when stated.

  ## Before you answer: audit what you missed

  Go back over the pages you were given and check each field in turn, not only the ones you filled:
  1. **Every field is either filled with a citation or null because no page states it.** Look again for the ones you left null: the name (cover page, form of tender), the closing date and time, the questions deadline, the start and completion dates, liquidated damages, the submission method and address.
  2. **Every pricing term.** Bid validity in days, taxes, contract form, prevailing wage, substitution deadline, alternates and separate prices, price adjustments, separate contracts, participation goals.
  3. **Lists.** Every party the document names by role (owner, consultant, designer, surety), every site visit or pre-bid meeting.
  4. **Each description** is a plain sentence, not a pasted clause.

  Fill what you missed, then answer.

  ## What NOT to Extract

  - **Addenda changes** — handled by addenda-processor
  - **Schedule of quantities / line items** — handled by vendor-grouper
</task>

<rule>
  - Extract ONLY from the main document sections — skip addenda sections
  - If a field is ambiguous or contradicted across sections, extract the value from the most authoritative section (instructions > general conditions) and note the conflict
  - If a field is genuinely not present, return `{"value": null, "citations": []}` — never guess
  - Dates use ISO 8601 without timezone offset — timezone is a separate IANA name field
  - liquidated_damages MUST use `amount` + `unit` fields — never `amount_per_day`, `daily_rate`, or similar invented names
  - Every field needs a citation via the citing-sources skill
</rule>
