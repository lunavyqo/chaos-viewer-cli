# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Fixed

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
  discovery): Overview, Heatmap, Priorities, Prompt, Claims.
- **Multi-repo projects** (`~/.config/chaos/projects.toml`): hub (`p`), save /
  switch / delete, resume last project; CLI `chaos projects …`, `--project` /
  `CHAOS_PROJECT`. Per-project **conventions** (`default` | `experimental`) and
  **`local_repo`** (TUI **`r`** or `chaos projects local-repo`).
- Overview: match filter (`m`), module sort (`s`), detail pane with scroll,
  batch badges; **Heatmap** treemap; priority lists (nearly / scaffolded /
  biggest).
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
