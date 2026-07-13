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
  locations as the web viewer); **multiple saved projects** with switch / resume
- Overview: modules + functions on top, **detail pane** underneath; **`m`**
  filters all / unmatched only / matched only; **`s`** sorts modules (name,
  worst/best by unmatched left, most functions, most bytes)
- **Heatmap** tab: view-only squarified byte treemap (same layout math as
  chaos-viewer) — green matched / grey unmatched / yellow claimed
- Priority lists: nearly done, best scaffolded, biggest unmatched
- Prompt builder (batch, max 16) with clipboard copy; **multiple templates**
  (built-in chaos-viewer + user TOML under `~/.config/chaos/templates`)
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

# Multi-repo profiles (saved under ~/.config/chaos/projects.toml)
chaos projects add sm64ds --source https://github.com/you/sm64ds-decomp --use-now
chaos projects list
chaos --project sm64ds
# TUI: p opens the projects hub; active project resumes on next launch

# Non-interactive
chaos stats --input path/to/chaos-db.json
chaos list --input path/to/chaos-db.json --priority nearly
chaos prompt --input path/to/chaos-db.json --id 'module:0x02000000'
chaos prompt --id '…' --template short
chaos templates list
chaos templates default short
```

Prompt templates: [`docs/prompt-templates.md`](docs/prompt-templates.md). In the
TUI Prompt page: **`t`** cycles, **`n`** new template, **`e`** edit current
user template (`$EDITOR`/`nano`), **`Shift+t`** sets the default.

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
