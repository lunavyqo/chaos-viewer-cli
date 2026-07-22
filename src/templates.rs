//! Prompt templates: built-in prompts plus user TOML templates.
//!
//! Layout on disk (override with `CHAOS_HOME`):
//! ```text
//! ~/.config/chaos/
//!   config.toml                 # default_template = "chaos-viewer"
//!   templates/
//!     short.toml                # user-defined
//! ```
//!
//! Built-ins:
//! - `chaos-viewer` — web parity (default / sm64ds)
//! - `chaos-experimental` — match task + mandatory matchProvenance reporting

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::claims::ClaimsSession;
use crate::prompt::{build_builtin_prompt, build_experimental_prompt, PromptOptions};
use crate::schema::{ChaosFunction, FunctionDetail, ProjectConfig};

pub const BUILTIN_ID: &str = "chaos-viewer";
pub const BUILTIN_NAME: &str = "Chaos Viewer (default)";
/// Experimental convention: provenance-aware match prompt.
pub const BUILTIN_EXPERIMENTAL_ID: &str = "chaos-experimental";
pub const BUILTIN_EXPERIMENTAL_NAME: &str = "Chaos Experimental (provenance)";

pub fn is_builtin_template_id(id: &str) -> bool {
    id == BUILTIN_ID || id == BUILTIN_EXPERIMENTAL_ID
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UserConfig {
    /// Template id used when none is selected explicitly.
    #[serde(default = "default_template_id")]
    pub default_template: String,
    /// Last / preferred project profile id (`projects.toml`).
    #[serde(default)]
    pub active_project: Option<String>,
    /// Default coding agent for **`g`**: `grok` | `codex` | `claude` | `antigravity`.
    #[serde(default)]
    pub default_agent: Option<String>,
    /// Path or name of the Grok Build binary (`grok`). Empty = PATH / `~/.grok/bin/grok`.
    #[serde(default)]
    pub grok_bin: Option<String>,
    /// Path or name of the Codex CLI binary.
    #[serde(default)]
    pub codex_bin: Option<String>,
    /// Path or name of the Claude Code binary.
    #[serde(default)]
    pub claude_bin: Option<String>,
    /// Path or name of the Antigravity CLI (`agy`).
    #[serde(default)]
    pub antigravity_bin: Option<String>,
    /// `interactive` (Grok TUI, default) or `run` (headless `--prompt-file`). Grok only.
    #[serde(default)]
    pub grok_mode: Option<String>,
    /// Extra CLI args for Grok (e.g. `["--always-approve"]`).
    #[serde(default)]
    pub grok_extra_args: Vec<String>,
    /// Extra CLI args for Codex.
    #[serde(default)]
    pub codex_extra_args: Vec<String>,
    /// Extra CLI args for Claude Code.
    #[serde(default)]
    pub claude_extra_args: Vec<String>,
    /// Extra CLI args for Antigravity.
    #[serde(default)]
    pub antigravity_extra_args: Vec<String>,
    /// Fallback local decomp path when the active project has no `local_repo`.
    #[serde(default)]
    pub grok_default_repo: Option<String>,
    /// External terminal host: `auto` | `terminal` | `iterm` | `linux` | `windows`.
    #[serde(default)]
    pub grok_terminal: Option<String>,
    /// Currently selected model slug (must be in [`PROVENANCE_MODELS`]).
    #[serde(default)]
    pub provenance_model: Option<String>,
    /// Selected reasoning level: high | xhigh | max | medium | low | none.
    #[serde(default = "default_provenance_reasoning")]
    pub provenance_reasoning: String,
    /// Selected harness slug from the fixed preset list.
    #[serde(default = "default_provenance_harness")]
    pub provenance_harness: String,
}

/// One fixed model option for experimental MATCH_RESULT prefill.
#[derive(Debug, Clone, Copy)]
pub struct ProvenanceModel {
    pub slug: &'static str,
    pub label: &'static str,
}

/// Fixed model list (Prompt **`m`** opens a picker — not cycled, not user-extensible).
pub const PROVENANCE_MODELS: &[ProvenanceModel] = &[
    ProvenanceModel {
        slug: "grok-4.5",
        label: "Grok 4.5",
    },
    ProvenanceModel {
        slug: "composer-2.5",
        label: "Composer 2.5",
    },
    ProvenanceModel {
        slug: "claude-sonnet-5",
        label: "Claude Sonnet 5",
    },
    ProvenanceModel {
        slug: "claude-opus-4.8",
        label: "Claude Opus 4.8",
    },
    ProvenanceModel {
        slug: "claude-opus-4.7",
        label: "Claude Opus 4.7",
    },
    ProvenanceModel {
        slug: "claude-opus-4.6",
        label: "Claude Opus 4.6",
    },
    ProvenanceModel {
        slug: "claude-fable-5",
        label: "Claude Fable 5",
    },
    ProvenanceModel {
        slug: "gpt-5.6-luna",
        label: "GPT 5.6 Luna",
    },
    ProvenanceModel {
        slug: "gpt-5.6-terra",
        label: "GPT 5.6 Terra",
    },
    ProvenanceModel {
        slug: "gpt-5.6-sol",
        label: "GPT 5.6 Sol",
    },
    ProvenanceModel {
        slug: "deepseek-v4-flash",
        label: "DeepSeek V4 Flash",
    },
    ProvenanceModel {
        slug: "deepseek-v4-pro",
        label: "DeepSeek V4 Pro",
    },
    ProvenanceModel {
        slug: "glm-5.2",
        label: "GLM 5.2",
    },
    ProvenanceModel {
        slug: "kimi-k3",
        label: "Kimi K3",
    },
    ProvenanceModel {
        slug: "kimi-3",
        label: "Kimi 3",
    },
    ProvenanceModel {
        slug: "hy3",
        label: "Hy3",
    },
    ProvenanceModel {
        slug: "stepfun-3.7",
        label: "StepFun 3.7",
    },
    ProvenanceModel {
        slug: "muse-spark-1.1",
        label: "Muse Spark 1.1",
    },
    ProvenanceModel {
        slug: "gemini-3.5-pro",
        label: "Gemini 3.5 Pro",
    },
    ProvenanceModel {
        slug: "gemini-3.5-flash",
        label: "Gemini 3.5 Flash",
    },
];

/// Fixed reasoning levels for experimental prompts (cycle with **`y`** on Prompt).
/// Highest effort first: max > xhigh > high > medium > low > none.
pub const PROVENANCE_REASONING_LEVELS: &[&str] = &["max", "xhigh", "high", "medium", "low", "none"];

/// Fixed harness presets (cycle with **`w`** on Prompt).
pub const PROVENANCE_HARNESS_PRESETS: &[&str] = &[
    "grok-build",
    "cursor-agent",
    "claude-code",
    "codex",
    "antigravity",
    "manual",
];

fn default_provenance_reasoning() -> String {
    "high".into()
}

fn default_provenance_harness() -> String {
    "grok-build".into()
}

/// Index of `slug` in [`PROVENANCE_MODELS`], if any.
pub fn provenance_model_index(slug: &str) -> Option<usize> {
    PROVENANCE_MODELS.iter().position(|m| m.slug == slug)
}

/// Display label for a known model slug, or the slug itself.
pub fn provenance_model_label(slug: &str) -> &str {
    provenance_model_index(slug)
        .map(|i| PROVENANCE_MODELS[i].label)
        .unwrap_or(slug)
}

fn default_template_id() -> String {
    BUILTIN_ID.into()
}

/// Ensure selected model / reasoning / harness are valid.
fn normalize_provenance_config(cfg: &mut UserConfig) {
    let selected_ok = cfg
        .provenance_model
        .as_ref()
        .is_some_and(|m| provenance_model_index(m).is_some());
    if !selected_ok {
        cfg.provenance_model = Some(PROVENANCE_MODELS[0].slug.to_string());
    }
    if !PROVENANCE_REASONING_LEVELS.contains(&cfg.provenance_reasoning.as_str()) {
        cfg.provenance_reasoning = default_provenance_reasoning();
    }
    if !PROVENANCE_HARNESS_PRESETS.contains(&cfg.provenance_harness.as_str()) {
        cfg.provenance_harness = default_provenance_harness();
    }
}

/// On-disk user template (`templates/<id>.toml`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserTemplateFile {
    /// Display name (defaults to file stem).
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// Rendered once at the top. See docs/prompt-templates.md for placeholders.
    #[serde(default)]
    pub header: String,
    /// Rendered once per function in the batch.
    pub function: String,
    /// Rendered once at the bottom.
    #[serde(default)]
    pub footer: String,
}

