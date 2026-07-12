# Saved projects (multi-repo)

`chaos` can remember multiple decomp atlases and switch between them.

## Files

```text
~/.config/chaos/            # or $CHAOS_HOME
  config.toml               # active_project = "sm64ds"
  projects.toml             # profile list
```

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
  - **j/k** — select saved project
  - **tab** — focus list ↔ freeform source input
  - **enter** — load selected project, or load typed source
  - **s** — save current source as a named profile (type id, enter)
  - **d** — delete selected profile
  - **esc** — back to Overview if something is already loaded

Header shows the active profile id when loaded.
