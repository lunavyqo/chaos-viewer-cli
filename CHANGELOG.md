# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Fixed

- Claims markdown parser no longer panics on Unicode placeholder rows (em dashes)
  such as empty electroplankton-style `CLAIMS.md` tables; also accepts that
  layout's Module/Range/Claimant columns.

### Changed

- TUI controls are always visible: numbered tabs (`1`–`5`), highlighted key hints
  in a bottom controls bar, and a `?` help overlay. `esc` no longer quits; use `q`.
- Batched functions show violet **`[B1]` / `[B2]`…** badges in Overview and
  Priorities lists, header batch count, detail/prompt titles, and a prompt roster.

### Fixed

- List row colors no longer look uniformly “stuck” until you move the cursor:
  styles now use full `Style::reset()` so highlight/background does not leak
  across function list rows.
- Selection no longer tints the cursor row and everything below it permanently:
  Overview/Priorities no longer use ratatui `List` (its highlight path was
  unreliable here). Rows are drawn manually with solid backgrounds and a
  full pane clear each frame.

### Added

- Initial Rust CLI/TUI scaffold for Chaos Viewer atlas browsing.
- Schema-compatible load of `chaos-db.json` from path, URL, or GitHub repo discovery.
- Priority lists (nearly done, best scaffolded, biggest unmatched).
- Prompt builder with clipboard support and optional claims session footer.
- Read-only claims merge from claims API and `CLAIMS.md`.
- Interactive TUI with overview, priorities, detail, prompt, and claims views.
