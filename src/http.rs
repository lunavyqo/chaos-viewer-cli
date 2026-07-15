//! Shared HTTP client for atlas / claims / discovery.

use std::time::Duration;

use anyhow::{Context, Result};
use reqwest::Client;

/// Build the default reqwest client used across the app.
///
/// Gzip is enabled so multi‑MB `chaos-db.json` files compress on the wire
/// (often ~200 KB instead of ~2 MB from raw.githubusercontent.com).
pub fn build_client() -> Result<Client> {
    Client::builder()
        .user_agent("chaos-viewer-cli/0.1")
        .connect_timeout(Duration::from_secs(10))
        // With gzip, a full atlas is typically well under a few seconds.
        .timeout(Duration::from_secs(60))
        .pool_idle_timeout(Duration::from_secs(30))
        .build()
        .context("build HTTP client")
}
