# Chaos Viewer CLI

A terminal progress atlas for matching-decompilation projects. Point it at any
project that publishes Chaos Viewer data (`chaos-db.json`), then browse modules
and functions, rank what is worth matching next, build AI matching prompts, and
watch optional live claims.

Inspired by [tangosdev/chaos-viewer](https://github.com/tangosdev/chaos-viewer).
Schema-compatible with that project's `ADAPTING.md` data format.

## Status

**v0.1.0** — Binary name: `chaos`. Schema-compatible with Chaos Viewer atlases.

## Install

### Prebuilt binaries (recommended)

Download the archive for your platform from
[GitHub Releases](https://github.com/lunavyqo/chaos-viewer-cli/releases):

| Platform | Target triple |
|---|---|
| Linux x86_64 | `x86_64-unknown-linux-gnu` |
| Windows x86_64 | `x86_64-pc-windows-msvc` |
| macOS Intel | `x86_64-apple-darwin` |
| macOS Apple Silicon | `aarch64-apple-darwin` |

Unpack and put `chaos` (or `chaos.exe`) on your `PATH`.

### From source

## Features

- Load atlas data from a local path, raw JSON URL, or GitHub repo (probes the same
  locations as the web viewer); **multiple saved projects** with switch / resume
  and per-project **conventions** (`default` vs `experimental` tracking)
- Overview: modules + functions on top, **detail pane** underneath; **`m`**
  filters all / unmatched only / matched only; **`s`** sorts modules (name,
  worst/best by unmatched left, most functions, most bytes)
- **Tools page (`5`)**: card catalog of typical decomp instruments (what each
  does / changes); ★ when found under project `local_repo`
- Priority lists: nearly done, best scaffolded, biggest unmatched
- Prompt builder (batch, max 16) with clipboard copy, **`g`** / **`Shift+g`** to
  launch **Grok / Codex / Claude / Antigravity** in a separate terminal (set
  `local_repo` per project; default agent configurable), and **`Shift+b`** clear
  batch; **multiple templates** (built-in `chaos-viewer` + experimental
  provenance prompt + user TOML under `~/.config/chaos/templates`)
- Optional **pluggable claims** coordination: any HTTP coordinator via
  `project.claimsApi` (not hardcoded to one host) + `CLAIMS.md` merge

## Requirements

- Rust 1.75+ (edition 2021; CI uses current stable)
- Network access for remote data / claims (optional for pure local files)

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
# Local decomp checkout for Grok launch (g in TUI) — independent of atlas URL
chaos projects local-repo sm64ds ~/path/to/sm64ds-decomp
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
user template (`$EDITOR`/`nano`), **`Shift+t`** sets the default. For
experimental provenance, **`m`** opens the model picker; **`y`** / **`w`**
cycle reasoning · harness (prefilled into `MATCH_RESULT`).

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

**System maps:**
[`docs/architecture.md`](docs/architecture.md) (chaos-focused) ·
[`docs/ecosystem.md`](docs/ecosystem.md) (full stack around a generic decomp
repo: tools by role, ledgers, agents, web viewer).

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
