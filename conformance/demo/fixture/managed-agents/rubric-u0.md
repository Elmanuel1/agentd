# Tender extraction rubric: first upload

Each `###` heading below is **one criterion**. Grade every criterion on its own, as **PASS** or **FAIL**:
- Reason first, then decide.
- Check values against the document text (`python3 /mnt/session/uploads/extraction/bin/pages.py {document_id} --pages N`), not against what you expect a tender to say.
- For every FAIL, name the value, say what is wrong, and give the page that shows it.
- A field the documents do not state passes when it is `null`.
- The artifact satisfies the rubric only when every criterion passes.

Rules that override the text below:
- **Dates** are in the local time of the tender's location, as the documents state them, with no offset, unless the documents explicitly state another zone. Code converts them to UTC later.
- **Checklists** cover only the pre-tender process, up to bid close. Award conditions, `obligations` and `requirements` are out of scope. They stay empty and are not missing.
- **Inferred values:** a value inferred from strong evidence (a currency from HST, for example) is correct when its `review_reason` says it is inferred and from what, and its citations quote that evidence. The quote does not have to name the value itself. The flag is the right answer, not a failure.
- **Review flags:** there are no `pipeline_outcome` or confidence values; code computes confidence later. An uncertain value carries `review_reason`.

## 1. Tender fields: one criterion per field

### 1.1 `name`
- It is the name of the **project**, as the documents title it ("Reconstruction of Cobden Street").
- It is not a document type ("Schedule of Approximate Quantities", "Instructions to Bidders", "Form of Tender", "Addendum No. 2").
- It is not the owner's name.
- If several titles are stated, it is the title on the cover or front page of the main tender document.
- It is cited to the page that states that title.

### 1.2 `reference_number`
- It is the owner's tender or contract number, exactly as written, including prefixes, suffixes, hyphens, spaces and `#` ("RFT-2025-155", "T25-231", "#2025-108").
- It is not a project or file number of the consultant, unless that is the only number stated.
- Not stated means `null`.

### 1.3 `location`
- It is where the work is built: the site's street, road, intersection or area, plus the municipality and province when stated.
- It is never the address of the owner, consultant, or bid delivery office.
- Address parts go in the right fields: `address`, `city`, `state_or_province`, `postal_code`, `country`. A part that isn't stated is `null`, never guessed.
- It is cited to the page naming the site.

### 1.4 `scope_of_work`
- It states the work to be built and where, in the documents' terms (e.g. "full road reconstruction including watermain, sanitary and storm sewer replacement").
- It names no item the documents don't include.
- It is a summary, not a copied paragraph, and it is cited to the scope or description clause.
- *Judged criterion:* graded by a reviewer or model grader, not by exact match.

### 1.5 `delivery_method`
- It is exactly one of `lump_sum`, `unit_price`, `stipulated_price`, `design_build`, `construction_management`, `cost_plus`, `time_and_materials` or `mixed`, in snake_case.
- It matches how the documents say the work is priced: a unit-price schedule is `unit_price`; one lump sum is `lump_sum` or `stipulated_price`, as stated; unit and lump items together are `mixed`.
- Not stated means `null`. It is never a human label ("Unit Price").

### 1.6 `currency`
- It is the ISO code of the currency bids are priced in (`CAD`, `USD`).
- **Stated:** a page names the currency ("Canadian funds", "CAD", "US dollars"). It is that code, cited to those words.
- **Inferred from strong evidence:** the documents print only "$", but tie the tender to one country's money: its sales tax (HST, GST, PST), a provincial or state law, licence or workers' compensation board, or a bond or deposit stated in that money ("lawful money of Canada"). It is that country's code, cited to the words that tie it, with `review_reason` saying it is inferred and from what.
- **Otherwise `null`.** A "$" alone, or the owner's or site's country alone, is not strong evidence.

