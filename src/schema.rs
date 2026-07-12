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
    pub author: Option<String>,
    pub div: Option<u64>,
    pub cat: Option<String>,
    pub floor: Option<String>,
    pub sim: Option<f64>,
    pub sibling: Option<String>,
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

    pub fn find_by_id(&self, id: &str) -> Option<&ChaosFunction> {
        self.functions.iter().find(|f| f.id == id)
    }

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
