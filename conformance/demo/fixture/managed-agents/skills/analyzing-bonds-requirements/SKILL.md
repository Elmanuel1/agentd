---
name: analyzing-bonds-requirements
description: Extracts detailed bond, insurance, and contractual requirement information from construction tender documents. Use when reading tender/bid documents to identify bid bonds, performance bonds, insurance minimums, warranty requirements, and compliance conditions.
---

# Analyzing Bonds and Requirements

Extract structured bond, insurance, and requirement information from `/workspace/blocks.json`. Produce output conforming to the bond-requirement-analyzer schema in `.claude/rules/output-schemas.md`.

## Where to Look

- **General Conditions / Supplementary General Conditions** — bond requirements, insurance minimums, indemnification
- **Instructions to Bidders** — bid bond requirements, bid security
- **Special Provisions / Special Conditions** — additional insurance, warranty terms
- **Form of Agreement** — performance bond, labour/material bond timing
- **Division 1 - General Requirements** — submittals, compliance, safety requirements

## Bonds

Extract each bond separately:
- **bid_bond** — required with submission, typically percentage of bid price
- **performance_bond** — required after award, typically 50-100% of contract
- **labour_material_bond** — protects subcontractors/suppliers
- **maintenance_bond** — post-completion warranty period

For each bond, populate fields as follows:

**`description`** — one plain sentence for the person reviewing the tender, saying what bond is required, for how much and when ("A performance bond for 50% of the contract price, due within 15 days of award"). Not the clause pasted from the page; the citation keeps the exact words.

**`conditions`** — a list of specific constraints or requirements attached to this bond. Extract each as a complete, properly written sentence as it would appear in a professional construction contract. Examples of what belongs here:
- "A certified cheque or bid bond in an acceptable form must be submitted with the bid."
- "The bond must be made payable to [Owner Name]."
- "The bond must be issued by a surety company licensed to operate in the province."
- "The bond must remain valid for a minimum of 90 days after the bid closing date."
- "A performance bond equal to 100% of the contract price is required within 10 days of award."
- "The performance bond must include a two-year maintenance period following acceptance of the work."
- "The bond must name [Owner Name] as the obligee."
- "The bond must cover all joint venture partners."
- "The bond amount must be expressed in Canadian dollars."

Do NOT leave `conditions: []` if the clause contains any of the above. Parse them out explicitly.

**Never include conditions that are cross-references.** A cross-reference condition is one that points to another section of the document rather than stating the actual requirement. Do NOT include conditions like:
- "The Bid Security as required by paragraph 5.2 of the Instructions to Tenderers"
- "As per section 3.4 of the General Conditions"
- "See clause 7.1 for bond requirements"
- "Refer to Article 12 of the Supplementary Conditions"

If a cross-reference points to text elsewhere in the document, follow it and extract the actual requirement text from the referenced section. The condition entry must contain the real, actionable requirement — never a pointer to find it elsewhere.

## Insurance

