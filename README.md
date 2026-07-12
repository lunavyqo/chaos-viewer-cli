# Chaos Viewer CLI

A terminal progress atlas for matching-decompilation projects. Point it at any
project that publishes Chaos Viewer data (`chaos-db.json`), then browse modules
and functions, rank what is worth matching next, build AI matching prompts, and
watch optional live claims.

Inspired by [tangosdev/chaos-viewer](https://github.com/tangosdev/chaos-viewer).
Schema-compatible with that project's `ADAPTING.md` data format.

## Status

Private early development. Binary name: `chaos`.

## Features (MVP)

- Load atlas data from a local path, raw JSON URL, or GitHub repo (probes the same
  locations as the web viewer)
- Overview stats and searchable module / function lists, with **`m`** to show
  all / unmatched only / matched only
- **Heatmap** tab: view-only squarified byte treemap (same layout math as
  chaos-viewer) — green matched / grey unmatched / yellow claimed
- Priority lists: nearly done, best scaffolded, biggest unmatched
- Function detail with optional lazy-loaded module detail chunks
- Prompt builder (single + batch, max 16) with clipboard copy
- Optional **pluggable claims** coordination: any HTTP coordinator via
  `project.claimsApi` (not hardcoded to one host) + `CLAIMS.md` merge

## Requirements

- Rust 1.85+ (edition 2024 workspace uses a recent stable toolchain)
- Network access for remote data / claims (optional for pure local files)

## Build

```bash
cargo build --release
./target/release/chaos --help
```

## Usage

```bash
# Interactive TUI (default)
chaos
chaos --input path/to/chaos-db.json
chaos --input https://example.com/chaos-db.json
chaos --repo https://github.com/you/your-decomp

# Non-interactive
chaos stats --input path/to/chaos-db.json
chaos list --input path/to/chaos-db.json --priority nearly
chaos prompt --input path/to/chaos-db.json --id 'module:0x02000000'
```

### Claims (optional, pluggable)

The coordinator URL comes from the atlas (`project.claimsApi`), or `--api`:

```bash
# List locks (API + CLAIMS.md)
chaos claims list --repo https://github.com/you/your-decomp

# Write path (any coordinator that implements try-lock / renew / release)
export CHAOS_CLAIMS_API_KEY='…'          # or CHAOS_CLAIMS_SESSION / CHAOS_CLAIMS_KEY
export CHAOS_CLAIMS_HANDLE='your-name'
chaos claims try-lock --module arm9 --start 0x2000000 --end 0x2000100 --note 'matching'
chaos claims renew --id clm_…
chaos claims release --id clm_…
chaos claims instructions --repo …
```

In the TUI: **`r`** refreshes claims; prompts include the agent lock footer when
a session is set. See [`docs/claims-api.md`](docs/claims-api.md) for the generic
contract (so you can run **your own** coordinator or use someone else’s).

## Data format

See [`docs/schema.md`](docs/schema.md), [`docs/claims-api.md`](docs/claims-api.md),
and upstream
[ADAPTING.md](https://github.com/tangosdev/chaos-viewer/blob/master/ADAPTING.md).

## Development

```bash
cargo fmt
cargo clippy --all-targets -- -D warnings
cargo test
```

See [`CONTRIBUTING.md`](CONTRIBUTING.md).

## License

MIT. See [`LICENSE`](LICENSE). Compatible with and inspired by Chaos Viewer (MIT).

No ROMs or game assets are included. Progress data comes only from files or URLs
you supply.