#[derive(Debug, Clone)]
pub enum TemplateKind {
    Builtin,
    User(UserTemplateFile),
}

#[derive(Debug, Clone)]
pub struct TemplateEntry {
    pub id: String,
    pub name: String,
    pub description: String,
    pub kind: TemplateKind,
    /// Set for user files.
    pub path: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct TemplateStore {
    pub config: UserConfig,
    pub config_path: PathBuf,
    pub templates_dir: PathBuf,
    pub entries: Vec<TemplateEntry>,
}

impl TemplateStore {
    pub fn load() -> Self {
        let home = chaos_home();
        let config_path = home.join("config.toml");
        let templates_dir = home.join("templates");
        let _ = fs::create_dir_all(&templates_dir);

        let mut config = load_user_config(&config_path);
        // Ensure provenance picker has a usable model list + valid selection.
        normalize_provenance_config(&mut config);
        // Seed example template once if the directory is empty (besides nothing).
        seed_example_template(&templates_dir);

        let mut entries = vec![
            TemplateEntry {
                id: BUILTIN_ID.into(),
                name: BUILTIN_NAME.into(),
                description: "Built-in prompt matching tangosdev/chaos-viewer".into(),
                kind: TemplateKind::Builtin,
                path: None,
            },
            TemplateEntry {
                id: BUILTIN_EXPERIMENTAL_ID.into(),
                name: BUILTIN_EXPERIMENTAL_NAME.into(),
                description:
                    "Experimental: match + required model/reasoning/harness (or human) provenance"
                        .into(),
                kind: TemplateKind::Builtin,
                path: None,
            },
        ];

        if let Ok(rd) = fs::read_dir(&templates_dir) {
            let mut files: Vec<PathBuf> = rd
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| {
                    p.extension()
                        .and_then(|e| e.to_str())
                        .is_some_and(|e| e.eq_ignore_ascii_case("toml"))
                })
                .collect();
            files.sort();
            for path in files {
                match load_user_template(&path) {
                    Ok(entry) => entries.push(entry),
                    Err(e) => {
                        // Keep going; bad files shouldn't break the app.
                        eprintln!("chaos: skip template {}: {e:#}", path.display());
                    }
                }
            }
        }

