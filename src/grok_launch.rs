//! Launch Grok Build with a chaos match prompt.
//!
//! Grok Build CLI (`grok`) accepts:
//! - interactive: `grok [PROMPT]` / `grok --cwd DIR`
//! - headless run: `grok --prompt-file PATH` (also `-p` / `--single`)
//!
//! Large batch prompts are always written to a file under CHAOS_HOME so we
//! never hit ARG_MAX. Default mode is **run** (headless with the prompt file).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};

use crate::templates::chaos_home;

/// How to start Grok Build.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GrokLaunchMode {
    /// Headless: execute the prompt and exit (`--prompt-file`).
    #[default]
    Run,
    /// Interactive TUI; bootstrap asks Grok to follow the prompt file.
    Interactive,
}

impl GrokLaunchMode {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "run" | "headless" | "p" | "single" => Some(Self::Run),
            "interactive" | "tui" | "i" => Some(Self::Interactive),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Run => "run",
            Self::Interactive => "interactive",
        }
    }
}

/// Resolve `grok` binary: config override, then PATH, then `~/.grok/bin/grok`.
pub fn find_grok_bin(override_path: Option<&str>) -> Result<PathBuf> {
    if let Some(p) = override_path.map(str::trim).filter(|s| !s.is_empty()) {
        let path = PathBuf::from(p);
        if path.is_file() {
            return Ok(path);
        }
        // Still try as command name on PATH
        if which_cmd(p).is_some() {
            return Ok(PathBuf::from(p));
        }
        bail!("grok binary not found at {p}");
    }
    if let Some(p) = which_cmd("grok") {
        return Ok(p);
    }
    let home_bin = dirs_home()
        .map(|h| h.join(".grok").join("bin").join("grok"))
        .filter(|p| p.is_file());
    if let Some(p) = home_bin {
        return Ok(p);
    }
    bail!(
        "grok not found on PATH or ~/.grok/bin/grok — install Grok Build \
(https://docs.x.ai/build/overview) or set grok_bin in ~/.config/chaos/config.toml"
    )
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

fn which_cmd(name: &str) -> Option<PathBuf> {
    let Ok(path) = std::env::var("PATH") else {
        return None;
    };
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// Write the match prompt under CHAOS_HOME for handoff / re-use.
pub fn write_prompt_file(prompt: &str) -> Result<PathBuf> {
    let home = chaos_home();
    fs::create_dir_all(&home).with_context(|| format!("mkdir {}", home.display()))?;
    let path = home.join("last-grok-prompt.md");
    fs::write(&path, prompt).with_context(|| format!("write {}", path.display()))?;
    Ok(path)
}

/// Best-effort local project directory for `grok --cwd` (local atlas path only).
pub fn cwd_from_load_input(
    load_input: Option<&str>,
    source_path: Option<&Path>,
) -> Option<PathBuf> {
    if let Some(p) = source_path {
        if p.is_dir() {
            return Some(p.to_path_buf());
        }
        if p.is_file() {
            return p.parent().map(|x| x.to_path_buf());
        }
    }
    let raw = load_input?.trim();
    if raw.is_empty() {
        return None;
    }
    // GitHub / URLs are not local cwd.
    if raw.contains("://") || raw.starts_with("git@") {
        return None;
    }
    let p = PathBuf::from(raw);
    if p.is_dir() {
        return Some(p);
    }
    if p.is_file() {
        return p.parent().map(|x| x.to_path_buf());
    }
    None
}

/// Spec for suspending the TUI and running Grok.
#[derive(Debug, Clone)]
pub struct GrokLaunch {
    pub bin: PathBuf,
    pub mode: GrokLaunchMode,
    pub prompt_path: PathBuf,
    pub cwd: Option<PathBuf>,
    /// Extra args from config (split by whitespace, simple).
    pub extra_args: Vec<String>,
}

impl GrokLaunch {
    pub fn prepare(
        prompt: &str,
        mode: GrokLaunchMode,
        bin_override: Option<&str>,
        cwd: Option<PathBuf>,
        extra_args: &[String],
    ) -> Result<Self> {
        if prompt.trim().is_empty() {
            bail!("prompt is empty");
        }
        let bin = find_grok_bin(bin_override)?;
        let prompt_path = write_prompt_file(prompt)?;
        Ok(Self {
            bin,
            mode,
            prompt_path,
            cwd,
            extra_args: extra_args.to_vec(),
        })
    }

    /// Run Grok in the foreground (caller must have left the alternate screen).
    pub fn run_foreground(&self) -> Result<()> {
        let mut cmd = Command::new(&self.bin);
        if let Some(cwd) = &self.cwd {
            cmd.current_dir(cwd);
            cmd.arg("--cwd");
            cmd.arg(cwd);
        }
        for a in &self.extra_args {
            cmd.arg(a);
        }
        match self.mode {
            GrokLaunchMode::Run => {
                // Headless: execute the full match prompt and exit when done.
                cmd.arg("--prompt-file");
                cmd.arg(&self.prompt_path);
                cmd.arg("--verbatim");
            }
            GrokLaunchMode::Interactive => {
                // Interactive TUI: bootstrap points at the saved prompt file
                // (avoids ARG_MAX for large multi-function prompts).
                let boot = format!(
                    "Read the full match task at {} and execute it completely \
(verify commands, bank/log if the project uses experimental tools). \
Do not ask me to paste the prompt — the file is the task.",
                    self.prompt_path.display()
                );
                cmd.arg("--verbatim");
                cmd.arg(boot);
            }
        }
        let status = cmd
            .status()
            .with_context(|| format!("spawn {}", self.bin.display()))?;
        if !status.success() {
            bail!(
                "grok exited with {}",
                status
                    .code()
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "signal".into())
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_parse() {
        assert_eq!(GrokLaunchMode::parse("run"), Some(GrokLaunchMode::Run));
        assert_eq!(
            GrokLaunchMode::parse("interactive"),
            Some(GrokLaunchMode::Interactive)
        );
        assert!(GrokLaunchMode::parse("nope").is_none());
    }

    #[test]
    fn write_prompt_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("CHAOS_HOME", dir.path());
        let p = write_prompt_file("hello match").unwrap();
        assert_eq!(fs::read_to_string(&p).unwrap(), "hello match");
        std::env::remove_var("CHAOS_HOME");
    }
}
