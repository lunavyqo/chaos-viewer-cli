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

### Added

- Initial Rust CLI/TUI scaffold for Chaos Viewer atlas browsing.
- Schema-compatible load of `chaos-db.json` from path, URL, or GitHub repo discovery.
- Priority lists (nearly done, best scaffolded, biggest unmatched).
- Prompt builder with clipboard support and optional claims session footer.
- Read-only claims merge from claims API and `CLAIMS.md`.
- Interactive TUI with overview, priorities, detail, prompt, and claims views.
