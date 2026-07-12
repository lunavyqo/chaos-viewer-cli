//! Saved decomp projects (multi-repo profiles).
//!
//! ```text
//! ~/.config/chaos/
//!   config.toml      # active_project = "sm64ds"
//!   projects.toml    # list of profiles
//! ```

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::templates::{chaos_home, load_user_config, save_user_config};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectProfile {
    /// Stable id (filename-safe).
    pub id: String,
    /// Display name.
    pub name: String,
    /// Same as TUI setup input: local path, raw JSON URL, or GitHub repo URL.
    pub source: String,
    /// Optional branch for GitHub discovery.
    #[serde(default)]
    pub branch: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct ProjectsFile {
    #[serde(default)]
    projects: Vec<ProjectProfile>,
}

#[derive(Debug, Clone)]
pub struct ProjectStore {
    pub path: PathBuf,
    pub projects: Vec<ProjectProfile>,
    pub config_path: PathBuf,
    pub active_id: Option<String>,
}

impl ProjectStore {
    pub fn load() -> Self {
        Self::load_from_home(&chaos_home())
    }

    pub fn load_from_home(home: &Path) -> Self {
        let path = home.join("projects.toml");
        let config_path = home.join("config.toml");
        let file = load_projects_file(&path);
        let config = load_user_config(&config_path);
        let active_id = config
            .active_project
            .filter(|id| file.projects.iter().any(|p| &p.id == id));
        Self {
            path,
            projects: file.projects,
            config_path,
            active_id,
        }
    }

    pub fn reload(&mut self) {
        *self = Self::load();
    }

    pub fn get(&self, id: &str) -> Option<&ProjectProfile> {
        self.projects.iter().find(|p| p.id == id)
    }

    pub fn index_of(&self, id: &str) -> Option<usize> {
        self.projects.iter().position(|p| p.id == id)
    }

    pub fn save(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let file = ProjectsFile {
            projects: self.projects.clone(),
        };
        let text = toml::to_string_pretty(&file).context("serialize projects")?;
        fs::write(&self.path, text).with_context(|| format!("write {}", self.path.display()))?;
        Ok(())
    }

    pub fn set_active(&mut self, id: Option<&str>) -> Result<()> {
        if let Some(id) = id {
            if self.get(id).is_none() {
                bail!("unknown project id '{id}'");
            }
        }
        self.active_id = id.map(str::to_string);
        let mut cfg = load_user_config(&self.config_path);
        cfg.active_project = self.active_id.clone();
        save_user_config(&self.config_path, &cfg)?;
        Ok(())
    }

    /// Add or replace a profile by id.
    pub fn upsert(&mut self, profile: ProjectProfile) -> Result<()> {
        sanitize_project_id(&profile.id)?;
        if let Some(slot) = self.projects.iter_mut().find(|p| p.id == profile.id) {
            *slot = profile;
        } else {
            self.projects.push(profile);
        }
        self.projects.sort_by(|a, b| a.id.cmp(&b.id));
        self.save()
    }

    pub fn remove(&mut self, id: &str) -> Result<bool> {
        let before = self.projects.len();
        self.projects.retain(|p| p.id != id);
        if self.projects.len() == before {
            return Ok(false);
        }
        if self.active_id.as_deref() == Some(id) {
            self.active_id = None;
            let mut cfg = load_user_config(&self.config_path);
            cfg.active_project = None;
            save_user_config(&self.config_path, &cfg)?;
        }
        self.save()?;
        Ok(true)
    }

    /// Suggest an id from a source string (repo name or file stem).
    ///
    /// Prefer the GitHub **repo name**, not a mangled full URL or `chaos-db`.
    pub fn suggest_id(source: &str) -> String {
        let s = source.trim().trim_end_matches('/');
        let base = if let Some((_, name)) = crate::discover::parse_github(s) {
            name
        } else if let Some(rest) = s
            .strip_prefix("https://raw.githubusercontent.com/")
            .or_else(|| s.strip_prefix("http://raw.githubusercontent.com/"))
        {
            // raw.githubusercontent.com/{owner}/{repo}/...
            rest.split('/').nth(1).unwrap_or("project").to_string()
        } else if s.ends_with(".json") {
            // Prefer parent folder over "chaos-db"
            let path = Path::new(s);
            let stem = path
                .file_stem()
                .and_then(|x| x.to_str())
                .unwrap_or("project");
            if stem.eq_ignore_ascii_case("chaos-db") || stem.eq_ignore_ascii_case("chaos_db") {
                path.parent()
                    .and_then(|p| p.file_name())
                    .and_then(|x| x.to_str())
                    .filter(|n| !n.is_empty() && *n != "data" && *n != "chaos-data")
                    .unwrap_or(stem)
                    .to_string()
            } else {
                stem.to_string()
            }
        } else {
            Path::new(s)
                .file_name()
                .and_then(|x| x.to_str())
                .unwrap_or("project")
                .to_string()
        };
        clean_id_slug(&base)
    }
}

fn clean_id_slug(base: &str) -> String {
    let mut id: String = base
        .trim_end_matches(".git")
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    while id.contains("--") {
        id = id.replace("--", "-");
    }
    id = id.trim_matches('-').to_string();
    if id.is_empty() {
        id = "project".into();
    }
    id.truncate(48);
    id
}

pub fn sanitize_project_id(raw: &str) -> Result<String> {
    let s = raw.trim();
    if s.is_empty() {
        bail!("project id is empty");
    }
    if s.len() > 64 {
        bail!("project id too long (max 64)");
    }
    let mut chars = s.chars();
    let first = chars.next().unwrap();
    if !first.is_ascii_alphanumeric() {
        bail!("project id must start with a letter or digit");
    }
    if !chars.all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        bail!("project id may only contain letters, digits, '-' and '_'");
    }
    Ok(s.to_string())
}

fn load_projects_file(path: &Path) -> ProjectsFile {
    match fs::read_to_string(path) {
        Ok(text) => toml::from_str(&text).unwrap_or_default(),
        Err(_) => ProjectsFile::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suggest_id_from_github() {
        assert_eq!(
            ProjectStore::suggest_id("https://github.com/you/sm64ds-decomp"),
            "sm64ds-decomp"
        );
        assert_eq!(
            ProjectStore::suggest_id(
                "https://raw.githubusercontent.com/you/electroplankton-decomp/chaos-data/chaos-db.json"
            ),
            "electroplankton-decomp"
        );
    }

    #[test]
    fn upsert_and_active() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = ProjectStore::load_from_home(dir.path());
        store
            .upsert(ProjectProfile {
                id: "demo".into(),
                name: "Demo".into(),
                source: "https://github.com/you/demo".into(),
                branch: None,
            })
            .unwrap();
        store.set_active(Some("demo")).unwrap();
        let store2 = ProjectStore::load_from_home(dir.path());
        assert_eq!(store2.active_id.as_deref(), Some("demo"));
        assert_eq!(store2.projects.len(), 1);
    }
}