### 1.7 `timezone`
- It is the IANA zone of the tender's local time (`America/Toronto`, `America/Winnipeg`, `America/Vancouver`).
- It comes from a stated zone ("EDT", "Central Time") or from the site's location.
- *Edge:* a document stating "local time" with the site known gives the site's zone.

### 1.8 `closing_date`
- It is the date **and time** bids are due, as the main documents state it, in the tender's local time with no offset (2:00 PM on 2025-05-07 in Timmins is `2025-05-07T14:00:00`).
- *Edge:* when the documents state an odd-second time like "2:00:59 PM", the stated time is kept as written.
- *Edge:* a date with no time keeps the date only, with no invented time.
- *Edge:* when the main documents state a closing date and an addendum changes it, the first extraction holds the main documents' date.

### 1.9 `inquiry_deadline`
- It is the last date-time for questions, under the same rules as `closing_date`.
- Not stated means `null`.

### 1.10 `start_date`
- It is the stated date work may or must start, converted to UTC. A date with no time stays date-only.
- *Edge:* "upon award" or "within 10 days of the contract" is not a date: the value is the stated wording ("Within seven (7) days of the date of the Tender Acceptance"), never a computed date.

### 1.11 `completion_date`
- It is the stated calendar completion date.
- *Edge:* a duration ("60 working days", "by substantial performance plus 30 days") is **not** turned into a calendar date: the value is the stated wording.

### 1.12 `submission_method`
- It is how bids are submitted, as stated (e.g. "electronic through the bids&tenders portal", "sealed envelope delivered to …").
- Not stated means `null`.

### 1.13 `submission_url`
- It is the exact URL bids are submitted to, only when one is stated.
- It is never built from the owner's name.

### 1.14 `liquidated_damages`
- `amount` is the stated number, with no symbols or commas (`500`).
- `currency` follows 1.6: stated, or inferred from strong evidence and flagged with `review_reason`; otherwise `null`.
- `unit` is `per_day`, `per_week` or `per_month`, as stated.
- `description` is the clause text as written.
- *Edge:* a percentage or capped amount is kept in `description`; `amount` is not computed.
- No liquidated damages clause means `null`.

### 1.15 `bid_validity_days`
- It is the number of days a bid stays irrevocable after closing, as an integer ("90 calendar days" becomes `90`).
- *Edge:* "60 working days" stays `60`, with "working days" kept in the citation; it is not converted.
- Not stated means `null`.

### 1.16 `taxes`
- `treatment` is one of `included`, `excluded` or `partly_included`, matching the stated clause ("prices shall exclude HST" is `excluded`).
- `taxes` lists the taxes as named (HST, GST, PST).
- `description` restates the clause.
- Not stated means `null`.

### 1.17 `contract_form`
- It is the standard contract form or general conditions with edition, as named ("CCDC 2 – 2020", "OPSS General Conditions, November 2023").
- Not stated means `null`.

### 1.18 `prevailing_wage`
- It is the wage schedule the contractor must pay, as stated (fair wage policy, prevailing wage, union rates).
- Not stated means `null`.

### 1.19 `substitution_deadline`
- It is the date, or the documents' own wording ("10 days before closing"), by which equal or substitute requests must be in.
- A date follows the date rules above. Not stated means `null`.

---

## 2. Lists: one criterion per list
Each list passes when every entry the documents state is there, one item per entry, each right, and nothing is invented.

### 2.1 `bonds`: one entry per bond the documents require
- `type` is exactly one of `bid_bond`, `performance_bond`, `labour_material_bond` or `maintenance_bond`.
- `percentage` is set when the bond is a percentage ("10% of the bid price" gives `10`), otherwise `null`.
- `amount` is set when it is a fixed amount ("$50,000.00" gives `50000`), otherwise `null`.
- `percentage` and `amount` both have a value only when both are stated.
- `currency` is set when an amount is stated, by the rule in 1.6: stated, or inferred from strong evidence and flagged with `review_reason`; otherwise `null`.
- `conditions` lists stated conditions (e.g. "issued by a surety licensed in Ontario", "valid for 90 days").
- `description` is one plain sentence saying who must provide what.
- *Edge:* a certified cheque or bank draft accepted **instead of** a bid bond is kept in `conditions`, not as a separate bond.
- *Edge:* a bond required only above a contract value is kept with that condition.

