//! Per-project data-tracking conventions.
//!
//! Profiles pick a convention in the Projects hub. **Default** is the current
//! chaos-viewer / sm64ds-compatible behavior. **Experimental** is a fork for
//! alternate tracking (match provenance, and future changes). Default profiles
//! never require experimental fields.

use serde::{Deserialize, Serialize};

use crate::schema::{ChaosFunction, MatchProvenance};

/// How this CLI interprets / tracks atlas data for a saved project.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Convention {
    /// Current upstream-compatible tracking (sm64ds and friends).
    #[default]
    Default,
    /// Opt-in fork for experimental tracking. Diverges from Default only where
    /// documented (e.g. required match provenance on matched functions).
    Experimental,
}

impl Convention {
    pub fn cycle(self) -> Self {
        match self {
            Self::Default => Self::Experimental,
            Self::Experimental => Self::Default,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Experimental => "experimental",
        }
    }

    /// Short tag for list rows and headers.
    pub fn short(self) -> &'static str {
        match self {
            Self::Default => "def",
            Self::Experimental => "exp",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "default" | "def" | "classic" | "sm64ds" => Some(Self::Default),
            "experimental" | "exp" | "experiment" => Some(Self::Experimental),
            _ => None,
        }
    }

    pub fn all() -> &'static [Self] {
        &[Self::Default, Self::Experimental]
    }
}

/// Tracking helpers that may diverge by convention.
///
/// Call sites that identify functions, scopes, or claims keys should go through
/// these methods so experimental schemes can change without touching Default.
#[derive(Debug, Clone, Copy)]
pub struct Tracking;

impl Tracking {
    /// Stable key used for selection, batch, and detail identity in the TUI.
    ///
    /// Both conventions currently use the atlas `function.id` field.
    pub fn function_key(convention: Convention, f: &ChaosFunction) -> String {
        match convention {
            Convention::Default | Convention::Experimental => f.id.clone(),
        }
    }

    /// Human-facing scope line (module + name); identical for now.
    pub fn function_label(convention: Convention, f: &ChaosFunction) -> String {
        match convention {
            Convention::Default | Convention::Experimental => {
                format!("{} · {}", f.module, f.name)
            }
        }
    }

    /// Whether two functions should be treated as the same unit of work.
    pub fn same_function(convention: Convention, a: &ChaosFunction, b: &ChaosFunction) -> bool {
        Self::function_key(convention, a) == Self::function_key(convention, b)
    }

    /// Match provenance for a function under this convention.
    ///
    /// - **Default**: returns atlas field if present (no requirement).
    /// - **Experimental**: same field; use [`Self::provenance_status`] to check completeness.
    pub fn match_provenance(
        _convention: Convention,
        f: &ChaosFunction,
    ) -> Option<&MatchProvenance> {
        f.match_provenance.as_ref()
    }

    /// Experimental rule: every **matched** function must record how it was matched
    /// (human, or AI with model + reasoning level + harness).
    pub fn requires_match_provenance(convention: Convention) -> bool {
        matches!(convention, Convention::Experimental)
    }

    /// Status of provenance for display / validation.
    pub fn provenance_status(convention: Convention, f: &ChaosFunction) -> ProvenanceStatus<'_> {
        if !f.matched {
            return ProvenanceStatus::NotMatched;
        }
        match convention {
            Convention::Default => match f.match_provenance.as_ref() {
                Some(p) => ProvenanceStatus::Present(p),
                None => ProvenanceStatus::OptionalMissing,
            },
            Convention::Experimental => match f.match_provenance.as_ref() {
                None => ProvenanceStatus::RequiredMissing,
                Some(p) if p.is_complete() => ProvenanceStatus::Present(p),
                Some(p) => ProvenanceStatus::Incomplete(p),
            },
        }
    }

    /// Lines for the detail pane (empty under default when nothing recorded).
    pub fn provenance_detail_lines(convention: Convention, f: &ChaosFunction) -> Vec<String> {
        match Self::provenance_status(convention, f) {
            ProvenanceStatus::NotMatched => Vec::new(),
            ProvenanceStatus::OptionalMissing => Vec::new(),
            ProvenanceStatus::Present(p) => {
                vec![format!("matched via: {}", p.summary())]
            }
            ProvenanceStatus::RequiredMissing => {
                vec!["matched via: ⚠ MISSING (experimental requires human or AI provenance)".into()]
            }
            ProvenanceStatus::Incomplete(p) => {
                vec![format!(
                    "matched via: ⚠ INCOMPLETE · {}  (AI needs model + reasoning + harness)",
                    p.summary()
                )]
            }
        }
    }
}

