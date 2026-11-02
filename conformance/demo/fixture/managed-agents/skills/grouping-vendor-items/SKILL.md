---
name: grouping-vendor-items
description: Classifies documents and extracts vendor groups — either line items from priced schedules (BOQ) or trade packages from spec-only documents. Covers document classification, extraction flows, item matching, and group assignment rules.
---

# Grouping Vendor Items

<task>
  Parse the document from the path specified in your prompt and extract vendor groups. Produce output conforming to the vendor-grouper schema in `.claude/schemas/vendor-grouper.schema.json`.

  ## Document Classification

  Scan the first ~30 blocks and sample throughout to classify:

  **Type A — Priced document**: Contains tables with quantities, units, unit prices, or dollar amounts. Look for headers like "Schedule of Quantities", "Bill of Quantities", "Schedule B", "Bid Items", "Schedule of Prices", "Form of Tender". Follow the **Priced Document Flow** below.

  **Type B — Spec-only document**: Contains specification sections, scope descriptions, trade packages, or work breakdowns but NO pricing tables. These may be organized by:
  - **CSI MasterFormat** divisions (Division 02, 03, 07, etc.) or section numbers (02 41 00, 07 50 00)
  - **NatSpec** (Australia) or **NBS** (UK) section references
  - **Work packages** (WP-01, WP-02, Scope A, Scope B)
  - **Trade headings** (Electrical, Plumbing, HVAC, Roofing)
  - **Flat trade lists** or scope-of-work sections

  Follow the **Spec-Only Document Flow** below.

  **Type C — No work scope**: Document contains no identifiable work scope at all (e.g., insurance certificates, meeting minutes, correspondence with no scope content). Return empty `vendor_groups: []`.

  ## RFQ Package Philosophy

  **Groups represent trade packages — what a single subcontractor or supplier would price in one quote.**
  The guiding question for every grouping decision is: *"Would one contractor price ALL of these items together in a single bid?"*

  - A paving contractor bids cold milling + granular base + hot-mix asphalt + pavement markings together → one "Roadworks & Paving" package
  - A concrete contractor bids curbs, gutters, walks, and cast-in-place concrete together → one "Concrete Works" package
  - An underground utilities contractor typically bids waterworks, sanitary, and storm sewers → can be one or two packages depending on project scale
  - An electrical contractor bids roadway lighting + traffic signals + control devices → one "Electrical & Lighting" package
  - A landscaping contractor bids topsoil, planting, and site furnishings together → one "Landscaping" package

  **Do NOT split a single trade into multiple groups just because the items have different MasterFormat codes.**
  A paving contractor doesn't submit four separate quotes for cold milling, granular base, hot-mix, and markings — they price it as one road-building package. Assign the most representative MasterFormat code for the package.

  ## Priced Document Flow

  1. **Find schedule tables**: Search for tables with headers like "Schedule of Quantities", "Bill of Quantities", "Schedule B", "Bid Items". In the text format, tables appear as blocks with pipe-delimited content like `[b:id] | Section | Description | Total |` and individual cell blocks like `[b:id] Mobilization/ Demobilization`.
  2. **Extract ALL line items** from every schedule table found
  3. **Group items into RFQ trade packages** (see RFQ Package Philosophy above) before populating the output. Each `vendor_group` is one package; each line item from the schedule is one entry in that group's `items` array.
  4. Populate the `items` array with every line item (item_id, description, quantity, unit)

     **Sub-items are line items.** A numbered row often has lettered sub-items, each with its own unit and quantity:
     ```
     A-4   a) CPP MH-1 - 3000mm Diameter   ea.  1.0
           b) CPP MH-2 3000mm Diameter     ea.  1.0
           c) CPP MH-3 - 3000mm Diameter   ea.  1.0
           d) CPP MH-4 - 3000mm Diameter   ea.  1.0
     ```
       - Output one item per sub-item: `A-4 a)`, `A-4 b)`, `A-4 c)`, `A-4 d)`, each with its own description, unit and quantity.
       - A sub-row's own mark is its first printed cell, whether that is a mark or a label: under `B-3`, a row printing `MH-2 | ea. | 1` is `B-3 MH-2`; under `C-7`, a row printing `Catch basin | ea. | 3` is `C-7 Catch basin`; under `2.4`, a row printing `Rs-1 Stop sign | ea. | 2` is `2.4 Rs-1 Stop sign`. Never letter sub-rows by position (`2.4a`), never repeat the mark, never join it with a hyphen, and never leave a sub-row's `item_id` blank when its parent prints a number.
       - Never merge them into one `A-4` item (for example "4 each"): a vendor prices each manhole separately.
       - The numbered row itself is an item only when it has no sub-items.

     **unit**: Use ONLY these exact snake_case values: square_meters, square_feet, meters, kg, lb, each, lot, lump_sum, hours, days, percent, etc.
       NEVER output: "Square Metres", "Linear Metres", "Each (EA)", "Pound"
       If unit is not in the list or unclear, output null

     **priced_by**: Some rows already carry the owner's price: allowances, provisional sums, contingency amounts and owner-set labour rates ("Allowance for Extra Work Ordered by the Engineer … $800,000.00", "Foreman … hrs. 50.0 … $112.97"). Bidders carry those amounts; they do not price them.
       - Set `priced_by` to "owner" and copy the owner's figures into `owner_unit_price` and `owner_total` (null for a figure the row does not show).
       - Keep the row in the package whose trade it belongs to, like any other row — never drop it. A line-painting allowance stays with traffic control; allowances with no trade go to "ungrouped".
       - Every other row: `priced_by` "bidder", `owner_unit_price` and `owner_total` null. A row with blank price columns is always "bidder", even under a heading like "Provisional Items".

  5. For each vendor group, populate the enrichment fields:
     - `masterformat_code` — assign the most representative code for the trade package. When a package spans multiple MasterFormat codes, use the primary/dominant one.
     - `section_ref` — the document's own section reference for this group (e.g. "4.1", "Schedule B"), or null if none.
     - `scope_summary` — **always populate**. 1-2 sentence description of the scope of work.

  ## Spec-Only Document Flow

  The output schema does NOT change for spec-only documents. Use the same fields — fill what you can, leave pricing fields null.

  1. **Scan the entire document** for how work is organized. Look for:
     - Section headings with division/trade names
     - Tables of contents listing specification sections
     - Scope-of-work sections describing what each trade must do
     - Work package breakdowns
     - Any structure that divides work into distinct trade responsibilities

  2. **Identify each distinct trade package**. The key question for each: *"Could a different subcontractor price and execute this scope independently?"* If yes, it is a vendor group.

  3. For each vendor group, populate the schema fields as follows:
     - `group_name` — the scope or work package name as it appears in the document
     - `group_type` — almost always "subcontractor" for spec sections describing installed work
     - `explanation` — describe the scope of work covered by this group and reference the document sections (e.g., "Covers Division 07 sections 07 50 00 through 07 70 00 — membrane roofing, flashing, and roof specialties")
     - `masterformat_code` — use the MasterFormat skills to look up the best matching code. Set to null only if truly no match exists.
     - `section_ref` — the document's own section reference for this group, in whatever format the document uses (e.g. "WP-03", "Scope A", "4.2", "07 50 00"). Set to null if no section reference exists.
     - `scope_summary` — **always populate**. A 1-2 sentence description of the scope of work for this group. What does this group need to price and execute?
     - `items` — create one item per identifiable scope section or work element:
       - `item_id` — the number the document prints for the row or section, exactly as printed
         ("07 50 00", "2.1", "B-12", "6)"). Nothing is added to it: no schedule name, table name,
         label or prefix. A tender split into sibling schedules (ONSITE and OFFSITE, Table 1 and
         Table 2) restarts its numbering, so the same `2.1` can appear twice: each stays `2.1`,
         and `scope_label` tells the schedules apart. A sub-item row is the parent's number, a
         space and the sub-item's own first printed cell, a mark or a label ("A-4 a)",
         "2.4 Rs-1 Stop sign", "C-7 Catch basin"; see step 2). When a row prints no number and has
         no numbered parent, `item_id` is an empty string with a `review_reason`; never make one up
         (no "Hourly-1", no "Task 6 - Contract Administration"). Code checks every `item_id` against the row its
         citation quotes.
       - `description` — the section title or scope description from the document
       - `quantity` — null (no pricing data)
       - `unit` — null (no pricing data)
       - `explanation` — brief note about what this section covers
       - `citations` — cite the **body content** of the section (requirements, scope description, materials list) — NOT the table of contents entry or section title heading alone. If the document has a TOC on early pages and section content on later pages, always cite the content pages, not the TOC. The text_snippet must be an actual specification requirement or scope description, never just a section number or title repeated from the TOC.

  4. **Do NOT return empty vendor_groups for spec-only documents.** If a document describes work scope — even without prices — extract the trade packages. The only reason to return empty is if the document truly contains no work scope at all (Type C above).

  **Grouping related spec sections:**
  - Sections within the same CSI division that would be executed by the same trade should be combined into one vendor group
  - Example: Sections 07 50 00 (Membrane Roofing), 07 60 00 (Flashing and Sheet Metal), 07 70 00 (Roof Specialties) all go under one "Roofing" vendor group with three items in the items array
  - Example: Sections 26 05 00 (Common Work Results for Electrical), 26 20 00 (Low-Voltage Electrical Power) go under one "Electrical" vendor group
  - When in doubt, ask: *"Would one subcontractor price both scopes in a single quote?"* If yes, combine them into one group. A concrete sub prices curbs + sidewalks + cast-in-place concrete together — that is one "Concrete Works" group, not three. An electrical sub prices conduit + wiring + fixtures together — one "Electrical" group.

  ## Matching Items to Groups

  - Match by trade/scope — "Supply and install 300mm PVC watermain" goes to the watermain group
  - A single line item belongs to exactly one group
  - When uncertain between two groups, choose the more specific one and explain why
  - Items with no clear group match go into a group named `"ungrouped"` exactly — do not rename it or add parenthetical descriptions
  - Truly ungrouped items (group_type: null) are only those with NO external vendor: Mobilization/Demobilization (GC's own cost), Bonding and Insurance (financial/administrative). These cannot be subcontractor or supplier.
  - Items that involve an external vendor showing up on site ARE classifiable: Traffic Control -> `subcontractor` (flaggers, signs, equipment on site); Materials Testing -> `subcontractor` (testing lab technicians on site). Do NOT default these to ungrouped/null.

  ## Assigning group_type

  `group_type` describes the **vendor's role on the project**, not the wording of item descriptions:

  **subcontractor** — A trade contractor who sends a crew and equipment to the site to perform work. They supply materials as part of their installed scope. Ask: *"Would this vendor show up on site with workers and machinery to perform the work?"* If yes -> `subcontractor`.
    Examples: sewer contractor, paving crew, concrete formwork crew, landscaper, CCTV inspection crew, roofer, electrician, plumber, HVAC contractor.

  **supplier** — A vendor who takes a purchase order and delivers a product or material to the site. No on-site installation labour — the GC or another subcontractor installs what they deliver. Ask: *"Does this vendor just deliver something and leave?"* If yes -> `supplier`.
    Examples:
    - Aggregate/granular material supply (quarry delivers, GC spreads and compacts)
    - Trucking and disposal (hauls material away, no installation work)
    - Pipe or fitting supply only (owner crew installs)
    - Precast concrete products delivered to site (owner installs)
    - Topsoil or fill supply (delivered, others place and compact)

  Evaluate each group independently by reading its line item descriptions (priced) or explanation (spec-only):
  - Items/scope containing "supply and install", "install", "construct", "place", or describing on-site labour -> `subcontractor`
  - Items/scope containing "supply", "furnish", "haul", "dispose of", "deliver", or describing material/product delivery with no install component -> `supplier`
  - For spec-only documents, most trade packages are `subcontractor` since specifications describe installed work

  ## Common Misclassification Traps

  - "Remove watermain" or "Remove water services" -> Removals belong to the SAME group as the replacement installation. The contractor who installs the new watermain also removes the old one:
    - Removing + installing watermain/water services -> "Underground Works — Watermain" (one group)
    - Removing + installing sanitary sewers -> "Underground Works — Sanitary Sewer" (one group)
    - Removing + installing storm sewers -> "Underground Works — Storm Drainage" (one group)
    - Removing asphalt -> assign to "Roadworks & Paving" (the paving sub removes old asphalt before paving)
    - Removing concrete (sidewalks, curbs) -> assign to "Concrete Works" (the concrete sub removes and replaces)
    - Removing gravel (driveways, surfaces) -> assign to "Site Preparation & Clearing" (clearing scope)
  - **Do NOT create a standalone "Removals" group** — removals are part of the trade that does the replacement
  - "Asbestos cement watermain" is a watermain, not a sewer — group by the pipe function, not the material
  - Curb and gutter items belong to the **same "Concrete Works" group** as sidewalks and cast-in-place concrete — a concrete sub prices all of this together, not separately from the road works paving crew
  - When an item description mentions a specific trade (watermain, sewer, electrical), that trade takes priority over the action (remove, supply, install)
  - **Manholes and catchbasins are NEVER a standalone RFQ package.** The underground contractor who installs the pipe also supplies and installs the manholes. Splitting them out creates interface gaps, separate mobilizations, and drives up total cost. Assign:
    - Catchbasins, side-inlet basins, ditch inlets → "Underground Works — Storm Drainage"
    - Sanitary manholes, inspection chambers, re-bench existing manholes → "Underground Works — Sanitary Sewer"
    - Drop manholes (Inside Drop Type) → "Underground Works — Sanitary Sewer" (always sanitary — H₂S prevention)
    - Generic manholes with no system designation: assign by size — 1050mm/1200mm → Sanitary Sewer; 1500mm+ → Storm Drainage (larger pipes, larger manholes)
    - Manhole risers → assign to the same group as the manhole they belong to

  ## Scope Label

  Each item also carries its own `scope_label`: the schedule or part it belongs to when the document has more than one ("Part A", "Schedule of Additional Unit Prices", "Section 2(ii) Storm Structures"), or null when it has one schedule.

  Return `scope_label`: the part of the project this document's schedule covers, when the document declares one. Tenders are often split into sibling documents that share a trade breakdown but cover different scopes — "ONSITE" and "OFFSITE", "Phase 1" and "Phase 2", "Building A" and "Building B". The label is usually in the schedule's heading or the document title.
  - Copy it in the document's own words, e.g. "ONSITE", "OFFSITE", "Phase 1".
  - null when the document declares no such split — most documents.
  - It is the scope of work, not the document type: "Addendum No. 2" and "Schedule of Quantities" are not scope labels.

  ## Multi-Page Tables

  Tables may break across pages. Watch for:
  - Continuation headers (same columns, no new section header)
  - Subtotals mid-table (skip these — they are not line items)
  - Section breaks within a schedule (e.g., "Section A: Civil Works", "Section B: Mechanical")

  ## MasterFormat Lookup

  After classifying the document (Type A/B/C), read **only the MasterFormat skill(s) relevant to the document's scope** — do not load all three on every call.

  | Document scope | Read this skill |
  |---|---|
  | Building or facility (divisions 02–28): concrete, masonry, metals, roofing, finishes, MEP, electrical | **masterformat-building** |
  | Civil or infrastructure (divisions 31–35): earthwork, paving, utilities, drainage, roads | **masterformat-civil** |
  | Process or industrial (divisions 40–48): process piping, treatment equipment, manufacturing | **masterformat-process** |
  | Mixed scope | Read whichever skill covers the majority; read both if genuinely split |

  Then assign `masterformat_code` to each vendor group:

  1. **If the document uses CSI MasterFormat section numbers** (e.g. `07 50 00`, `33 10 00`): use the document's codes directly. Validate each code exists in the skill table — if not, set `masterformat_code: null`.
  2. **If the document is NOT CSI MasterFormat** (NatSpec, NBS, work packages, trade lists): look up the scope description in the table and assign the closest matching code.
  3. **If uncertain** about the best match: set `masterformat_code: null`. Never force a match.
</task>

<rule>
  - Classify the document (Type A/B/C) before extracting — do not skip classification
  - Include ALL line items (priced) or ALL identifiable trade packages (spec-only) — do not skip any
  - Preserve the original description text, quantity, and unit exactly as written in priced documents
  - **`item_id` is the number the row prints, exactly**, never built or extended; an empty string when the row prints none (see step 3)
  - **A schedule row's citation is its own printed row**, copied exactly as one line of `pages.py --rows` prints it: item number first and every cell left to right, joined by ` | ` (`1.02 | A2, N-1204 | Preconstruction Survey | 1 | LS`). Never quote only the description or part of the line, and never rebuild a row from several lines or reorder its cells. A row that wraps over several printed lines is quoted by its first line only. A sub-row is cited by its own line (`Manhole | ea. | 4`); its parent's heading is not cited for it.
  - **A heading row is not an item, and neither is a subtotal, total, tax or "Total Fees" row.** A heading names the rows under it and has no quantity or price of its own. A row or column the bidder fills a price or rate into is never a heading: in an hourly rate table laid out as roles across columns ("Project Manager | Clerical | Other" over "$ /hr | $ /hr | $ /hr"), each role is an item.
  - **Rows under a heading carry its words.** When rows continue a heading row ("Removal of Existing Items / off-site Disposal" over "Existing Asphalt Roadway" and "Concrete curb"), each row's `description` starts with the heading's words: "Removal of Existing Items / off-site Disposal - Existing Asphalt Roadway". Two rows that would read alike without their heading ("Prior to 1-year Final Inspection" under "Sewers" and under "Laterals") are told apart this way.
  - Every item or group needs a citation via the citing-sources skill
  - For Lump Sum (LS) items (document shows "LS", "lump sum", or "100%" in the unit/quantity column), set `quantity: 1` and `unit: "lump_sum"`. Never leave quantity null for LS items.
  - **Task and fee lines:** a row priced as one sum with no stated quantity or unit (a fee form's "Task 3 – Design Development | $") is an item with `quantity: 1` and `unit: "lump_sum"`. Every row of the price form is an item: no task is skipped, the optional ones included.
  - **Additional unit prices:** a Schedule of Additional (provisional, contingent) Unit Prices, and its rates for extra work, labour by the hour and equipment by the hour, is a schedule of items too. Every row is an item, with `quantity: null` when no quantity is stated, and the schedule's own heading as the item's `scope_label` ("Schedule of Additional Unit Prices").
  - For spec-only documents, create items with `quantity: null` and `unit: null`
  - Only load the MasterFormat skill(s) relevant to the document's scope — do not load all three on every call
  - Return empty `vendor_groups: []` ONLY for Type C documents (no work scope at all)
  - Items with no clear group match go into a group named `"ungrouped"` exactly — do not rename it
  - Never force a masterformat_code match — set null if uncertain
  - Citations for spec-only items must point to body content, not TOC entries
  - Groups represent RFQ trade packages, NOT MasterFormat subdivisions — one group per vendor quote, not one group per division code
  - NEVER create a separate group for each MasterFormat code — if one contractor prices all of it, it is one group
  - Target ~6-12 groups for a typical civil/infrastructure BOQ — if you have 20+ groups, you are splitting too finely
</rule>

## Worked Examples

These examples show the correct RFQ-package grouping. Read them before extracting.

---

### Example 1 — Road Reconstruction BOQ (Priced Document)

**WRONG — too granular, one group per MasterFormat code:**
```
Group 1: Cold Milling (32 01 17)
  - Remove asphalt pavement, 50mm depth — 4,200 m²
Group 2: Granular Base (31 05 16)
  - Granular 'A' base course, 300mm — 4,200 m²
Group 3: Hot Mix Asphalt (32 12 16)
  - HL8 binder course — 410 tonnes
  - HL3 surface course — 205 tonnes
Group 4: Pavement Markings (32 17 23)
  - Thermoplastic centre line — 420 m
  - Stop bars — 12 each
```

**CORRECT — one RFQ package, one paving contractor prices it all:**
```
Group 1: Roadworks & Paving (32 12 16)
  - Remove asphalt pavement, 50mm depth — 4,200 m²
  - Granular 'A' base course, 300mm — 4,200 m²
  - HL8 binder course — 410 tonnes
  - HL3 surface course — 205 tonnes
  - Thermoplastic centre line — 420 m
  - Stop bars — 12 each
```
*Why: A paving contractor prices milling, granular base, asphalt, and markings as a single road-building quote. They do not submit four separate quotes.*

---

### Example 2 — Municipal Infrastructure BOQ

**WRONG — removals split into separate groups for each removed material:**
```
Group 1: Remove Watermain
Group 2: Remove Sanitary Sewer
Group 3: Remove Storm Sewer
Group 4: Install Watermain
Group 5: Install Sanitary Sewer
Group 6: Install Storm Sewer
```

**CORRECT — group by trade, removals belong to the trade that does the same pipe, manholes go with their system:**
```
Group 1: Underground Works — Watermain (33 11 00)
  - Remove existing 150mm AC watermain — 285 m
  - Supply and install 200mm PVC watermain — 312 m
  - Water service connections — 28 each
  - Hydrant assembly — 2 each
Group 2: Underground Works — Sanitary Sewer (33 31 00)
  - Remove existing 200mm clay sanitary sewer — 180 m
  - Supply and install 200mm PVC sanitary sewer — 195 m
  - Manhole 1050mm diameter (supply & install) — 4 each    ← sanitary manholes go HERE
  - Manhole riser 1050mm diameter — 6 m
  - Inside Drop Type Manhole (extra over) — 1 each         ← always sanitary
  - Sanitary service inspection chamber — 5 each
Group 3: Underground Works — Storm Drainage (33 40 00)
  - Remove existing storm sewer — 95 m
  - Supply and install 375mm HDPE storm sewer — 110 m
  - Manhole 1500mm diameter (supply & install) — 2 each    ← large manholes go HERE
  - Manhole riser 1500mm diameter — 4 m
  - Catchbasin top and side inlet — 10 each               ← catchbasins go HERE
  - Catchbasin lead 150mm PVC — 12 m
```
*Why: The underground contractor who installs the pipe also supplies and installs the manholes. A separate "Manholes" RFQ package creates interface gaps and drives up cost — you pay two mobilizations and lose the material markup the underground sub would have included. Never split manholes out.*

---

### Example 3 — Building Renovation (Spec-Only Document)

**WRONG — one group per CSI section:**
```
Group 1: 07 50 00 Membrane Roofing
Group 2: 07 60 00 Flashing and Sheet Metal
Group 3: 07 70 00 Roof Specialties and Accessories
```

**CORRECT — all roofing work in one vendor group:**
```
Group 1: Roofing (07 50 00)
  Items:
  - 07 50 00: Membrane Roofing — TPO single-ply over rigid insulation
  - 07 60 00: Flashing and Sheet Metal — parapet caps, counter-flashing
  - 07 70 00: Roof Specialties — drains, vents, skylights
```
*Why: A single roofing sub prices and installs the full roof assembly — membrane, flashing, and specialties are inseparable scopes.*

---

### Example 4 — Civil BOQ with Mixed Scopes

**Correct output for a typical 8-10 group civil road project:**
```
Group 1: General Items (01 50 00)
  - Mobilization/Demobilization, Bonding, Surveying, Traffic Control, Testing
  [group_type: null for Mob/Demob and Bonding; subcontractor for Traffic Control and Testing]

Group 2: Site Preparation & Clearing (31 10 00)
  - Tree removal, grubbing, topsoil stripping, remove gravel driveways

Group 3: Earthworks (31 20 00)
  - Roadway excavation, subgrade preparation, fill placement, compaction

Group 4: Underground — Watermain (33 11 00)
  - Remove old watermain, install new PVC/DI watermain, service connections, hydrants

Group 5: Underground — Sanitary Sewer (33 31 00)
  - Remove old sewer, install new sewer, services
  - Sanitary manholes (1050mm, 1200mm), inspection chambers, drop manholes

Group 6: Underground — Storm Drainage (33 40 00)
  - Storm sewer pipe, ditch grading
  - Storm manholes (1500mm+), catchbasins, side-inlet basins, catchbasin leads

Group 7: Concrete Works (03 30 00)
  - Curb and gutter, sidewalks, driveways, concrete medians

Group 8: Roadworks & Paving (32 12 16)
  - Cold milling, granular base, asphalt paving, pavement markings

Group 9: Landscaping & Restoration (32 90 00)
  - Topsoil, seeding, sodding, plantings, site cleanup
```
*8–9 groups covering a full road reconstruction — this is the target density for a civil BOQ.*

## Before you answer: audit what you missed

Go back over the schedule pages and check:

1. **Every row of the schedule is an item** in exactly one package, including the last rows of each page, rows that continue onto the next page, every task or fee row of a fee form, and every row of a schedule of additional unit prices.
2. **Every sub-item** (A-4 a), b), c)) is its own item, not folded into its parent.
3. **Owner-priced rows** (allowances, provisional sums, owner-set rates) are marked `priced_by: owner` with the owner's figures.
4. **Each item's unit and quantity** are the ones its own row shows.

Add what you missed, then answer.
