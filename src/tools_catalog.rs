//! Catalog of typical decomp instruments (generic repo).
//!
//! Names are conventional (`match.py`, `bank.py`, …). A given checkout may use
//! a subset or renames — the **role** is what matters. Used by the TUI Tools page.

use std::path::{Path, PathBuf};

/// High-level bucket for filtering cards.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolCategory {
    Core,
    Atlas,
    Experimental,
    Automation,
    Coordination,
    Optional,
}

impl ToolCategory {
    pub const ALL: [ToolCategory; 6] = [
        ToolCategory::Core,
        ToolCategory::Atlas,
        ToolCategory::Experimental,
        ToolCategory::Automation,
        ToolCategory::Coordination,
        ToolCategory::Optional,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Core => "core",
            Self::Atlas => "atlas",
            Self::Experimental => "experimental",
            Self::Automation => "automation",
            Self::Coordination => "coordination",
            Self::Optional => "optional",
        }
    }

    pub fn cycle(self) -> Self {
        match self {
            Self::Core => Self::Atlas,
            Self::Atlas => Self::Experimental,
            Self::Experimental => Self::Automation,
            Self::Automation => Self::Coordination,
            Self::Coordination => Self::Optional,
            Self::Optional => Self::Core,
        }
    }
}

/// One instrument card.
#[derive(Debug, Clone, Copy)]
pub struct ToolCard {
    /// Short id / primary filename.
    pub name: &'static str,
    pub category: ToolCategory,
    /// One-line job.
    pub summary: &'static str,
    /// What this tool changes / produces (artifacts, ledgers, outputs).
    pub changes: &'static str,
    /// Relative paths under `local_repo` that mark the tool as present.
    pub detect: &'static [&'static str],
}