        Self {
            config,
            config_path,
            templates_dir,
            entries,
        }
    }

    pub fn default_id(&self) -> &str {
        if self
            .entries
            .iter()
            .any(|e| e.id == self.config.default_template)
        {
            self.config.default_template.as_str()
        } else {
            BUILTIN_ID
        }
    }

    pub fn get(&self, id: &str) -> Option<&TemplateEntry> {
        self.entries.iter().find(|e| e.id == id)
    }

    pub fn index_of(&self, id: &str) -> Option<usize> {
        self.entries.iter().position(|e| e.id == id)
    }

    pub fn cycle_id(&self, current: &str, delta: isize) -> String {
        if self.entries.is_empty() {
            return BUILTIN_ID.into();
        }
        let i = self.index_of(current).unwrap_or(0) as isize;
        let n = self.entries.len() as isize;
        let next = ((i + delta) % n + n) % n;
        self.entries[next as usize].id.clone()
    }

    pub fn set_default(&mut self, id: &str) -> Result<()> {
        if self.get(id).is_none() {
            bail!("unknown template id '{id}'");
        }
        self.config.default_template = id.to_string();
        save_user_config(&self.config_path, &self.config)?;
        Ok(())
    }

    /// Persist the default coding agent for Prompt **`g`**.
    pub fn set_default_agent(&mut self, agent_id: &str) -> Result<()> {
        self.config.default_agent = Some(agent_id.to_string());
        save_user_config(&self.config_path, &self.config)?;
        Ok(())
    }

    /// Currently selected model slug for MATCH_RESULT prefill.
    pub fn provenance_model(&self) -> &str {
        self.config
            .provenance_model
            .as_deref()
            .filter(|s| provenance_model_index(s).is_some())
            .unwrap_or(PROVENANCE_MODELS[0].slug)
    }

    /// Display label for the selected model.
    pub fn provenance_model_label(&self) -> &str {
        provenance_model_label(self.provenance_model())
    }

    /// Currently selected reasoning level.
    pub fn provenance_reasoning(&self) -> &str {
        let r = self.config.provenance_reasoning.as_str();
        if PROVENANCE_REASONING_LEVELS.contains(&r) {
            r
        } else {
            "high"
        }
    }

    /// Currently selected harness slug.
    pub fn provenance_harness(&self) -> &str {
        let h = self.config.provenance_harness.as_str();
        if PROVENANCE_HARNESS_PRESETS.contains(&h) {
            h
        } else {
            "grok-build"
        }
    }

    /// Select a fixed model by slug (Prompt model picker · enter). Persists.
    pub fn set_provenance_model(&mut self, slug: &str) -> Result<&str> {
        if provenance_model_index(slug).is_none() {
            bail!("unknown model slug '{slug}'");
        }
        self.config.provenance_model = Some(slug.to_string());
        save_user_config(&self.config_path, &self.config)?;
        Ok(self.provenance_model())
    }