### 2.2 `insurance`: one entry per required policy
- `type` is exactly one of these, chosen by the policy's own name or heading:
  - `cgl`: commercial general liability (IBC 2100; *responsabilité civile générale*). Pollution cover inside the CGL policy ("hostile fire", "sudden and accidental") stays here.
  - `auto_liability`: owned, leased or hired vehicles, including certificates from a government insurer (ICBC, SGI, MPI, SAAQ; *RC automobile*).
  - `non_owned_auto`: non-owned automobile liability (SPF No. 6, OPCF 6, *formule des non-propriétaires*). One sentence requiring auto liability "for owned and non-owned" vehicles is two entries: `auto_liability` and `non_owned_auto`.
  - `umbrella`: umbrella or excess liability above other policies (*RC « umbrella »*).
  - `wrap_up_liability`: a project wrap-up liability policy: wrap-up, OCIP, CCIP or project-specific liability (*RC « wrap-up »*).
  - `professional_liability`: professional liability, including one policy named "professional liability (errors and omissions)" (*RC professionnelle*).
  - `errors_omissions`: errors and omissions insurance named as its own policy.
  - `pollution_liability`: contractor's pollution liability or environmental impairment, as its own policy or endorsement (*RC environnementale*).
  - `builders_risk`: broad form property on the work: builder's risk, course of construction, all-risk construction (*tous risques de chantier*, *assurance des chantiers*). One policy named for both builder's risk and an installation floater is `builders_risk`.
  - `installation_floater`: materials and equipment to be installed in the work, while stored, in transit or being installed (*risques d'installation*).
  - `equipment_floater`: the contractor's own or rented construction equipment, machinery and tools (contractor's equipment, all-risks; *police flottante pour le matériel*).
  - `boiler_machinery`: boiler and machinery or equipment breakdown, including testing and commissioning.
  - `aircraft_watercraft`: liability for owned or non-owned manned aircraft or watercraft, including aircraft passenger hazard.
  - `unmanned_aircraft`: drone or unmanned aerial vehicle liability.
  - `crane_liability`: hook, on-hook, crane, hoist or riggers liability: property while it is lifted or rigged.
  - `railway_protective`: railway or railroad protective liability for work on or near a railway.
  - `owners_contractors_protective`: owner's and contractor's protective liability (*RC indirecte du propriétaire ou de l'entrepreneur*).
  - `property`: property insurance on the contractor's own property or property in its care, not on the work (*assurance des biens*).
  - `cyber_liability`: cyber or privacy liability.
  - `workers_comp`: a workers' compensation **policy** with a stated coverage limit. A WSIB, WorkSafeBC, WCB or CNESST clearance certificate, or proof of good standing, is never insurance: it is a `requirements` entry, category `compliance` (2.3).
  - `other`: a required policy none of these types describes. `description` names it as printed, and `review_reason` says no type fits.
- A policy forced into a type it is not fails. A policy filed as `other` and flagged passes.
- `minimum_coverage` is the stated limit as a number (`5000000`).
- *Edge:* per-occurrence and aggregate limits are both stated: `minimum_coverage` is the per-occurrence limit, and the aggregate goes in `conditions`.
- `currency` is set by the rule in 1.6: stated, or inferred from strong evidence and flagged with `review_reason`; otherwise `null`.
- `certificate_required` is `true` only when the documents require a certificate.
- `conditions` lists named additional insureds, cross-liability and similar terms.
- *Edge:* pollution coverage is never filed under `cgl`, and non-owned auto never under `auto_liability`. Each has its own type.

### 2.4 `conditions`: rules about bidding itself, up to bid close
- In this run only `eligibility`, `bidding_period` and `bid_submission`; an `award` condition is out of scope.
- `applies_at` holds every stage it applies at, at least one, from:
  - `eligibility`: what the bidder must be or hold before bidding;
  - `bid_submission`: what the bid must contain or how it is submitted;
  - `bidding_period`: what happens between issue and close;
  - `award`: how the winner is chosen.
- `description` is one plain sentence saying who must do what and by when.
- Examples:
  - "acknowledge all addenda in the bid form" (`bid_submission`);
  - "award to the lowest compliant bid" (`award`);
  - "only registered plan takers may bid" (`eligibility`);
  - "questions must be submitted 7 days before close" (`bidding_period`).

### 2.5 `events`: one entry per site visit or pre-bid meeting
- `type` is `site_visit` or `pre_bid_meeting`.
  - `pre_bid_meeting`: any meeting held for bidders before close, on site or not ("site meeting", "pre-tender meeting", "bidders' tour").
  - `site_visit`: only an unaccompanied or self-guided visit of the site.
- `mandatory` is `true` only when stated as mandatory or compulsory; otherwise `false`.
- `date` is the date-time converted to UTC, under the date rules.
- `location` is the meeting place as stated.
- `details` holds registration or attendance notes.
- *Edge:* "no site meeting will be held" gives no event. A mention of it may be a condition.

### 2.6 `parties`: one entry per named party
- `role` is exactly one of `owner`, `consultant`, `designer`, `contractor`, `subcontractor`, `surety` or `legal`.
- `name` is the organization as written.
- `contact_person`, `phone`, `email` and `address` are set when stated, and `null` otherwise.
- The owner and the consultant/engineer are separate entries.

### 2.8 `alternates`: prices asked for beside the base bid
- `kind` is `alternate` (an add or deduct option) or `separate_price`.
- It is never a line of the base-bid schedule.

### 2.9 `price_adjustments`: cost-index clauses
- `material` is named as in the documents (asphalt cement, fuel, steel).
- `description` states the index and the trigger.

### 2.10 `separate_contracts`: parts of the work bid or awarded separately
- One entry per part, with `name` as stated. The list is empty when the tender is one contract.

### 2.11 `participation_goals`: DBE, MBE, WBE, local, Indigenous or social procurement goals
- `program` is named as stated.
- `percent` is the stated number, or `null`.

---

## 3. Schedule, packages and hard failures
### 3. Schedule of prices: one criterion per line

Every line of every price schedule in the main documents:
- **Present once:** the line exists exactly once in the result.
- **`item_id`:** the number the row prints, exactly as written ("4.1", "B-12", "RC-4.6", "1.5.1 S", "6)"), with nothing added: no schedule name, table name or label. Sibling schedules that restart their numbering keep their repeated numbers, told apart by `scope_label`. A sub-row is the parent's number, a space and the sub-row's own first printed cell, whether that is a mark or a label: "B-3 MH-2", "2.4 Rs-1 Stop sign", "C-7 Catch basin". A row that prints nothing in its first cell and has no parent number keeps `item_id` "" with a `review_reason`. There is no `numbered_by_position` field at this stage; do not ask for one.
- **Paired by the row, not the number:** a line is found by the schedule row it cites; its `item_id` is a criterion of its own (section 3b of the report).
- **`description`:** means the same as the line's description, with no added or dropped scope words (material, size, method). A sub-row's description starts with its parent heading's words ("Payment for Testing - Concrete cylinders"); that is correct, not an altered description.
- **`quantity`:** the stated number, with no commas (`1250.5`).
- **`unit`:** exactly one of the schema's snake_case units:
  - `square_meters`, `square_feet`, `square_yards`;
  - `cubic_meters`, `cubic_feet`, `cubic_yards`;
  - `meters`, `feet`, `inches`;
  - `kg`, `lb`, `tonne`;
  - `liters`, `gallons`;
  - `each`, `set`, `lot`, `lump_sum`;
  - `hours`, `days`, `weeks`, `months`;
  - `percent`.

  Each maps from the stated unit: "m" or "lin.m" gives `meters`, "m²" gives `square_meters`, "m³" gives `cubic_meters`, "L.S." gives `lump_sum`, "ea" gives `each`, "t" gives `tonne`.
  - *Edge:* when the printed unit fits none of these (for example "cup", "-" or a blank cell), `unit` is `null` and the line's `review_reason` says what is printed. That passes: a null unit flagged this way is the right answer, not a missing value. A unit forced into an allowed value that the row does not print fails.
- **`priced_by`:** `owner` when the owner has filled in the price (a provisional sum, allowance or contingency), and `bidder` otherwise.
- **`owner_unit_price` / `owner_total`:** the owner's stated figures, as numbers, only when `priced_by` is `owner`; otherwise `null`.
- **Edges:**
  - A line split across two pages or two table rows is one line.
  - A subtotal, total or tax row is **not** a line.
  - A section heading row is **not** a line.
  - Two schedules (onsite/offsite, Part A/Part B) keep their lines separate, with `scope_label` set.
  - A **Schedule of Additional Unit Prices** (also called provisional or contingent unit rates) is a schedule of lines too. That includes rates for extra work, labour by the hour and equipment by the hour. Every row is a line with its own `scope_label`, and its quantity is `null` when no quantity is stated.
  - A line repeated in two schedules with different item numbers counts as two lines.
  - A quantity stated as "1" with unit "L.S." gives quantity `1`, unit `lump_sum`.
  - **Task and fee lines:** a line priced as one sum with no stated quantity or unit ("Task 3 – Design Development", "L.S." with no quantity) is quantity `1`, unit `lump_sum`. Every row of the price form is a line, so no task is skipped.
  - A line with no quantity ("rate only") gives quantity `null`.

### 4. Packages
- Every line is in exactly one package.
- No package is empty.
- `group_type` is `subcontractor`, `supplier` or `null`.
- `section_ref` is the document's own section reference when stated.
- *Ignored:* package names, how many packages there are, and `masterformat_code` (not graded until a separate criterion exists).

---

### 5. Fails whatever else is right

Each of these is its own criterion, checked across the whole result:
1. A value is set for a field the documents don't state.
2. A schedule line is not in any schedule.
3. Two lines are merged into one, or one line is split into two.
4. A line is in two packages, or in none.
5. A quantity, unit price or total is computed or guessed, not read.
6. A date comes from somewhere other than the documents.
7. A quantity, dimension or scope is read from a drawing.
8. An enum field holds a value outside its list, or a human label.
9. Any citation fails a rule in section 9.

---

### Review flags
- A value that is wrong, inferred, or read from garbled text carries `review_reason`, one sentence saying why.
- A value the page states plainly has no `review_reason`.

## 4. Citations: one criterion per rule

Every value, entry, line and change carries citations. Each citation must pass every rule below, and each rule is its own criterion.

### C1. Present. At least one citation per value, entry, line and change. A value with none is `no_citation`.

### C2. Verbatim. `text_snippet` is copied exactly from the page.
- Matching is lowercase, with any run of punctuation or spacing treated as one space.
- Words are never reworded, shortened, completed or reordered.

### C3. Locatable. The snippet is found on the page as consecutive text.
- In a value check, it is found within 8 consecutive lines.
- In a change, it is found exactly, within 40 lines, in line order or table-row order.

### C4. Right page. `page_number` is the page the snippet is found on. A snippet found only on another page is moved to that page, and the value is not passed on the original page.

### C5. Supports the value. The quoted text itself states the value, not just text near it. Read the quote together with the surrounding page text.
- For conditions, obligations and requirements, the quote alone must state the entry; otherwise `weak_citation`.

### C6. Shortest span. The quote is the shortest span that states the value: one or two sentences, not a whole clause or page.

### C7. Tables. A table row is quoted as the page gives it, cells left to right joined by ` | `, item number first when the row reads that way.

### C8. Numbers in place. Every number stays where the page puts it, including a clause number on its own line.
