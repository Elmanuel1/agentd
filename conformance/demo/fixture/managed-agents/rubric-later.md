# Tender extraction rubric: later upload

Each `###` heading below is **one criterion**. Grade every criterion on its own, as **PASS** or **FAIL**:
- Reason first, then decide.
- Check against the later document's text (`python3 /mnt/session/uploads/extraction/bin/pages.py {document_id} --pages N`) and the current tender (`/mnt/session/outputs/u0/result.json`).
- For every FAIL, name the change or item, say what is wrong, and give the page that shows it.
- The artifact satisfies the rubric only when every criterion passes.

Rules that override the text below:
- **Dates** are in the local time of the tender's location, as the document states them, with no offset, unless it explicitly states another zone.
- **Checklists** cover only the pre-tender process. A change to award conditions, obligations or requirements is out of scope.
- **Flags:** an uncertain change carries `review_reason`; there are no `pipeline_outcome` or confidence values.

## 1. Changes: one criterion per rule
Each criterion applies once per expected change.

### 8.1 What a change is
- Each change names its field and operation (`update`, `add` or `remove`), the new value, and a verbatim quote with its page.
- **Update or remove of a list entry or line:** `existing` is an exact copy of that entry or line as the current tender shows it. That's every key shown, every value character for character, and no ids.
- **Update of a tender field:** `existing` is `null`.
- **Line changes:** changes are to lines only, never adding, removing or renaming a package. `package` names the package the line is in, exactly as shown.
- **Unnamed package:** when the document names no package, the change is listed once per package that holds the item.

### 8.2 Every item is decided
- The document's items are all listed first: numbered items, specification clauses, drawing sheets, questions and answers, instructions, schedule rows.
- Each listed item is marked `change` or `no_change`, with a reason.
- Every change points back to an item.
- Every numbered item of the letter, and every sheet the document lists, appears among the items.

### 8.3 Operations
- **`remove`** only when the document says so in words: deleted, cancelled, withdrawn, superseded, no longer required. Absence is never a removal.
- **`add`** only for an entry or line the tender doesn't have. An added line goes to:
  1. the package the agent named, if the tender has it;
  2. otherwise the single package holding the item's section prefix;
  3. otherwise the package a Jev pick chooses;
  4. otherwise unplaced, for the user to choose.
- **`update`** keeps every field of the line the document doesn't change (item_id, quantity, unit, priced_by, owner figures). A field is never blanked.

### 8.4 Revisions that change a line
- A specification revision, drawing revision, or question-and-answer that changes a line's work or material is an `update` of that line.
- The description is amended with the revised requirement, in the document's words, naming the section or drawing it comes from.
- **Requirements with no revision wording:** sentences like "joints are to be restrained" are checked against the lines, and each line they change gets its update.
- **Drawings:**
  - Only the title block of a drawing page is read: sheet number, title, revision, date.
  - Never a quantity, dimension or scope.
  - A revised sheet updates only the lines its work covers, named by number and title.
  - Sheet titles come from the drawing-list row, never from title-block labels ("DRAWN BY", "DATE").
- **A revised schedule** is compared line by line:
  - a changed description, quantity or unit is an `update`;
  - a new line is an `add`;
  - a deleted line is a `remove` only where the document says so.

### 8.5 Several documents
- Each document is compared on its own against the same current tender.
- **Same line, same amendment:** several documents amending one line's description, and agreeing on item_id, quantity and unit, merge into **one** proposal. It cites every document, with the amendments in issue order.
- **Disagreement** (different values) is a conflict: several proposals, ranked.
- **Ranking:**
  - A document named as superseded by another document ranks last.
  - Otherwise the latest-issued document ranks first, comparing issue date, then revision. This holds whatever the confidence or review reason.
  - When the order can't be told, upload order is kept with no recommendation.
- **One document, two values:** shown as "This document gives more than one value; choose one."

### 8.6 Where an answer or revision lands
- An answer or revision that changes priced work updates the line(s) it names. When it names none, it updates the line(s) whose work it changes; the choice is fixed by the documents (the line whose description covers that work), so repeated runs land on the same line(s).
- An answer that adds a rule the bidder must follow when bidding is a `conditions` add.
- An answer that adds a contract term binding the contractor (2.3) is a `requirements` add; a construction-phase duty (2.7) is an `obligations` add.
- An answer that confirms or restates the documents is `no_change`.
- Every answer is one `questions_answers` entry (section 12), linked to the changes it makes.

### 8.7 Bidder actions
- Anything the document asks the bidder to do is a `conditions` add at `bid_submission`, e.g. "acknowledge receipt of this addendum on the Form of Tender".
- When the current tender (the first upload's `result.json`, with the changes of earlier uploads) requires bidders to acknowledge every addendum, the result has a `conditions` add at `bid_submission` acknowledging this document by its own name or number, quoting that name or number as printed. Fails when that entry is missing.

### 8.8 Not a change
- A value restated unchanged.
- A question-and-answer that restates the documents.
- A revision that changes no field, entry or line.

### 8.9 Document facts
- `issued_date` and `revision` are as written, with their quote and page (or the file name when no page states them).

### 8.10 Dates in changes
- A date-time written in local time is converted to UTC with the tender's timezone.

---

## 2. Questions and answers
### Questions and answers
Every question and answer the documents state, in a first upload or a later one, is one entry of `questions_answers`.

- `number` is the question's number as written ("Q3", "Question 12"), or its position when unnumbered.
- `question` and `answer` are verbatim.
- `document` is the document that states it, and it carries its own citation.
- `effect` is one of:
  - `changes`: it changes a field, entry or line; the entry links the changes it makes (their indexes, or the field, entry or line they change);
  - `adds_requirement`: it adds a contract term or construction-phase duty;
  - `adds_condition`: it adds a rule for bidding;
  - `no_change`: it confirms or restates the documents.
- On a later upload, every answer either links to its changes or is `no_change`. An answer left unlinked or undecided fails.
- The same answer lands on the same target in every run (8.6).
- A change made from an answer states every fact the answer gives (a distance, an amount, a method), not part of them.
- An answer that something is not required across the contract ("concrete collars are not required") is one `conditions` add, effect `adds_condition`. A line is updated too only when its own description includes that item.

## 3. Citations: one criterion per rule
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
- An acknowledgement entry added because the tender requires bidders to acknowledge every addendum quotes this document's own name or number ("Addendum #2"). That quote supports it: the instruction is in the tender, the quote shows which addendum it is.

### C6. Shortest span. The quote is the shortest span that states the value: one or two sentences, not a whole clause or page.

### C7. Tables. A table row is quoted as the page gives it, cells left to right joined by ` | `, item number first when the row reads that way.

### C8. Numbers in place. Every number stays where the page puts it, including a clause number on its own line.
