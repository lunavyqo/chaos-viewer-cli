//! Prompt builder for AI matching tasks.
//!
//! Text layout is kept in lock-step with
//! `tangosdev/chaos-viewer` `src/App.tsx` (`promptHeader` / `promptSection` /
//! `promptFooter`), joined as `parts.join("\n\n")`.

use crate::claims::ClaimsSession;
use crate::schema::{ChaosFunction, FunctionDetail, ProjectConfig};

const BATCH_MAX: usize = 16;
const MAX_DISASM_LINES: usize = 90;

#[derive(Debug, Clone, Default)]
pub struct PromptOptions {
    pub claims_session: Option<ClaimsSession>,
}

pub fn batch_max() -> usize {
    BATCH_MAX
}

/// Build a full prompt the same way the web viewer does:
/// `[header, ...sections, footer].join("\n\n")`.
///
/// Prefer [`crate::templates::TemplateStore::render`] when a user template id
/// is selected; this is the built-in `chaos-viewer` body.
pub fn build_prompt(
    project: &ProjectConfig,
    functions: &[(ChaosFunction, Option<FunctionDetail>)],
    opts: &PromptOptions,
) -> String {
    build_builtin_prompt(project, functions, opts)
}

/// Built-in chaos-viewer prompt (exact web parity).
pub fn build_builtin_prompt(
    project: &ProjectConfig,
    functions: &[(ChaosFunction, Option<FunctionDetail>)],
    opts: &PromptOptions,
) -> String {
    // Web uses batch.length / 1; empty selection is handled by the caller.
    let n = if functions.is_empty() {
        1
    } else {
        functions.len()
    };
    let mut parts: Vec<String> = Vec::new();
    parts.push(prompt_header(project, n));
    for (fn_, det) in functions {
        parts.push(prompt_section(project, fn_, det.as_ref()));
    }
    parts.push(prompt_footer(project, n, opts));
    parts.join("\n\n")
}

/// Built-in **experimental** prompt: chaos-viewer match task + mandatory
/// match-provenance reporting (model, reasoning level, harness — or human).
///
/// For projects on the experimental convention. Does not change the default
/// `chaos-viewer` template (sm64ds / upstream parity).
pub fn build_experimental_prompt(
    project: &ProjectConfig,
    functions: &[(ChaosFunction, Option<FunctionDetail>)],
    opts: &PromptOptions,
) -> String {
    let n = if functions.is_empty() {
        1
    } else {
        functions.len()
    };
    let mut parts: Vec<String> = Vec::new();
    parts.push(prompt_header_experimental(project, n));
    for (fn_, det) in functions {
        parts.push(prompt_section(project, fn_, det.as_ref()));
        parts.push(prompt_provenance_block(fn_));
    }
    parts.push(prompt_footer_experimental(project, n, opts));
    parts.join("\n\n")
}

fn prompt_header_experimental(project: &ProjectConfig, n: usize) -> String {
    let mut base = prompt_header(project, n);
    base.push_str(
        r#"

======================================================================
EXPERIMENTAL CONVENTION — MATCH PROVENANCE (MANDATORY)
======================================================================
This project tracks HOW each function was matched. You MUST report provenance
for every function in this batch. Incomplete AI provenance is a failed task.

When you are an AI agent matching code:
  kind       = ai
  model      = the exact model id you are (e.g. claude-opus-4, gpt-5.2)
  reasoning  = your configured reasoning/effort level (high|medium|low|none|…)
  harness    = the tool/pipeline running you (e.g. cursor-agent, claude-code,
               codex-cli, fanout-v3, chaos-batch — short stable token)

When a human matched without AI:
  kind = human
  by   = their handle if known

Do NOT invent a match. VERIFY with the command below until it reports MATCH.
Do NOT skip the MATCH_RESULT block(s) at the end of your reply.
Do NOT put secrets, API keys, or full chain-of-thought dumps into provenance —
only model id, reasoning level, and harness name.
"#,
    );
    base
}