    /// Cycle fixed reasoning levels (Prompt **`y`**).
    pub fn cycle_provenance_reasoning(&mut self, delta: isize) -> Result<&str> {
        let levels = PROVENANCE_REASONING_LEVELS;
        let cur = levels
            .iter()
            .position(|l| *l == self.config.provenance_reasoning.as_str())
            .unwrap_or(0) as isize;
        let n = levels.len() as isize;
        let next = ((cur + delta).rem_euclid(n)) as usize;
        self.config.provenance_reasoning = levels[next].to_string();
        save_user_config(&self.config_path, &self.config)?;
        Ok(self.provenance_reasoning())
    }

    /// Cycle fixed harness presets (Prompt **`w`**).
    pub fn cycle_provenance_harness(&mut self, delta: isize) -> Result<&str> {
        let presets = PROVENANCE_HARNESS_PRESETS;
        let cur = presets
            .iter()
            .position(|h| *h == self.config.provenance_harness.as_str())
            .unwrap_or(0) as isize;
        let n = presets.len() as isize;
        let next = ((cur + delta).rem_euclid(n)) as usize;
        self.config.provenance_harness = presets[next].to_string();
        save_user_config(&self.config_path, &self.config)?;
        Ok(self.provenance_harness())
    }

    /// Reload entries from disk (after creating/editing a template).
    pub fn reload(&mut self) {
        let fresh = Self::load();
        self.config = fresh.config;
        self.entries = fresh.entries;
    }

    /// Path to the on-disk file for a user template, if any.
    pub fn path_for(&self, id: &str) -> Option<&Path> {
        self.get(id).and_then(|e| e.path.as_deref())
    }

    /// Resolve a user template path for editing. Built-in has no file.
    pub fn editable_path(&self, id: &str) -> Result<PathBuf> {
        if is_builtin_template_id(id) {
            bail!(
                "'{id}' is built-in and has no file; press n (or `chaos templates new`) to make an editable copy"
            );
        }
        match self.path_for(id) {
            Some(p) if p.exists() => Ok(p.to_path_buf()),
            Some(p) => bail!("template file missing: {}", p.display()),
            None => bail!("unknown template id '{id}'"),
        }
    }

    /// Create `templates/<id>.toml` as an editable copy of the chaos-viewer prompt.
    /// Returns the path written. Does not open an editor.
    pub fn create_from_builtin(&self, id: &str, display_name: Option<&str>) -> Result<PathBuf> {
        let id = sanitize_template_id(id)?;
        if is_builtin_template_id(&id) {
            bail!("'{id}' is reserved for a built-in template");
        }
        fs::create_dir_all(&self.templates_dir)?;
        let path = self.templates_dir.join(format!("{id}.toml"));
        if path.exists() {
            bail!("template already exists: {}", path.display());
        }
        let name = display_name
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(id.as_str());
        let body = chaos_viewer_scaffold_toml(name);
        fs::write(&path, body).with_context(|| format!("write {}", path.display()))?;
        Ok(path)
    }

    pub fn render(
        &self,
        id: &str,
        project: &ProjectConfig,
        functions: &[(ChaosFunction, Option<FunctionDetail>)],
        opts: &PromptOptions,
    ) -> Result<String> {
        let entry = self
            .get(id)
            .with_context(|| format!("unknown template '{id}'"))?;
        match &entry.kind {
            TemplateKind::Builtin if entry.id == BUILTIN_EXPERIMENTAL_ID => {
                Ok(build_experimental_prompt(project, functions, opts))
            }
            TemplateKind::Builtin => Ok(build_builtin_prompt(project, functions, opts)),
            TemplateKind::User(t) => Ok(render_user_template(t, project, functions, opts)),
        }
    }
}

/// Config / templates root.
pub fn chaos_home() -> PathBuf {
    if let Ok(p) = std::env::var("CHAOS_HOME") {
        return PathBuf::from(p);
    }
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        return PathBuf::from(xdg).join("chaos");
    }
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".config").join("chaos")
}

pub fn load_user_config(path: &Path) -> UserConfig {
    match fs::read_to_string(path) {
        Ok(text) => toml::from_str(&text).unwrap_or_default(),
        Err(_) => UserConfig::default(),
    }
}

pub fn save_user_config(path: &Path, cfg: &UserConfig) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let text = toml::to_string_pretty(cfg).context("serialize config")?;
    fs::write(path, text).with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

fn load_user_template(path: &Path) -> Result<TemplateEntry> {
    let text = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let mut file: UserTemplateFile =
        toml::from_str(&text).with_context(|| format!("parse {}", path.display()))?;
    let id = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("template")
        .to_string();
    if file.name.is_empty() {
        file.name = id.clone();
    }
    if file.function.trim().is_empty() {
        bail!("template '{id}' is missing a non-empty `function` string");
    }
    Ok(TemplateEntry {
        id,
        name: file.name.clone(),
        description: file.description.clone(),
        kind: TemplateKind::User(file),
        path: Some(path.to_path_buf()),
    })
}

