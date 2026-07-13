//! Priority ranking (matches web viewer rules).

use std::collections::HashMap;

use crate::schema::ChaosFunction;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PriorityMode {
    Nearly,
    Scaffolded,
    Biggest,
}

impl PriorityMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Nearly => "nearly",
            Self::Scaffolded => "scaffolded",
            Self::Biggest => "biggest",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "nearly" | "near" | "near-miss" => Some(Self::Nearly),
            "scaffolded" | "scaffold" | "sim" => Some(Self::Scaffolded),
            "biggest" | "bytes" | "size" => Some(Self::Biggest),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Nearly => "Nearly done",
            Self::Scaffolded => "Best scaffolded",
            Self::Biggest => "Biggest bytes",
        }
    }
}

const TOP_N: usize = 25;

/// Rank unmatched, unclaimed functions for the given mode.
pub fn priority_rows<'a>(
    functions: &'a [ChaosFunction],
    locked_by: &HashMap<String, String>,
    mode: PriorityMode,
) -> Vec<&'a ChaosFunction> {
    let mut un: Vec<&ChaosFunction> = functions
        .iter()
        .filter(|f| !f.matched && !locked_by.contains_key(&f.id))
        .collect();

    match mode {
        PriorityMode::Nearly => {
            un.retain(|f| {
                f.div.is_some() && !f.cat.as_deref().unwrap_or("").contains("materialization")
            });
            un.sort_by(|a, b| {
                a.div
                    .cmp(&b.div)
                    .then_with(|| a.size.cmp(&b.size))
                    .then_with(|| a.name.cmp(&b.name))
            });
        }
        PriorityMode::Scaffolded => {
            un.retain(|f| f.sim.is_some() && f.floor.is_none());
            un.sort_by(|a, b| {
                b.sim
                    .partial_cmp(&a.sim)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| a.name.cmp(&b.name))
            });
        }
        PriorityMode::Biggest => {
            un.retain(|f| f.floor.is_none());
            un.sort_by(|a, b| b.size.cmp(&a.size).then_with(|| a.name.cmp(&b.name)));
        }
    }

    un.into_iter().take(TOP_N).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fn_(
        id: &str,
        matched: bool,
        size: u64,
        div: Option<u64>,
        sim: Option<f64>,
        floor: Option<&str>,
        cat: Option<&str>,
    ) -> ChaosFunction {
        ChaosFunction {
            id: id.into(),
            module: "arm9".into(),
            name: id.into(),
            addr: 0x2000000,
            size,
            matched,
            src_path: None,
            author: None,
            div,
            cat: cat.map(str::to_string),
            floor: floor.map(str::to_string),
            sim,
            sibling: None,
            match_provenance: None,
        }
    }

    #[test]
    fn nearly_sorts_by_div_then_size() {
        let fns = vec![
            fn_("a", false, 100, Some(3), None, None, None),
            fn_("b", false, 50, Some(1), None, None, None),
            fn_("c", false, 10, Some(1), None, None, None),
            fn_("d", true, 10, Some(0), None, None, None),
            fn_(
                "e",
                false,
                5,
                Some(1),
                None,
                None,
                Some("materialization leak"),
            ),
        ];
        let locked = HashMap::new();
        let rows = priority_rows(&fns, &locked, PriorityMode::Nearly);
        assert_eq!(
            rows.iter().map(|f| f.id.as_str()).collect::<Vec<_>>(),
            vec!["c", "b", "a"]
        );
    }

    #[test]
    fn biggest_excludes_floor_and_claimed() {
        let fns = vec![
            fn_("big", false, 1000, None, None, None, None),
            fn_("floor", false, 2000, None, None, Some("parked"), None),
            fn_("mid", false, 500, None, None, None, None),
        ];
        let mut locked = HashMap::new();
        locked.insert("big".into(), "alice".into());
        let rows = priority_rows(&fns, &locked, PriorityMode::Biggest);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "mid");
    }
}
