//! Per-project data-tracking conventions.
//!
//! Profiles pick a convention in the Projects hub. **Default** is the current
//! chaos-viewer / sm64ds-compatible behavior. **Experimental** is a fork point
//! for alternate tracking schemes; for now it behaves identically to Default so
//! you can opt a personal repo into experimental without breaking sm64ds work.
//! Future tracking changes land only under [`Convention::Experimental`].

use serde::{Deserialize, Serialize};

use crate::schema::ChaosFunction;

/// How this CLI interprets / tracks atlas data for a saved project.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Convention {
    /// Current upstream-compatible tracking (sm64ds and friends).
    #[default]
    Default,
    /// Opt-in fork for experimental tracking. Same as Default until tracking
    /// changes are applied — then only Experimental profiles diverge.
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
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let f = ChaosFunction {
            id: "arm9:foo".into(),
            module: "arm9".into(),
            name: "foo".into(),
            addr: 0x200,
            size: 16,
            matched: false,
            src_path: None,
            author: None,
            div: None,
            cat: None,
            floor: None,
            sim: None,
            sibling: None,
        };
        assert_eq!(
            Tracking::function_key(Convention::Default, &f),
            Tracking::function_key(Convention::Experimental, &f)
        );
        assert!(Tracking::same_function(Convention::Default, &f, &f));
    }
}
