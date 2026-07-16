//! Chaos Viewer data schema (compatible with upstream ADAPTING.md).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChaosDb {
    #[serde(default)]
    pub generated_at: String,
    #[serde(default)]
    pub project: Option<ProjectConfig>,
    pub stats: Stats,
    #[serde(default)]
    pub functions: Vec<ChaosFunction>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Stats {
    #[serde(default)]
    pub total_functions: u64,
    #[serde(default)]
    pub matched_functions: u64,
    #[serde(default)]
    pub total_bytes: u64,
    #[serde(default)]
    pub matched_bytes: u64,
    #[serde(default)]
    pub module_count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ProjectConfig {
    #[serde(default)]
    pub name: String,
    pub title: Option<String>,
    pub tagline: Option<String>,
    #[serde(default)]
    pub github: String,
    pub compiler: Option<String>,
    pub cpp_note: Option<String>,
    pub setup: Option<String>,
    pub verify_command: Option<String>,
    pub read_first: Option<String>,
    pub rules: Option<String>,
    pub near_miss_note: Option<String>,
    pub data_url: Option<String>,
    pub claims_api: Option<String>,
    pub claims_auth_url: Option<String>,
    pub discord: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChaosFunction {
    pub id: String,
    pub module: String,
    pub name: String,
    pub addr: u64,
    pub size: u64,
    pub matched: bool,
    pub src_path: Option<String>,
    /// Who matched it (GitHub login / contributor credit).
    ///
    /// Same field as classic chaos-viewer: treemap contributor colors, credit.
    /// This is the only “who” field — do **not** put the operator in
    /// `matchProvenance` (that object is “how” only).
    pub author: Option<String>,
    pub div: Option<u64>,
    pub cat: Option<String>,
    pub floor: Option<String>,
    pub sim: Option<f64>,
    pub sibling: Option<String>,
    /// **How** this function was matched (experimental convention; optional for default).
    ///
    /// Answers method only (`human` vs `ai` + model/reasoning/harness). Credit /
    /// “who” stays on [`Self::author`]. Generators for experimental profiles should
    /// set this on every matched function. Default / sm64ds atlases omit it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub match_provenance: Option<MatchProvenance>,
}

/// **How** a function was matched — required under the **experimental** convention.
///
/// “Who” is always [`ChaosFunction::author`], never this object.
///
/// ```json
/// { "kind": "ai", "model": "grok-4.5", "reasoning": "high", "harness": "grok-build" }
/// { "kind": "human", "note": "optional" }
/// ```
///
/// Legacy `by` keys inside provenance may still deserialize for old atlases but
/// are ignored for display/completeness — use `author` instead.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum MatchProvenance {
    /// Matched by a person (no model/harness). Credit → `author`.
    #[serde(rename = "human")]
    Human {
        /// Deprecated: use function `author`. Kept for reading old ledgers only.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        by: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        note: Option<String>,
    },
    /// Matched with AI — model, reasoning, harness required for a complete record.
    /// Operator credit → `author`, not `by`.
    #[serde(rename = "ai")]
    Ai {
        /// Model id slug (e.g. `claude-opus-4`, `grok-4.5`).
        model: String,
        /// Reasoning / effort level (e.g. `high`, `medium`, `none`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reasoning: Option<String>,
        /// Tooling / pipeline id (e.g. `fanout-v3`, `grok-build`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        harness: Option<String>,
        /// Deprecated: use function `author`. Kept for reading old ledgers only.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        by: Option<String>,
    },
}

impl MatchProvenance {
    /// One-line summary of **method** (not who — see `author`).
    pub fn summary(&self) -> String {
        match self {
            Self::Human { note, .. } => match note {
                Some(n) if !n.is_empty() => format!("human · {n}"),
                _ => "human".into(),
            },
            Self::Ai {
                model,
                reasoning,
                harness,
                ..
            } => {
                let mut parts = vec![format!("ai · model={model}")];
                if let Some(r) = reasoning.as_ref().filter(|s| !s.is_empty()) {
                    parts.push(format!("reasoning={r}"));
                }
                if let Some(h) = harness.as_ref().filter(|s| !s.is_empty()) {
                    parts.push(format!("harness={h}"));
                }
                parts.join(" · ")
            }
        }
    }

    /// True when experimental **how** rules are satisfied.
    ///
    /// - **human**: complete (optional note only)
    /// - **ai**: non-empty `model`, `reasoning`, and `harness`
    pub fn is_complete(&self) -> bool {
        match self {
            Self::Human { .. } => true,
            Self::Ai {
                model,
                reasoning,
                harness,
                ..
            } => {
                !model.trim().is_empty()
                    && reasoning
                        .as_ref()
                        .map(|s| !s.trim().is_empty())
                        .unwrap_or(false)
                    && harness
                        .as_ref()
                        .map(|s| !s.trim().is_empty())
                        .unwrap_or(false)
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct FunctionDetail {
    pub callees: Option<Vec<String>>,
    pub called_by: Option<Vec<String>>,
    pub disasm: Option<Vec<String>>,
    pub pool: Option<Vec<String>>,
    pub draft: Option<String>,
    pub draft_div: Option<u64>,
}

impl ChaosDb {
    pub fn match_pct_functions(&self) -> f64 {
        pct(self.stats.matched_functions, self.stats.total_functions)
    }

    pub fn match_pct_bytes(&self) -> f64 {
        pct(self.stats.matched_bytes, self.stats.total_bytes)
    }

    pub fn project_name(&self) -> &str {
        self.project
            .as_ref()
            .map(|p| p.name.as_str())
            .filter(|s| !s.is_empty())
            .unwrap_or("unknown")
    }

    /// Linear scan by id. Prefer an external `id → index` map for hot paths
    /// (TUI keeps one on `App`); this is fine for CLI one-shots.
    pub fn find_by_id(&self, id: &str) -> Option<&ChaosFunction> {
        self.functions.iter().find(|f| f.id == id)
    }

    /// Linear scan by name. Prefer indexes for interactive paths.
    pub fn find_by_name(&self, name: &str) -> Option<&ChaosFunction> {
        self.functions.iter().find(|f| f.name == name)
    }
}

fn pct(n: u64, d: u64) -> f64 {
    if d == 0 {
        0.0
    } else {
        (n as f64 / d as f64) * 100.0
    }
}

pub fn format_pct(n: u64, d: u64) -> String {
    format!("{:.2}", pct(n, d))
}
