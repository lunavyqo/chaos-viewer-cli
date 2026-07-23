//! Prompt builder for AI matching tasks.
//!
//! Text layout is kept in lock-step with
//! `tangosdev/chaos-viewer` `src/App.tsx` (`promptHeader` / `promptSection` /
//! `promptFooter`), joined as `parts.join("\n\n")`.

use std::path::{Path, PathBuf};

use crate::claims::ClaimsSession;
use crate::schema::{ChaosFunction, FunctionDetail, ProjectConfig};

const BATCH_MAX: usize = 16;
const MAX_DISASM_LINES: usize = 90;

/// Resolved near-miss tip C (from details or local `nearmiss/db.jsonl`).
#[derive(Debug, Clone)]
pub struct NearMissDraft {
    pub text: String,
    pub draft_div: Option<u64>,
    /// `details` | `nearmiss_db` | `src_path`
    pub source: &'static str,
}

#[derive(Debug, Clone)]
pub struct PromptOptions {
    pub claims_session: Option<ClaimsSession>,
    /// When true, attach a stored near-miss / NONMATCHING C draft from details
    /// and/or local `nearmiss/db.jsonl` (not Ghidra). Default **true**.
    pub include_near_miss_draft: bool,
    /// When true, attach a Ghidra decompiler C scaffold (local `ghidra_out/` and/or
    /// a detail draft tagged as GHIDRA SCAFFOLD). Default **true** — no-op when
    /// no draft is available.
    pub include_ghidra_draft: bool,
    /// Directory of `0xXXXXXXXX.c` files (default: search `ghidra_out` under
    /// `local_repo` / cwd when building from the TUI).
    pub ghidra_dir: Option<PathBuf>,
    /// Decomp checkout root (`local_repo`). Used to read `nearmiss/db.jsonl`
    /// tip C when details are missing or stale.
    pub local_repo: Option<PathBuf>,
    /// Prefill `matchProvenance.model` in experimental MATCH_RESULT (slug).
    /// Empty / None → example default `grok-4.5`.
    pub provenance_model: Option<String>,
    /// Prefill `matchProvenance.reasoning` (`high` | `medium` | `low` | `none`).
    pub provenance_reasoning: Option<String>,
    /// Prefill `matchProvenance.harness` (slug, e.g. `grok-build`).
    pub provenance_harness: Option<String>,
}

impl Default for PromptOptions {
    fn default() -> Self {
        Self {
            claims_session: None,
            include_near_miss_draft: true,
            include_ghidra_draft: true,
            ghidra_dir: None,
            local_repo: None,
            provenance_model: None,
            provenance_reasoning: None,
            provenance_harness: None,
        }
    }
}

/// True if text looks like a Ghidra / decompiler scaffold (not a human near-miss).
pub fn is_ghidra_scaffold_text(s: &str) -> bool {
    let head = s.chars().take(400).collect::<String>();
    head.contains("GHIDRA SCAFFOLD")
        || head.contains("Ghidra") && head.contains("decompiler")
        || head.contains("/* The decompiler")
}

/// Load `ghidra_out/0x{addr:08x}.c` (and a few name variants).
pub fn load_ghidra_draft_file(dir: &Path, addr: u64) -> Option<String> {
    if !dir.is_dir() {
        return None;
    }
    let candidates = [
        dir.join(format!("0x{addr:08x}.c")),
        dir.join(format!("0x{addr:x}.c")),
        dir.join(format!("{addr:08x}.c")),
    ];
    for p in candidates {
        if let Ok(text) = std::fs::read_to_string(&p) {
            let t = text.trim();
            if !t.is_empty() {
                return Some(t.to_string());
            }
        }
    }
    None
}

/// Resolve Ghidra C for a function: local file first, then detail.draft if tagged.
pub fn resolve_ghidra_draft(
    fn_: &ChaosFunction,
    det: Option<&FunctionDetail>,
    opts: &PromptOptions,
) -> Option<String> {
    if !opts.include_ghidra_draft {
        return None;
    }
    if let Some(dir) = opts.ghidra_dir.as_ref() {
        if let Some(text) = load_ghidra_draft_file(dir, fn_.addr) {
            return Some(text);
        }
    }
    if let Some(draft) = det.and_then(|d| d.draft.as_ref()) {
        if is_ghidra_scaffold_text(draft) {
            return Some(draft.trim().to_string());
        }
    }
    None
}

/// Load tip C from sm64ds-shaped `nearmiss/db.jsonl` under a decomp root.
///
/// Record keys: `module`, `addr` (hex string), `c_source`, `divergences`.
pub fn load_nearmiss_db_tip(repo: &Path, module: &str, addr: u64) -> Option<NearMissDraft> {
    let path = repo.join("nearmiss").join("db.jsonl");
    let text = std::fs::read_to_string(path).ok()?;
    let want_addr = format!("0x{addr:08x}");
    let want_addr_short = format!("0x{addr:x}");
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let v: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let m = v.get("module").and_then(|x| x.as_str()).unwrap_or("");
        if m != module {
            continue;
        }
        let a = v.get("addr").and_then(|x| x.as_str()).unwrap_or("");
        let addr_ok = a.eq_ignore_ascii_case(&want_addr)
            || a.eq_ignore_ascii_case(&want_addr_short)
            || a.parse::<u64>()
                .ok()
                .or_else(|| u64::from_str_radix(a.trim_start_matches("0x"), 16).ok())
                == Some(addr);
        if !addr_ok {
            continue;
        }
        let c = v
            .get("c_source")
            .and_then(|x| x.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())?;
        let draft_div = v.get("divergences").and_then(|x| x.as_u64()).or_else(|| {
            v.get("divergences")
                .and_then(|x| x.as_i64())
                .map(|i| i as u64)
        });
        return Some(NearMissDraft {
            text: c.to_string(),
            draft_div,
            source: "nearmiss_db",
        });
    }
    None
}

/// Near-miss / NONMATCHING C: details draft first, then local `nearmiss/db.jsonl`.
pub fn resolve_near_miss_draft(
    fn_: &ChaosFunction,
    det: Option<&FunctionDetail>,
    opts: &PromptOptions,
) -> Option<NearMissDraft> {
    if !opts.include_near_miss_draft {
        return None;
    }
    if let Some(draft) = det.and_then(|d| d.draft.as_ref()) {
        if !is_ghidra_scaffold_text(draft) {
            let t = draft.trim();
            if !t.is_empty() {
                return Some(NearMissDraft {
                    text: t.to_string(),
                    draft_div: det.and_then(|d| d.draft_div).or(fn_.div),
                    source: "details",
                });
            }
        }
    }
    if let Some(repo) = opts.local_repo.as_ref() {
        if let Some(mut tip) = load_nearmiss_db_tip(repo, &fn_.module, fn_.addr) {
            if tip.draft_div.is_none() {
                tip.draft_div = fn_.div;
            }
            return Some(tip);
        }
    }
    None
}

pub fn batch_max() -> usize {
    BATCH_MAX
}

