//! Prompt templates: built-in chaos-viewer prompt plus user TOML templates.
//!
//! Layout on disk (override with `CHAOS_HOME`):
//! ```text
//! ~/.config/chaos/
//!   config.toml                 # default_template = "chaos-viewer"
//!   templates/
//!     short.toml                # user-defined
//! ```
//!
//! Builtin id `chaos-viewer` is always available (compiled-in parity with the web app).

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::claims::ClaimsSession;
use crate::prompt::{build_builtin_prompt, PromptOptions};
use crate::schema::{ChaosFunction, FunctionDetail, ProjectConfig};

pub const BUILTIN_ID: &str = "chaos-viewer";
pub const BUILTIN_NAME: &str = "Chaos Viewer (default)";

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UserConfig {
    /// Template id used when none is selected explicitly.
    #[serde(default = "default_template_id")]
    pub default_template: String,
}

fn default_template_id() -> String {
    BUILTIN_ID.into()
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

        let config = load_user_config(&config_path);
        // Seed example template once if the directory is empty (besides nothing).
        seed_example_template(&templates_dir);

        let mut entries = vec![TemplateEntry {
            id: BUILTIN_ID.into(),
            name: BUILTIN_NAME.into(),
            description: "Built-in prompt matching tangosdev/chaos-viewer".into(),
            kind: TemplateKind::Builtin,
            path: None,
        }];

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
        if id == BUILTIN_ID {
            bail!(
                "'{BUILTIN_ID}' is built-in and has no file; press n (or `chaos templates new`) to make an editable copy"
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
        if id == BUILTIN_ID {
            bail!("'{BUILTIN_ID}' is reserved for the built-in template");
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

fn load_user_config(path: &Path) -> UserConfig {
    match fs::read_to_string(path) {
        Ok(text) => toml::from_str(&text).unwrap_or_default(),
        Err(_) => UserConfig::default(),
    }
}

fn save_user_config(path: &Path, cfg: &UserConfig) -> Result<()> {
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
        parts.push(expand_function(&t.function, project, fn_, det.as_ref()));
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
) -> String {
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

    let (draft, draft_div, section_draft) = match det.and_then(|d| d.draft.as_ref()) {
        Some(draft) => {
            let div = det
                .and_then(|d| d.draft_div)
                .map(|d| d.to_string())
                .unwrap_or_else(|| "undefined".into());
            let block = format!(
                "A NEAR-MISS DRAFT EXISTS ({div} instruction(s) from matching) - START FROM THIS, do not re-decompile:\n```c\n{}\n```",
                draft.trim_end()
            );
            (draft.trim_end().to_string(), div, block)
        }
        None => (String::new(), String::new(), String::new()),
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
    let (Some(api), Some(session)) = (project.claims_api.as_deref(), session) else {
        return String::new();
    };
    let handle = if session.handle.is_empty() {
        "chaos-viewer-user"
    } else {
        session.handle.as_str()
    };
    let each = if n > 1 {
        "EACH function"
    } else {
        "the function"
    };
    format!(
        "CLAIMS (coordination lock; do this BEFORE writing code): my claims api key is {} - send it as the X-Api-Key header on every claims call.\n\
For {each} above: POST {api}/try-lock with JSON {{\"module\": \"<module>\", \"start\": \"0x<addr>\", \"end\": \"0x<addr+size>\", \"handle\": \"{handle}\"}}.\n\
Save the returned claim.id; renew while working (POST {api}/{{id}}/renew with {{\"handle\": \"{handle}\"}}) and release when done (POST {api}/{{id}}/release, same body).\n\
If try-lock returns a conflict, someone else has it - skip that function. If calls return 401 the short-lived key expired - continue without locking and tell me to re-sign-in. Full contract: GET {api}/instructions.",
        session.token
    )
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
        assert_eq!(store.default_id(), BUILTIN_ID);
        // example short.toml seeded
        assert!(dir.path().join("templates/short.toml").exists());
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
}
