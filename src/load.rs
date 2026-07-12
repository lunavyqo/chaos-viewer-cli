//! Load chaos-db.json and optional detail chunks from path or URL.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, Context, Result};
use reqwest::Client;

use crate::discover::discover_data_url;
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

pub async fn load_chaos_db(
    client: &Client,
    input: Option<&str>,
    repo: Option<&str>,
    branch: Option<&str>,
) -> Result<(ChaosDb, DataSource)> {
    if let Some(repo) = repo {
        let url = discover_data_url(client, repo, branch).await?;
        let db = fetch_json::<ChaosDb>(client, &url).await?;
        return Ok((db, DataSource::Url(url)));
    }

    let input = input.ok_or_else(|| {
        anyhow!("provide --input <path|url> or --repo <github-url> (or open the TUI and enter one)")
    })?;

    let source = DataSource::from_input(input);
    let db = match &source {
        DataSource::Path(p) => {
            let text =
                std::fs::read_to_string(p).with_context(|| format!("read {}", p.display()))?;
            serde_json::from_str(&text).context("parse chaos-db.json")?
        }
        DataSource::Url(u) => fetch_json(client, u).await?,
    };
    Ok((db, source))
}

async fn fetch_json<T: serde::de::DeserializeOwned>(client: &Client, url: &str) -> Result<T> {
    let busted = cache_bust(url);
    let resp = client
        .get(&busted)
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
}

pub async fn load_function_detail(
    client: &Client,
    cache: &DetailCache,
    module: &str,
    name: &str,
) -> Result<Option<FunctionDetail>> {
    {
        let guard = cache.inner.lock().expect("detail cache lock");
        if let Some(mod_map) = guard.get(module) {
            return Ok(mod_map.get(name).cloned());
        }
    }

    let path_or_url = format!("{}{module}.json", cache.base);
    let map: HashMap<String, FunctionDetail> = if path_or_url.starts_with("http://")
        || path_or_url.starts_with("https://")
    {
        match fetch_json(client, &path_or_url).await {
            Ok(m) => m,
            Err(_) => {
                cache
                    .inner
                    .lock()
                    .expect("detail cache lock")
                    .insert(module.to_string(), HashMap::new());
                return Ok(None);
            }
        }
    } else {
        let p = PathBuf::from(&path_or_url);
        if !p.exists() {
            cache
                .inner
                .lock()
                .expect("detail cache lock")
                .insert(module.to_string(), HashMap::new());
            return Ok(None);
        }
        let text = std::fs::read_to_string(&p).with_context(|| format!("read {}", p.display()))?;
        serde_json::from_str(&text).context("parse detail chunk")?
    };

    let detail = map.get(name).cloned();
    cache
        .inner
        .lock()
        .expect("detail cache lock")
        .insert(module.to_string(), map);
    Ok(detail)
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