/// Build the stock match prompt (match task + attempt tree + provenance).
///
/// Prefer [`crate::templates::TemplateStore::render`] when a user template id
/// is selected. This is the built-in `chaos-viewer` / `chaos-experimental` body
/// (experimental was merged into default).
pub fn build_prompt(
    project: &ProjectConfig,
    functions: &[(ChaosFunction, Option<FunctionDetail>)],
    opts: &PromptOptions,
) -> String {
    build_builtin_prompt(project, functions, opts)
}

/// Stock built-in prompt: retail match task **plus** MATCH_RESULT / attempt tree /
/// matchProvenance (formerly `chaos-experimental` only).
pub fn build_builtin_prompt(
    project: &ProjectConfig,
    functions: &[(ChaosFunction, Option<FunctionDetail>)],
    opts: &PromptOptions,
) -> String {
    let n = if functions.is_empty() {
        1
    } else {
        functions.len()
    };
    let author = operator_github_handle(opts);
    let mut parts: Vec<String> = Vec::new();
    // Session scope for attempt logging: solo vs multi-function context.
    let (session_scope, batch_size) = if n <= 1 {
        ("focused", 1usize)
    } else {
        ("batch", n)
    };
    parts.push(prompt_header_with_provenance(
        project,
        n,
        &author,
        session_scope,
        batch_size,
        opts,
    ));
    for (fn_, det) in functions {
        parts.push(prompt_section(project, fn_, det.as_ref(), opts));
        parts.push(prompt_provenance_block(
            fn_,
            det.as_ref(),
            &author,
            session_scope,
            batch_size,
            opts,
        ));
    }
    parts.push(prompt_footer_with_provenance(
        project,
        n,
        opts,
        &author,
        session_scope,
        batch_size,
    ));
    parts.join("\n\n")
}

/// Alias kept for callers/templates that still name the old experimental stock.
pub fn build_experimental_prompt(
    project: &ProjectConfig,
    functions: &[(ChaosFunction, Option<FunctionDetail>)],
    opts: &PromptOptions,
) -> String {
    build_builtin_prompt(project, functions, opts)
}

/// Operator GitHub login for the classic **`author`** credit field.
///
/// Same sources as claims handle. Not stored inside matchProvenance (that is
/// “how” only).
fn operator_github_handle(opts: &PromptOptions) -> String {
    if let Some(s) = &opts.claims_session {
        let h = s.handle.trim();
        if !h.is_empty() {
            return h.to_string();
        }
    }
    for key in ["CHAOS_CLAIMS_HANDLE", "CHAOS_GITHUB_HANDLE", "GITHUB_USER"] {
        if let Ok(v) = std::env::var(key) {
            let v = v.trim();
            if !v.is_empty() {
                return v.to_string();
            }
        }
    }
    "YOUR_GITHUB_LOGIN".into()
}

