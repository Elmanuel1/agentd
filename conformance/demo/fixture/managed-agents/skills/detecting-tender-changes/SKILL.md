---
name: detecting-tender-changes
description: Finds what one newly uploaded tender document changes in the tender as it stands. Use when comparing an addendum, bulletin, revised schedule, revised drawing set or reissued form with the current tender, to list each update, addition and removal with an exact copy of the entry or line it changes and the quote that makes the change, including the schedule lines a specification, drawing or answer revises, and every action it asks of the bidder.
---

# Detecting Tender Changes

Compare one document with the current tender and list what it changes. Each change is one target and one new value, with the words of the document that make it.

## List every item first

Before deciding anything, list in `items` every item the document contains:

- each numbered item of the letter;
- each specification section or clause it revises or adds;
- each drawing sheet it lists;
- each question and answer;
- each instruction to bidders;
- each schedule row it changes.

Each item has its `ref` as written ("3", "Q2", "2.4", "C-101"), its `kind`, a verbatim `quote` and its `page`. Then decide each one: `outcome` is `change` or `no_change`, with a `reason`, and a change item lists in `change_indexes` the positions of the changes it makes. Every change points back to an item; no item is left undecided. Each item that adds a list entry (a requirement, condition, obligation, insurance, bond or event) adds its own entry, in that item's words: never one entry summarising several items. Several items amending the same schedule line still share that line's one update. A no_change reason says why: it restates the tender unchanged, or it changes no line, entry or field.

Code checks the list against the page: a numbered item or listed sheet missing from `items`, or an item with no outcome, comes back to you once to decide.

## The three operations

| `op` | When | `existing` | `value` |
|---|---|---|---|
| `update` | The document changes a tender field, a list entry or a schedule line | An exact copy of the entry or line (null for a tender field) | The new value |
| `add` | The document adds an entry or line the tender does not have | null | The new entry or line |
| `remove` | The document says an entry or line is deleted, cancelled, withdrawn, superseded or no longer required | An exact copy of it | null |

An item's changed work is a `vendor_groups` update of its line, and only that: an answer that says where an item's material goes or how it is done ("clean concrete is accepted at the Public Works Yard") updates the line, not an obligation as well. An item's own measurement or payment ("paid by the tonne on the weigh ticket") is the line's unit, never a `requirements` add.

An `add` is one requirement, condition or entry. A revised section, clause or answer that states several (a test, its interval, a report, a warranty, the traffic control during it) is one `add` for each, each quoting its own words; never one `add` that points to the section ("as specified in SSP N-3218").

`remove` needs the document's own words. A document that does not mention an entry leaves it as it is: absence is never a removal.

An addendum almost always changes something. Read every item of it: each numbered item, each revised specification section, each listed drawing, each question and answer, each instruction to bidders. Each one is a change unless **What is not a change** says otherwise.

## `field`

A result key:

- a tender field: `name`, `reference_number`, `location`, `scope_of_work`, `delivery_method`, `currency`, `timezone`, `closing_date`, `start_date`, `completion_date`, `inquiry_deadline`, `submission_method`, `submission_url`, `liquidated_damages`;
- a list: `bonds`, `insurance`, `requirements`, `conditions`, `events`, `parties`, `obligations`;
- `vendor_groups` for a line of the price schedule.

## `existing`: an exact copy

Copy the entry or line from **The current tender** exactly as it is shown: every key it shows, each value character for character, numbers as written (`12` is not `12.0`). Do not add keys it does not show, fix a typo, reorder words or fill in a missing value. Code finds the entry by exact equality; a copy that differs in one character is not found.

Never write an id, in `existing` or in `value`. Ids are not shown to you.

## `package`: lines only

Changes are line changes. A package itself is never added, removed or renamed: there is no change whose target is a package.

For a line, `package` is the name of the package the line is in, exactly as the current tender shows it. It tells code which line you mean when the same line is in two packages, and where an added line goes. A line for a package the tender does not have gets null; code places it. `package` is null for everything that is not a line.

Code changes every line equal to your copy of `existing`, in whichever packages hold it, and checks each against the quote and its section heading. When the document changes an item without naming a package, list the change once for each package that holds the item, each with that package's name and an exact copy of that package's line; the lines may differ in format (`1.0` in one, `1` in the other).

