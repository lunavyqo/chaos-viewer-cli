//! Chaos Viewer CLI library: schema, load/discover, priorities, prompts, claims.

pub mod claims;
pub mod clipboard;
pub mod conventions;
pub mod discover;
pub mod grok_launch;
pub mod http;
pub mod load;
pub mod prioritize;
pub mod projects;
pub mod prompt;
pub mod schema;
pub mod templates;
pub mod tools_catalog;
pub mod tui;

pub use conventions::{Convention, Tracking};

pub use claims::{
    merge_locked_map, normalize_claims_api_base, parse_claims_md, Claim, ClaimsClient,
    ClaimsSession,
};
pub use discover::{discover_chaos_db, discover_data_url};
pub use http::build_client;
pub use load::{
    details_base_from_source, ensure_module_chunk, load_chaos_db, load_chaos_db_opts,
    load_function_detail, DataSource, DetailCache, DETAIL_PREWARM_CONCURRENCY,
};
pub use prioritize::{priority_rows, PriorityMode};
pub use prompt::{build_prompt, PromptOptions};
pub use schema::{ChaosDb, ChaosFunction, FunctionDetail, MatchProvenance, ProjectConfig, Stats};
