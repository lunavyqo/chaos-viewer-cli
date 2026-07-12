# Prompt templates

`chaos` ships one **built-in** prompt (`chaos-viewer`) that matches
[tangosdev/chaos-viewer](https://github.com/tangosdev/chaos-viewer). You can add
more templates, pick them in the TUI, and set your own default.

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
| `Shift+t` | Save current as default |
| `c` | Copy rendered prompt |

Title shows the template name; `★` means it is the saved default.

## CLI

```bash
chaos prompt --id 'arm9:0x02000000' --repo https://github.com/you/decomp
chaos prompt --id '…' --template short
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
