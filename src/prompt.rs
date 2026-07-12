//! Prompt builder for AI matching tasks (ports web viewer text).

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

pub fn build_prompt(
    project: &ProjectConfig,
    functions: &[(ChaosFunction, Option<FunctionDetail>)],
    opts: &PromptOptions,
) -> String {
    let n = functions.len().max(1);
    let mut parts = Vec::new();
    parts.push(prompt_header(project, n));
    for (fn_, det) in functions {
        parts.push(prompt_section(project, fn_, det.as_ref()));
    }
    parts.push(prompt_footer(project, n, opts));
    parts.join("\n\n")
}

fn fill_template(t: &str, project: &ProjectConfig, fn_: &ChaosFunction) -> String {
    t.replace("{github}", &project.github)
        .replace("{name}", &fn_.name)
        .replace("{module}", &fn_.module)
        .replace("{addr}", &fn_.addr.to_string())
        .replace("{addrHex}", &format!("{:x}", fn_.addr))
        .replace("{size}", &fn_.size.to_string())
        .replace("{sizeHex}", &format!("{:x}", fn_.size))
}

fn prompt_header(project: &ProjectConfig, n: usize) -> String {
    let name = if project.name.is_empty() {
        "decomp"
    } else {
        project.name.as_str()
    };
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
    if let Some(note) = &project.cpp_note {
        lines.push(note.clone());
    }
    if let Some(read) = &project.read_first {
        lines.push(String::new());
        lines.push(format!("READ FIRST: {read}"));
    }
    lines.join("\n")
}

fn prompt_section(
    project: &ProjectConfig,
    fn_: &ChaosFunction,
    det: Option<&FunctionDetail>,
) -> String {
    let mut lines = Vec::new();
    lines.push("=".repeat(70));
    lines.push(format!(
        "FUNCTION: {}   module: {}   addr: 0x{:x}   size: {} bytes",
        fn_.name, fn_.module, fn_.addr, fn_.size
    ));
    if let Some(cmd) = &project.verify_command {
        lines.push("VERIFY every attempt (relocation-aware byte compare):".into());
        lines.push(format!("  {}", fill_template(cmd, project, fn_)));
    }
    if let Some(sib) = &fn_.sibling {
        lines.push(format!(
            "CLOSEST MATCHED SIBLING (opcode similarity {:?}): src/{sib}.c[pp] - use it as your scaffold.",
            fn_.sim
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
            lines.push(format!(
                "A NEAR-MISS DRAFT EXISTS ({} instruction(s) from matching) - START FROM THIS, do not re-decompile:",
                det.draft_div.map(|d| d.to_string()).unwrap_or_else(|| "?".into())
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

fn prompt_footer(project: &ProjectConfig, n: usize, opts: &PromptOptions) -> String {
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

    #[test]
    fn builds_minimal_prompt() {
        let project = ProjectConfig {
            name: "demo".into(),
            github: "https://github.com/you/demo".into(),
            verify_command: Some("verify {name} 0x{addrHex}".into()),
            ..Default::default()
        };
        let fn_ = ChaosFunction {
            id: "arm9:0x1".into(),
            module: "arm9".into(),
            name: "func_a".into(),
            addr: 0x20,
            size: 16,
            matched: false,
            src_path: None,
            author: None,
            div: None,
            cat: None,
            floor: None,
            sim: None,
            sibling: None,
        };
        let text = build_prompt(&project, &[(fn_, None)], &PromptOptions::default());
        assert!(text.contains("Match one demo function"));
        assert!(text.contains("func_a"));
        assert!(text.contains("verify func_a 0x20"));
        assert!(text.contains("open a pull request to https://github.com/you/demo"));
    }
}
