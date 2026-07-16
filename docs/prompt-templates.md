# Prompt templates

`chaos` ships **built-in** prompts plus optional user TOML templates. You can
pick them in the TUI and set a default.

| Built-in id | When to use |
|-------------|-------------|
| **`chaos-viewer`** | Default / sm64ds — matches [tangosdev/chaos-viewer](https://github.com/tangosdev/chaos-viewer) |
| **`chaos-experimental`** | Experimental — match task + **`author`** (GitHub credit, prefilled from claims handle when set) + **`matchProvenance`** (how only: model / reasoning / harness, or human) |

Loading an **experimental** project while the active template is still
`chaos-viewer` auto-selects `chaos-experimental` (custom templates are left alone).

## Where files live

Override the root with `CHAOS_HOME` (useful in tests). Otherwise:

```text
~/.config/chaos/                 # or $XDG_CONFIG_HOME/chaos
  config.toml                    # default_template = "chaos-viewer"
  templates/
    short.toml                   # seeded example on first run
    my-style.toml                # your templates (id = file stem)
```

```bash
chaos templates dir
chaos templates list
chaos templates default short    # set default
chaos templates default          # print default
```

## TUI

On the **Prompt** page (`4`):

| Key | Action |
|-----|--------|
| `t` | Next template |
| `n` | **New** template: type an id, Enter → creates a chaos-viewer copy and opens the editor |
| `e` | **Edit** current user template in `$EDITOR` / nano (not the built-in) |
| `Shift+t` | Save current as default |
| `m` | **Model picker** (fixed list, like agent picker) → prefills `matchProvenance.model` |
| `y` | Cycle **reasoning** level: `high` → `medium` → `low` → `none` |
| `w` | Cycle **harness** preset: `grok-build` · `cursor-agent` · `claude-code` · `codex` · `antigravity` · `manual` |
| `c` | Copy rendered prompt |
| `g` | Launch the **default** coding agent in a new terminal (chaos stays open) |
| `Shift+g` | **Agent picker**: Grok / Codex / Claude / Antigravity · enter launch · **`d`** set default |
| `d` | Toggle **stored near-miss drafts** (details `draft` and/or local `nearmiss/db.jsonl`) — off = ignore them |
| `h` | Toggle **Ghidra C draft** (from `local_repo/ghidra_out` or detail draft) |

### Provenance pickers (experimental MATCH_RESULT)

On the Prompt page, **model / reasoning / harness** are pickers so you do not retype
them into every try. Selection is saved in `~/.config/chaos/config.toml` and
prefilled into the `chaos-experimental` `MATCH_RESULT.matchProvenance` block.

| Setting | Source | Keys |
|---------|--------|------|
| **model** | Fixed built-in list (picker menu) | **`m`** open · j/k · enter |
| **reasoning** | Fixed: `high` · `medium` · `low` · `none` | `y` |
| **harness** | Fixed presets (not free-form) | `w` |

Models (display → slug): Grok 4.5, Composer 2.5, Claude Sonnet 5,
Claude Opus 4.8/4.7/4.6, Claude Fable 5, GPT 5.6 Luna/Terra/Sol,
DeepSeek V4 Flash/Pro, GLM 5.2, Kimi K3, Hy3, StepFun 3.7, Muse Spark 1.1,
Gemini 3.5 Pro/Flash.

```toml
# ~/.config/chaos/config.toml (written automatically by the TUI)
provenance_model = "grok-4.5"
provenance_reasoning = "high"
provenance_harness = "grok-build"
```

`chaos prompt` also reads these values when rendering experimental templates.

**Fresh matching (no existing C):** turn **drafts off** (`d`) and optionally Ghidra off
(`h`), so the prompt is disasm + verify only.

Ghidra scaffolds (optional): dump with the decomp’s `tools/ghidra_dump.py`, then set
`local_repo` so chaos finds `ghidra_out/`. CLI:

```bash
chaos prompt --id '…'                 # drafts + Ghidra on by default
chaos prompt --id '…' --no-drafts     # ignore stored near-miss C
# Local tip C (sm64ds-shaped): set CHAOS_LOCAL_REPO=/path/to/decomp or cwd with nearmiss/db.jsonl
chaos prompt --id '…' --no-ghidra     # ignore Ghidra scaffolds
chaos prompt --id '…' --ghidra-dir PATH
```

Title shows the template name; `★` means it is the saved default.

### Launch coding agents (`g` / `Shift+g`)

Supported CLIs (interactive TUI in a **separate terminal**):

| Agent | Binary | Notes |
|---|---|---|
| **Grok Build** (default) | `grok` | `--fullscreen` + bootstrap; opt-in headless via `grok_mode` |
| **Codex** | `codex` | `codex -C <repo> "<bootstrap>"` |
| **Claude Code** | `claude` | `claude --add-dir <repo> "<bootstrap>"` |
| **Antigravity** | `agy` | `agy --add-dir <repo> --prompt-interactive "<bootstrap>"` |

Set the **local decomp path** so tools run in the right tree:

```bash
chaos projects local-repo <id> /path/to/your-decomp
```

Resolution order for repo cwd + prompt preamble:

1. Profile `local_repo` in `projects.toml`
2. `grok_default_repo` in `config.toml` (shared fallback name)
3. Heuristic from a **local** atlas path (not GitHub URLs)

Optional keys in `~/.config/chaos/config.toml`:

```toml
default_agent = "grok"            # grok | codex | claude | antigravity  (`g` uses this)
grok_bin = "grok"
codex_bin = "codex"
claude_bin = "claude"
antigravity_bin = "agy"
grok_mode = "interactive"         # Grok only: interactive | run (headless)
grok_extra_args = []
codex_extra_args = []
claude_extra_args = []
antigravity_extra_args = []
grok_default_repo = "~/src/my-decomp"
grok_terminal = "auto"            # auto | terminal | iterm | linux | windows
```

- Full match text is written to `~/.config/chaos/last-agent-prompt.md` (and
  legacy `last-grok-prompt.md`). Agents get a short bootstrap pointing at that
  file so argv stays small.
- Launcher script: `~/.config/chaos/last-agent-run.command` (macOS `open`).
- Prompt is always copied to the clipboard as a fallback.
- In the picker, **`d`** saves `default_agent` for next **`g`**.

### New template flow

1. Press **`n`** on Prompt.
2. Edit the id (default `my-template`; letters, digits, `-`, `_` only).
3. **Enter** writes `~/.config/chaos/templates/<id>.toml` as an editable copy of the
   built-in chaos-viewer layout, then opens it in your editor.
4. Save and quit the editor. Chaos reloads the list and selects your new id.

Editor resolution: **`$VISUAL`**, then **`$EDITOR`**, then **`nano`**.

Examples:

```bash
export EDITOR=vim
export VISUAL='code -w'   # VS Code, wait until closed
```

## CLI

```bash
chaos prompt --id 'arm9:0x02000000' --repo https://github.com/you/decomp
chaos prompt --id '…' --template short

# Create + open in editor (same scaffold as TUI n)
chaos templates new my-style
chaos templates new my-style --name "My style"
chaos templates new my-style --no-edit   # only write the file

# Edit an existing user template (default id if omitted)
chaos templates edit my-style
chaos templates edit
```

## User template format (TOML)

```toml
name = "Short"
description = "Optional blurb for templates list"

header = """
Match {n} {project_name} function(s).
Compiler: {compiler}
"""

# Required. Emitted once per batched function.
function = """
======================================================================
FUNCTION: {name}   module: {module}   addr: 0x{addrHex}   size: {size}
{section_verify}
{section_sibling}
{section_draft}
{section_disasm}
"""

footer = """
{section_claims}
Rules: {rules}
"""
```

Rendered as `header`, then each `function`, then `footer`, joined with blank lines
(`\n\n`). Empty `header` / `footer` are skipped.

### Placeholders — header / footer

| Token | Meaning |
|-------|---------|
| `{n}` | Batch size (or 1) |
| `{project_name}` | `project.name` |
| `{github}` | Project GitHub URL |
| `{github_target}` | ` to {github}` or empty |
| `{compiler}` `{setup}` `{rules}` `{read_first}` `{cpp_note}` `{near_miss_note}` | From atlas `project` block |
| `{claims_api}` | Claims API base if any |
| `{section_claims}` | Full claims agent block if session env is set; else empty |

### Placeholders — function body

| Token | Meaning |
|-------|---------|
| `{name}` `{module}` `{id}` | Function identity |
| `{addr}` `{addrHex}` `{size}` `{sizeHex}` | Address / size |
| `{verify}` | Filled `project.verifyCommand` |
| `{section_verify}` | `VERIFY:` block or empty |
| `{sibling}` `{sim}` `{section_sibling}` | Scaffold sibling |
| `{floor}` `{section_floor}` `{div}` `{cat}` `{author}` | Metadata |
| `{draft}` `{draft_div}` `{section_draft}` | Near-miss draft (fenced) |
| `{disasm}` `{section_disasm}` | Annotated disassembly (truncated) |
| `{pool}` | Pool lines joined |

The built-in `chaos-viewer` id is **not** a TOML file; it always uses the compiled
web-parity builder.
