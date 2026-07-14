//! Load chaos-db.json and optional detail chunks from path or URL.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, Context, Result};
use reqwest::Client;

use crate::discover::{discover_chaos_db, try_load_atlas};
use crate::schema::{ChaosDb, FunctionDetail};

#[derive(Debug, Clone)]
pub enum DataSource {
    Path(PathBuf),
    Url(String),
}

impl DataSource {
    pub fn from_input(input: &str) -> Self {
        if input.starts_with("http://") || input.starts_with("https://") {
            Self::Url(input.to_string())
        } else {
            Self::Path(PathBuf::from(input))
        }
    }

    pub fn display(&self) -> String {
        match self {
            Self::Path(p) => p.display().to_string(),
            Self::Url(u) => u.clone(),
        }
    }
}

pub fn details_base_from_source(source: &DataSource) -> String {
    match source {
        DataSource::Path(p) => {
            let parent = p.parent().unwrap_or_else(|| Path::new("."));
            parent.join("details").to_string_lossy().into_owned() + std::path::MAIN_SEPARATOR_STR
        }
        DataSource::Url(u) => {
            // strip filename; append details/
            if let Some(pos) = u.rfind('/') {
                format!("{}details/", &u[..=pos])
            } else {
                format!("{u}/details/")
            }
        }
    }
}

/// Load atlas data.
///
/// - `preferred_atlas_url`: last-known raw JSON URL (skips full GitHub discovery
///   when still valid — makes project reopen much faster).
/// - `fresh`: when true, cache-bust remote GETs (used by TUI **`u`** update).
pub async fn load_chaos_db(
    client: &Client,
    input: Option<&str>,
    repo: Option<&str>,
    branch: Option<&str>,
) -> Result<(ChaosDb, DataSource)> {
    load_chaos_db_opts(client, input, repo, branch, None, false).await
}

/// Like [`load_chaos_db`] with reopen cache + optional cache-bust.
pub async fn load_chaos_db_opts(
    client: &Client,
    input: Option<&str>,
    repo: Option<&str>,
    branch: Option<&str>,
    preferred_atlas_url: Option<&str>,
    fresh: bool,
) -> Result<(ChaosDb, DataSource)> {
    // Fast path: last known raw atlas URL (reopen saved GitHub projects).
    if let Some(url) = preferred_atlas_url.map(str::trim).filter(|s| !s.is_empty()) {
        match fetch_json::<ChaosDb>(client, url, fresh).await {
            Ok(db) => return Ok((db, DataSource::Url(url.to_string()))),
            Err(_) => {
                // Fall through to full discovery / input path.
            }
        }
    }

    if let Some(repo) = repo {
        // Discover downloads + parses once — do not re-fetch the multi-MB body.
        let (url, db) = discover_chaos_db(client, repo, branch).await?;
        return Ok((db, DataSource::Url(url)));
    }

    let input = input.ok_or_else(|| {
        anyhow!("provide --input <path|url> or --repo <github-url> (or open the TUI and enter one)")
    })?;

    // Bare GitHub repo typed as --input still goes through discovery once.
    if input.contains("github.com/")
        && !input.contains("raw.githubusercontent.com")
        && !input.ends_with(".json")
    {
        let (url, db) = discover_chaos_db(client, input, branch).await?;
        return Ok((db, DataSource::Url(url)));
    }

    let source = DataSource::from_input(input);
    let db = match &source {
        DataSource::Path(p) => {
            let text =
                std::fs::read_to_string(p).with_context(|| format!("read {}", p.display()))?;
            serde_json::from_str(&text).context("parse chaos-db.json")?
        }
        DataSource::Url(u) => {
            if fresh {
                fetch_json(client, u, true).await?
            } else {
                try_load_atlas(client, u).await?
            }
        }
    };
    Ok((db, source))
}

async fn fetch_json<T: serde::de::DeserializeOwned>(
    client: &Client,
    url: &str,
    fresh: bool,
) -> Result<T> {
    let req_url = if fresh {
        cache_bust(url)
    } else {
        url.to_string()
    };
    let resp = client
        .get(&req_url)
        .send()
        .await
        .with_context(|| format!("GET {url}"))?
        .error_for_status()
        .with_context(|| format!("HTTP error for {url}"))?;
    let text = resp.text().await.context("response body")?;
    serde_json::from_str(&text).with_context(|| format!("parse JSON from {url}"))
}

fn cache_bust(url: &str) -> String {
    let t = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    if url.contains('?') {
        format!("{url}&t={t}")
    } else {
        format!("{url}?t={t}")
    }
}

/// Session-scoped detail cache keyed by module name.
pub struct DetailCache {
    inner: Mutex<HashMap<String, HashMap<String, FunctionDetail>>>,
    base: String,
}

impl DetailCache {
    pub fn new(base: String) -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
            base,
        }
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    /// If this module chunk is already in memory, return `Some(detail_or_none)`.
    /// If the module has never been loaded, return `None` (caller should fetch).
    pub fn get_if_module_loaded(&self, module: &str, name: &str) -> Option<Option<FunctionDetail>> {
        let guard = self.inner.lock().expect("detail cache lock");
        let mod_map = guard.get(module)?;
        Some(mod_map.get(name).cloned())
    }

    /// Whether the module detail JSON has been fetched (even if empty / missing).
    pub fn is_module_loaded(&self, module: &str) -> bool {
        self.inner
            .lock()
            .expect("detail cache lock")
            .contains_key(module)
    }

    pub fn loaded_module_count(&self) -> usize {
        self.inner.lock().expect("detail cache lock").len()
    }
}

/// Ensure a module's detail chunk is in the cache (full module map).
///
/// Missing remote/local files are cached as empty maps so we do not retry forever.
pub async fn ensure_module_chunk(client: &Client, cache: &DetailCache, module: &str) -> Result<()> {
    {
        let guard = cache.inner.lock().expect("detail cache lock");
        if guard.contains_key(module) {
            return Ok(());
        }
    }

    let path_or_url = format!("{}{module}.json", cache.base);
    let map: HashMap<String, FunctionDetail> =
        if path_or_url.starts_with("http://") || path_or_url.starts_with("https://") {
            fetch_json(client, &path_or_url, false)
                .await
                .unwrap_or_default()
        } else {
            let p = PathBuf::from(&path_or_url);
            if !p.exists() {
                HashMap::new()
            } else {
                let text =
                    std::fs::read_to_string(&p).with_context(|| format!("read {}", p.display()))?;
                serde_json::from_str(&text).context("parse detail chunk")?
            }
        };

    cache
        .inner
        .lock()
        .expect("detail cache lock")
        .insert(module.to_string(), map);
    Ok(())
}

pub async fn load_function_detail(
    client: &Client,
    cache: &DetailCache,
    module: &str,
    name: &str,
) -> Result<Option<FunctionDetail>> {
    ensure_module_chunk(client, cache, module).await?;
    let guard = cache.inner.lock().expect("detail cache lock");
    Ok(guard.get(module).and_then(|m| m.get(name).cloned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn details_base_from_path() {
        let s = DataSource::Path(PathBuf::from("/tmp/data/chaos-db.json"));
        let base = details_base_from_source(&s);
        assert!(base.ends_with("details/") || base.ends_with("details\\"));
    }

    #[test]
    fn details_base_from_url() {
        let s = DataSource::Url(
            "https://raw.githubusercontent.com/o/r/chaos-data/chaos-db.json".into(),
        );
        assert_eq!(
            details_base_from_source(&s),
            "https://raw.githubusercontent.com/o/r/chaos-data/details/"
        );
    }
}
