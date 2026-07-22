# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- **Mass batcher** — multiple prompt batches (1, 2, 3, …). Switch with **`,`** /
  **`.`** (or **`<`** / **`>`**). **`+`** / **`=`** opens a new empty batch after
  the active slot (no-op if active is already empty). Each slot still max 16
  functions; adding past 16 also auto-opens the next batch. Badges show `[B3]`
  for a single batch or `[2:3]` when multi. **`g`** / agent picker launch
  **every non-empty batch** in its own agent window (preferred agent); handoff
  files use `last-agent-prompt-batchN.md` so windows do not clobber each other.
  **`c`** and the Prompt page still use the active slot; **Shift+b** clears the
  active slot only.

- **Priorities: Smallest functions** — new sub-list (cycle with `n`, or
  `chaos list --priority smallest`): unmatched, unclaimed, no floor, sorted by
  size ascending (whole-repo quick wins). Top 25, same filters as Biggest.

- **Local near-miss tip DB** — when `local_repo` points at a decomp with
  `nearmiss/db.jsonl` (sm64ds-shaped tip C + `div`), prompts include that tip even
  if published details lack a draft. CLI: `CHAOS_LOCAL_REPO` or decomp cwd; TUI:
  project `local_repo`. Toggle still **`d`** / `--no-drafts`.
- **Architecture + ecosystem maps** —
  [`docs/architecture.md`](docs/architecture.md) (chaos layers / knobs) and
  [`docs/ecosystem.md`](docs/ecosystem.md) (generic decomp stack: tool roles,
  ledgers, agents, web viewer, god graph).
- **Tools page (`5`)** — card grid of typical decomp instruments (purpose +
  what each changes). Filters by category (`n`); marks ★ when found under the
  project `local_repo`.
- **Prompt builder: Ghidra C draft** — when available, prompts can include a
  decompiler scaffold (`ghidra_out/0x….c` or a detail draft tagged GHIDRA
  SCAFFOLD). TUI Prompt page: **`h`** toggles; CLI: default on, `--no-ghidra` to
  disable, `--ghidra-dir` / `CHAOS_GHIDRA_DIR` to set the dump folder. Labeled as
  approximate — not a match.
- **Prompt builder: skip stored drafts** — force matching without existing
  near-miss / NONMATCHING C. TUI Prompt: **`d`** toggles; CLI: `--no-drafts`.
  (Ghidra remains controlled separately via **`h`** / `--no-ghidra`.)
- **Experimental MATCH_RESULT draft trackers** — required independent booleans
  `usedNearMissDraft` and `usedGhidraDraft` (pre-filled from what the prompt
  included). **Inherit** from `parentAttemptId`: if an ancestor used Ghidra
  (or a near-miss draft), descendants keep that flag true. `base.kind` may be
  `ghidra_scaffold` or `mixed`.
- **Prompt provenance pickers** — on the Prompt page, **`m`** opens a **model
  picker** (fixed list, agent-picker style); **`y`** / **`w`** cycle reasoning
  and harness. Selection is stored in `config.toml` and prefilled into
  experimental `MATCH_RESULT.matchProvenance` (no retyping each try).

### Changed

- **Default prompts: mandatory CLAIMS.md + permuter cleanup** — stock
  `chaos-viewer` (and shared template claims block) now hard-require reading /
  updating **CLAIMS.md**, claiming before edits, releasing when done, and
  **tree-killing permuter workers** on session end. Live API try-lock remains
  when a session key is present, but no longer replaces CLAIMS.md.

- **TUI header progress** now shows both function and byte match meters
  (`matched/total fn (%)` and `matched/total B (%)`), matching `chaos stats`.
  Load / update status lines include the same pair.

- **Experimental attempt logging** is an **attempt tree**, not a flat diary:
  `MATCH_RESULT` requires tree links (`parentAttemptId`, `base`) plus **stable
  identity**: `schemaVersion`, atlas `functionId`, unique `attemptId`
  (ULID/UUID — not `a1`). Do **not** log wall-clock times. Dead ends stay
  siblings; improved near-misses become the node you continue from.
  Documented in `docs/projects.md`; stock `chaos-experimental` prompt updated.

### Removed

- **Heatmap tab** (terminal squarify treemap). A full chaos-viewer-style map does
  not translate well to a TUI; Overview + Priorities remain the navigation
  surface. Pages are now **1** Overview · **2** Priorities · **3** Prompt ·
  **4** Claims · **5** Tools. The unused `treemap` module is gone.

### Fixed

