//! Discover published chaos-db.json for a GitHub repository.

use anyhow::{anyhow, Context, Result};
use reqwest::Client;

use crate::schema::ChaosDb;

/// Probe a GitHub repo for a published Chaos Viewer data file.
///
/// Returns the raw URL **and** the already-parsed atlas so callers do not need
/// a second multi‑MB download. Order matches the web viewer in
/// tangosdev/chaos-viewer.
pub async fn discover_chaos_db(
    client: &Client,
    github: &str,
    branch: Option<&str>,
) -> Result<(String, ChaosDb)> {
    let (owner, name) =
        parse_github(github).ok_or_else(|| anyhow!("not a github.com repo URL: {github}"))?;

    let mut cands: Vec<String> = Vec::new();
    if let Some(b) = branch.map(str::trim).filter(|s| !s.is_empty()) {
        let b = b.trim_matches('/');
        cands.push(format!(
            "https://raw.githubusercontent.com/{owner}/{name}/{b}/chaos-db.json"
        ));
        cands.push(format!(
            "https://raw.githubusercontent.com/{owner}/{name}/{b}/data/chaos-db.json"
        ));
    }
    cands.extend([
        format!("https://raw.githubusercontent.com/{owner}/{name}/chaos-data/chaos-db.json"),
        format!("https://raw.githubusercontent.com/{owner}/{name}/chaos-data/data/chaos-db.json"),
        format!("https://raw.githubusercontent.com/{owner}/{name}/main/data/chaos-db.json"),
        format!("https://raw.githubusercontent.com/{owner}/{name}/main/chaos-db.json"),
        format!("https://raw.githubusercontent.com/{owner}/{name}/master/data/chaos-db.json"),
        format!("https://raw.githubusercontent.com/{owner}/{name}/master/chaos-db.json"),
        format!("https://raw.githubusercontent.com/{owner}/{name}/main/docs/chaos-db.json"),
        format!("https://{owner}.github.io/{name}/data/chaos-db.json"),
        format!("https://{owner}.github.io/{name}/chaos-db.json"),
    ]);

    let mut last_err: Option<String> = None;
    let mut saw_timeout = false;
    for url in &cands {
        match try_load_atlas(client, url).await {
            Ok(db) => return Ok((url.clone(), db)),
            Err(e) => {
                let msg = format!("{e:#}");
                // Prefer reporting timeouts / network errors on primary candidates.
                if msg.contains("timed out")
                    || msg.contains("timeout")
                    || msg.contains("error sending request")
                {
                    saw_timeout = true;
                    last_err = Some(format!("{url}: {msg}"));
                } else if last_err.is_none() {
                    last_err = Some(format!("{url}: {msg}"));
                }
                continue;
            }
        }
    }

    if saw_timeout {
        Err(anyhow!(
            "could not load chaos-db.json for {github}: network timed out. \
Last error: {}",
            last_err.unwrap_or_else(|| "unknown".into())
        ))
    } else {
        Err(anyhow!(
            "no published chaos-db.json found for {github} (tried chaos-data, main, master, pages). \
Last probe: {}",
            last_err.unwrap_or_else(|| "no details".into())
        ))
    }
}

/// Probe only; returns the URL of the first valid atlas.
pub async fn discover_data_url(
    client: &Client,
    github: &str,
    branch: Option<&str>,
) -> Result<String> {
    Ok(discover_chaos_db(client, github, branch).await?.0)
}

/// Try one candidate URL. Rejects non-success HTTP without downloading a body
/// when possible; validates schema when the body is present.
pub async fn try_load_atlas(client: &Client, url: &str) -> Result<ChaosDb> {
    let resp = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("GET {url}"))?;
    if !resp.status().is_success() {
        anyhow::bail!("HTTP {} for {url}", resp.status());
    }
    let text = resp.text().await.context("read candidate body")?;
    let db: ChaosDb = serde_json::from_str(&text).context("parse chaos-db.json")?;
    if db.stats.total_functions > 0 || !db.functions.is_empty() {
        return Ok(db);
    }
    // empty but valid schema still counts (setup projects)
    if text.contains("\"functions\"") && text.contains("\"stats\"") {
        return Ok(db);
    }
    anyhow::bail!("empty or invalid atlas at {url}");
}

pub fn parse_github(github: &str) -> Option<(String, String)> {
    let re = regex_lite_match(github)?;
    Some(re)
}

/// True if both strings refer to the same GitHub owner/repo (or are equal).
pub fn sources_equivalent(a: &str, b: &str) -> bool {
    let a = a.trim().trim_end_matches('/');
    let b = b.trim().trim_end_matches('/');
    if a.eq_ignore_ascii_case(b) {
        return true;
    }
    if let (Some((o1, n1)), Some((o2, n2))) = (parse_github(a), parse_github(b)) {
        return o1.eq_ignore_ascii_case(&o2) && n1.eq_ignore_ascii_case(&n2);
    }
    // raw.githubusercontent.com/owner/repo/...
    if let (Some((o1, n1)), Some((o2, n2))) = (parse_raw_github(a), parse_raw_github(b)) {
        return o1.eq_ignore_ascii_case(&o2) && n1.eq_ignore_ascii_case(&n2);
    }
    false
}

fn parse_raw_github(url: &str) -> Option<(String, String)> {
    let s = url.trim();
    let rest = s
        .strip_prefix("https://raw.githubusercontent.com/")
        .or_else(|| s.strip_prefix("http://raw.githubusercontent.com/"))?;
    let mut parts = rest.split('/').filter(|p| !p.is_empty());
    let owner = parts.next()?.to_string();
    let name = parts.next()?.to_string();
    if owner.is_empty() || name.is_empty() {
        return None;
    }
    Some((owner, name))
}

/// Minimal github.com owner/repo extraction without a regex crate.
fn regex_lite_match(github: &str) -> Option<(String, String)> {
    let s = github.trim().trim_end_matches('/');
    let idx = s.find("github.com/")?;
    let rest = &s[idx + "github.com/".len()..];
    let mut parts = rest.split('/').filter(|p| !p.is_empty());
    let owner = parts.next()?.to_string();
    let name = parts.next()?.trim_end_matches(".git").to_string();
    if owner.is_empty() || name.is_empty() {
        return None;
    }
    Some((owner, name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_github_urls() {
        assert_eq!(
            parse_github("https://github.com/tangosdev/sm64ds-decomp").unwrap(),
            ("tangosdev".into(), "sm64ds-decomp".into())
        );
        assert_eq!(
            parse_github("https://github.com/you/repo.git").unwrap(),
            ("you".into(), "repo".into())
        );
    }

    #[test]
    fn sources_equivalent_github() {
        assert!(sources_equivalent(
            "https://github.com/you/sm64ds-decomp",
            "https://github.com/you/sm64ds-decomp/"
        ));
        assert!(sources_equivalent(
            "https://github.com/You/Repo",
            "https://github.com/you/repo.git"
        ));
    }
}
