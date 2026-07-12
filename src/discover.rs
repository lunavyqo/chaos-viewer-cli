//! Discover published chaos-db.json for a GitHub repository.

use anyhow::{anyhow, Context, Result};
use reqwest::Client;

use crate::schema::ChaosDb;

/// Probe a GitHub repo for a published Chaos Viewer data file.
/// Order matches the web viewer in tangosdev/chaos-viewer.
pub async fn discover_data_url(
    client: &Client,
    github: &str,
    branch: Option<&str>,
) -> Result<String> {
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

    for url in cands {
        match client.get(&url).send().await {
            Ok(resp) if resp.status().is_success() => {
                let text = resp.text().await.context("read candidate body")?;
                if let Ok(db) = serde_json::from_str::<ChaosDb>(&text) {
                    if db.stats.total_functions > 0 || !db.functions.is_empty() {
                        return Ok(url);
                    }
                    // empty but valid schema still counts (setup projects)
                    if text.contains("\"functions\"") && text.contains("\"stats\"") {
                        return Ok(url);
                    }
                }
            }
            _ => continue,
        }
    }

    Err(anyhow!(
        "no published chaos-db.json found for {github} (tried chaos-data, main, master, pages)"
    ))
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
            parse_github("https://github.com/you/repo.git/").unwrap(),
            ("you".into(), "repo".into())
        );
    }
}