/// Result of checking match provenance under a convention.
#[derive(Debug, Clone, Copy)]
pub enum ProvenanceStatus<'a> {
    NotMatched,
    /// Default atlas without the field — fine.
    OptionalMissing,
    /// Experimental matched function without provenance — not fine.
    RequiredMissing,
    Present(&'a MatchProvenance),
    /// Present but AI record lacks model / reasoning / harness.
    Incomplete(&'a MatchProvenance),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::MatchProvenance;

    fn sample_fn(matched: bool, prov: Option<MatchProvenance>) -> ChaosFunction {
        ChaosFunction {
            id: "arm9:foo".into(),
            module: "arm9".into(),
            name: "foo".into(),
            addr: 0x200,
            size: 16,
            matched,
            src_path: None,
            author: None,
            div: None,
            cat: None,
            floor: None,
            sim: None,
            sibling: None,
            match_provenance: prov,
        }
    }

    #[test]
    fn cycle_and_parse() {
        assert_eq!(Convention::Default.cycle(), Convention::Experimental);
        assert_eq!(Convention::Experimental.cycle(), Convention::Default);
        assert_eq!(Convention::parse("exp"), Some(Convention::Experimental));
        assert_eq!(Convention::parse("DEFAULT"), Some(Convention::Default));
        assert!(Convention::parse("nope").is_none());
    }

    #[test]
    fn tracking_keys_match_for_both_conventions() {
        let f = sample_fn(false, None);
        assert_eq!(
            Tracking::function_key(Convention::Default, &f),
            Tracking::function_key(Convention::Experimental, &f)
        );
        assert!(Tracking::same_function(Convention::Default, &f, &f));
    }

    #[test]
    fn experimental_requires_complete_provenance_on_matched() {
        let bare = sample_fn(true, None);
        assert!(matches!(
            Tracking::provenance_status(Convention::Experimental, &bare),
            ProvenanceStatus::RequiredMissing
        ));
        assert!(matches!(
            Tracking::provenance_status(Convention::Default, &bare),
            ProvenanceStatus::OptionalMissing
        ));

        let human = sample_fn(
            true,
            Some(MatchProvenance::Human {
                by: None,
                note: None,
            }),
        );
        assert!(matches!(
            Tracking::provenance_status(Convention::Experimental, &human),
            ProvenanceStatus::Present(_)
        ));

        let partial_ai = sample_fn(
            true,
            Some(MatchProvenance::Ai {
                model: "gpt-5".into(),
                reasoning: None,
                harness: Some("fanout".into()),
                by: None,
            }),
        );
        assert!(matches!(
            Tracking::provenance_status(Convention::Experimental, &partial_ai),
            ProvenanceStatus::Incomplete(_)
        ));

        let full_ai = sample_fn(
            true,
            Some(MatchProvenance::Ai {
                model: "gpt-5".into(),
                reasoning: Some("high".into()),
                harness: Some("fanout-v3".into()),
                by: None,
            }),
        );
        assert!(matches!(
            Tracking::provenance_status(Convention::Experimental, &full_ai),
            ProvenanceStatus::Present(_)
        ));
        let lines = Tracking::provenance_detail_lines(Convention::Experimental, &full_ai);
        assert!(lines[0].contains("model=gpt-5"));
        assert!(lines[0].contains("reasoning=high"));
        assert!(lines[0].contains("harness=fanout-v3"));
        assert!(!lines[0].contains("by="));
    }

    #[test]
    fn match_provenance_serde_roundtrip() {
        let ai = MatchProvenance::Ai {
            model: "claude-opus".into(),
            reasoning: Some("high".into()),
            harness: Some("batch".into()),
            by: None,
        };
        let v = serde_json::to_value(&ai).unwrap();
        assert_eq!(v["kind"], "ai");
        assert_eq!(v["model"], "claude-opus");
        let back: MatchProvenance = serde_json::from_value(v).unwrap();
        assert_eq!(back, ai);

        let human = MatchProvenance::Human {
            by: Some("alice".into()),
            note: None,
        };
        let v = serde_json::to_value(&human).unwrap();
        assert_eq!(v["kind"], "human");
        let back: MatchProvenance = serde_json::from_value(v).unwrap();
        assert_eq!(back, human);
    }
}