/// Preferred text editor: `$VISUAL`, then `$EDITOR`, then `nano`.
pub fn preferred_editor() -> String {
    std::env::var("VISUAL")
        .or_else(|_| std::env::var("EDITOR"))
        .unwrap_or_else(|_| "nano".into())
}

/// Open `path` in the preferred editor and wait until it exits.
pub fn open_in_editor(path: &Path) -> Result<()> {
    let editor = preferred_editor();
    // Allow multi-word EDITOR values like `code -w` via the shell.
    let status = Command::new("sh")
        .arg("-c")
        .arg(format!(
            "{} \"{}\"",
            editor,
            path.display().to_string().replace('"', "\\\"")
        ))
        .status()
        .with_context(|| format!("spawn editor '{editor}'"))?;
    if !status.success() {
        bail!("editor '{editor}' exited with {status}");
    }
    Ok(())
}

/// Valid template id: start with letter/digit, then letters, digits, `-`, `_`.
pub fn sanitize_template_id(raw: &str) -> Result<String> {
    let s = raw.trim();
    if s.is_empty() {
        bail!("template id is empty");
    }
    if s.len() > 64 {
        bail!("template id too long (max 64)");
    }
    let mut chars = s.chars();
    let first = chars.next().unwrap();
    if !first.is_ascii_alphanumeric() {
        bail!("template id must start with a letter or digit");
    }
    if !chars.all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        bail!("template id may only contain letters, digits, '-' and '_'");
    }
    Ok(s.to_string())
}

/// TOML scaffold that mirrors the built-in chaos-viewer prompt structure.
pub fn chaos_viewer_scaffold_toml(display_name: &str) -> String {
    format!(
        r#"# Prompt template for chaos-viewer-cli
# File stem = template id. Edit, save, quit the editor — chaos reloads the list.
# Placeholders: docs/prompt-templates.md
# This file starts as an editable copy of the built-in chaos-viewer layout.

name = "{name}"
description = "Editable copy of the chaos-viewer prompt"

header = """
Match {{n}} {{project_name}} function(s) to the retail binary, byte-for-byte.

SETUP (once): {{setup}}

COMPILER: {{compiler}}
{{cpp_note}}

READ FIRST: {{read_first}}
"""

function = """
======================================================================
FUNCTION: {{name}}   module: {{module}}   addr: 0x{{addrHex}}   size: {{size}} bytes
{{section_verify}}
{{section_sibling}}
{{section_floor}}
{{section_draft}}
{{section_disasm}}
"""

footer = """
Rules: {{rules}}
{{section_claims}}

Matched means byte-identical - iterate until the verify command reports a MATCH.
When it matches, fork the repo and open a pull request{{github_target}} against its default branch
(one function or a small related family per PR; note the compiler version and the function address).

{{near_miss_note}}
"""
"#,
        name = display_name.replace('"', "'"),
    )
}

fn seed_example_template(dir: &Path) {
    let path = dir.join("short.toml");
    if path.exists() {
        return;
    }
    // Only seed when the templates dir has no .toml files yet.
    let has_toml = fs::read_dir(dir)
        .ok()
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .any(|e| e.path().extension().and_then(|x| x.to_str()) == Some("toml"))
        })
        .unwrap_or(false);
    if has_toml {
        return;
    }
    let example = r#"# Example user prompt template for chaos-viewer-cli.
# Copy and edit; filename stem = template id (here: "short").
# Placeholders: see docs/prompt-templates.md

name = "Short"
description = "Compact match task (header + function + footer)"

header = """
Match {n} {project_name} function(s) byte-for-byte.
Compiler: {compiler}
Setup: {setup}
"""

function = """
======================================================================
FUNCTION: {name}   module: {module}   addr: 0x{addrHex}   size: {size}
{section_verify}
{section_sibling}
{section_draft}
{section_disasm}
"""

footer = """
{section_claims}
Rules: {rules}
When it matches, open a PR{github_target}.
"""
"#;
    let _ = fs::write(path, example);
}

