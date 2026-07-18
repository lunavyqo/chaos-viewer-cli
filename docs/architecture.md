# Architecture map — chaos-viewer-cli

One-page map of the whole system: what lives where, what talks to what, and
which knobs matter. Use this when the TUI / config / provenance surface feels
too large to hold in your head.

**Want every role: tools, ledgers, agents, god graph (generic decomp)?** →
[`ecosystem.md`](ecosystem.md).

Related detail docs:

| Doc | Scope |
|-----|--------|
| [`schema.md`](schema.md) | `chaos-db.json` fields, provenance, attempt log |
| [`projects.md`](projects.md) | Multi-repo profiles, conventions |
| [`prompt-templates.md`](prompt-templates.md) | Templates + provenance pickers |
| [`claims-api.md`](claims-api.md) | Optional lock coordinator |

---

## 1. Big picture — three worlds

Chaos is **not** the decompiler and **not** the model. It is a **browser +
prompt factory** that sits between published atlas data and your matching tools.

```mermaid
flowchart TB
  subgraph remote["Remote / published"]
    GH["GitHub decomp repo"]
    ATLAS["chaos-db.json + details/*.json"]
    CLAIMS["Claims API optional"]
  end

  subgraph chaos["chaos binary TUI + CLI"]
    LOAD["Load / discover atlas"]
    TUI["TUI browse · batch · prompt"]
    PROMPT["Prompt builder"]
    LAUNCH["Agent launch g / Shift+g"]
  end

  subgraph local["Your machine"]
    CFG["~/.config/chaos/"]
    REPO["local_repo decomp checkout"]
    GHIDRA["ghidra_out/ optional"]
    AGENT["Grok / Codex / Claude / Cursor / …"]
  end

  subgraph decomp_tools["Inside the decomp repo optional"]
    VERIFY["match / verify scripts"]
    LOG["match_attempts.jsonl"]
  end

  GH --> ALAS
  ALAS --> LOAD
  CLAIMS -.->|locks| TUI
  CFG --> TUI
  LOAD --> TUI
  TUI --> PROMPT
  REPO --> PROMPT
  GHIDRA -.->|drafts| PROMPT
  PROMPT -->|clipboard or file| AGENT
  LAUNCH --> AGENT
  AGENT --> REPO
  AGENT --> VERIFY
  VERIFY --> LOG
  VERIFY -->|progress regen| ALAS
```

**Mental model:**

| World | Owns | Chaos’s job |
|-------|------|-------------|
| **Atlas** | What’s matched, names, addrs | Load, browse, rank, show |
| **Chaos config** | Projects, templates, last model/harness | Remember operator prefs |
| **Decomp + agent** | Actual C, compile, verify, attempt log | Build prompt; optional launch |

---

## 2. Layers inside the binary

```mermaid
flowchart LR
  subgraph ui["UI"]
    TUI2["tui/"]
    MAIN["main.rs CLI"]
  end

  subgraph domain["Domain"]
    LOAD2["load / discover / http"]
    SCHEMA["schema"]
    PRIO["prioritize"]
    CONV["conventions"]
    PROMPT2["prompt"]
    TPL["templates"]
    PROJ["projects"]
    CLAIMS2["claims"]
    LAUNCH2["grok_launch"]
  end

  MAIN --> LOAD2
  MAIN --> PROMPT2
  MAIN --> PROJ
  TUI2 --> LOAD2
  TUI2 --> PROMPT2
  TUI2 --> TPL
  TUI2 --> PROJ
  TUI2 --> CLAIMS2
  TUI2 --> LAUNCH2
  PROMPT2 --> SCHEMA
  TPL --> PROMPT2
  CONV --> SCHEMA
```

| Module | Responsibility |
|--------|----------------|
| `load` / `discover` / `http` | Find and fetch `chaos-db` + detail chunks |
| `schema` | Atlas types (`ChaosFunction`, `matchProvenance`, …) |
| `projects` | Saved multi-repo profiles + convention + `local_repo` |
| `conventions` | default vs experimental tracking rules |
| `prioritize` | Nearly / scaffolded / biggest / smallest lists |
| `prompt` | Built-in prompt bodies (viewer + experimental) |
| `templates` | User TOML templates + `config.toml` + provenance pickers |
| `claims` | Optional lock list / try-lock session |
| `grok_launch` | Spawn coding agent in a new terminal |
| `tui` | Screens, keys, batch, pickers |

---

## 3. TUI screens and the operator loop

