//! Chaos Viewer CLI library: schema, load/discover, priorities, prompts, claims.

pub mod claims;
pub mod clipboard;
pub mod discover;
pub mod load;
pub mod prioritize;
pub mod prompt;
pub mod schema;
pub mod tui;

pub use claims::{
    merge_locked_map, normalize_claims_api_base, parse_claims_md, Claim, ClaimsClient,
    ClaimsSession,
};
pub use discover::discover_data_url;
pub use load::{details_base_from_source, load_chaos_db, load_function_detail, DataSource};
pub use prioritize::{priority_rows, PriorityMode};
pub use prompt::{build_prompt, PromptOptions};
pub use schema::{ChaosDb, ChaosFunction, FunctionDetail, ProjectConfig, Stats};