fn render_user_template(
    t: &UserTemplateFile,
    project: &ProjectConfig,
    functions: &[(ChaosFunction, Option<FunctionDetail>)],
    opts: &PromptOptions,
) -> String {
    let n = if functions.is_empty() {
        1
    } else {
        functions.len()
    };
    let mut parts = Vec::new();
    if !t.header.trim().is_empty() {
        parts.push(expand_global(&t.header, project, n, opts));
    }
    for (fn_, det) in functions {
        parts.push(expand_function(
            &t.function,
            project,
            fn_,
            det.as_ref(),
            opts,
        ));
    }
    if !t.footer.trim().is_empty() {
        parts.push(expand_global(&t.footer, project, n, opts));
    }
    parts.join("\n\n")
}

fn expand_global(
    template: &str,
    project: &ProjectConfig,
    n: usize,
    opts: &PromptOptions,
) -> String {
    let mut s = template.to_string();
    let project_name = if project.name.is_empty() {
        "decomp"
    } else {
        project.name.as_str()
    };
    let github_target = if project.github.is_empty() {
        String::new()
    } else {
        format!(" to {}", project.github)
    };
    let claims = claims_block(project, n, opts.claims_session.as_ref());
    let map = [
        ("{n}", n.to_string()),
        ("{project_name}", project_name.into()),
        ("{github}", project.github.clone()),
        ("{github_target}", github_target),
        ("{compiler}", project.compiler.clone().unwrap_or_default()),
        (
            "{setup}",
            project
                .setup
                .as_ref()
                .map(|t| t.replace("{github}", &project.github))
                .unwrap_or_default(),
        ),
        ("{rules}", project.rules.clone().unwrap_or_default()),
        (
            "{read_first}",
            project.read_first.clone().unwrap_or_default(),
        ),
        ("{cpp_note}", project.cpp_note.clone().unwrap_or_default()),
        (
            "{near_miss_note}",
            project.near_miss_note.clone().unwrap_or_default(),
        ),
        (
            "{claims_api}",
            project.claims_api.clone().unwrap_or_default(),
        ),
        ("{section_claims}", claims),
    ];
    for (k, v) in map {
        s = s.replace(k, &v);
    }
    s
}

