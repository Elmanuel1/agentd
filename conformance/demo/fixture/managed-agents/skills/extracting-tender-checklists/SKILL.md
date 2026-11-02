---
name: extracting-tender-checklists
description: Extracts the pre-bid checklist from a construction tender - the conditions a bidder must meet up to bid close (eligibility, bidding period, bid submission). Each entry is staged, typed, cited and written for review. Use when reading tender documents to list what a bidder must do to bid. Award, obligations after signing and contract requirements are out of scope.
metadata:
  author: tosspaper
  version: "1.0"
---

# Extracting tender checklists

One list, the pre-bid checklist, read by a person who ticks items off: `conditions` at `eligibility`, `bidding_period` and `bid_submission`, everything a bidder must be, hold or do up to bid close.

**Scope.** Only the pre-tender process, up to bid close. Nothing at award or after it:
- no `award` conditions;
- no `obligations`;
- no `requirements`.

Return `"obligations": []` and `"requirements": []`. A condition that applies at bid submission and again at award is listed with `applies_at: ["bid_submission"]` only.

Bonds and insurance have their own lists (not these). A condition may name a bond or policy as a document to deliver; it never repeats its amount or terms.

## Where to look: your reading plan

1. Run the outline and find the bid-stage sections: the invitation or notice to bidders, the instructions to bidders and any special instructions, the form of tender and bid forms, and the submission requirements. They are usually near the front and at the form of tender.
2. Read those pages in full.
3. Do not read the general conditions, special provisions, specifications, drawings or price schedule page by page. Find a rule about the bid itself there by searching (for example `bid|tender|bidder|submit|closing|addend|irrevocab|security|deposit`), and read only the pages the search points to.

## Written for review

Every `description` is one plain sentence in sentence case: who must do what, and by when. Do not paste the page: no ALL CAPS, clause numbers, "see section" references or cut-off text. The citation's `text_snippet` keeps the exact words from the page.

## Conditions

A step a bidder must complete, or a qualification it must hold, for its bid to be accepted or for the contract to be signed. Each is `{"applies_at", "description", "citations"}`. `applies_at` lists every stage it applies at:

- `eligibility`: what the bidder must be or hold before bidding: prequalified or invited bidders, plan-taker registration, a contractor licence or business registration, minimum experience, bonding capacity, a certificate held at closing (COR, SECOR, ISO 45001, workers' compensation in good standing), obtaining the standard documents the tender relies on (its own copy of the MMCD).
- `bid_submission`: what must be in the bid: the signed form of tender, bid security, a subcontractor list, a proposed schedule or work plan, declarations and affidavits, addenda acknowledged; and how the bid must be priced: the currency ("submit all prices in Canadian dollars"), every item priced, optional work priced, whether taxes are included in the bid prices. Tax registration numbers and invoicing after award are `payment_terms` requirements, not conditions.
- `bidding_period`: a rule to follow before closing: attend a mandatory meeting or site visit, keep the bid open for the stated days.
Award (what the successful bidder delivers after award) is out of scope.

Go through the three stages in order and list what the document states for each. Select every stage within scope that a condition applies at.

- One deliverable per entry, each named. The form of tender, bid security, a subcontractor list and a declaration due with the bid are separate entries. "Submit the required documents" is not an entry: name each document the tender requires.
- Not a condition: anything at award or after it (delivering bonds or insurance after award, signing the agreement, starting work, permits, coordination, seasonal limits); a bidder's right (to revoke before closing, to ask questions); how the work is done; how the contractor is paid (actual quantities times unit prices) — a `payment_terms` requirement; a date that has its own field (the closing date, the questions deadline, the completion date).
- Cite the words that state the requirement. For "the bid shall include: a) a bid bond …", cite the item ("a bid bond") as well as the rule.

## Rules

- Extract only what the document states; every entry is cited (see the citing-sources skill).
- One entry per item; two sentences stating the same item are one entry, however many pages state it.
- A cross-reference is not an entry: follow it and extract what the referenced text requires.

## Topics

Every entry has a `topic`: what it is about, one of the topics below. The topic is the type you extract by: read a page for the topics it states, and give one entry per item of each. Two entries with the same topic are one item stated twice unless each names something the other does not (two different forms, two different declarations).

Not an entry, and no topic fits: a specification's technique (materials, dimensions, methods, products); what one item is measured, paid or priced by, or what its price includes (that is the line's own scope); general contract terms (changes and extra work, claims, disputes, indemnity, termination, which document governs); bonds and insurance (their own lists); price adjustments (their own list); a date that has its own field (the closing date, the question deadline); anything at award or after it.

### Conditions

| `topic` | What it is |
|---|---|
| `registration` | Registration to bid: a bidding-system account, plan-taker registration, the documents obtained from the owner's portal. |
| `prequalification` | Prequalification: only prequalified or invited bidders may bid. |
| `licence_registration` | A licence, certificate or registration the bidder must hold (a trade licence, a business licence, Ontario company registration, PEO). |
| `experience_references` | Experience or references the bidder must show (similar projects, reference contacts). |
| `single_entity` | The bidder is one legal entity; rules for joint bids. |
| `conduct` | Rules of conduct: no contact with staff other than the named contact, no lobbying, collusion, media contact, conflicts of interest to declare. |
| `questions` | How and by when questions are asked; questions after the deadline are not answered. |
| `site_visit_meeting` | A site visit or pre-bid meeting, and whether it is mandatory. |
| `addenda` | Addenda: how they are issued and that bidders must acknowledge them. |
| `irrevocability` | How long the bid stays irrevocable or open for acceptance; withdrawal before closing. |
| `closing_late_bids` | When bids are due and that late bids are rejected. |
| `submission_method` | How the bid is submitted (electronically through the bidding system, file formats, no hard copies). |
| `forms_declarations` | Forms, signatures and declarations the bid must include (the form of tender, statements, affidavits). |
| `pricing_rules` | A rule for pricing the whole bid: every item priced, taxes, currency, provisional and contingency items, alternates, no $0 unless meant. Not what one item's price includes ("price to include gates and posts"): that is the line's own scope, not an entry. |
| `subcontractors` | Subcontractors the bid must list, and limits on subletting. |
| `schedule_workplan` | A schedule, work plan or methodology submitted with the bid. |
| `key_personnel` | Key personnel or supervisory staff the bid must name. |
| `source_of_material` | A source of material statement submitted with the bid. |

See [worked examples](references/EXAMPLES.md) for entries from real tenders, right and wrong. Use only its pre-bid condition examples.

## Before you answer: audit what you missed

Go back over the pages you were given, not over your list, and check:

1. **Each stage, in order.** For `eligibility`, `bidding_period` and `bid_submission`, find every sentence that states one, and check an entry cites it. What goes in the bid is usually listed together in one clause: one entry each.
2. **Each topic, searched.** For every condition topic above, run one `pages.py --grep` with its words and read the hits, then check each rule found is listed. A topic that is easy to miss reads like boilerplate:
   - `conduct`: `lobby|media|collusi|conflict of interest|contact`
   - `pricing_rules`: `\$0|zero dollar|blank|every item|all items|currency|taxes`
   - `submission_method`: `upload|file|format|corrupt|readable|hard cop`
   - `site_visit_meeting`: `site visit|examine the site|inspect the site|pre-bid|meeting`
   - `forms_declarations`: `declar|affidavit|sign|form of tender|statement`
   - `irrevocability`: `irrevocab|open for acceptance|withdraw`
3. **Each entry.** It names one deliverable or rule, it applies up to bid close (not at award or after), and its description is a plain sentence.

Add what you missed, split what you bundled, drop what is at award or after, then answer.
