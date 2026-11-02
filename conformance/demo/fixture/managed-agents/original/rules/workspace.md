# Workspace Rules

The microVM workspace at /workspace/ contains all input and output files.
The VM has NO database access. All context is provided as files.

## Input Files (read-only - prepared by Rust launcher)

```
/workspace/
├── {document-uuid-1}/
│   └── blocks.json     — flat array of ParseBlocks for document 1
├── {document-uuid-2}/
│   └── blocks.json     — flat array of ParseBlocks for document 2
├── ... (one directory per document)
├── {document-uuid-N}/
│   └── blocks.json     — flat array of ParseBlocks for document N
```

## Output Files (per agent per document)

```
/workspace/output/
├── bid-extractor-{doc-uuid-1}.json
├── bid-extractor-{doc-uuid-2}.json
├── ...
├── vendor-grouper-{doc-uuid-1}.json
├── addenda-processor-{doc-uuid-1}.json
├── bond-requirement-analyzer-{doc-uuid-1}.json
├── review-bid-extractor-{doc-uuid-1}.json
├── review-vendor-grouper-{doc-uuid-1}.json
├── ...
├── merged.json         — (Rust launcher - combines all per-document outputs)
└── result.json         — (Rust launcher - final with confidence scores)
```

Output filename pattern: `{agent-name}-{document-uuid}.json`
Review filename pattern: `review-{agent-name}-{document-uuid}.json`

## Rules

- NEVER modify input files — they are read-only context
- Each agent processes ONE document at a time (receives path in prompt)
- Output is written directly to the file path specified in the agent's prompt
- merged.json and result.json are written by the Rust launcher
- Do NOT access any network resources except through configured MCP servers
- Do NOT attempt database connections — the VM has no DB access
