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
///
/// Supports both sm64ds-style rows:
/// `| ov002 func_x (0x021019d0, size 0x5470) | handle | date | status |`
/// and electroplankton-style rows:
/// `| Module | Range | Claimant | Date | Status | Notes |`
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
        // drop leading/trailing empties from outer pipes
        let cells: Vec<&str> = raw[1..raw.len() - 1].to_vec();
        if cells.len() < 4 {
            continue;
        }

        // Layout detection: if first cell looks like a module-only column and
        // second cell holds the range, use electroplankton-style columns
        // (Module | Range | Claimant | Date | Status | Notes).
        let (range, who, status) = if cells.len() >= 5
            && looks_like_module_cell(cells[0])
            && (cells[1].contains("0x")
                || cells[1].contains("0X")
                || is_placeholder_cell(cells[1]))
        {
            (cells[1], cells[2], cells[4])
        } else {
            // sm64ds-style: Range | Who | Date | Status
            (cells[0], cells[1], cells[3])
        };

        if is_placeholder_cell(range)
            || is_separator_cell(range)
            || (range.starts_with('_') && range.ends_with('_'))
        {
            continue;
        }
        let status_l = status.to_ascii_lowercase();
        if ["done", "merged", "example", "abandoned", "released"]
            .iter()
            .any(|s| status_l.contains(s))
        {
            continue;
        }
        if is_header_range_cell(range) {
            continue;
        }
        // No address payload → not a claim row (headers, placeholders, notes)
        if !range.contains("0x") && !range.contains("0X") {
            continue;
        }

        let module_hint = extract_module(range)
            .or_else(|| cells.first().and_then(|c| extract_module(c)))
            .or_else(|| {
                // electroplankton Module column
                if looks_like_module_cell(cells[0]) {
                    Some(cells[0].to_string())
                } else {
                    None
                }
            });

        let mut found = false;
        let mut search_from = 0;
        while search_from < range.len() {
            // Always advance on UTF-8 char boundaries.
            if !range.is_char_boundary(search_from) {
                search_from += 1;
                continue;
            }
            match parse_span_from(range, search_from) {
                Some((mod_tok, start, size, next)) => {
                    found = true;
                    let module = mod_tok
                        .and_then(extract_module)
                        .or_else(|| module_hint.clone())
                        .unwrap_or_else(|| "arm9".into());
                    out.push(Claim {
                        id: None,
                        module,
                        start: AddrValue::Num(start),
                        end: AddrValue::Num(start + size),
                        handle: Some(who.to_string()),
                        note: None,
                    });
                    search_from = next.max(search_from + 1);
                }
                None => {
                    // Step one Unicode scalar, not one byte.
                    match range[search_from..].chars().next() {
                        Some(c) => search_from += c.len_utf8(),
                        None => break,
                    }
                }
            }
        }
        if !found {
            if let Some((s, e)) = parse_bare_range(range) {
                let module = module_hint.unwrap_or_else(|| "arm9".into());
                out.push(Claim {
                    id: None,
                    module,
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

fn is_placeholder_cell(s: &str) -> bool {
    let t = s.trim();
    t.is_empty()
        || t == "—"
        || t == "–"
        || t == "-"
        || t.chars().all(|c| matches!(c, '-' | '—' | '–' | ' ' | '\t'))
}

fn is_separator_cell(s: &str) -> bool {
    let t = s.trim();
    !t.is_empty() && t.chars().all(|c| c == '-' || c == ':')
}

fn is_header_range_cell(s: &str) -> bool {
    let t = s.trim();
    t.eq_ignore_ascii_case("Range")
        || t == "范围"
        || t.to_ascii_lowercase().starts_with("range")
        || t.eq_ignore_ascii_case("Module")
}

fn looks_like_module_cell(s: &str) -> bool {
    let t = s.trim();
    if t.is_empty() || is_placeholder_cell(t) {
        return false;
    }
    if extract_module(t).is_some() {
        return true;
    }
    // bare module labels without addresses
    let lower = t.to_ascii_lowercase();
    lower == "arm7"
        || lower == "arm9"
        || lower.starts_with("ov")
        || (!t.contains("0x") && !t.contains('(') && t.len() < 32)
}

fn extract_module(s: &str) -> Option<String> {
    // ovNNN via char-safe search
    let lower = s.to_ascii_lowercase();
    if let Some(i) = lower.find("ov") {
        let rest = &s[i..];
        let bytes = rest.as_bytes();
        if bytes.len() >= 5
            && bytes[0].eq_ignore_ascii_case(&b'o')
            && bytes[1].eq_ignore_ascii_case(&b'v')
            && bytes[2].is_ascii_digit()
            && bytes[3].is_ascii_digit()
            && bytes[4].is_ascii_digit()
        {
            return Some(rest[..5].to_string());
        }
    }
    let lower = s.to_ascii_lowercase();
    if lower.contains("arm9") {
        return Some("arm9".into());
    }
    if lower.contains("arm7") {
        return Some("arm7".into());
    }
    None
}

/// Find next `(0xADDR, size 0xSZ)` span at or after `from` (byte index, char boundary).
fn parse_span_from(s: &str, from: usize) -> Option<(Option<&str>, u64, u64, usize)> {
    if from > s.len() || !s.is_char_boundary(from) {
        return None;
    }
    let rest = &s[from..];
    let open_rel = rest.find("(0x").or_else(|| rest.find("(0X"))?;
    let open_abs = from + open_rel;
    let after_paren = open_abs + 1; // '(' is ASCII
    if after_paren >= s.len() {
        return None;
    }
    let after = &s[after_paren..];
    let close_rel = after.find(')')?;
    let inside = after.get(..close_rel)?;
    // inside: 0xADDR, size 0xSZ
    let comma = inside.find(',')?;
    let addr_s = inside.get(..comma)?.trim();
    let after_comma = inside.get(comma + 1..)?.trim();
    let size_l = after_comma.to_ascii_lowercase();
    let size_idx = size_l.find("size")?;
    let size_s = after_comma.get(size_idx + 4..)?.trim();
    let start = parse_hex(addr_s)?;
    let size = parse_hex(size_s)?;
    let end_abs = after_paren + close_rel + 1;

    let prefix = s.get(from..open_abs)?.trim();
    let mod_tok = if prefix.is_empty() {
        None
    } else {
        Some(prefix.split_whitespace().last().unwrap_or(prefix))
    };
    Some((mod_tok, start, size, end_abs))
}

fn parse_bare_range(range: &str) -> Option<(u64, u64)> {
    // Prefer "0xA-0xB" / "0xA - 0xB" forms; ASCII hyphen or en/em dash.
    let mut hexs = Vec::new();
    let mut i = 0;
    while i < range.len() {
        if !range.is_char_boundary(i) {
            i += 1;
            continue;
        }
        let rest = &range[i..];
        if let Some(rel) = rest.find("0x").or_else(|| rest.find("0X")) {
            let start = i + rel;
            if let Some(val) = extract_hex(&range[start..]) {
                hexs.push(val);
                // advance past this hex token
                let tok = &range[start..];
                let end = tok
                    .char_indices()
                    .skip(2)
                    .find(|(_, c)| !c.is_ascii_hexdigit())
                    .map(|(j, _)| start + j)
                    .unwrap_or(range.len());
                i = end;
                continue;
            }
        }
        match range[i..].chars().next() {
            Some(c) => i += c.len_utf8(),
            None => break,
        }
    }
    if hexs.len() >= 2 {
        Some((hexs[0], hexs[1]))
    } else {
        None
    }
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
    fn parse_claims_md_em_dash_placeholder_no_panic() {
        // electroplankton-style empty table uses Unicode em dashes
        let md = r#"
| Module | Range (inclusive) | Claimant | Date (UTC) | Status | Notes |
|--------|-------------------|----------|------------|--------|-------|
| —      | —                 | —        | —          | —      | empty |
"#;
        let claims = parse_claims_md(md);
        assert!(claims.is_empty());
    }

    #[test]
    fn parse_claims_md_electroplankton_columns() {
        let md = r#"
| Module | Range (inclusive) | Claimant | Date (UTC) | Status | Notes |
|--------|-------------------|----------|------------|--------|-------|
| arm9   | 0x02000000-0x02001000 | alice | 2026-07-12 | active | working |
"#;
        let claims = parse_claims_md(md);
        assert_eq!(claims.len(), 1);
        assert_eq!(claims[0].module, "arm9");
        assert_eq!(claims[0].start.to_u64(), 0x02000000);
        assert_eq!(claims[0].end.to_u64(), 0x02001000);
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