## Revisions that change a line: `vendor_groups` updates

A specification revision, a drawing revision, or an answer that accepts an alternative, extends or limits the scope, or changes a requirement, changes the schedule lines whose work or material it covers. Look for them actively: for every specification section, drawing sheet and answer the document gives, check the current tender's lines for ones it changes.

Each line it changes is a `vendor_groups` `update` of that line, matched by an exact copy as usual, with its `package`:

- the new value keeps `item_id`, `quantity` and `unit` unless the document states new ones;
- the description is amended to state the revised requirement in the document's own words, with the section or drawing it comes from: `"Culvert Pipe 600 mm"` becomes `"Culvert Pipe 600 mm, HDPE to ASTM F2306 (Section 33 42 13 §2.3)"`;
- every structure name, size and material the document states for the line is in the new description ("CBMH5 and CBMH7", "300mm concrete lead", "450mm PVC"): a description that says only that the line changed is not the change;
- the quote is the revision text, verbatim, with its page.

When one revision changes several lines, in any package, give one update per line. When several items of the document change the same line, give one update for the line that states them all, quoting the most specific one.

A revision or answer that changes no line and no tender field is not output.

### Drawings

A drawing page shows you only its title block (sheet number, title, revision, date). Its linework and notes are not read: never take a quantity, a dimension or a scope from a drawing. A drawing revision changes a line only as the letter, the schedule or a specification states it, usually as a list of revised sheets, each with its number, title and revision.

Update each line whose work a revised sheet covers, naming only the sheets that apply to that line, with their numbers and titles as the letter writes them, e.g. `"Catch Basin (Bulletin 2: drawings D-104 Typical Catch Basin rev 2)"`, never "the revised drawing set". Keep quantity and unit as they are, and quote the letter's line for the sheet. A sheet that applies to no line is not output.

When several documents amend the same line, each adds its own requirement to the description; they are combined, not ranked against each other.

### A revised schedule

A revised schedule of quantities is compared with the tender line by line: a line whose description, quantity or unit differs is an `update`; a line the tender does not have is an `add` in the package the schedule shows it under; a line the revision deletes is a `remove` only where it says so.

## Questions and answers: `questions_answers`

Every question the document answers is one entry in `questions_answers`, and also an item of kind `question_answer` in `items`. An entry holds:
- `number`: as written ("Q2", "3", "Question 14");
- `question` and `answer`: verbatim, as the page gives them;
- `page`;
- `effect`;
- `change_indexes`: the changes it makes;
- `reason`.

Decide the effect from what the answer says, not from the question:

| `effect` | When | Its changes |
|---|---|---|
| `changes` | It changes priced work, a tender field, or a list entry | Line updates, field updates, entry updates or removes |
| `adds_requirement` | It adds a contract term the contractor is bound by (a document to provide, a standard to meet, an insurance) | A `requirements` (or `insurance`) add |
| `adds_condition` | It adds a rule for the bid (include a cost in the price, submit a form, a rule for references or page limits) | A `conditions` add |
| `no_change` | It confirms or restates the documents ("as stated in the Instructions to Bidders", "Yes, the closing date is unchanged") | None |

Where an answer that changes priced work goes:
- **It names an item or section:** update that line.
- **It names none:** update every line whose work it changes, the same lines however the question is asked. An answer about the scope of design work changes the design tasks that do that work, never a line chosen at random.
- **It also adds a rule for the bid:** it has both the line updates and a `conditions` add, all in its `change_indexes`.
- **It says something is not required across the contract** ("concrete collars are not required", "settlement joints are not required"): one `conditions` add stating it, effect `adds_condition`. A line is updated too only when its own description includes that item; a line that merely could involve it is left as it is.

Every number, distance, rate and standard the answer states stays in the changed value ("a 200 m radius", "the Manual of Engineering and Development Standards").