```mermaid
stateDiagram-v2
  [*] --> Setup: chaos
  Setup --> Overview: load project / URL / path
  Overview --> Priorities: 2
  Priorities --> Overview: 1
  Overview --> Prompt: 3 batch not empty
  Priorities --> Prompt: 3
  Prompt --> Claims: 4
  Claims --> Overview: 1
  Overview --> Setup: p projects hub
  Prompt --> Agent: g / Shift+g
  Agent --> Prompt: agent in other terminal
```

| Screen | Job | High-traffic keys |
|--------|-----|-------------------|
| **Setup / projects** | Pick atlas source, convention, `local_repo` | enter, v, r, Shift+s |
| **Overview** | Modules + functions + detail | j/k h/l m s / b |
| **Priorities** | Ranked work queue | n b enter |
| **Prompt** | Build / copy / launch / provenance | t d h m y w c g |
| **Claims** | Read-only locks | r |

**Daily loop (happy path):**

```text
pick project → filter unmatched → b batch → Prompt
  → set model/reasoning/harness if experimental
  → d/h drafts as needed
  → c copy  or  g launch agent
  → agent matches in local_repo
  → (optional) log MATCH_RESULT → jsonl
  → u update atlas when progress lands
```

---

## 4. Config surfaces — every knob, one table

Too many settings feel worse when they live in different places. Here they are
grouped by **where they live** and **how often you touch them**.

### A. Per-project (`~/.config/chaos/projects.toml`)

| Knob | What | Touch often? |
|------|------|--------------|
| `source` | Atlas path / URL / GitHub | rare |
| `convention` | `default` \| `experimental` | rare |
| `local_repo` | Decomp checkout for agent cwd + ghidra | once per machine |

### B. Global chaos prefs (`~/.config/chaos/config.toml`)

| Knob | What | Touch often? |
|------|------|--------------|
| `default_template` | Prompt template id | rare |
| `active_project` | Last project | automatic |
| `default_agent` | `g` launches this | rare |
| `*_bin`, `*_extra_args`, `grok_mode`, `grok_terminal` | Agent plumbing | once |
| `grok_default_repo` | Fallback cwd if no `local_repo` | rare |
| **`provenance_model`** | Prefill MATCH_RESULT model | **per wave** |
| **`provenance_reasoning`** | high / medium / low / none | per wave |
| **`provenance_harness`** | which tool ran the try | per wave |

### C. Session-only (TUI memory, not config)

| Knob | What |
|------|------|
| Batch of function ids | What the prompt is about |
| Draft toggles `d` / `h` | Include near-miss C / Ghidra C |
| Search / match filter / module sort | Navigation |
| Active template id | Until you Shift+t default |

### D. Secrets / env (never in git)

| Env | What |
|-----|------|
| `CHAOS_CLAIMS_*` | Claims session |
| `CHAOS_HOME` | Config root override |
| `CHAOS_GHIDRA_DIR` | Force ghidra dump path |
| `CHAOS_PROJECT` / CLI flags | Non-TUI selection |

### E. Outside chaos (decomp owns these)

| Artifact | What |
|----------|------|
| `chaos-db.json` | Published progress atlas |
| `details/*.json` | Per-module disasm / drafts |
| `ghidra_out/` | Optional decompiler scaffolds |
| `config/match_attempts.jsonl` | Experimental attempt tree (metadata) |
| `nearmiss/db.jsonl` | Best near-miss tip **C** + `div` (sm64ds-shaped) |
| match/verify scripts | Ground truth for matched |

```mermaid
flowchart TB
  subgraph touch_often["Touch often session"]
    BATCH["batch b"]
    DRAFTS["drafts d / h"]
    MODEL["model m"]
    REASON["reasoning y"]
    HARN["harness w"]
  end

  subgraph touch_rare["Touch rarely setup"]
    PROJ["project + local_repo"]
    CONV["convention"]
    AGENT["default agent + bins"]
    TPL["default template"]
  end

  subgraph never["Never commit"]
    SECRETS["claims tokens"]
  end

  touch_often --> PROMPT["prompt text"]
  touch_rare --> TUI["TUI / launch"]
  SECRETS --> CLAIMS["claims footer"]
```

---

## 5. Prompt assembly (what actually goes into the model)

```mermaid
flowchart TB
  BATCH2["Batch ≤16 functions"] --> DET["Load details for each"]
  DET --> DRAFT{"near-miss draft on?"}
  DET --> GH{"Ghidra on + file?"}
  DRAFT -->|yes| NM["NEAR-MISS block"]
  GH -->|yes| GD["GHIDRA block"]
  TPL2{"template"}
  TPL2 -->|chaos-viewer| BV["builtin web-parity body"]
  TPL2 -->|chaos-experimental| EX["same match body + MATCH_RESULT"]
  TPL2 -->|user TOML| UT["header / function / footer"]
  NM --> OUT["Rendered prompt"]
  GD --> OUT
  BV --> OUT
  EX --> OUT
  UT --> OUT
  PROV["model · reasoning · harness"] --> EX
  CLAIMS3["claims session?"] -.->|footer| OUT
```

