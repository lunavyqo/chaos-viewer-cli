# Chaos atlas data schema

This CLI is schema-compatible with
[Chaos Viewer `ADAPTING.md`](https://github.com/tangosdev/chaos-viewer/blob/master/ADAPTING.md).

## `chaos-db.json` (required)

```json
{
  "generatedAt": "2026-07-02 12:00",
  "project": {
    "name": "your-decomp",
    "github": "https://github.com/you/your-decomp",
    "compiler": "optional",
    "setup": "optional",
    "verifyCommand": "optional with {name} {module} {addr} {addrHex} {size} {sizeHex} {github}",
    "readFirst": "optional",
    "rules": "optional",
    "nearMissNote": "optional",
    "claimsApi": "optional",
    "dataUrl": "optional"
  },
  "stats": {
    "totalFunctions": 0,
    "matchedFunctions": 0,
    "totalBytes": 0,
    "matchedBytes": 0,
    "moduleCount": 0
  },
  "functions": [
    {
      "id": "module:0x02012345",
      "module": "module",
      "name": "func_02012345",
      "addr": 33628997,
      "size": 164,
      "matched": false,
      "srcPath": "optional",
      "author": "optional",
      "div": 2,
      "cat": "optional",
      "floor": "optional",
      "sim": 0.87,
      "sibling": "optional",
      "matchProvenance": {
        "kind": "ai",
        "model": "optional under default; required under experimental when matched",
        "reasoning": "optional reasoning / effort level (e.g. high)",
        "harness": "optional pipeline id (e.g. fanout-v3)"
      }
    }
  ]
}
```

### `matchProvenance` (experimental)

Optional on **default** atlases (sm64ds / chaos-viewer ignore it). Under the
**experimental** project convention, every **matched** function should record
how it was matched:

| kind | fields |
|---|---|
| `"human"` | optional `by`, `note` |
| `"ai"` | required `model`, `reasoning`, `harness`; optional `by` |

Examples:

```json
{ "kind": "human", "by": "lunavyqo" }
{ "kind": "ai", "model": "claude-opus-4", "reasoning": "high", "harness": "fanout-v3", "by": "op" }
```

The CLI shows this on the Overview detail pane when the active profile is
experimental, and flags matched functions that omit or incompletely fill it.

## Detail chunks (optional)

`details/<module>.json` next to the atlas file:

```json
{
  "func_02012345": {
    "callees": ["a"],
    "calledBy": ["b"],
    "disasm": ["  push {r4, lr}"],
    "pool": ["+0x9c: &x"],
    "draft": "int f(void) { ... }",
    "draftDiv": 2
  }
}
```

## Discovery order (GitHub repo)

When given a GitHub repo URL, the CLI probes (first hit wins):

1. Explicit branch `chaos-db.json` / `data/chaos-db.json` (if `--branch` set)
2. `chaos-data` branch: `chaos-db.json`, `data/chaos-db.json`
3. `main` / `master`: `data/chaos-db.json`, `chaos-db.json`, `docs/chaos-db.json`
4. GitHub Pages: `data/chaos-db.json`, `chaos-db.json`

## Priority rules (match web viewer)

- **Nearly done:** unmatched, not claimed, `div` set, category does not include
  `materialization`; sort by `div` asc, then `size` asc; top 25.
- **Best scaffolded:** unmatched, not claimed, `sim` set, no `floor`; sort by
  `sim` desc; top 25.
- **Biggest:** unmatched, not claimed, no `floor`; sort by `size` desc; top 25.
