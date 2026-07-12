# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Overview **detail pane scroll**: `PgUp`/`PgDn` (page) and `[`/`]` (line);
  title shows `lines a–b/total`. Full draft/disasm/pool are included (no longer
  hard-truncated to a few lines).

### Changed

- **Detail is part of Overview**: modules and functions on top, a full-width
  detail pane underneath (no separate Detail tab). Tabs are now 1 Overview ·
  2 Heatmap · 3 Priorities · 4 Prompt · 5 Claims. Detail loads as you move
  `j`/`k` on Overview. From Priorities, **enter** jumps to Overview with that
  function selected.

### Added

- Overview **`m` match filter**: cycle all → unmatched only → matched only
  (hide finished or hide open work in the function list).

### Fixed

- Prompt page / `c` copy no longer fall back to the Overview cursor when the
  batch is empty; prompts are built from the batch only (add with `b` first).

### Added

- **Heatmap** TUI screen (`2` / tab): view-only squarified byte treemap (same
  layout math as chaos-viewer), painted with block glyphs (`░` unmatched, `▓`
  matched, `▒` claimed, `█` selected). Selection comes from Overview/Priorities;
  no heatmap-local controls. Tabs renumbered: 1 Overview · 2 Heatmap ·
  3 Priorities · 4 Detail · 5 Prompt · 6 Claims.

### Fixed

- Claims markdown parser no longer panics on Unicode placeholder rows (em dashes)
  such as empty electroplankton-style `CLAIMS.md` tables; also accepts that
  layout's Module/Range/Claimant columns.

### Added

- **`u` update progress** in the TUI: re-fetch the current chaos-db (and claims)
  so match % / function lists stay current while you work; keeps screen, module,
  selection, and batch entries that still exist.
- **Pluggable claims coordinator** client + CLI (`chaos claims list|try-lock|renew|release|instructions|github-exchange`).
  Uses `project.claimsApi` (any host) or `--api`; never hardcodes a vendor.
  Documented in `docs/claims-api.md`.

### Fixed

- Prompt builder text matches chaos-viewer (`promptHeader` / `promptSection` /
  `promptFooter`); TUI always lazy-loads detail chunks (disasm/draft/pool) for
  single and batch prompts before preview/copy, same as the web app.

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
- Theme uses terminal-safe colours (ANSI + 256-colour greys, not 24-bit RGB)
  for macOS 12 Terminal.app compatibility; each list cell is fully repainted
  every frame. Background is charcoal grey rather than pure black.

### Added

- Initial Rust CLI/TUI scaffold for Chaos Viewer atlas browsing.
- Schema-compatible load of `chaos-db.json` from path, URL, or GitHub repo discovery.
- Priority lists (nearly done, best scaffolded, biggest unmatched).
- Prompt builder with clipboard support and optional claims session footer.
- Read-only claims merge from claims API and `CLAIMS.md`.
- Interactive TUI with overview, priorities, detail, prompt, and claims views.