**Experimental-only extras on the prompt:**

- `MATCH_RESULT` YAML scaffold (tree ids, draft trackers, prefilled provenance)
- Inheritance rules for `usedGhidraDraft` / `usedNearMissDraft`

**Default / sm64ds path:** no MATCH_RESULT requirement; simpler mental load.

---

## 6. Data products after a try

```mermaid
flowchart LR
  TRY["One matching try"]
  TRY --> C["C source in decomp"]
  TRY --> V{"verify"}
  V -->|match| ATLAS2["chaos-db regen later"]
  V -->|near_miss| DRAFT2["draft / NONMATCHING"]
  TRY --> NODE["MATCH_RESULT node"]
  NODE --> JSONL["match_attempts.jsonl"]
  ATLAS2 --> WEB["web viewer / next chaos load"]
```

| Store | Size | Role |
|-------|------|------|
| **Atlas** | lean | Final `matched` + `author` + optional `matchProvenance` |
| **jsonl attempt tree** | fat history | Every try, dead ends, parent links |
| **Source tree** | the work | Actual C that compiles |

---

## 7. Two conventions (don’t mix them in your head)

```mermaid
flowchart TB
  subgraph def["convention = default"]
    D1["Browse like chaos-viewer"]
    D2["Prompt chaos-viewer"]
    D3["author credit only"]
    D4["No MATCH_RESULT required"]
  end

  subgraph exp["convention = experimental"]
    E1["Same browse"]
    E2["Prompt chaos-experimental"]
    E3["author + matchProvenance"]
    E4["Attempt tree logging"]
    E5["Model / reasoning / harness pickers"]
  end
```

If you are **not** running experimental logging, most provenance UI is noise —
use a **default** project (e.g. sm64ds) and ignore `m` / `y` / `w`.

---

## 8. Complexity map — what earns its weight

| Area | Complexity | Value | Keep if… |
|------|------------|-------|----------|
| Multi-project + `local_repo` | medium | high | You switch decomps |
| Overview + priorities + batch | medium | **core** | Always |
| Prompt templates | medium | high | Custom project rules |
| Agent launch | medium | high | You use `g` daily |
| Draft / Ghidra toggles | low–med | high for hard fns | You use scaffolds |
| Experimental MATCH_RESULT | **high** | high for research | You mine the jsonl |
| Provenance pickers | low | high for experimental | Same |
| Claims | medium | situational | Multi-person races |
| Duration / extra log fields | — | low for now | Skip until needed |

---

## 9. Suggested “what next” decision tree

Use this when choosing the next slice of work:

```mermaid
flowchart TD
  Q1{"Is daily browse/prompt smooth?"}
  Q1 -->|no| FIX["Fix TUI / load / UX bugs only"]
  Q1 -->|yes| Q2{"Using experimental logging?"}
  Q2 -->|no| CORE["Ship polish for default path: docs, release, claims optional"]
  Q2 -->|yes| Q3{"Can you log a full try end-to-end today?"}
  Q3 -->|no| PIPE["Wire decomp log_attempt + one real function try"]
  Q3 -->|yes| Q4{"Is the TUI settings surface painful?"}
  Q4 -->|yes| SIMP["Simplify: defaults, fewer keys, presets"]
  Q4 -->|no| ANAL["Analytics / tree viewer / waves later"]
```

**Concrete next options (pick one lane):**

1. **Stabilize** — commit the WIP feature set, cut a clean PR stack, freeze knobs  
2. **Simplify** — presets (“cheap wave” / “premium wave”) instead of raw m/y/w  
3. **Close the loop** — ensure EP (or your decomp) actually appends MATCH_RESULT  
4. **Observe** — small script to summarize jsonl (wins by model, dead-end rate)  
5. **Do not add** — duration, more log fields, more models, more harnesses (for now)

---

## 10. One-screen cheatsheet

```text
LOAD        projects.toml source → chaos-db + details
BROWSE      Overview / Priorities → batch[b]
PROMPT      template[t] drafts[d/h] provenance[m/y/w] copy[c] agent[g]
WORK        agent in local_repo → verify
LOG         MATCH_RESULT → jsonl   (experimental only)
PUBLISH     regen atlas → next load [u]
```

Settings that are **setup once:** project, local_repo, agent bins, default template.  
Settings that are **per session / wave:** batch, drafts, model, reasoning, harness.