fn prompt_header_with_provenance(
    project: &ProjectConfig,
    n: usize,
    author: &str,
    session_scope: &str,
    batch_size: usize,
    opts: &PromptOptions,
) -> String {
    let mut base = prompt_header(project, n, opts);
    let author_line = if author == "YOUR_GITHUB_LOGIN" {
        "  author     = REQUIRED credit field: operator GitHub login (same as classic \
chaos-viewer).\n               Put it on MATCH_RESULT.author — NOT inside matchProvenance."
            .to_string()
    } else {
        format!(
            "  author     = REQUIRED credit: already set to \"{author}\" (GitHub login).\n\
               Put this on MATCH_RESULT.author — NOT inside matchProvenance. \
Same field as contributor colors."
        )
    };
    let scope_line = if session_scope == "focused" {
        format!(
            "  sessionScope = focused  (this prompt is for ONE function; batchSize={batch_size})\n\
               Keep sessionScope=focused and batchSize=1 on every MATCH_RESULT."
        )
    } else {
        format!(
            "  sessionScope = batch  (this prompt covers {batch_size} functions together)\n\
               Keep sessionScope=batch and batchSize={batch_size} on every MATCH_RESULT.\n\
               Do not claim focused unless you later re-ran a solo session for one function."
        )
    };
    base.push_str(&format!(
        r#"

======================================================================
WHO vs HOW vs ATTEMPT TREE
======================================================================
WHO (credit, contributor colors) → function field `author` (GitHub login)
HOW  (final method when banked)  → `matchProvenance` only
EVERY TRY (including dead ends)  → one MATCH_RESULT node in an **attempt tree**

ATTEMPT TREE (required mental model — not a flat list of anonymous tries):
  Each try is a **node**. Links make history reconstructable later:

    arm9:0x020009e0  (functionId — stable; never log name alone)
    ├─ near_miss div=40  base=scratch          [attemptId=01J…]
    │  ├─ no_progress    parent=01J…           [01K…]  ← same base, no win
    │  └─ near_miss div=12 improved parent=…   [01L…]  ← new settings, better
    │     └─ matched     parent=01L…           [01M…]  ← continued from best tip

  IDENTITY (required every try — without this the log cannot be queried later):
  - functionId  = atlas function.id (e.g. module:0xaddr). Stable key.
  - attemptId   = unique id for THIS node. Prefer ULID/UUID. NEVER reuse.
    NEVER "a1"/"try2". Do not embed wall-clock times in ids.
  - parentAttemptId = attemptId of the node you built on, or null for a new root.
  - schemaVersion = 1  (bump only when field meanings change).
  - Privacy: do NOT record finish timestamps or any wall-clock times.

  STATUS RULES (required — same meaning as log_attempt / match tools):
  - matched     — only after verify reports MATCH (never guess or soft-claim)
  - near_miss   — only when this try **improves the tip** (better score than
    prevBestDivergences; set improvedNearMiss: true)
  - no_progress — same-or-worse score, settings retry with no win, or an idea
    you abandon but still log (prefer no_progress over silence)
  - compile_error / failed / skipped — tool/session outcomes; still one node

  Rules:
  - First try for a function: parentAttemptId = null, base.kind = scratch
    (or matched_sibling / imported draft if you truly started from one).
  - Every later try MUST set parentAttemptId to the node you **built on**
    (usually the best near-miss so far, not "whatever you last typed").
  - no_progress / compile_error / failed still get a node under that parent
    so dead ends stay visible as siblings, not erased history.
  - When you improve a near-miss, parent = the previous best node you forked
    from; set improvedNearMiss: true. Next work continues from the new node.
  - When you abandon a branch and restart from scratch or from an older node,
    parentAttemptId must reflect that fork (not pretend it was linear).
  - Never invent a parentAttemptId that was not logged earlier for this functionId.

  DRAFT SOURCE TRACKERS (required every try — two independent booleans):
  - usedNearMissDraft: true if this try used a stored near-miss / NONMATCHING C
    draft (detail draft, // NONMATCHING src, previous best C tip).
  - usedGhidraDraft: true if this try used a Ghidra decompiler scaffold
    (GHIDRA SCAFFOLD block / ghidra_out).

  INHERITANCE (important — lineage, not only "opened the file this session"):
  - If parentAttemptId is set, OR in the parent's flags:
      usedGhidraDraft    = (this try used Ghidra)    OR parent.usedGhidraDraft
      usedNearMissDraft  = (this try used near-miss) OR parent.usedNearMissDraft
  - Example: try A used Ghidra → near_miss with usedGhidraDraft=true.
    Try B continues from A's C only (no Ghidra block) → still
    usedGhidraDraft=true because the tip was Ghidra-descended.
    Try C matches from B → usedGhidraDraft=true again.
  - Set a flag false only if neither this try nor any ancestor used that source.
  - Trackers are SEPARATE (both may be true). They do not replace base.kind /
    parentAttemptId — still set those.

CONTEXT FOCUS (required on EVERY attempt — same tier as model/harness):
  sessionScope + batchSize must appear on every MATCH_RESULT, every try.
  focused — session was only for this one function
  batch   — multi-function session (this target was one of N)

{scope_line}

You MUST emit a MATCH_RESULT for **each function in this batch on every
attempt**, even when:
  - nothing improved
  - near-miss did not beat the previous best
  - compile failed
  - you gave up / skipped

status values:
  matched | near_miss | no_progress | compile_error | failed | skipped
  (matched only after verify; near_miss only when tip improves; else no_progress)

matchProvenance answers HOW only:
  kind=ai    → model + reasoning + harness (slug tokens, no spaces)
  kind=human → human match (optional note); credit still goes in `author`

{author_line}

TOKEN RULES for matchProvenance (ai):
  - model:   GOOD: grok-4.5  claude-opus-4   BAD: "Grok 4.5"
  - harness: GOOD: grok-build  cursor-agent  BAD: "Grok Build"
  - reasoning: max | xhigh | high | medium | low | none  (max is highest)
  Do NOT put the operator name in matchProvenance (no `by` field).

Do NOT invent a match. VERIFY until MATCH.
Do NOT omit MATCH_RESULT because the try was "useless" — useless tries are data.
Do NOT put secrets or full chain-of-thought dumps into the log.
"#,
        scope_line = scope_line,
        author_line = author_line,
    ));
    base
}

fn prompt_provenance_block(
    fn_: &ChaosFunction,
    det: Option<&FunctionDetail>,
    author: &str,
    session_scope: &str,
    batch_size: usize,
    opts: &PromptOptions,
) -> String {
    let author_comment = if author == "YOUR_GITHUB_LOGIN" {
        "# REQUIRED GitHub login for credit (classic author field). Replace placeholder."
    } else {
        "# REQUIRED GitHub login for credit — keep this value (claims / env)."
    };
    // Pre-fill from what THIS prompt actually included (agent should correct if unused).
    let used_near_miss = resolve_near_miss_draft(fn_, det, opts).is_some();
    let used_ghidra = resolve_ghidra_draft(fn_, det, opts).is_some();
    let base_kind = if used_near_miss && used_ghidra {
        "mixed"
    } else if used_near_miss {
        "near_miss_draft"
    } else if used_ghidra {
        "ghidra_scaffold"
    } else {
        "scratch"
    };
    // Operator-selected provenance (TUI m/y/w); fall back to example defaults.
    let model = opts
        .provenance_model
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("grok-4.5");
    let reasoning = opts
        .provenance_reasoning
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("high");
    let harness = opts
        .provenance_harness
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("grok-build");
    format!(
        r#"----------------------------------------------------------------------
MATCH_RESULT — emit ONE node per function for THIS try
(even if status=no_progress / compile_error / failed)

Tree fields (attemptId / parentAttemptId / base) separate siblings and
branches so a later reader can rebuild the attempt tree — not a flat diary.

```yaml
MATCH_RESULT:
  schemaVersion: 1
  # --- identity (required — stable keys for the attempt log) ---
  functionId: "{id}"            # atlas function.id — NOT optional; not name alone
  function: {name}              # display name (may change; functionId does not)
  module: {module}
  addr: "0x{addr:x}"
  size: {size}
  attemptId: "01JEXAMPLE0000000000000000"  # UNIQUE this node: ULID/UUID (never a1/try2)
  parentAttemptId: null         # null = new root; else a real prior attemptId for this functionId
  status: no_progress   # matched | near_miss | no_progress | compile_error | failed | skipped
  # --- attempt tree base ---
  base:
    kind: {base_kind}           # scratch | previous_attempt | near_miss_draft | ghidra_scaffold | matched_sibling | mixed
    # attemptId: "01J…"         # set when kind=previous_attempt (same as parentAttemptId)
    # divergences: 40           # score of the base you started from, if known
  # DRAFT SOURCES (required — two independent trackers; both may be true):
  usedNearMissDraft: {used_near_miss}   # this try OR any ancestor (inherit from parent)
  usedGhidraDraft: {used_ghidra}     # this try OR any ancestor (inherit from parent)
  # Pre-filled from what this prompt included. If parentAttemptId is set and the
  # parent had a tracker true, KEEP it true even if you did not re-open that draft.
  # REQUIRED every run (same tier as model/harness — never omit):
  sessionScope: {session_scope}   # focused | batch
  batchSize: {batch_size}         # 1 if focused; N if batch
  # WHO (classic credit — required when status=matched; preferred always):
  author: "{author}"            {author_comment}
  # HOW this try was run (required for kind=ai on every attempt).
  # model / reasoning / harness are pre-filled from the Prompt pickers (m / y / w).
  # Change them only if this try actually used different settings.
  matchProvenance:
    kind: ai                    # ai | human
    model: "{model}"            # slug; NOT display names like "Grok 4.5"
    reasoning: "{reasoning}"    # max | xhigh | high | medium | low | none
    harness: "{harness}"        # slug; NOT display names like "Grok Build"
  # Score this try when known (still log if null / no improvement):
  divergences: null             # int instruction divergence, or null if un scored
  prevBestDivergences: null     # best known before this try, if any
  improvedNearMiss: false       # true only if divergences < prevBestDivergences
  note: ""                      # short note: what changed vs parent (settings, idea)
  # If near_miss and you have a draft (path or inline for the operator):
  # nearMiss:
  #   divergences: <int>
  #   c_source: |
  #     ...
```

Example after a few tries (sketch only — emit the YAML node, not this art):
  functionId=arm9:0x20009e0
  01JA…  near_miss div=40   parent=null   usedNearMiss=false usedGhidra=false
  01JB…  no_progress        parent=01JA…  usedNearMiss=true  (edited prior tip, no win)
  01JC…  near_miss div=12   parent=01JA…  usedGhidra=true    improvedNearMiss=true
  01JD…  matched            parent=01JC…  usedNearMiss=true
"#,
        id = fn_.id,
        name = fn_.name,
        module = fn_.module,
        addr = fn_.addr,
        size = fn_.size,
        author = author,
        author_comment = author_comment,
        session_scope = session_scope,
        batch_size = batch_size,
        base_kind = base_kind,
        used_near_miss = used_near_miss,
        used_ghidra = used_ghidra,
        model = model,
        reasoning = reasoning,
        harness = harness,
    )
}

fn prompt_footer_with_provenance(
    project: &ProjectConfig,
    n: usize,
    opts: &PromptOptions,
    author: &str,
    session_scope: &str,
    batch_size: usize,
) -> String {
    let mut lines = prompt_footer(project, n, opts);
    let author_rule = if author == "YOUR_GITHUB_LOGIN" {
        "   - author → on MATCH_RESULT.author (classic credit). Required when matched.\n\
             Replace YOUR_GITHUB_LOGIN. Not inside matchProvenance."
            .to_string()
    } else {
        format!(
            "   - author → use \"{author}\" on MATCH_RESULT.author when known/matched.\n\
             Not inside matchProvenance."
        )
    };
    lines.push_str(&format!(
        r#"

======================================================================
BEFORE YOU FINISH
======================================================================
1. For EACH function, emit a filled MATCH_RESULT **node** for this try.
2. Identity (required): schemaVersion=1, functionId (atlas id), unique attemptId
   (ULID/UUID — never a1/try2), parentAttemptId, base. Do NOT log wall-clock times.
3. Draft trackers (required, independent): usedNearMissDraft and usedGhidraDraft.
   Pre-filled from this prompt; INHERIT true from parentAttemptId's node if the
   parent had that flag true (Ghidra/near-miss lineage sticks until a true restart
   with parentAttemptId=null and no draft used).
4. status must reflect reality:
   - matched only after verify MATCH
   - near_miss only when the tip improves (improvedNearMiss: true)
   - same-or-worse / no win → no_progress (prefer that over silence)
5. ALWAYS set sessionScope={session_scope} and batchSize={batch_size} on every
   MATCH_RESULT (every function, every try) — not optional; like model/harness.
   focused = solo session; batch = multi-function session.
6. Tree links:
   - parentAttemptId = the node you actually edited/built from
   - no_progress under the same parent as siblings of later improved tries
   - after an improved near_miss, continue with parent = that new node
7. If status=matched (verify says MATCH):
   - matchProvenance kind=ai → model + reasoning + harness (slug tokens)
   - matchProvenance kind=human → no model fields; optional note only
{author_rule}
8. If the tip improved: status=near_miss, include divergences (+ draft when
   available), improvedNearMiss: true. If score did NOT beat prevBest →
   status=no_progress (still log the dead-end sibling; improvedNearMiss: false).
9. MUST call tools/log_attempt.py after EVERY try (not only matches). Pass
   model + reasoning + harness for kind=ai, session-scope + batch-size,
   parent-attempt-id when forking a tip, --used-near-miss-draft /
   --used-ghidra-draft when true. Prefer --src on near_miss so tip C lands in
   nearmiss/db.jsonl. Do not only paste MATCH_RESULT in chat — the log tool
   writes the durable store. Preserve functionId / attemptId / parentAttemptId
   / base / usedNearMissDraft / usedGhidraDraft. Never log wall-clock times.
10. On MATCH: call stamp_provenance (or bank when it stamps how) with the same
    AI model + reasoning + harness. That is NOT a new try and does NOT replace
    log_attempt for the session. If bank is only fan-out JSON verify, use
    stamp_provenance when present.
11. Open a PR when matched; PR author should match `author`.
12. Claims + cleanup (same as default footer — still required here):
    - You must have claimed via CLAIMS.md and/or the live API before work.
    - On byte-identical MATCH: do NOT unclaim/release that function — set
      CLAIMS.md status to **done** (keep credit). Only release/unclaim work you
      did **not** match.
    - On exit without MATCH: release API locks / mark CLAIMS.md released.
    - Tree-kill any permuter / grind processes you started. Do not leave workers.

Refuse to claim "matched" without verify succeeding.
Never skip logging a failed/empty try — it is a leaf on the tree.
Never reuse attemptId. Never key history by function name alone — use functionId.
Never skip CLAIMS.md / permuter shutdown because the try failed.
Never unclaim a function you matched byte-identical.
"#,
        author_rule = author_rule,
        session_scope = session_scope,
        batch_size = batch_size,
    ));
    lines
}

/// Template fill matching web `fillTemplate` placeholders exactly.
fn fill_template(t: &str, project: &ProjectConfig, fn_: &ChaosFunction) -> String {
    t.replace("{github}", &project.github)
        .replace("{name}", &fn_.name)
        .replace("{module}", &fn_.module)
        .replace("{addr}", &fn_.addr.to_string())
        // JS Number#toString(16) — no leading-zero pad
        .replace("{addrHex}", &format!("{:x}", fn_.addr))
        .replace("{size}", &fn_.size.to_string())
        .replace("{sizeHex}", &format!("{:x}", fn_.size))
}

/// Port of `promptHeader(n)` from chaos-viewer App.tsx (+ draft policy).
fn prompt_header(project: &ProjectConfig, n: usize, opts: &PromptOptions) -> String {
    let name = if project.name.is_empty() {
        "decomp"
    } else {
        project.name.as_str()
    };
    // Match `${n === 1 ? `one ${P.name} function` : `${n} ${P.name} functions`}`
    let mut lines = vec![if n == 1 {
        format!("Match one {name} function to the retail binary, byte-for-byte.")
    } else {
        format!("Match {n} {name} functions to the retail binary, byte-for-byte.")
    }];
    if let Some(setup) = &project.setup {
        lines.push(String::new());
        lines.push(format!(
            "SETUP (once): {}",
            setup.replace("{github}", &project.github)
        ));
    }
    if let Some(compiler) = &project.compiler {
        lines.push(String::new());
        lines.push(format!("COMPILER: {compiler}"));
    }
    // cppNote is pushed without a blank line before it (same as web).
    if let Some(note) = &project.cpp_note {
        lines.push(note.clone());
    }
    if let Some(read) = &project.read_first {
        lines.push(String::new());
        lines.push(format!("READ FIRST: {read}"));
    }
    // Binding policy even when agent has local_repo (omit paste ≠ ban opening files).
    lines.push(String::new());
    lines.push(draft_policy_block(opts));
    lines.join("\n")
}

/// Operator draft policy: include/omit C in the body AND bind local file use.
fn draft_policy_block(opts: &PromptOptions) -> String {
    let near_line = if opts.include_near_miss_draft {
        "Near-miss / NONMATCHING: INCLUDE when available + YOU MUST USE (allowed)"
    } else {
        "Near-miss / NONMATCHING: NOT included + YOU MUST NOT USE (forbidden on disk too)"
    };
    let ghidra_line = if opts.include_ghidra_draft {
        "Ghidra scaffolds:        INCLUDE when available + YOU MUST USE (allowed)"
    } else {
        "Ghidra scaffolds:        NOT included + YOU MUST NOT USE (forbidden on disk too)"
    };
    let mut lines = vec![
        "======================================================================".into(),
        "DRAFT POLICY (operator toggles — two effects each)".into(),
        "======================================================================".into(),
        "1) PROMPT BODY: near-miss / Ghidra C is either INCLUDED below or NOT.".into(),
        "2) WORK RULES: you MUST / MUST NOT use those sources (including local files).".into(),
        String::new(),
        near_line.into(),
        ghidra_line.into(),
        String::new(),
    ];
    if opts.include_near_miss_draft {
        lines.push(
            "NEAR-MISS ON: Prefer C blocks marked \"NEAR-MISS DRAFT — INCLUDED\". \
You may also open nearmiss/db.jsonl tips, // NONMATCHING, or scratch for these functions. \
Still VERIFY to MATCH."
                .into(),
        );
    } else {
        lines.push(
            "NEAR-MISS OFF: No near-miss C is pasted. Do NOT open nearmiss/db.jsonl, \
src/** NONMATCHING, scratch/, or similar tips — even if the repo has them."
                .into(),
        );
    }
    if opts.include_ghidra_draft {
        lines.push(
            "GHIDRA ON: Prefer blocks marked \"GHIDRA … INCLUDED\". Local ghidra_out/ OK \
as extra hint. Rewrite until verify MATCH; never bank decompiler C as-is."
                .into(),
        );
    } else {
        lines.push(
            "GHIDRA OFF: No Ghidra C is pasted. Do NOT open ghidra_out/ or GHIDRA SCAFFOLD \
files — even if present."
                .into(),
        );
    }
    lines.push(String::new());
    lines.push(
        "If both OFF: fresh try from TARGET DISASSEMBLY only. \
Respect the USE / MUST NOT rules above for every attempt. \
MATCH_RESULT usedNearMissDraft / usedGhidraDraft must match what you actually used."
            .into(),
    );
    lines.join("\n")
}

/// Port of `promptSection(fn, det)` from chaos-viewer App.tsx.
///
/// Extra (CLI/TUI): optional Ghidra scaffold block when
/// [`PromptOptions::include_ghidra_draft`] is on.
fn prompt_section(
    project: &ProjectConfig,
    fn_: &ChaosFunction,
    det: Option<&FunctionDetail>,
    opts: &PromptOptions,
) -> String {
    let mut lines: Vec<String> = Vec::new();
    lines.push("=".repeat(70));
    // addr: 0x${fn.addr.toString(16)}  — no zero-pad
    lines.push(format!(
        "FUNCTION: {}   module: {}   addr: 0x{:x}   size: {} bytes",
        fn_.name, fn_.module, fn_.addr, fn_.size
    ));
    if let Some(cmd) = &project.verify_command {
        lines.push("VERIFY every attempt (relocation-aware byte compare):".into());
        lines.push(format!("  {}", fill_template(cmd, project, fn_)));
    }
    if let Some(sib) = &fn_.sibling {
        // Web: `opcode similarity ${fn.sim}` — bare number / undefined, not Option debug
        let sim = fn_
            .sim
            .map(|s| s.to_string())
            .unwrap_or_else(|| "undefined".into());
        lines.push(format!(
            "CLOSEST MATCHED SIBLING (opcode similarity {sim}): src/{sib}.c[pp] - use it as your scaffold."
        ));
    }
    if let Some(floor) = &fn_.floor {
        lines.push(format!(
            "WARNING: previously parked as \"{floor}\" - check the sec 6e-6g levers before grinding."
        ));
    }

    lines.push(String::new());
    lines.push(format!(
        "DRAFTS FOR THIS FUNCTION: near-miss {} · Ghidra {}",
        if opts.include_near_miss_draft {
            "USE (allowed)"
        } else {
            "DO NOT USE"
        },
        if opts.include_ghidra_draft {
            "USE (allowed)"
        } else {
            "DO NOT USE"
        },
    ));

    // Human near-miss tip (details draft and/or local nearmiss/db.jsonl — not Ghidra).
    if let Some(near) = resolve_near_miss_draft(fn_, det, opts) {
        lines.push(String::new());
        let draft_div = near
            .draft_div
            .map(|d| d.to_string())
            .unwrap_or_else(|| "undefined".into());
        let via = match near.source {
            "nearmiss_db" => " (from local nearmiss/db.jsonl)",
            "details" => " (from details draft)",
            other => {
                let _ = other;
                ""
            }
        };
        lines.push(format!(
            "NEAR-MISS DRAFT — INCLUDED BELOW{via}. USE IT as the starting C \
({draft_div} instruction(s) from matching). Do not re-decompile from scratch. Still VERIFY to MATCH. \
You may also open related // NONMATCHING / scratch under src/ for this function only."
        ));
        lines.push("```c".into());
        lines.push(near.text.trim_end().to_string());
        lines.push("```".into());
    } else if opts.include_near_miss_draft {
        lines.push(
            "NEAR-MISS: USE is allowed, but no near-miss C was attached in this prompt. \
You may open local nearmiss/db.jsonl tip, NONMATCHING, or scratch for this function if present; \
otherwise start from disasm."
                .into(),
        );
    } else {
        lines.push(
            "NEAR-MISS: DO NOT USE — C not included below. Do NOT open nearmiss/db.jsonl, \
src/** NONMATCHING, scratch/, or other near-miss tips for this function even if present on disk."
                .into(),
        );
    }

    // Optional Ghidra decompiler scaffold (local ghidra_out/ or tagged detail draft).
    if let Some(ghidra) = resolve_ghidra_draft(fn_, det, opts) {
        lines.push(String::new());
        lines.push(
            "GHIDRA DECOMPILER DRAFT — INCLUDED BELOW. USE IT for structure/types/callees only \
(approximate — NOT a match). REWRITE until verify MATCH. Local ghidra_out/ also OK."
                .into(),
        );
        lines.push("```c".into());
        lines.push(ghidra.trim_end().to_string());
        lines.push("```".into());
    } else if opts.include_ghidra_draft {
        lines.push(
            "GHIDRA: USE is allowed, but no Ghidra scaffold was attached in this prompt. \
You may open local ghidra_out/0x….c for this addr if it exists; else ignore Ghidra."
                .into(),
        );
    } else {
        lines.push(
            "GHIDRA: DO NOT USE — scaffold not included below. Do NOT open ghidra_out/ or \
GHIDRA SCAFFOLD files for this function even if present on disk."
                .into(),
        );
    }

    if let Some(det) = det {
        if let Some(disasm) = &det.disasm {
            if !disasm.is_empty() {
                let truncated = disasm.len() > MAX_DISASM_LINES;
                let mut dis: Vec<String> = if truncated {
                    let mut v: Vec<String> =
                        disasm.iter().take(MAX_DISASM_LINES).cloned().collect();
                    v.push(format!(
                        "... ({} more lines omitted to keep this prompt pasteable - in the repo run  python tools/abrow.py --name {}  for the full annotated listing)",
                        disasm.len() - MAX_DISASM_LINES,
                        fn_.name
                    ));
                    v
                } else {
                    disasm.clone()
                };
                lines.push(String::new());
                if truncated {
                    lines.push(format!(
                        "TARGET DISASSEMBLY (first {MAX_DISASM_LINES} of {} lines, annotated):",
                        disasm.len()
                    ));
                } else {
                    lines.push("TARGET DISASSEMBLY (annotated, callees resolved):".into());
                }
                lines.push("```".into());
                lines.append(&mut dis);
                if let Some(pool) = &det.pool {
                    if !pool.is_empty() {
                        lines.push(String::new());
                        lines.push("pool slots:".into());
                        for pl in pool.iter().take(40) {
                            lines.push(format!("  {pl}"));
                        }
                    }
                }
                lines.push("```".into());
            }
        }
    }
    lines.join("\n")
}

/// Port of `promptFooter(n)` from chaos-viewer App.tsx, plus mandatory claims /
/// cleanup rules models often skip when only mentioned softly in READ FIRST.
fn prompt_footer(project: &ProjectConfig, n: usize, opts: &PromptOptions) -> String {
    // Web starts with an empty line entry.
    let mut lines = vec![String::new()];
    if let Some(rules) = &project.rules {
        lines.push(format!("Rules: {rules}"));
    }
    lines.push(String::new());
    lines.extend(claims_and_cleanup_block(
        project,
        n,
        opts.claims_session.as_ref(),
    ));
    let target = if project.github.is_empty() {
        String::new()
    } else {
        format!(" to {}", project.github)
    };
    lines.push(String::new());
    let multi = if n > 1 {
        " for each function, one at a time (verify before moving on)"
    } else {
        ""
    };
    lines.push(format!(
        "Matched means byte-identical - iterate until the verify command reports a MATCH{multi}."
    ));
    lines.push(format!(
        "When it matches, fork the repo and open a pull request{target} against its default branch"
    ));
    lines.push(
        "(one function or a small related family per PR; note the compiler version and the function address)."
            .into(),
    );
    if opts.include_near_miss_draft {
        if let Some(note) = &project.near_miss_note {
            lines.push(String::new());
            lines.push(note.clone());
        }
    }
    lines.join("\n")
}

/// Mandatory CLAIMS.md + optional live API + permuter shutdown.
///
/// Always emitted on the default (and experimental) footers so agents cannot
/// treat claims as optional flavor text. API key lines only appear when a
/// session is configured.
pub(crate) fn claims_and_cleanup_block(
    project: &ProjectConfig,
    n: usize,
    session: Option<&ClaimsSession>,
) -> Vec<String> {
    let each = if n > 1 {
        "EACH function above"
    } else {
        "the function above"
    };
    let handle = session
        .map(|s| s.handle.as_str())
        .filter(|h| !h.is_empty())
        .unwrap_or("YOUR_HANDLE");

    let mut lines = vec![
        "======================================================================".into(),
        "REQUIRED — CLAIMS (do this BEFORE writing code; do not skip)".into(),
        "======================================================================".into(),
        "Human and agent workers coordinate via CLAIMS.md at the decomp repo root.".into(),
        "Ignoring claims wastes parallel effort. Treat this as a hard gate.".into(),
        String::new(),
        "BEFORE matching / editing:".into(),
        "  1. Open and read CLAIMS.md (repo root). Note active rows.".into(),
        format!(
            "  2. For {each}: if someone else already has an active claim on that \
module/addr range, SKIP it (do not match over their claim)."
        ),
        format!("  3. Claim your work under handle \"{handle}\" BEFORE the first edit:"),
        "     a) Prefer the live claims API when a key is provided below (try-lock).".into(),
        "     b) Always keep CLAIMS.md honest: add/update a table row for your \
range/function(s) — Range, Who, date, status=active — even if the API also holds a lock."
            .into(),
        "     c) If tools/claims.py exists and you have CLAIMS_API_KEY / claims_key.txt, \
you may use: python tools/claims.py lock --module … --start 0x… --end 0x…"
            .into(),
        "  4. Do not start matching until the claim is recorded (API and/or CLAIMS.md).".into(),
        String::new(),
        "WHILE working:".into(),
        "  - Renew API locks if you hold claim ids (TTL is finite).".into(),
        "  - Do not expand into unclaimed ranges without claiming them first.".into(),
        String::new(),
        "WHEN FINISHED (match, near-miss park, give-up, or session end) — MANDATORY:".into(),
        "  1. CLAIMS after a BYTE-IDENTICAL MATCH (verify reports MATCH):".into(),
        "     - Do NOT unclaim / release / drop the claim for that function.".into(),
        "     - Keep credit: set CLAIMS.md status to **done** (or equivalent), \
with a short note that it matched — never status=released and never delete \
the row as if you abandoned the work."
            .into(),
        "     - If the live API still holds a lock on a fully matched range, \
prefer leaving credit intact (done) over release; do **not** unclaim a match."
            .into(),
        "  2. CLAIMS after give-up / no match / session end without MATCH:".into(),
        "     - Release every API lock you took (POST …/release or tools/claims.py release)."
            .into(),
        "     - Update CLAIMS.md: status=released (or remove the row). \
Do not leave stale active rows on abandoned work."
            .into(),
        "  3. Stop every decomp-permuter process you started (see below).".into(),
    ];

    if let (Some(api), Some(session)) = (project.claims_api.as_deref(), session) {
        let api = api.trim().trim_end_matches('/');
        lines.push(String::new());
        lines.push(format!(
            "CLAIMS API KEY (send as X-Api-Key on every write): {}",
            session.token
        ));
        lines.push(format!("API base: {api}"));
        lines.push(format!(
            "For {each}: POST {api}/try-lock with JSON \
{{\"module\": \"<module>\", \"start\": \"0x<addr>\", \"end\": \"0x<addr+size>\", \
\"handle\": \"{handle}\", \"note\": \"optional\"}}."
        ));
        lines.push(format!(
            "Save claim.id; renew: POST {api}/{{id}}/renew {{\"handle\":\"{handle}\"}} · \
release: POST {api}/{{id}}/release same body."
        ));
        lines.push(
            "Conflict → skip that function. 401 → key expired; fall back to CLAIMS.md only \
and tell the operator to re-sign-in. Full contract: GET {api}/instructions."
                .replace("{api}", api),
        );
        lines.push("API lock does NOT replace CLAIMS.md — still update the markdown table.".into());
    } else if project
        .claims_api
        .as_ref()
        .is_some_and(|s| !s.trim().is_empty())
    {
        lines.push(String::new());
        lines.push(format!(
            "Live claims API is configured ({}) but no session key is in this prompt — \
use CLAIMS.md (and tools/claims.py if the operator set CLAIMS_API_KEY).",
            project.claims_api.as_deref().unwrap_or("")
        ));
    } else {
        lines.push(String::new());
        lines.push(
            "No live claimsApi in project config — CLAIMS.md is the coordination surface. \
Do not invent an API host."
                .into(),
        );
    }

    lines.push(String::new());
    lines.push("======================================================================".into());
    lines.push("REQUIRED — PERMUTER / BACKGROUND JOB CLEANUP".into());
    lines.push("======================================================================".into());
    lines.push(
        "If you run tools/permuter (permuter.py, crunch.py, batch.py, import_func, etc.):".into(),
    );
    lines.push(
        "  - The permuter spawns worker PROCESSES (-j). Killing only the parent or \
exiting the chat leaves them running and they burn CPU/RAM."
            .into(),
    );
    lines.push(
        "  - When the job ends (match, timeout, give-up, or you stop the session), \
STOP THE WHOLE PROCESS TREE before you finish:"
            .into(),
    );
    lines.push(
        "      Unix/macOS: kill the process group (e.g. kill -- -PGID) or \
pkill -P <parent_pid> then kill <parent_pid>; confirm with pgrep/ps that no \
permuter.py / mwccarm worker remains."
            .into(),
    );
    lines.push("      Windows: taskkill /F /T /PID <pid> (tree kill).".into());
    lines.push(
        "  - Prefer a hard time budget + tree-kill on timeout (same pattern as \
tools/permuter/crunch.py). Never leave permuters running after the task ends."
            .into(),
    );
    lines.push(
        "  - Same rule for any other long-running grind you started (crack loops, \
overnight sweeps): shut them down on exit."
            .into(),
    );
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_project() -> ProjectConfig {
        ProjectConfig {
            name: "demo".into(),
            github: "https://github.com/you/demo".into(),
            setup: Some("clone {github}".into()),
            compiler: Some("mwccarm -O4,p".into()),
            verify_command: Some(
                "python tools/match.py --func {name} --addr 0x{addrHex} --size 0x{sizeHex}".into(),
            ),
            read_first: Some("README.md".into()),
            rules: Some("no ROM".into()),
            near_miss_note: Some("save drafts".into()),
            ..Default::default()
        }
    }

    fn sample_fn() -> ChaosFunction {
        ChaosFunction {
            id: "arm9:0x20009e0".into(),
            module: "arm9".into(),
            name: "func_020009e0".into(),
            addr: 0x0200_09e0,
            size: 0x78,
            matched: false,
            src_path: None,
            author: None,
            div: None,
            cat: None,
            floor: None,
            sim: Some(0.87),
            sibling: Some("func_scaffold".into()),
            match_provenance: None,
        }
    }

    #[test]
    fn ghidra_draft_included_when_enabled() {
        let project = sample_project();
        let fn_ = sample_fn();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(format!("0x{:08x}.c", fn_.addr));
        std::fs::write(&path, "/* GHIDRA SCAFFOLD */\nint f(void) { return 0; }\n").unwrap();
        let opts = PromptOptions {
            include_ghidra_draft: true,
            ghidra_dir: Some(dir.path().to_path_buf()),
            ..Default::default()
        };
        let text = build_builtin_prompt(&project, &[(fn_.clone(), None)], &opts);
        assert!(text.contains("GHIDRA DECOMPILER DRAFT"));
        assert!(text.contains("int f(void)"));

        let opts_off = PromptOptions {
            include_ghidra_draft: false,
            ghidra_dir: Some(dir.path().to_path_buf()),
            ..Default::default()
        };
        let text_off = build_builtin_prompt(&project, &[(fn_, None)], &opts_off);
        assert!(!text_off.contains("GHIDRA DECOMPILER DRAFT"));
    }

    #[test]
    fn near_miss_draft_omitted_when_disabled() {
        let project = sample_project();
        let fn_ = sample_fn();
        let det = FunctionDetail {
            draft: Some("int near_miss(void) { return 1; }\n".into()),
            draft_div: Some(3),
            ..Default::default()
        };
        let on = PromptOptions::default();
        let text_on = build_builtin_prompt(&project, &[(fn_.clone(), Some(det.clone()))], &on);
        assert!(text_on.contains("NEAR-MISS DRAFT — INCLUDED"));
        assert!(text_on.contains("near_miss"));
        assert!(text_on.contains("DRAFT POLICY"));

        let off = PromptOptions {
            include_near_miss_draft: false,
            ..Default::default()
        };
        let text_off = build_builtin_prompt(&project, &[(fn_, Some(det))], &off);
        assert!(!text_off.contains("NEAR-MISS DRAFT — INCLUDED"));
        assert!(text_off.contains("NEAR-MISS: DO NOT USE"));
        assert!(text_off.contains("YOU MUST NOT USE"));
    }

    #[test]
    fn near_miss_from_local_nearmiss_db() {
        let project = sample_project();
        let fn_ = sample_fn();
        let dir = tempfile::tempdir().unwrap();
        let nm = dir.path().join("nearmiss");
        std::fs::create_dir_all(&nm).unwrap();
        let row = serde_json::json!({
            "module": "arm9",
            "addr": format!("0x{:08x}", fn_.addr),
            "name": fn_.name,
            "divergences": 5,
            "c_source": "int tip_from_db(void) { return 42; }\n",
            "source": "test"
        });
        std::fs::write(nm.join("db.jsonl"), format!("{row}\n")).unwrap();
        let opts = PromptOptions {
            include_near_miss_draft: true,
            local_repo: Some(dir.path().to_path_buf()),
            ..Default::default()
        };
        // No details draft — should still pull tip C from nearmiss/db.jsonl.
        let text = build_builtin_prompt(&project, &[(fn_.clone(), None)], &opts);
        assert!(text.contains("NEAR-MISS DRAFT — INCLUDED"));
        assert!(text.contains("nearmiss/db.jsonl"));
        assert!(text.contains("tip_from_db"));
        assert!(text.contains("42"));

        let off = PromptOptions {
            include_near_miss_draft: false,
            local_repo: Some(dir.path().to_path_buf()),
            ..Default::default()
        };
        let text_off = build_builtin_prompt(&project, &[(fn_, None)], &off);
        assert!(!text_off.contains("tip_from_db"));
    }

    #[test]
    fn experimental_prompt_requires_provenance_blocks() {
        let project = sample_project();
        let fn_ = sample_fn();
        let text = build_experimental_prompt(&project, &[(fn_, None)], &PromptOptions::default());
        assert!(text.contains("WHO vs HOW vs ATTEMPT TREE"));
        assert!(text.contains("MATCH_RESULT"));
        assert!(text.contains("matchProvenance"));
        assert!(text.contains("no_progress"));
        assert!(text.contains("func_020009e0"));
        assert!(text.contains("model:"));
        assert!(text.contains("reasoning:"));
        assert!(text.contains("harness:"));
        assert!(text.contains("BEFORE YOU FINISH"));
        assert!(text.contains("improvedNearMiss"));
        assert!(text.contains("sessionScope"));
        assert!(text.contains("batchSize"));
        assert!(text.contains("focused")); // single-fn sample → focused
                                           // Credit uses classic author field, not provenance.by
        assert!(text.contains("author:"));
        assert!(text.contains("YOUR_GITHUB_LOGIN") || text.contains("GitHub"));
        assert!(!text.contains("by: \""));
        assert!(text.contains("byte-for-byte"));
        assert!(text.contains("VERIFY"));
        // Attempt tree identity (stable keys + parent links + base of work).
        assert!(text.contains("attemptId"));
        assert!(text.contains("parentAttemptId"));
        assert!(text.contains("functionId"));
        assert!(text.contains("schemaVersion"));
        assert!(!text.contains("loggedAt"));
        assert!(text.contains("ATTEMPT TREE"));
        assert!(text.contains("STATUS RULES"));
        assert!(text.contains("only after verify"));
        assert!(text.contains("only when") && text.contains("improves"));
        assert!(text.contains("MUST call tools/log_attempt.py"));
        assert!(text.contains("stamp_provenance"));
        assert!(text.contains("previous_attempt"));
        assert!(text.contains("usedNearMissDraft"));
        assert!(text.contains("usedGhidraDraft"));
        // Sample function id is baked into the MATCH_RESULT template.
        assert!(text.contains("arm9:0x20009e0") || text.contains(&sample_fn().id));
        // Default prefill when pickers not set.
        assert!(text.contains("model: \"grok-4.5\""));
        assert!(text.contains("reasoning: \"high\""));
        assert!(text.contains("harness: \"grok-build\""));
    }

    #[test]
    fn experimental_prompt_prefills_selected_provenance() {
        let project = sample_project();
        let fn_ = sample_fn();
        let opts = PromptOptions {
            provenance_model: Some("claude-opus-4.8".into()),
            provenance_reasoning: Some("medium".into()),
            provenance_harness: Some("cursor-agent".into()),
            ..Default::default()
        };
        let text = build_experimental_prompt(&project, &[(fn_, None)], &opts);
        assert!(text.contains("model: \"claude-opus-4.8\""));
        assert!(text.contains("reasoning: \"medium\""));
        assert!(text.contains("harness: \"cursor-agent\""));
        assert!(!text.contains("model: \"grok-4.5\""));
    }

    #[test]
    fn draft_trackers_preflect_prompt_inclusions() {
        let project = sample_project();
        let fn_ = sample_fn();
        let det = FunctionDetail {
            draft: Some("int near_miss(void) { return 1; }\n".into()),
            draft_div: Some(2),
            ..Default::default()
        };
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(format!("0x{:08x}.c", fn_.addr)),
            "/* GHIDRA SCAFFOLD */\nint g(void) { return 0; }\n",
        )
        .unwrap();
        let opts = PromptOptions {
            include_near_miss_draft: true,
            include_ghidra_draft: true,
            ghidra_dir: Some(dir.path().to_path_buf()),
            ..Default::default()
        };
        let text = build_experimental_prompt(&project, &[(fn_, Some(det))], &opts);
        assert!(text.contains("usedNearMissDraft: true"));
        assert!(text.contains("usedGhidraDraft: true"));
        assert!(text.contains("kind: mixed") || text.contains("base:\n    kind: mixed"));
    }

    #[test]
    fn experimental_prompt_prefill_author_from_claims_handle() {
        let project = sample_project();
        let fn_ = sample_fn();
        let opts = PromptOptions {
            claims_session: Some(ClaimsSession {
                token: "t".into(),
                handle: "lunavyqo".into(),
            }),
            ..Default::default()
        };
        let text = build_experimental_prompt(&project, &[(fn_, None)], &opts);
        assert!(text.contains("author: \"lunavyqo\""));
        assert!(!text.contains("YOUR_GITHUB_LOGIN"));
        // method block has no by:
        assert!(!text.contains("\n    by:"));
    }

    #[test]
    fn default_builtin_prompt_includes_provenance_and_attempt_tree() {
        // Experimental tracking is now the stock default body.
        let project = sample_project();
        let fn_ = sample_fn();
        let text = build_builtin_prompt(&project, &[(fn_, None)], &PromptOptions::default());
        assert!(text.contains("MATCH_RESULT"));
        assert!(text.contains("matchProvenance"));
        assert!(text.contains("usedNearMissDraft"));
        assert!(text.contains("usedGhidraDraft"));
        assert!(text.contains("sessionScope"));
        assert!(text.contains("log_attempt"));
        assert!(text.contains("stamp_provenance"));
        assert!(text.contains("WHO vs HOW vs ATTEMPT TREE"));
        assert!(text.contains("DRAFT POLICY"));
        assert!(!text.contains("EXPERIMENTAL —"));
    }

    #[test]
    fn default_prompt_requires_claims_md_and_permuter_shutdown() {
        // Always present even without a claims session (models skip soft hints).
        let project = sample_project();
        let fn_ = sample_fn();
        let text = build_builtin_prompt(&project, &[(fn_, None)], &PromptOptions::default());
        assert!(text.contains("REQUIRED — CLAIMS"));
        assert!(text.contains("CLAIMS.md"));
        assert!(text.contains("BEFORE matching"));
        assert!(text.contains("Do NOT unclaim") || text.contains("do NOT unclaim"));
        assert!(text.contains("BYTE-IDENTICAL MATCH"));
        assert!(text.contains("REQUIRED — PERMUTER"));
        assert!(
            text.contains("PROCESS TREE")
                || text.contains("process tree")
                || text.contains("process group")
        );
        assert!(text.contains("taskkill") || text.contains("pkill"));
        // Without session, must not invent a fake API key line.
        assert!(!text.contains("CLAIMS API KEY"));
    }

    #[test]
    fn default_prompt_includes_api_when_session_present() {
        let mut project = sample_project();
        project.claims_api = Some("https://tangos.dev/api/claims".into());
        let fn_ = sample_fn();
        let opts = PromptOptions {
            claims_session: Some(ClaimsSession {
                token: "test-key".into(),
                handle: "tester".into(),
            }),
            ..Default::default()
        };
        let text = build_builtin_prompt(&project, &[(fn_, None)], &opts);
        assert!(text.contains("CLAIMS.md"));
        assert!(text.contains("CLAIMS API KEY"));
        assert!(text.contains("test-key"));
        assert!(text.contains("try-lock"));
        assert!(text.contains("API lock does NOT replace CLAIMS.md"));
        assert!(text.contains("REQUIRED — PERMUTER"));
    }

    #[test]
    fn experimental_prompt_batch_scope_when_multiple_functions() {
        let project = sample_project();
        let a = sample_fn();
        let mut b = sample_fn();
        b.name = "func_other".into();
        b.id = "arm9:0x20009e1".into();
        let text =
            build_experimental_prompt(&project, &[(a, None), (b, None)], &PromptOptions::default());
        assert!(text.contains("sessionScope: batch"));
        assert!(text.contains("batchSize: 2"));
        assert!(text.contains("sessionScope = batch"));
    }

    #[test]
    fn stock_prompt_keeps_match_task_shape_plus_provenance() {
        let project = sample_project();
        let fn_ = sample_fn();
        let det = FunctionDetail {
            disasm: Some(vec![
                "  020009e0:  ldr      r0, [pc, #0x6c]".into(),
                "  020009e4:  ldr      r1, [r0]".into(),
            ]),
            draft: Some("int f(void) { return 0; }\n".into()),
            draft_div: Some(2),
            ..Default::default()
        };
        let text = build_prompt(&project, &[(fn_, Some(det))], &PromptOptions::default());

        // Match-task core
        assert!(text.contains("Match one demo function to the retail binary, byte-for-byte."));
        assert!(text.contains(
            "FUNCTION: func_020009e0   module: arm9   addr: 0x20009e0   size: 120 bytes"
        ));
        assert!(text.contains("NEAR-MISS DRAFT — INCLUDED BELOW"));
        assert!(text.contains("DRAFT POLICY"));
        assert!(text.contains("TARGET DISASSEMBLY (annotated, callees resolved):"));
        assert!(text.contains("Rules: no ROM"));
        assert!(text.contains("Matched means byte-identical"));
        // Provenance / attempt tree (now stock default)
        assert!(text.contains("MATCH_RESULT"));
        assert!(text.contains("matchProvenance"));
        assert!(text.contains("WHO vs HOW vs ATTEMPT TREE"));
        assert!(text.contains("BEFORE YOU FINISH"));
    }

    #[test]
    fn multi_function_header_wording() {
        let project = sample_project();
        let a = sample_fn();
        let mut b = sample_fn();
        b.name = "func_b".into();
        b.id = "arm9:0x2".into();
        let text = build_prompt(&project, &[(a, None), (b, None)], &PromptOptions::default());
        assert!(text.starts_with("Match 2 demo functions to the retail binary, byte-for-byte."));
        assert!(text.contains(
            "Matched means byte-identical - iterate until the verify command reports a MATCH for each function, one at a time (verify before moving on)."
        ));
    }
}