fn prompt_provenance_block(fn_: &ChaosFunction) -> String {
    format!(
        r#"----------------------------------------------------------------------
MATCH_RESULT template for {name} (fill this in your FINAL reply; copy one
block per function; leave status=near_miss|failed if not matched yet)

```yaml
MATCH_RESULT:
  function: {name}
  module: {module}
  addr: "0x{addr:x}"
  size: {size}
  status: matched   # matched | near_miss | failed
  # If status=matched, provenance is REQUIRED:
  matchProvenance:
    kind: ai        # ai | human
    model: "<YOUR_MODEL_ID>"
    reasoning: "<YOUR_REASONING_OR_EFFORT_LEVEL>"
    harness: "<YOUR_HARNESS_OR_AGENT_NAME>"
    # by: "<optional operator handle>"
  # If near_miss: include draft C and estimated instruction divergence:
  # nearMiss:
  #   divergences: <int>
  #   c_source: |
  #     ...
```
"#,
        name = fn_.name,
        module = fn_.module,
        addr = fn_.addr,
        size = fn_.size,
    )
}

fn prompt_footer_experimental(project: &ProjectConfig, n: usize, opts: &PromptOptions) -> String {
    // Start from default footer, then append experimental banking rules.
    let mut lines = prompt_footer(project, n, opts);
    lines.push_str(
        r#"

======================================================================
EXPERIMENTAL — BEFORE YOU FINISH
======================================================================
1. For EACH function above, emit a filled MATCH_RESULT YAML block (see template).
2. If status=matched (verify says MATCH):
   - kind=ai  → model + reasoning + harness MUST be non-empty real values
     (not placeholders like YOUR_MODEL_ID).
   - kind=human → only when no model was used; set by if known.
3. If status=near_miss: still emit MATCH_RESULT with nearMiss draft so the
   operator can ingest it; provenance on near-miss is optional but helpful.
4. Atlas / ledger operators will store matchProvenance on the matched function
   in chaos-db.json. Your MATCH_RESULT is the source of truth for that record.
5. Opening a PR is still required when matched (see above); mention model +
   harness briefly in the PR body.

Refuse to claim "matched" without the verify command succeeding.
"#,
    );
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

/// Port of `promptHeader(n)` from chaos-viewer App.tsx.
fn prompt_header(project: &ProjectConfig, n: usize) -> String {
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
    lines.join("\n")
}

/// Port of `promptSection(fn, det)` from chaos-viewer App.tsx.
fn prompt_section(
    project: &ProjectConfig,
    fn_: &ChaosFunction,
    det: Option<&FunctionDetail>,
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
    if let Some(det) = det {
        if let Some(draft) = &det.draft {
            lines.push(String::new());
            // Web: `${det.draftDiv}` may print "undefined"; mirror that loosely
            let draft_div = det
                .draft_div
                .map(|d| d.to_string())
                .unwrap_or_else(|| "undefined".into());
            lines.push(format!(
                "A NEAR-MISS DRAFT EXISTS ({draft_div} instruction(s) from matching) - START FROM THIS, do not re-decompile:"
            ));
            lines.push("```c".into());
            lines.push(draft.trim_end().to_string());
            lines.push("```".into());
        }
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
                // Web joins disasm with '\n' then pushes as one string; line-by-line is equivalent
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

/// Port of `promptFooter(n)` from chaos-viewer App.tsx.
fn prompt_footer(project: &ProjectConfig, n: usize, opts: &PromptOptions) -> String {
    // Web starts with an empty line entry.
    let mut lines = vec![String::new()];
    if let Some(rules) = &project.rules {
        lines.push(format!("Rules: {rules}"));
    }
    if let (Some(api), Some(session)) =
        (project.claims_api.as_deref(), opts.claims_session.as_ref())
    {
        let handle = if session.handle.is_empty() {
            "chaos-viewer-user"
        } else {
            session.handle.as_str()
        };
        let each = if n > 1 {
            "EACH function"
        } else {
            "the function"
        };
        lines.push(String::new());
        lines.push(format!(
            "CLAIMS (coordination lock; do this BEFORE writing code): my claims api key is {} - send it as the X-Api-Key header on every claims call.",
            session.token
        ));
        lines.push(format!(
            "For {each} above: POST {api}/try-lock with JSON {{\"module\": \"<module>\", \"start\": \"0x<addr>\", \"end\": \"0x<addr+size>\", \"handle\": \"{handle}\"}}."
        ));
        lines.push(format!(
            "Save the returned claim.id; renew while working (POST {api}/{{id}}/renew with {{\"handle\": \"{handle}\"}}) and release when done (POST {api}/{{id}}/release, same body)."
        ));
        lines.push(format!(
            "If try-lock returns a conflict, someone else has it - skip that function. If calls return 401 the short-lived key expired - continue without locking and tell me to re-sign-in. Full contract: GET {api}/instructions."
        ));
    }
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
    if let Some(note) = &project.near_miss_note {
        lines.push(String::new());
        lines.push(note.clone());
    }
    lines.join("\n")
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
    fn experimental_prompt_requires_provenance_blocks() {
        let project = sample_project();
        let fn_ = sample_fn();
        let text = build_experimental_prompt(&project, &[(fn_, None)], &PromptOptions::default());
        assert!(text.contains("EXPERIMENTAL CONVENTION"));
        assert!(text.contains("MATCH_RESULT"));
        assert!(text.contains("matchProvenance"));
        assert!(text.contains("func_020009e0"));
        assert!(text.contains("model:"));
        assert!(text.contains("reasoning:"));
        assert!(text.contains("harness:"));
        assert!(text.contains("BEFORE YOU FINISH"));
        // Still includes core match task bits
        assert!(text.contains("byte-for-byte"));
        assert!(text.contains("VERIFY"));
    }

    #[test]
    fn matches_web_header_section_footer_shape() {
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

        // Header
        assert!(text.starts_with(
            "Match one demo function to the retail binary, byte-for-byte.\n\nSETUP (once): clone https://github.com/you/demo\n\nCOMPILER: mwccarm -O4,p\n\nREAD FIRST: README.md"
        ));
        // Section separators / address style (no 0-pad beyond toString(16))
        assert!(text.contains(
            "FUNCTION: func_020009e0   module: arm9   addr: 0x20009e0   size: 120 bytes"
        ));
        assert!(text.contains(
            "VERIFY every attempt (relocation-aware byte compare):\n  python tools/match.py --func func_020009e0 --addr 0x20009e0 --size 0x78"
        ));
        assert!(text.contains(
            "CLOSEST MATCHED SIBLING (opcode similarity 0.87): src/func_scaffold.c[pp] - use it as your scaffold."
        ));
        assert!(text.contains(
            "A NEAR-MISS DRAFT EXISTS (2 instruction(s) from matching) - START FROM THIS, do not re-decompile:"
        ));
        assert!(text.contains("```c\nint f(void) { return 0; }\n```"));
        assert!(text.contains("TARGET DISASSEMBLY (annotated, callees resolved):\n```\n  020009e0:  ldr      r0, [pc, #0x6c]\n  020009e4:  ldr      r1, [r0]\n```"));
        // Footer
        assert!(text.contains("Rules: no ROM"));
        assert!(text.contains(
            "Matched means byte-identical - iterate until the verify command reports a MATCH."
        ));
        assert!(text.contains(
            "When it matches, fork the repo and open a pull request to https://github.com/you/demo against its default branch\n(one function or a small related family per PR; note the compiler version and the function address)."
        ));
        assert!(text.contains("save drafts"));
        // Join style: header / section / footer separated by blank lines
        assert!(text.contains(
            "\n\n======================================================================\n"
        ));
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