An answer that adds a requirement is the change it states: required insurance is an `insurance` add with its `minimum_coverage`, `currency` and `description`; a document the contractor must provide (a workers' compensation clearance, a permit) is a `requirements` add.

## What the bidder must do: `conditions`

Anything the document requires the bidder to do is a `conditions` add: acknowledge the addendum, attend a meeting, submit a form or a price. When it is for the bid, its stage is `bid_submission`:

```text
{"field": "conditions", "op": "add", "package": null, "existing": null,
 "value": {"type": "compliance", "category": "bid_submission",
           "description": "Acknowledge receipt of this addendum on the Form of Tender."},
 "quote": "Bidders shall acknowledge receipt of this addendum on the Form of Tender.", "page": 1}
```

When the current tender already requires bidders to acknowledge every addendum (a condition such as "acknowledge each addendum issued" or a bid form with a line per addendum), each addendum is a new item to acknowledge, so it adds its own `conditions` entry at `bid_submission` even when it does not repeat the instruction. The description names the addendum as it names itself and where the tender says to acknowledge it ("Acknowledge Addendum No. 3 on the Form of Tender."); the quote is the addendum's own title or number as printed on its page. Only when neither the tender nor the document asks for acknowledgement is there no acknowledgement entry.

## `value`: the result's shape

The new value has the shape the field, entry or line has in the current tender:

- a tender field: the value itself (`"2026-02-03T14:00:00"`, or an address object for `location`);
- a list entry: the whole entry with its new values, e.g. `{"type": "site_visit", "mandatory": true, "date": "2026-01-20T10:00:00", "location": "..."}`;
- a line: `item_id`, `description`, `quantity`, `unit`, `priced_by` and, when the owner prices it, `owner_unit_price` and `owner_total`.

A stated amount, percentage or date goes in its own structured field, as a number or a date, not only in a description or a condition. A bond of $50,000.00 has `"amount": 50000` and `"currency"` (the currency the tender uses); a bond of 10% has `"percentage": 10`; an event on January 20 has its `date`. The description says the same thing in words.

Bond example. The addendum says on page 3: "Bid Security shall be a Bid Bond in the amount of $50,000.00."

```text
{"field": "bonds", "op": "update", "package": null,
 "existing": {"type": "bid_bond", "percentage": 10, "description": "Bid bond of 10% of the tender price."},
 "value": {"type": "bid_bond", "percentage": null, "amount": 50000, "currency": "CAD",
           "description": "Bid bond of $50,000.", "conditions": []},
 "quote": "Bid Security shall be a Bid Bond in the amount of $50,000.00.", "page": 3}
```

## Dates and times

Write a date-time as the document states it, in the tender's local time, with no offset and no `Z`: "March 4 at 2:00 PM local time" is `"2026-03-04T14:00:00"`. Code resolves it to UTC with the tender's timezone.

## `quote` and `page`

`quote` is the exact text of this document that makes the change, copied from the page, and `page` is the page it is on.

Quote the shortest verbatim span that makes the change: one or two sentences, not a whole clause.

Never reword, shorten, complete or reorder a quote. Code looks for it as consecutive text on the page; a quote with its words in another order is not found, and the change loses its citation.

Copy in the page's line order, with every number where the page puts it. A layout can put a clause number on a line of its own after the sentence it numbers: copy it there, or end the quote before it. Never move or drop it.

A table row is shown to you with its cells joined by ` | `, left to right. Quote a row as the page text gives it: its cells in the page's order, the item number first when the row reads that way. The page reads `4.2 | Gate Valve 200 mm | each | Quantity revised from 4 to 6`:

- right: `"4.2 | Gate Valve 200 mm | each | Quantity revised from 4 to 6"`
- wrong: `"Gate Valve 200 mm Quantity revised from 4 to 6 each 4.2"` (the cells reordered)

## `issued_date` and `revision`

Every document says when it was issued and which revision it is, somewhere. Look on every page, not only the first paragraph: headers, footers, drawing title blocks, revision tables, issue stamps, corner text. The section **Lines from every page that may state the document's date or revision** collects candidate lines from every page.

- `revision`: the label as written, starting at the document-type word: "Addendum No. 2", "Bulletin #3", "Rev C", "Issued for Tender R1".
- `issued_date`: the date the document was prepared or issued, as written: "January 14, 2026", "2026-01-16".

Copy each as written, with no reformatting, and give `source: "page"`, the `quote` and its `page`.

When no page states one, read it from the file name if the file name has it: `Schedule_RevC_2026-01-16.pdf` gives revision `RevC` and issued date `2026-01-16`. Give `source: "file_name"`, `quote: null` and `page: null`, and copy the value exactly as it appears in the file name. When neither states it, return null.

## What is not a change

Only these:

- a value the document restates unchanged (a closing date it repeats as it already is);
- a question and answer that restates the documents unchanged ("Is the closing date still March 3? Yes, as stated in the Instructions to Bidders").

Everything else the document says that changes a tender field, list entry or schedule line (including a line whose work or material a revision or answer changes), or asks of the bidder, is a change.

## Example

The current tender has package `Drainage` with line `{"item_id": "3.2", "description": "Culvert Pipe 600 mm", "quantity": 45, "unit": "m"}`, and `lists.events` has `{"type": "site_visit", "mandatory": true, "date": "2026-01-12T10:00:00"}`.

Addendum No. 2 says on page 1: "ADDENDUM NO. 2", "The mandatory site visit is cancelled.", "Bidders shall acknowledge receipt of this addendum on the Form of Tender." On page 2: "Section 33 42 13, 2.3: Replace 2.3 with Culvert pipe shall be HDPE to ASTM F2306.", then "Q1. Is traffic control to be included in the bid price? A1. Yes, include all traffic control in the lump sum price." and "Q2. Is the closing date still January 20? A2. Yes, the closing date is unchanged."

```json
{
  "source_agent": "change-detector",
  "issued_date": null,
  "revision": {"value": "ADDENDUM NO. 2", "source": "page", "quote": "ADDENDUM NO. 2", "page": 1},
  "items": [
    {"ref": "1", "kind": "numbered_item", "quote": "The mandatory site visit is cancelled.", "page": 1,
     "outcome": "change", "reason": "Removes the site visit event.", "change_indexes": [0]},
    {"ref": "2.3", "kind": "specification_clause", "quote": "Replace 2.3 with Culvert pipe shall be HDPE to ASTM F2306.", "page": 2,
     "outcome": "change", "reason": "Changes the material of the culvert pipe line.", "change_indexes": [1]},
    {"ref": "acknowledgement", "kind": "instruction", "quote": "Bidders shall acknowledge receipt of this addendum on the Form of Tender.", "page": 1,
     "outcome": "change", "reason": "A bidder action for the bid.", "change_indexes": [2]},
    {"ref": "Q1", "kind": "question_answer", "quote": "Q1. Is traffic control to be included in the bid price?", "page": 2,
     "outcome": "change", "reason": "Adds what the bid price must include.", "change_indexes": [3]},
    {"ref": "Q2", "kind": "question_answer", "quote": "Q2. Is the closing date still January 20?", "page": 2,
     "outcome": "no_change", "reason": "Confirms the closing date unchanged.", "change_indexes": []}
  ],
  "questions_answers": [
    {"number": "Q1", "question": "Is traffic control to be included in the bid price?",
     "answer": "Yes, include all traffic control in the lump sum price.", "page": 2,
     "effect": "adds_condition", "change_indexes": [3], "reason": "Adds what the bid price must include."},
    {"number": "Q2", "question": "Is the closing date still January 20?",
     "answer": "Yes, the closing date is unchanged.", "page": 2,
     "effect": "no_change", "change_indexes": [], "reason": "Confirms the closing date."}
  ],
  "changes": [
    {"field": "events", "op": "remove", "package": null,
     "existing": {"type": "site_visit", "mandatory": true, "date": "2026-01-12T10:00:00"},
     "value": null, "quote": "The mandatory site visit is cancelled.", "page": 1},
    {"field": "vendor_groups", "op": "update", "package": "Drainage",
     "existing": {"item_id": "3.2", "description": "Culvert Pipe 600 mm", "quantity": 45, "unit": "m"},
     "value": {"item_id": "3.2", "description": "Culvert Pipe 600 mm, HDPE to ASTM F2306 (Section 33 42 13 §2.3)", "quantity": 45, "unit": "m"},
     "quote": "Replace 2.3 with Culvert pipe shall be HDPE to ASTM F2306.", "page": 2},
    {"field": "conditions", "op": "add", "package": null, "existing": null,
     "value": {"type": "compliance", "category": "bid_submission",
               "description": "Acknowledge receipt of this addendum on the Form of Tender."},
     "quote": "Bidders shall acknowledge receipt of this addendum on the Form of Tender.", "page": 1},
    {"field": "conditions", "op": "add", "package": null, "existing": null,
     "value": {"type": "compliance", "category": "bid_submission",
               "description": "Include all traffic control in the lump sum price."},
     "quote": "Yes, include all traffic control in the lump sum price.", "page": 2}
  ]
}
```

## Worked examples

(a) A letter listing revised drawing sheets. The tender's package `Storm` has lines `{"item_id": "5.1", "description": "Catch Basin 600 mm"}` and `{"item_id": "5.2", "description": "Manhole 1200 mm"}`. The letter lists `D-104 | Typical Catch Basin | 2` and `D-110 | Landscape Details | 1`. D-104 covers the catch basin; D-110 covers no line:

```text
items: [{"ref": "D-104", "kind": "drawing_sheet", "quote": "D-104 | Typical Catch Basin | 2", "outcome": "change",
         "reason": "The catch basin line is built to this sheet.", "change_indexes": [0]},
        {"ref": "D-110", "kind": "drawing_sheet", "quote": "D-110 | Landscape Details | 1", "outcome": "no_change",
         "reason": "No line's work is on this sheet."}]
changes: [{"field": "vendor_groups", "op": "update", "package": "Storm",
           "existing": {"item_id": "5.1", "description": "Catch Basin 600 mm"},
           "value": {"item_id": "5.1", "description": "Catch Basin 600 mm (Bulletin 2: drawings D-104 Typical Catch Basin rev 2)"},
           "quote": "D-104 | Typical Catch Basin | 2"}]
```

(b) A specification requirement with no revision verb. A revised section says "2.4 All curb joints are to be sealed with polyurethane sealant." The tender has `{"item_id": "A-12", "description": "Concrete Curb and Gutter"}`:

```text
items: [{"ref": "2.4", "kind": "specification_clause", "quote": "2.4 All curb joints are to be sealed with polyurethane sealant.",
         "outcome": "change", "reason": "Adds a requirement to the curb line's work.", "change_indexes": [0]}]
changes: [{"field": "vendor_groups", "op": "update", "package": "Roadworks",
           "existing": {"item_id": "A-12", "description": "Concrete Curb and Gutter"},
           "value": {"item_id": "A-12", "description": "Concrete Curb and Gutter, joints sealed with polyurethane sealant (Section 32 16 13 §2.4)"},
           "quote": "2.4 All curb joints are to be sealed with polyurethane sealant."}]
```

(c) A revised schedule table. Its rows read `A-13 | Asphalt Pavement 50 mm | m2 | 2400` (the tender has 2000) and `A-14 | Pavement Markings | m | 400` (the tender has no A-14):

```text
items: [{"ref": "A-13", "kind": "schedule_row", "quote": "A-13 | Asphalt Pavement 50 mm | m2 | 2400", "outcome": "change",
         "reason": "The quantity changes from 2000 to 2400.", "change_indexes": [0]},
        {"ref": "A-14", "kind": "schedule_row", "quote": "A-14 | Pavement Markings | m | 400", "outcome": "change",
         "reason": "A line the tender does not have.", "change_indexes": [1]}]
changes: [{"field": "vendor_groups", "op": "update", "package": "Roadworks",
           "existing": {"item_id": "A-13", "description": "Asphalt Pavement 50 mm", "quantity": 2000, "unit": "m2"},
           "value": {"item_id": "A-13", "description": "Asphalt Pavement 50 mm", "quantity": 2400, "unit": "m2"},
           "quote": "A-13 | Asphalt Pavement 50 mm | m2 | 2400"},
          {"field": "vendor_groups", "op": "add", "package": "Roadworks", "existing": null,
           "value": {"item_id": "A-14", "description": "Pavement Markings", "quantity": 400, "unit": "m"},
           "quote": "A-14 | Pavement Markings | m | 400"}]
```
