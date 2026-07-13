# Saved projects (multi-repo)

`chaos` can remember multiple decomp atlases and switch between them.

## Files

```text
~/.config/chaos/            # or $CHAOS_HOME
  config.toml               # active_project = "sm64ds"
  projects.toml             # profile list
```

## Important: what gets saved

Profiles store the **source you typed** (GitHub repo URL or path), **not** the
discovered raw `chaos-db.json` URL. That way switching projects re-runs discovery
for the right repo instead of reloading a stashed electroplankton/sm64 raw file.

Suggested ids use the **repo name** (`sm64ds-decomp`), never a mangled full URL.

Each profile also stores a **data-tracking convention** (see below).

## Conventions

Per-project switch for how this CLI tracks / interprets atlas data:

| Convention | Meaning |
|---|---|
| **default** | Current chaos-viewer / sm64ds-compatible behavior. Keep sm64ds (and other upstream-shaped repos) on this. |
| **experimental** | Opt-in fork for alternate tracking. Divergences from default are listed below; future tracking experiments apply only here so default work stays stable. |

Missing `convention` keys in older `projects.toml` files load as **default**.

### Experimental divergences (so far)

1. **Match method + classic author**  
   - **`author`** — who matched (GitHub login); same as default / chaos-viewer  
   - **`matchProvenance`** — how: `human` or `ai` with **model / reasoning / harness**  

   Experimental requires complete `matchProvenance` on matched functions; credit
   still uses `author` only. Default profiles never require provenance.

2. **Every attempt logged**  
   Experimental work should append **every try** (including `no_progress` and
   non-improving near-misses) to the decomp’s attempt log (e.g.
   `config/match_attempts.jsonl` via `tools/log_attempt.py`). The
   `chaos-experimental` prompt requires a MATCH_RESULT per function per
   iteration. Full history is **not** stuffed into `chaos-db.json` (size);
   the atlas keeps lean credit + final how.

3. **Stock prompt `chaos-experimental`**  
   Emits `MATCH_RESULT` with `author` (credit) + `matchProvenance` (method) +
   attempt status/scores. Auto-selected when loading an experimental profile.

## CLI

```bash
# Add profiles
chaos projects add sm64ds --source https://github.com/tangosdev/sm64ds-decomp --use-now
chaos projects add my-exp --source https://github.com/you/my-repo --convention experimental
chaos projects add electro --source /path/to/electroplankton/chaos-db.json
chaos projects add ep-url --source https://raw.githubusercontent.com/…/chaos-db.json

# List / select / convention
chaos projects list
chaos projects use electro
chaos projects convention my-exp experimental
chaos projects convention sm64ds default
chaos projects remove old-id
chaos projects dir

# Use without changing active default
chaos --project sm64ds
chaos stats --project electro
export CHAOS_PROJECT=sm64ds
```

## TUI

- Startup with no flags resumes **`active_project`** when set.
- **`p`** anytime opens the **Projects hub** (same as first screen).
- Hub defaults to the **saved project list**.
- Hub keys:
  - **j/k** — select saved project
  - **enter** — load selected project (or typed source if input focused)
  - **v** — cycle selected project’s convention (`default` ↔ `experimental`) and save
  - **tab** — focus list ↔ freeform source input
  - **type** — start a source path / URL (switches focus to the input)
  - **Shift+s** — save current source as a named profile (type id, enter)
  - **d** — delete selected profile (asks **y/n** first)
  - **esc** — back to Overview if something is already loaded

Header shows the active profile id and convention when loaded.
List rows show `[default]` / `[experimental]` next to each id.