- **Default prompt leaked experimental jargon** — shared draft-policy text told
  agents about `MATCH_RESULT` / `usedNearMissDraft` / `usedGhidraDraft` even on
  the stock `chaos-viewer` (sm64ds / default convention) template. Agent launch
  preamble also stopped hard-coding `log_attempt` (experimental). Those fields
  remain only on `chaos-experimental`.

- **Priorities batch/`b` ignored cursor** — j/k only moved the list highlight;
  `selected_id` stayed on the Overview pick, so every `b` toggled the same
  function. Cursor and selection now stay in sync (including on mode cycle /
  screen enter).

- **Experimental prompt logging rules** — status: `matched` only after verify,
  `near_miss` only when the tip improves, else `no_progress`. Agents **must**
  call `log_attempt` every try and `stamp_provenance` (or bank-how) on MATCH;
  bank is not a new try. Privacy wording no longer names removed timestamp
  fields (so prompts never suggest inventing them). Tools catalog adds
  `stamp_provenance.py` and clarifies bank/log roles.
- **TUI performance (major):** stop free-spinning at ~60 fps. The event loop
  now redraws only after input/resize/state changes, so idle CPU is near zero
  and key handling is not fighting continuous full-frame paints.
- Claims merge indexes ranges by module instead of O(functions × claims).
- Overview search: ASCII case-insensitive match without lowercasing every
  name/id string on each rebuild.
- **Module detail hitch:** first open of a module no longer freezes the UI for
  ~1–2s while `details/{module}.json` downloads/parses. Chunks load in the
  background (max 2 concurrent), the session prewarms all modules after load
  (selected + neighbors first), and `h`/`l` stay responsive with a loading
  detail pane until the chunk lands.
- HTTP: enable **gzip** on reqwest (atlases were downloaded uncompressed —
  ~2 MB instead of ~200 KB). Timeout back to a normal 60s, not 180s.
- Removed automatic prefetch of **every** module detail chunk (arm9 alone is
  multi‑MB; full prefetch was a network storm). Details load on demand again.
- Overview lag when switching modules / scrolling large lists: per-module
  function index, viewport-only drawing, coalesced **h/l**.
- Repo reopen freeze: GitHub atlas load no longer **downloads the multi‑MB
  `chaos-db.json` twice** (discovery + second fetch). Normal loads skip
  cache-bust query params so CDNs can help. Saved projects cache the last
  raw `atlas_url` for a single-GET reopen.

## [0.1.0] — 2026-07-13

First public release of **chaos-viewer-cli** (`chaos` binary).

### Added

- Interactive TUI for Chaos Viewer `chaos-db.json` atlases (path, URL, or GitHub
  discovery): Overview, Priorities, Prompt, Claims.
- **Multi-repo projects** (`~/.config/chaos/projects.toml`): hub (`p`), save /
  switch / delete, resume last project; CLI `chaos projects …`, `--project` /
  `CHAOS_PROJECT`. Per-project **conventions** (`default` | `experimental`) and
  **`local_repo`** (TUI **`r`** or `chaos projects local-repo`).
- Overview: match filter (`m`), module sort (`s`), detail pane with scroll,
  batch badges; priority lists (nearly / scaffolded / biggest).
- Prompt builder (batch max 16): copy (`c`), clear batch (`Shift+b`), templates
  (`t` / `n` / `e` / `Shift+t`) — built-in `chaos-viewer` +
  `chaos-experimental` + user TOML under `~/.config/chaos/templates/`.
- **Agent launch** (`g` default, `Shift+g` picker): **Grok Build**, **Codex**,
  **Claude Code**, **Antigravity** (`agy`) in a separate terminal; configurable
  `default_agent` and per-agent bins/args; prompt handoff via
  `last-agent-prompt.md` + `last-agent-run.command` (macOS `open`).
- Experimental tracking: `matchProvenance` (how), classic `author` (who);
  attempt logging + required `sessionScope` in the stock experimental prompt.
- Pluggable claims client/CLI (`project.claimsApi` or `--api`); `CLAIMS.md`
  merge; **`u`** re-fetch progress.
- Non-interactive: `stats`, `list`, `prompt`, `templates`, `claims`.
- **GitHub Release CI**: multi-platform binaries on tag `v*`
  (Linux x86_64, Windows x86_64, macOS Intel, Apple Silicon).

### Fixed

- Projects hub: list-first focus; Shift+s save; confirmed delete; no raw atlas
  URL sticky profiles.
- Overview performance and list/theme painting; claims markdown Unicode rows.
- Prompt builds from batch only (not stray Overview selection).

## Links

- [0.1.0]: https://github.com/lunavyqo/chaos-viewer-cli/releases/tag/v0.1.0