fn expand_function(
    template: &str,
    project: &ProjectConfig,
    fn_: &ChaosFunction,
    det: Option<&FunctionDetail>,
    opts: &PromptOptions,
) -> String {
    use crate::prompt::{resolve_ghidra_draft, resolve_near_miss_draft};

    let mut s = template.to_string();
    let verify = project
        .verify_command
        .as_ref()
        .map(|c| fill_fn_placeholders(c, project, fn_))
        .unwrap_or_default();
    let section_verify = if verify.is_empty() {
        String::new()
    } else {
        format!("VERIFY:\n  {verify}")
    };
    let section_sibling = match (&fn_.sibling, fn_.sim) {
        (Some(sib), sim) => {
            let sim = sim
                .map(|s| s.to_string())
                .unwrap_or_else(|| "undefined".into());
            format!(
                "CLOSEST MATCHED SIBLING (opcode similarity {sim}): src/{sib}.c[pp] - use it as your scaffold."
            )
        }
        _ => String::new(),
    };
    let section_floor = fn_
        .floor
        .as_ref()
        .map(|f| {
            format!(
                "WARNING: previously parked as \"{f}\" - check the sec 6e-6g levers before grinding."
            )
        })
        .unwrap_or_default();

    // Near-miss tip (details draft and/or local nearmiss/db.jsonl) + optional Ghidra.
    let (draft, draft_div, mut section_draft) = if let Some(near) =
        resolve_near_miss_draft(fn_, det, opts)
    {
        let div = near
            .draft_div
            .map(|d| d.to_string())
            .unwrap_or_else(|| "undefined".into());
        let block = format!(
                "A NEAR-MISS DRAFT EXISTS ({div} instruction(s) from matching) - START FROM THIS, do not re-decompile:\n```c\n{}\n```",
                near.text.trim_end()
            );
        (near.text.trim_end().to_string(), div, block)
    } else {
        (String::new(), String::new(), String::new())
    };
    if let Some(ghidra) = resolve_ghidra_draft(fn_, det, opts) {
        let gblock = format!(
            "GHIDRA DECOMPILER DRAFT (approximate C — NOT byte-matching). \
Use for structure / types / callees, then REWRITE so mwccarm + verify MATCH:\n```c\n{}\n```",
            ghidra.trim_end()
        );
        if section_draft.is_empty() {
            section_draft = gblock;
        } else {
            section_draft = format!("{section_draft}\n\n{gblock}");
        }
    }
    // {draft} raw text: prefer human near-miss, else Ghidra body.
    let draft = if draft.is_empty() {
        resolve_ghidra_draft(fn_, det, opts).unwrap_or_default()
    } else {
        draft
    };

    let (disasm, section_disasm) = match det.and_then(|d| d.disasm.as_ref()) {
        Some(dis) if !dis.is_empty() => {
            const MAX: usize = 90;
            let truncated = dis.len() > MAX;
            let mut body: Vec<String> = if truncated {
                dis.iter().take(MAX).cloned().collect()
            } else {
                dis.clone()
            };
            if truncated {
                body.push(format!("... ({} more lines omitted)", dis.len() - MAX));
            }
            if let Some(pool) = det.and_then(|d| d.pool.as_ref()) {
                if !pool.is_empty() {
                    body.push(String::new());
                    body.push("pool slots:".into());
                    for pl in pool.iter().take(40) {
                        body.push(format!("  {pl}"));
                    }
                }
            }
            let joined = body.join("\n");
            let header = if truncated {
                format!(
                    "TARGET DISASSEMBLY (first {MAX} of {} lines, annotated):",
                    dis.len()
                )
            } else {
                "TARGET DISASSEMBLY (annotated, callees resolved):".into()
            };
            let block = format!("{header}\n```\n{joined}\n```");
            (joined, block)
        }
        _ => (String::new(), String::new()),
    };

    let pool = det
        .and_then(|d| d.pool.as_ref())
        .map(|p| p.join("\n"))
        .unwrap_or_default();

    let map: HashMap<&str, String> = [
        ("{name}", fn_.name.clone()),
        ("{module}", fn_.module.clone()),
        ("{id}", fn_.id.clone()),
        ("{addr}", fn_.addr.to_string()),
        ("{addrHex}", format!("{:x}", fn_.addr)),
        ("{size}", fn_.size.to_string()),
        ("{sizeHex}", format!("{:x}", fn_.size)),
        ("{github}", project.github.clone()),
        (
            "{project_name}",
            if project.name.is_empty() {
                "decomp".into()
            } else {
                project.name.clone()
            },
        ),
        ("{verify}", verify),
        ("{section_verify}", section_verify),
        ("{sibling}", fn_.sibling.clone().unwrap_or_default()),
        (
            "{sim}",
            fn_.sim
                .map(|s| s.to_string())
                .unwrap_or_else(|| "undefined".into()),
        ),
        ("{section_sibling}", section_sibling),
        ("{floor}", fn_.floor.clone().unwrap_or_default()),
        ("{section_floor}", section_floor),
        ("{div}", fn_.div.map(|d| d.to_string()).unwrap_or_default()),
        ("{cat}", fn_.cat.clone().unwrap_or_default()),
        ("{author}", fn_.author.clone().unwrap_or_default()),
        ("{draft}", draft),
        ("{draft_div}", draft_div),
        ("{section_draft}", section_draft),
        ("{disasm}", disasm),
        ("{section_disasm}", section_disasm),
        ("{pool}", pool),
    ]
    .into_iter()
    .collect();

    // Longer keys first so {section_draft} wins over {draft}
    let mut keys: Vec<&str> = map.keys().copied().collect();
    keys.sort_by_key(|k| std::cmp::Reverse(k.len()));
    for k in keys {
        if let Some(v) = map.get(k) {
            s = s.replace(k, v);
        }
    }
    s
}

fn fill_fn_placeholders(t: &str, project: &ProjectConfig, fn_: &ChaosFunction) -> String {
    t.replace("{github}", &project.github)
        .replace("{name}", &fn_.name)
        .replace("{module}", &fn_.module)
        .replace("{addr}", &fn_.addr.to_string())
        .replace("{addrHex}", &format!("{:x}", fn_.addr))
        .replace("{size}", &fn_.size.to_string())
        .replace("{sizeHex}", &format!("{:x}", fn_.size))
}

