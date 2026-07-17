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

Each profile also stores a **data-tracking convention** (see below) and an
optional **`local_repo`** path (the decomp checkout on this machine, used when
launching Grok with `g` in the TUI).

## Local decomp path (`local_repo`)

Atlas `source` is often a **GitHub URL**. Grok still needs the **local clone**
to run tools (`bank`, `log_attempt`, edit `src/`). Set it once per profile:

```bash
# Path may use ~/
chaos projects local-repo electroplankton ~/Documents/SGH/electroplankton-decomp

# Or when adding
chaos projects add electro \
  --source https://github.com/you/electroplankton-decomp \
  --local-repo ~/Documents/SGH/electroplankton-decomp \
  --convention experimental \
  --use-now

# Clear
chaos projects local-repo electro -
chaos projects list   # shows local_repo per profile
```

Fallback if a profile has no `local_repo`: `grok_default_repo` in
`~/.config/chaos/config.toml`, then a heuristic from a local atlas path only.

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

2. **Every attempt logged as a tree**  
   Experimental work should append **every try** (including `no_progress` and
   non-improving near-misses) to the decomp’s attempt log (e.g.
   `config/match_attempts.jsonl` via `tools/log_attempt.py`). Best tip **C**
   belongs in `nearmiss/db.jsonl` (pass `--src` on near_miss logs). The
   `chaos-experimental` prompt requires a **MATCH_RESULT node** per function
   per try, with **stable ids + tree links** so history is not a flat diary:

   | Field | Role |
   |---|---|
   | `schemaVersion` | `1` — bump only when meanings change |
   | `functionId` | Atlas `function.id` (stable; never name alone) |
   | `attemptId` | Unique id for **this** node (ULID/UUID; never reuse `a1`/`try2`) |
   | `parentAttemptId` | Node you built on (`null` = new root/branch) |
   | `base.kind` | `scratch` · `previous_attempt` · `near_miss_draft` · `ghidra_scaffold` · `matched_sibling` · `mixed` |

   Privacy: do **not** log wall-clock times (`loggedAt` / `ts`).
   | `usedNearMissDraft` | **true/false** — this try used a near-miss draft **or** any ancestor did |
   | `usedGhidraDraft` | **true/false** — this try used Ghidra **or** any ancestor did (lineage sticks) |

   Inheritance: when `parentAttemptId` is set, each flag is
   `(this try) OR (parent's flag)`. Example: Ghidra → near_miss → match keeps
   `usedGhidraDraft: true` on the match even if Ghidra was not re-opened.
   | `divergences` / `improvedNearMiss` | Score vs previous best |

   Sketch (one function over a few sessions):

   ```text
   functionId = arm9:0x020009e0
   ├─ 01JA…  near_miss div=40   parent=null   base=scratch
   │  ├─ 01JB…  no_progress     parent=01JA…  (settings retry, no win)
   │  └─ 01JC…  near_miss div=12 parent=01JA… improved — new branch tip
   │     └─ 01JD…  matched      parent=01JC…  continued from best near-miss
   ```

   Full history stays out of `chaos-db.json` (size); the atlas keeps lean credit
   + final how. The jsonl is where the tree lives.

3. **Stock prompt `chaos-experimental`**  
   Emits `MATCH_RESULT` with tree ids, `author` (credit) + `matchProvenance`
   (method) + attempt status/scores. Auto-selected when loading an experimental
   profile. Model / reasoning / harness are chosen once in the TUI (`m` model
   picker · `y` / `w` for reasoning / harness) and prefilled into each
   `MATCH_RESULT` so operators do not retype them every try.

## CLI

```bash
# Add profiles
chaos projects add sm64ds --source https://github.com/tangosdev/sm64ds-decomp --use-now
chaos projects add my-exp --source https://github.com/you/my-repo --convention experimental
chaos projects add electro --source /path/to/electroplankton/chaos-db.json
chaos projects add ep-url --source https://raw.githubusercontent.com/…/chaos-db.json

# List / select / convention / local_repo
chaos projects list
chaos projects use electro
chaos projects convention my-exp experimental
chaos projects convention sm64ds default
chaos projects local-repo electro ~/path/to/electroplankton-decomp
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
  - **r** — set **local decomp path** for the selected project (Grok `g` cwd).
    Prefills the current value; type a path (`~/…` ok), **enter** to save
    (must be an existing directory), empty + enter clears, **esc** cancels.
  - **tab** — focus list ↔ freeform source input
  - **type** — start a source path / URL (switches focus to the input)
  - **Shift+s** — save current source as a named profile (type id, enter)
  - **d** — delete selected profile (asks **y/n** first)
  - **esc** — back to Overview if something is already loaded

Header shows the active profile id and convention when loaded.
List rows show `[default]` / `[experimental]` and `local:…` / `local:(unset)`
next to each id.
