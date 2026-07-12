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

## CLI

```bash
# Add profiles
chaos projects add sm64ds --source https://github.com/tangosdev/sm64ds-decomp --use-now
chaos projects add electro --source /path/to/electroplankton/chaos-db.json
chaos projects add ep-url --source https://raw.githubusercontent.com/…/chaos-db.json

# List / select
chaos projects list
chaos projects use electro
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
- Hub keys:
  - **type** — source path / URL (always; focuses the input line)
  - **tab** — focus list ↔ freeform source input
  - **j/k** — select saved project (list focused)
  - **enter** — load typed source, or selected project if list focused
  - **Shift+s** — save current source as a named profile (type id, enter)
  - **d** — delete selected profile (list focused)
  - **esc** — back to Overview if something is already loaded

Header shows the active profile id when loaded.
