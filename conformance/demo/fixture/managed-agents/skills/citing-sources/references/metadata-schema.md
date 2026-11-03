# blocks.json Schema

A flat array of ParseBlock objects. Each block represents a Textract-detected element
(page, line, word, table, cell, layout section, etc.). Parent-child relationships are
encoded via the `parent` field (which holds the `block_id` of the parent block).

```json
[
  {
    "type": "page | line | word | table | cell | merged_cell | layout_title | layout_text | layout_header | layout_footer | layout_section_header | layout_page_number | layout_list | layout_figure | layout_table | layout_key_value | key_value_set | form_key | form_value | selection_element | signature | query",
    "block_id": "uuid",
    "content": "string | null",
    "confidence": 98.5,
    "parent": "uuid | null (block_id of parent block)",
    "bbox": {
      "left": 0.05,
      "top": 0.04,
      "width": 0.80,
      "height": 0.02,
      "page": 1,
      "original_page": 1
    }
  }
]
```

## Block Hierarchy

Textract blocks form a tree via `parent`:

- `page` — root block for each page (parent = null)
  - `layout_title`, `layout_text`, `layout_header`, etc. — layout regions (parent = page)
    - `line` — a line of text (parent = layout block or page)
      - `word` — individual word (parent = line)
  - `table` — a detected table (parent = page)
    - `cell` / `merged_cell` — table cells (parent = table)
      - `word` — words inside cells (parent = cell)
  - `form_key` / `form_value` — key-value pairs (parent = page)

## Finding Blocks

- **By page**: filter where `bbox.page == N`
- **By type**: filter where `type == "table"`, `type == "line"`, etc.
- **Children of a block**: filter where `parent == block.block_id`
- **Text content**: search `content` field for substring matches (content is present on all blocks in `full` mode)

## Field Reference

| Field | Type | Description |
|-------|------|-------------|
| `type` | string | Textract block type in lowercase |
| `block_id` | uuid | Unique identifier for this block |
| `content` | string/null | Text content (null in parent_only/child_only modes) |
| `confidence` | float/null | Textract confidence 0-100 (null for PAGE blocks) |
| `parent` | uuid/null | block_id of parent block (null for root PAGE blocks) |
| `bbox.left` | float | Left edge as fraction of page width (0.0-1.0) |
| `bbox.top` | float | Top edge as fraction of page height (0.0-1.0) |
| `bbox.width` | float | Width as fraction of page width |
| `bbox.height` | float | Height as fraction of page height |
| `bbox.page` | int | Page number (1-based) |
| `bbox.original_page` | int | Original Textract page number |