Extract each required policy as its own entry. Pick the type by the policy's own name or heading; the names in brackets are how it is printed in other provinces and in French:
- `cgl` — commercial general liability (IBC 2100; *responsabilité civile générale*). Pollution cover inside the CGL policy ("hostile fire", "sudden and accidental") stays here.
- `auto_liability` — owned, leased or hired vehicles, including certificates from a government insurer (ICBC, SGI, MPI, SAAQ; *RC automobile*).
- `non_owned_auto` — non-owned automobile liability (SPF No. 6, OPCF 6, *formule des non-propriétaires*). One sentence requiring auto liability "for owned and non-owned" vehicles is two entries: `auto_liability` and `non_owned_auto`.
- `umbrella` — umbrella or excess liability above other policies (*RC « umbrella »*).
- `wrap_up_liability` — a project wrap-up liability policy: wrap-up, OCIP, CCIP or project-specific liability (*RC « wrap-up »*).
- `professional_liability` — professional liability, including one policy named "professional liability (errors and omissions)" (*RC professionnelle*).
- `errors_omissions` — errors and omissions insurance named as its own policy.
- `pollution_liability` — contractor's pollution liability or environmental impairment, as its own policy or endorsement (*RC environnementale*).
- `builders_risk` — broad form property on the work: builder's risk, course of construction, all-risk construction (*tous risques de chantier*, *assurance des chantiers*). One policy named for both builder's risk and an installation floater is `builders_risk`.
- `installation_floater` — materials and equipment to be installed in the work, while stored, in transit or being installed (*risques d'installation*).
- `equipment_floater` — the contractor's own or rented construction equipment, machinery and tools (contractor's equipment, all-risks; *police flottante pour le matériel*).
- `boiler_machinery` — boiler and machinery or equipment breakdown, including testing and commissioning.
- `aircraft_watercraft` — liability for owned or non-owned manned aircraft or watercraft, including aircraft passenger hazard.
- `unmanned_aircraft` — drone or unmanned aerial vehicle liability.
- `crane_liability` — hook, on-hook, crane, hoist or riggers liability: property while it is lifted or rigged.
- `railway_protective` — railway or railroad protective liability for work on or near a railway.
- `owners_contractors_protective` — owner's and contractor's protective liability (*RC indirecte du propriétaire ou de l'entrepreneur*).
- `property` — property insurance on the contractor's own property or property in its care, not on the work (*assurance des biens*).
- `cyber_liability` — cyber or privacy liability.
- `workers_comp` — a workers' compensation **policy** with a stated coverage limit. A WSIB, WorkSafeBC, WCB or CNESST clearance certificate, or proof of good standing, is never insurance: it is a `requirements` entry, category `compliance` (2.3).
- `other` — a required policy none of these types describes. `description` names it as printed, and `review_reason` says no type fits.

Never force a policy into a type it is not. When none fits, use `other`, name the policy in `description` as printed, and set `review_reason`.

For each: type, minimum coverage amount, currency, whether certificate required, additional insured requirements, duration.

`currency` follows the tender's currency rule: the code a page states, or the code inferred from strong evidence (HST, GST or PST, a provincial or state law or board, money named in a bond or deposit) with `review_reason` saying so. A "$" alone gives `null`.

An insurance entry is a policy the documents require by its own name or heading. Wording inside another policy's terms belongs to that policy: an "error or omission" sentence among a pollution policy's endorsements is part of the pollution policy, not a professional liability policy.

When two places require the same policy with different limits (the instructions to bidders say $5,000,000, the supplementary conditions $2,000,000), give both entries, each citing its own place. Do not choose one; the review shows both.

**`conditions`** — specific constraints on this insurance coverage. Examples:
- "The Owner and the Consultant must be named as additional insureds on the policy."
- "The insurer must provide a minimum of 30 days written notice of cancellation to the Owner."
- "A certificate of insurance must be submitted to the Owner before work commences on site."
- "The policy must include a cross-liability clause."
- "The policy must include a waiver of subrogation in favour of the Owner."
- "Coverage must be maintained for the duration of the contract and for two years following substantial completion."

Do NOT leave `conditions: []` if any of the above appear in the document.

**Never include conditions that are cross-references.** A cross-reference condition is one that points to another section of the document rather than stating the actual requirement. Do NOT include conditions like:
- "The Bid Security as required by paragraph 5.2 of the Instructions to Tenderers"
- "As per section 3.4 of the General Conditions"
- "See clause 7.1 for bond requirements"
- "Refer to Article 12 of the Supplementary Conditions"

If a cross-reference points to text elsewhere in the document, follow it and extract the actual requirement text from the referenced section. The condition entry must contain the real, actionable requirement — never a pointer to find it elsewhere.

## Completeness

The lists are read by a contractor deciding whether to bid and what to price.
- Read the whole document for these, not only the instructions to bidders: general conditions, supplementary conditions and special provisions state most of them.
- Every insurance type the document requires is an entry, even when it names no dollar limit.
- Requirements (warranty, safety, compliance, holdback, payment terms) are covered by the extracting-tender-checklists skill.

## Rules

- Extract ONLY from the source document — never infer
- If a value is ambiguous, extract the most conservative interpretation
- Every entry needs a citation via the citing-sources skill
- Do not duplicate bond entries already captured — each unique bond appears once

## Before you answer: audit what you missed

Go back over the pages you were given and check:

1. **Each bond type.** Bid security (bid bond, certified cheque), performance bond, labour and material payment bond, maintenance bond: is each one the document requires listed, with its amount or percentage and when it is due?
2. **Each insurance type.** Commercial general liability, automobile, non-owned automobile, professional, errors and omissions, pollution, builder's risk, installation floater, workers' compensation, umbrella: is each one required listed, with its limit when stated, even with no limit?
3. **Each entry.** One bond or policy per entry, its description a plain sentence, its conditions actual requirements (not cross-references).

Add what you missed, then answer.
