//! Claims API polling and CLAIMS.md parsing.

use std::collections::HashMap;

use anyhow::{Context, Result};
use reqwest::Client;
use serde::Deserialize;

use crate::discover::parse_github;
use crate::schema::ChaosFunction;

#[derive(Debug, Clone, Deserialize)]
pub struct Claim {
    pub id: Option<String>,
    pub module: String,
    pub start: AddrValue,
    pub end: AddrValue,
    pub handle: Option<String>,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum AddrValue {
    Num(u64),
    Str(String),
}

impl AddrValue {
    pub fn to_u64(&self) -> u64 {
        match self {
            Self::Num(n) => *n,
            Self::Str(s) => {
                let s = s.trim();
                if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
                    u64::from_str_radix(hex, 16).unwrap_or(0)
                } else {
                    s.parse().unwrap_or(0)
                }
            }
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ClaimsSession {
    pub token: String,
    pub handle: String,
}

impl ClaimsSession {
    pub fn from_env() -> Option<Self> {
        let token = std::env::var("CHAOS_CLAIMS_SESSION").ok()?;
        if token.is_empty() {
            return None;
        }
        let handle =
            std::env::var("CHAOS_CLAIMS_HANDLE").unwrap_or_else(|_| "chaos-viewer-user".into());
        Some(Self { token, handle })
    }
}

#[derive(Debug, Deserialize)]
struct ClaimsApiResponse {
    #[serde(default)]
    claims: Vec<Claim>,
}

/// Parse a repo CLAIMS.md table into locks (ports web viewer logic).
pub fn parse_claims_md(text: &str) -> Vec<Claim> {
    let mut out = Vec::new();
    for line in text.lines() {
        if !line.trim_start().starts_with('|') {
            continue;
        }
        let raw: Vec<&str> = line.split('|').map(str::trim).collect();
        if raw.len() < 5 {
            continue;
        }
        // indices 1..len-1 are cells
        let cells: Vec<&str> = raw[1..raw.len() - 1].to_vec();
        if cells.len() < 4 {
            continue;
        }
        let range = cells[0];
        let who = cells[1];
        let status = cells[3];
        if range.chars().all(|c| c == '-') || (range.starts_with('_') && range.ends_with('_')) {
            continue;
        }
        let status_l = status.to_ascii_lowercase();
        if ["done", "merged", "example", "abandoned", "released"]
            .iter()
            .any(|s| status_l.contains(s))
        {
            continue;
        }
        if range.eq_ignore_ascii_case("Range") || range == "范围" {
            continue;
        }

        let row_mod = extract_module(range);
        let mut found = false;
        // (optional_token)(0xADDR, size 0xSZ)
        let bytes = range.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if let Some((mod_tok, start, size, end_idx)) = parse_span_at(range, i) {
                found = true;
                let module = mod_tok
                    .and_then(extract_module)
                    .or_else(|| row_mod.clone())
                    .unwrap_or_else(|| "arm9".into());
                out.push(Claim {
                    id: None,
                    module,
                    start: AddrValue::Num(start),
                    end: AddrValue::Num(start + size),
                    handle: Some(who.to_string()),
                    note: None,
                });
                i = end_idx;
            } else {
                i += 1;
            }
        }
        if !found {
            if let (Some(mod_name), Some((s, e))) = (row_mod, parse_bare_range(range)) {
                out.push(Claim {
                    id: None,
                    module: mod_name,
                    start: AddrValue::Num(s),
                    end: AddrValue::Num(e),
                    handle: Some(who.to_string()),
                    note: None,
                });
            }
        }
    }
    out
}

fn extract_module(s: &str) -> Option<String> {
    // ovNNN
    let bytes = s.as_bytes();
    for i in 0..bytes.len().saturating_sub(4) {
        if bytes[i] == b'o'
            && bytes[i + 1] == b'v'
            && bytes[i + 2].is_ascii_digit()
            && bytes[i + 3].is_ascii_digit()
            && bytes
                .get(i + 4)
                .map(|c| c.is_ascii_digit())
                .unwrap_or(false)
        {
            return Some(s[i..i + 5].to_string());
        }
    }
    if s.to_ascii_lowercase().contains("arm9") {
        return Some("arm9".into());
    }
    None
}

fn parse_span_at(s: &str, start_idx: usize) -> Option<(Option<&str>, u64, u64, usize)> {
    // look for (0x...., size 0x....) starting at or after start_idx
    let rest = &s[start_idx..];
    let open = rest.find("(0x")?;
    let after_open = open + 1;
    let rest2 = &rest[after_open..];
    let comma = rest2.find(',')?;
    let addr_s = rest2[..comma].trim();
    let after_comma = &rest2[comma + 1..];
    let size_key = after_comma.to_ascii_lowercase();
    let size_pos = size_key.find("size")?;
    let after_size = after_comma[size_pos + 4..].trim_start();
    let close = after_size.find(')')?;
    let size_s = after_size[..close].trim();
    let start = parse_hex(addr_s)?;
    let size = parse_hex(size_s)?;
    let end_idx = start_idx
        + after_open
        + comma
        + 1
        + size_pos
        + 4
        + (after_size.len() - after_size[close..].len())
        + close
        + 1;
    // prefix token before (
    let prefix = rest[..open].trim();
    let mod_tok = if prefix.is_empty() {
        None
    } else {
        Some(prefix.split_whitespace().last().unwrap_or(prefix))
    };
    // simplify end_idx
    let abs_close = s[start_idx..].find("(0x")? + start_idx;
    let close_abs = s[abs_close..].find(')')? + abs_close + 1;
    Some((mod_tok, start, size, close_abs.max(end_idx)))
}

fn parse_bare_range(range: &str) -> Option<(u64, u64)> {
    // 0xA-0xB
    let lower = range;
    let dash = lower.find('-')?;
    let left = lower[..dash].trim();
    let right = lower[dash + 1..].trim();
    // find hex on each side
    let s = extract_hex(left)?;
    let e = extract_hex(right)?;
    Some((s, e))
}

fn extract_hex(s: &str) -> Option<u64> {
    if let Some(i) = s.find("0x").or_else(|| s.find("0X")) {
        let rest = &s[i..];
        let end = rest
            .char_indices()
            .skip(2)
            .find(|(_, c)| !c.is_ascii_hexdigit())
            .map(|(i, _)| i)
            .unwrap_or(rest.len());
        parse_hex(&rest[..end])
    } else {
        None
    }
}

fn parse_hex(s: &str) -> Option<u64> {
    let s = s.trim();
    let hex = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X"))?;
    u64::from_str_radix(hex, 16).ok()
}

pub fn merge_locked_map(functions: &[ChaosFunction], claims: &[Claim]) -> HashMap<String, String> {
    let mut m = HashMap::new();
    if claims.is_empty() {
        return m;
    }
    for f in functions {
        if f.matched {
            continue;
        }
        for c in claims {
            if c.module != f.module {
                continue;
            }
            let s = c.start.to_u64();
            let e = c.end.to_u64();
            if f.addr < e && f.addr + f.size > s {
                m.insert(
                    f.id.clone(),
                    c.handle.clone().unwrap_or_else(|| "someone".into()),
                );
                break;
            }
        }
    }
    m
}

pub async fn load_claims(
    client: &Client,
    claims_api: Option<&str>,
    github: Option<&str>,
) -> Result<(Vec<Claim>, bool)> {
    let mut collected = Vec::new();
    let mut any_live = false;

    if let Some(api) = claims_api.filter(|s| !s.is_empty()) {
        let url = api.trim_end_matches('/');
        match client.get(url).send().await {
            Ok(resp) if resp.status().is_success() => {
                if let Ok(body) = resp.json::<ClaimsApiResponse>().await {
                    collected.extend(body.claims);
                    any_live = true;
                } else if let Ok(list) = client.get(url).send().await {
                    // try raw array
                    let _ = list;
                }
            }
            _ => {}
        }
        // also try parsing flexible: { ok, claims }
        if !any_live {
            if let Ok(resp) = client.get(url).send().await {
                if resp.status().is_success() {
                    if let Ok(v) = resp.json::<serde_json::Value>().await {
                        if let Some(arr) = v.get("claims").and_then(|c| c.as_array()) {
                            for item in arr {
                                if let Ok(c) = serde_json::from_value::<Claim>(item.clone()) {
                                    collected.push(c);
                                    any_live = true;
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    if let Some(gh) = github {
        if let Some((owner, name)) = parse_github(gh) {
            for br in ["main", "master"] {
                let url =
                    format!("https://raw.githubusercontent.com/{owner}/{name}/{br}/CLAIMS.md");
                match client.get(&url).send().await {
                    Ok(resp) if resp.status().is_success() => {
                        let text = resp.text().await.context("CLAIMS.md body")?;
                        collected.extend(parse_claims_md(&text));
                        any_live = true;
                        break;
                    }
                    _ => continue,
                }
            }
        }
    }

    Ok((collected, any_live))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_claims_md_active_row() {
        let md = r#"
| Range | Who | Date | Status |
|-------|-----|------|--------|
| ov002 func_x (0x021019d0, size 0x100) | alice | 2026-07-02 | active |
| ov002 func_y (0x02102000, size 0x80) | bob | 2026-07-02 | done |
"#;
        let claims = parse_claims_md(md);
        assert_eq!(claims.len(), 1);
        assert_eq!(claims[0].module, "ov002");
        assert_eq!(claims[0].start.to_u64(), 0x021019d0);
        assert_eq!(claims[0].end.to_u64(), 0x021019d0 + 0x100);
        assert_eq!(claims[0].handle.as_deref(), Some("alice"));
    }

    #[test]
    fn merge_locks_by_range() {
        let fns = vec![ChaosFunction {
            id: "ov002:0x21019d0".into(),
            module: "ov002".into(),
            name: "func_x".into(),
            addr: 0x021019d0,
            size: 0x50,
            matched: false,
            src_path: None,
            author: None,
            div: None,
            cat: None,
            floor: None,
            sim: None,
            sibling: None,
        }];
        let claims = vec![Claim {
            id: None,
            module: "ov002".into(),
            start: AddrValue::Num(0x021019d0),
            end: AddrValue::Num(0x021019d0 + 0x100),
            handle: Some("alice".into()),
            note: None,
        }];
        let m = merge_locked_map(&fns, &claims);
        assert_eq!(m.get("ov002:0x21019d0").map(String::as_str), Some("alice"));
    }
}
