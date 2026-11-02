# Worked examples

From two tenders the pipeline is tested on: South Larkin Watermain Phase 3 (Township of
Spallumcheen, MMCD 2009) and Peel Region 2026-296T (1200 mm sanitary sewer, OPSS).

## Conditions

| Page text | Entry |
|---|---|
| "The tender will be irrevocable and open for acceptance … for a period of forty-five (45) calendar days" | `bidding_period`: "Keep the bid open for acceptance for 45 calendar days after closing." |
| "Only the following prequalified General Contractors …" | `eligibility`: "Only the prequalified general contractors listed may bid." |
| "Bidders must possess a valid Acceptable OHSMS Certification at the Document closing" | `["eligibility", "bid_submission"]`: "Hold a valid OHSMS certification at closing and confirm it in the online forms." |
| "If the Schedule … includes any tender prices for Optional Work … complete all the unit prices" | `bid_submission`: "Complete the unit prices for all optional work in the schedule." |
| "Within 15 Days of receipt of the written Notice of Award, deliver … a Performance Bond and a Labour and Material Payment Bond … a Baseline Construction Schedule … a clearance letter … a copy of the insurance policies" | Four `award` entries: the bonds, the baseline schedule, the WorkSafe BC clearance letter, the insurance policies — each "within 15 days of the notice of award". |

Wrong:

- "Deliver the required documents within 15 days of award." — names nothing; list each document.
- "Deliver the bonds, schedule, clearance letter and insurance within 15 days." — four deliverables in one entry.
- "Commence the Work within two days of the Notice to Proceed." as `award` — it is after signing: an obligation, `notice`, `before_start`.
- "Coordinate with CP Rail; no construction December to March." as a condition — obligations: `coordination` and `site_constraint`, `during_work`.
- "Tenders may be revoked until closing." — a bidder's right, not an entry.

## Obligations

| Page text | Entry |
|---|---|
| "CP PERMIT REQUIRED BEFORE BEGINNING ANY WORK WITHIN CP'S RIGHT-OF-WAY." | `permit`, `before_start`: "Obtain a CP Rail permit before starting any work within CP's right-of-way." |
| "Submit a track settlement monitoring plan … at least two weeks prior to beginning trenchless work" | `submittal`, `before_start`: "Submit a track settlement monitoring plan at least two weeks before trenchless work begins." |
| "The Contractor shall retain the services of an independent testing agency …" | `testing`, `during_work`: "Retain and pay an independent testing agency supervised by a professional engineer." |
| "provide record drawing information … upon Substantial Performance" | `closeout_document`, `closeout`: "Provide hand-marked as-constructed drawings and a digital alignment survey at substantial performance." |

Wrong: "Maintain a minimum pipe cover of 1.5 m." — a dimension from the specification; not extracted.

## Requirements

| Page text | Entry |
|---|---|
| "A holdback in the amount of $10,000.00 will be retained until the digital and hand marked as-constructed information has been received" | `holdback`: "The owner holds back $10,000 until the as-constructed information is received and accepted." |
| "interest calculated at 2% per annum over the prime commercial lending rate" | `payment_terms`: "Late payments by the owner bear interest at 2% a year over prime." |
| "manage excess soil in accordance with Ontario Regulation 406/19" | `compliance`: "Manage excess soil under Ontario Regulation 406/19." |

Wrong: "Bidders must submit all pricing in Canadian dollars." as a requirement — a `bid_submission` condition.