/// Built-in catalog (generic decomp roles).
pub const TOOL_CARDS: &[ToolCard] = &[
    ToolCard {
        name: "unpack.py",
        category: ToolCategory::Core,
        summary: "Extract module binaries from a local ROM.",
        changes: "Writes arm9.bin / arm7.bin (and overlays) for match + disasm.",
        detect: &["tools/unpack.py"],
    },
    ToolCard {
        name: "disasm.py",
        category: ToolCategory::Core,
        summary: "Disassemble a binary range (inspect / debug).",
        changes: "Stdout assembly only — does not write ledgers or src.",
        detect: &["tools/disasm.py"],
    },
    ToolCard {
        name: "match.py",
        category: ToolCategory::Core,
        summary: "Compile candidate C and byte-compare to target (verify).",
        changes: "Judge for matches: match / near_miss / error scores. No bank by itself.",
        detect: &["tools/match.py"],
    },
    ToolCard {
        name: "bank.py",
        category: ToolCategory::Core,
        summary: "Promote a verified match (src + credit). NOT a new try.",
        changes: "Final ledger / src promote. May stamp how (EP) or only fan-out \
verify (SM64DS) — use stamp_provenance when bank does not stamp how.",
        detect: &["tools/bank.py"],
    },
    ToolCard {
        name: "progress.py",
        category: ToolCategory::Core,
        summary: "Report matched % vs function universe.",
        changes: "Read-only report (optional README progress bar rewrite).",
        detect: &["tools/progress.py"],
    },
    ToolCard {
        name: "log_attempt.py",
        category: ToolCategory::Experimental,
        summary: "REQUIRED after every try: append attempt-tree node (any status).",
        changes: "Appends config/match_attempts.jsonl. matched only after verify; \
near_miss when tip improves; else no_progress. No wall-clock times.",
        detect: &["tools/log_attempt.py"],
    },
    ToolCard {
        name: "stamp_provenance.py",
        category: ToolCategory::Experimental,
        summary: "On MATCH: stamp final how (model / reasoning / harness).",
        changes: "Writes match_provenance.jsonl how-row. NOT a new try; does not \
replace log_attempt. Prefer when bank is fan-out only.",
        detect: &["tools/stamp_provenance.py"],
    },
    ToolCard {
        name: "match_attempts.py",
        category: ToolCategory::Experimental,
        summary: "Library: attempt-tree schema, ids, draft-flag inheritance.",
        changes: "I/O helpers for the attempt jsonl — used by log_attempt.",
        detect: &["tools/match_attempts.py"],
    },
    ToolCard {
        name: "match_provenance.py",
        category: ToolCategory::Experimental,
        summary: "Library: final how-ledger (model / reasoning / harness).",
        changes: "Reads/writes match_provenance.jsonl for banked matches.",
        detect: &["tools/match_provenance.py"],
    },
    ToolCard {
        name: "chaos_db_ci.py",
        category: ToolCategory::Atlas,
        summary: "CI-safe atlas rebuild from committed data (no ROM).",
        changes: "Writes chaos-db.json (matched flags, stats, provenance on funcs).",
        detect: &["tools/chaos_db_ci.py"],
    },
    ToolCard {
        name: "generate_details.py",
        category: ToolCategory::Atlas,
        summary: "Build per-module detail chunks (disasm text ± drafts).",
        changes: "Writes details/<module>.json for viewer lazy load.",
        detect: &["tools/generate_details.py"],
    },
    ToolCard {
        name: "generate-chaos-db.py",
        category: ToolCategory::Atlas,
        summary: "Full atlas generator (often in web tree or decomp scripts).",
        changes: "chaos-db.json + details — may use bins and local tools.",
        detect: &["scripts/generate-chaos-db.py", "tools/generate-chaos-db.py"],
    },
    ToolCard {
        name: "chaosviewer.config.json",
        category: ToolCategory::Atlas,
        summary: "Project blurb for prompts (github, rules, verify_command).",
        changes: "Feeds atlas project block / chaos prompt project text.",
        detect: &["tools/chaosviewer.config.json", "chaosviewer.config.json"],
    },
    ToolCard {
        name: "pr_validate.py",
        category: ToolCategory::Optional,
        summary: "PR / CI hygiene checks for the decomp.",
        changes: "Exit status only — no progress ledgers.",
        detect: &["tools/pr_validate.py"],
    },
    ToolCard {
        name: "claims.py",
        category: ToolCategory::Coordination,
        summary: "Lock coordination client for multi-person matching.",
        changes: "Remote claims API (+ optional CLAIMS.md); not match status.",
        detect: &["tools/claims.py"],
    },
    ToolCard {
        name: "worklist.py",
        category: ToolCategory::Automation,
        summary: "Emit target batch + context for agents / automation.",
        changes: "Writes worklist JSONL (inputs for swarm/cascade/agents).",
        detect: &["tools/worklist.py"],
    },
    ToolCard {
        name: "triage.py",
        category: ToolCategory::Automation,
        summary: "Bucket unmatched functions by difficulty / tier.",
        changes: "Planning output only — does not bank matches.",
        detect: &["tools/triage.py"],
    },
    ToolCard {
        name: "swarm.py",
        category: ToolCategory::Automation,
        summary: "Zero-token / template matching tier.",
        changes: "May write candidate C; verify still decides.",
        detect: &["tools/swarm.py"],
    },
    ToolCard {
        name: "cascade.py",
        category: ToolCategory::Automation,
        summary: "Cheap LLM pass on near-miss / template failures.",
        changes: "Candidate C via API; optional apply of wins.",
        detect: &["tools/cascade.py"],
    },
    ToolCard {
        name: "clone.py",
        category: ToolCategory::Automation,
        summary: "Free structural clone matcher (no LLM).",
        changes: "Banks or proposes clones when structure matches.",
        detect: &["tools/clone.py"],
    },
    ToolCard {
        name: "coddog.py",
        category: ToolCategory::Automation,
        summary: "Opcode-similarity scheduler for related targets.",
        changes: "Worklists / ordering — not final match by itself.",
        detect: &["tools/coddog.py"],
    },
    ToolCard {
        name: "nearmiss_db.py",
        category: ToolCategory::Optional,
        summary: "Persistent best near-miss C store.",
        changes: "Updates near-miss database / best draft scores.",
        detect: &["tools/nearmiss_db.py"],
    },
    ToolCard {
        name: "m2c_draft.py",
        category: ToolCategory::Optional,
        summary: "m2c semantic C draft for one function.",
        changes: "Produces draft C for a human/agent to refine.",
        detect: &["tools/m2c_draft.py"],
    },
    ToolCard {
        name: "nonmatching.py",
        category: ToolCategory::Optional,
        summary: "NONMATCHING hatch for logic-correct non-byte matches.",
        changes: "Src markers / decompiled-not-matched accounting.",
        detect: &["tools/nonmatching.py"],
    },
    ToolCard {
        name: "ghidra_out/",
        category: ToolCategory::Optional,
        summary: "Ghidra decompiler scaffolds (folder, not a .py).",
        changes: "Approx C dumps; chaos Prompt h may attach them.",
        detect: &["ghidra_out"],
    },
    ToolCard {
        name: "mwccarm/",
        category: ToolCategory::Core,
        summary: "Compiler toolchain used by match/bank (when present).",
        changes: "Builds objects for byte compare — shared by verify tools.",
        detect: &["tools/mwccarm", "tools/mwccarm/mwccarm.exe"],
    },
];

/// Whether any detect path exists under `repo`.
pub fn tool_present(repo: &Path, card: &ToolCard) -> bool {
    card.detect.iter().any(|rel| {
        let p = repo.join(rel);
        p.is_file() || p.is_dir()
    })
}

/// First existing detect path (for display), if any.
pub fn tool_found_path(repo: &Path, card: &ToolCard) -> Option<PathBuf> {
    card.detect.iter().find_map(|rel| {
        let p = repo.join(rel);
        if p.is_file() || p.is_dir() {
            Some(p)
        } else {
            None
        }
    })
}

/// Indices into [`TOOL_CARDS`] matching an optional category filter.
pub fn filtered_indices(category: Option<ToolCategory>) -> Vec<usize> {
    TOOL_CARDS
        .iter()
        .enumerate()
        .filter(|(_, c)| match category {
            None => true,
            Some(cat) => c.category == cat,
        })
        .map(|(i, _)| i)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_nonempty_unique_names() {
        assert!(TOOL_CARDS.len() >= 10);
        let mut names = std::collections::HashSet::new();
        for c in TOOL_CARDS {
            assert!(names.insert(c.name), "duplicate {}", c.name);
            assert!(!c.summary.is_empty());
            assert!(!c.changes.is_empty());
            assert!(!c.detect.is_empty());
        }
    }

    #[test]
    fn filter_experimental() {
        let idx = filtered_indices(Some(ToolCategory::Experimental));
        assert!(!idx.is_empty());
        for i in idx {
            assert_eq!(TOOL_CARDS[i].category, ToolCategory::Experimental);
        }
    }
}