fn claims_block(project: &ProjectConfig, n: usize, session: Option<&ClaimsSession>) -> String {
    // Same mandatory CLAIMS.md + cleanup text as the builtin footer.
    crate::prompt::claims_and_cleanup_block(project, n, session).join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::ChaosFunction;

    fn sample_project() -> ProjectConfig {
        ProjectConfig {
            name: "demo".into(),
            github: "https://github.com/you/demo".into(),
            setup: Some("clone {github}".into()),
            compiler: Some("cc".into()),
            verify_command: Some("verify {name} 0x{addrHex}".into()),
            rules: Some("no ROM".into()),
            ..Default::default()
        }
    }

    fn sample_fn() -> ChaosFunction {
        ChaosFunction {
            id: "arm9:0x1".into(),
            module: "arm9".into(),
            name: "foo".into(),
            addr: 0x100,
            size: 16,
            matched: false,
            src_path: None,
            author: None,
            div: None,
            cat: None,
            floor: None,
            sim: None,
            sibling: None,
            match_provenance: None,
        }
    }

    #[test]
    fn user_template_expands_placeholders() {
        let t = UserTemplateFile {
            name: "t".into(),
            description: String::new(),
            header: "H {n} {project_name}".into(),
            function: "F {name} 0x{addrHex} {section_verify}".into(),
            footer: "Foot {rules}".into(),
        };
        let text = render_user_template(
            &t,
            &sample_project(),
            &[(sample_fn(), None)],
            &PromptOptions::default(),
        );
        assert!(text.contains("H 1 demo"));
        assert!(text.contains("F foo 0x100"));
        assert!(text.contains("VERIFY:\n  verify foo 0x100"));
        assert!(text.contains("Foot no ROM"));
    }

    #[test]
    fn store_includes_builtin() {
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("CHAOS_HOME", dir.path());
        let store = TemplateStore::load();
        assert!(store.get(BUILTIN_ID).is_some());
        assert!(store.get(BUILTIN_EXPERIMENTAL_ID).is_some());
        assert_eq!(store.default_id(), BUILTIN_ID);
        // example short.toml seeded
        assert!(dir.path().join("templates/short.toml").exists());
        std::env::remove_var("CHAOS_HOME");
    }

    #[test]
    fn experimental_builtin_renders_provenance_prompt() {
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("CHAOS_HOME", dir.path());
        let store = TemplateStore::load();
        let project = ProjectConfig {
            name: "exp-demo".into(),
            github: "https://github.com/you/exp".into(),
            ..Default::default()
        };
        let fn_ = sample_fn();
        let text = store
            .render(
                BUILTIN_EXPERIMENTAL_ID,
                &project,
                &[(fn_, None)],
                &PromptOptions::default(),
            )
            .unwrap();
        assert!(text.contains("MATCH_RESULT"));
        assert!(text.contains("matchProvenance"));
        std::env::remove_var("CHAOS_HOME");
    }

    #[test]
    fn create_from_builtin_writes_file() {
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("CHAOS_HOME", dir.path());
        let store = TemplateStore::load();
        let path = store
            .create_from_builtin("my-copy", Some("My Copy"))
            .unwrap();
        assert!(path.exists());
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("name = \"My Copy\""));
        assert!(text.contains("section_disasm"));
        assert!(sanitize_template_id("bad id").is_err());
        std::env::remove_var("CHAOS_HOME");
    }

    #[test]
    fn provenance_pickers_select_and_cycle() {
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("CHAOS_HOME", dir.path());
        let mut store = TemplateStore::load();
        assert_eq!(PROVENANCE_MODELS.len(), 20);
        assert_eq!(store.provenance_model(), "grok-4.5");
        assert_eq!(store.provenance_model_label(), "Grok 4.5");
        assert_eq!(store.provenance_reasoning(), "high");
        assert_eq!(store.provenance_harness(), "grok-build");

        assert_eq!(
            store.set_provenance_model("claude-opus-4.8").unwrap(),
            "claude-opus-4.8"
        );
        assert_eq!(store.provenance_model_label(), "Claude Opus 4.8");
        assert!(store.set_provenance_model("not-a-real-model").is_err());
        assert_eq!(store.set_provenance_model("kimi-3").unwrap(), "kimi-3");
        assert_eq!(store.provenance_model_label(), "Kimi 3");
        assert_eq!(
            store.set_provenance_model("claude-opus-4.8").unwrap(),
            "claude-opus-4.8"
        );

        // Default high; +1 walks max…none list: high → medium → low → none → max → xhigh → high
        assert_eq!(store.cycle_provenance_reasoning(1).unwrap(), "medium");
        assert_eq!(store.cycle_provenance_reasoning(1).unwrap(), "low");
        assert_eq!(store.cycle_provenance_reasoning(1).unwrap(), "none");
        assert_eq!(store.cycle_provenance_reasoning(1).unwrap(), "max");
        assert_eq!(store.cycle_provenance_reasoning(1).unwrap(), "xhigh");
        assert_eq!(store.cycle_provenance_reasoning(1).unwrap(), "high");

        assert_eq!(store.cycle_provenance_harness(1).unwrap(), "cursor-agent");

        // Persist to disk (read the same config path — avoid racing CHAOS_HOME
        // with other tests that also set the env var).
        let saved = std::fs::read_to_string(&store.config_path).unwrap();
        let cfg: UserConfig = toml::from_str(&saved).unwrap();
        assert_eq!(cfg.provenance_model.as_deref(), Some("claude-opus-4.8"));
        assert_eq!(cfg.provenance_reasoning, "high");
        assert_eq!(cfg.provenance_harness, "cursor-agent");

        std::env::remove_var("CHAOS_HOME");
    }
}
