//! Launch coding-agent CLIs with a chaos match prompt in a **separate terminal**.
//!
//! Supported agents: **Grok Build**, **Codex**, **Claude Code**, **Antigravity** (`agy`).
//! Chaos stays open. Each agent gets an explicit local repo path + prompt preamble.
//!
//! Large prompts are written under CHAOS_HOME (`last-agent-prompt.md`).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{bail, Context, Result};

use crate::templates::chaos_home;

/// Which coding-agent CLI to open.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AgentKind {
    #[default]
    Grok,
    Codex,
    Claude,
    Antigravity,
}

impl AgentKind {
    pub const ALL: [AgentKind; 4] = [
        AgentKind::Grok,
        AgentKind::Codex,
        AgentKind::Claude,
        AgentKind::Antigravity,
    ];

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "grok" | "grok-build" | "xai" => Some(Self::Grok),
            "codex" | "openai-codex" | "oai" => Some(Self::Codex),
            "claude" | "claude-code" | "anthropic" => Some(Self::Claude),
            "antigravity" | "agy" | "anti-gravity" => Some(Self::Antigravity),
            _ => None,
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Self::Grok => "grok",
            Self::Codex => "codex",
            Self::Claude => "claude",
            Self::Antigravity => "antigravity",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Grok => "Grok Build",
            Self::Codex => "Codex",
            Self::Claude => "Claude Code",
            Self::Antigravity => "Antigravity",
        }
    }

    pub fn short_bin(self) -> &'static str {
        match self {
            Self::Grok => "grok",
            Self::Codex => "codex",
            Self::Claude => "claude",
            Self::Antigravity => "agy",
        }
    }

    /// PATH / well-known names to try, in order.
    pub fn bin_candidates(self) -> &'static [&'static str] {
        match self {
            Self::Grok => &["grok", "agent"],
            Self::Codex => &["codex"],
            Self::Claude => &["claude"],
            Self::Antigravity => &["agy", "antigravity"],
        }
    }

    pub fn index(self) -> usize {
        Self::ALL.iter().position(|&a| a == self).unwrap_or(0)
    }
}

/// How Grok itself runs inside the new terminal (other agents are always TUI).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GrokLaunchMode {
    /// Interactive Grok Build TUI; short bootstrap points at the prompt file.
    #[default]
    Interactive,
    /// Headless single-turn: `grok --prompt-file` prints to stdout and exits.
    Run,
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

/// Which outer terminal host to open (macOS/Linux/Windows).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TerminalHost {
    #[default]
    Auto,
    /// macOS Terminal.app (new window with the command).
    MacTerminal,
    /// macOS iTerm2.
    ITerm,
    /// `$TERMINAL -e` / common Linux terminals.
    Linux,
    /// Windows `cmd /c start`.
    Windows,
}

impl TerminalHost {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "auto" | "" => Some(Self::Auto),
            "terminal" | "macos" | "apple" => Some(Self::MacTerminal),
            "iterm" | "iterm2" => Some(Self::ITerm),
            "linux" | "xdg" => Some(Self::Linux),
            "windows" | "cmd" | "wt" => Some(Self::Windows),
            _ => None,
        }
    }
}

/// Resolve agent binary: config override, then PATH, then well-known install paths.
pub fn find_agent_bin(agent: AgentKind, override_path: Option<&str>) -> Result<PathBuf> {
    if let Some(p) = override_path.map(str::trim).filter(|s| !s.is_empty()) {
        let path = PathBuf::from(p);
        if path.is_file() {
            return Ok(path);
        }
        if which_cmd(p).is_some() {
            return Ok(PathBuf::from(p));
        }
        bail!("{} binary not found at {p}", agent.label());
    }
    for name in agent.bin_candidates() {
        if let Some(p) = which_cmd(name) {
            return Ok(p);
        }
    }
    // Well-known install locations.
    if let Some(home) = dirs_home() {
        let candidates: Vec<PathBuf> = match agent {
            AgentKind::Grok => vec![
                home.join(".grok/bin/grok"),
                home.join(".local/bin/grok"),
                home.join(".local/bin/agent"),
            ],
            AgentKind::Codex => vec![
                home.join(".local/bin/codex"),
                home.join(".npm-global/bin/codex"),
            ],
            AgentKind::Claude => {
                vec![
                    home.join(".local/bin/claude"),
                    home.join(".npm-global/bin/claude"),
                ]
            }
            AgentKind::Antigravity => {
                vec![
                    home.join(".local/bin/agy"),
                    home.join(".local/bin/antigravity"),
                ]
            }
        };
        if let Some(p) = candidates.into_iter().find(|p| p.is_file()) {
            return Ok(p);
        }
    }
    bail!(
        "{} (`{}`) not found on PATH — install it or set `{}_bin` in ~/.config/chaos/config.toml",
        agent.label(),
        agent.short_bin(),
        agent.id()
    )
}

/// Backward-compatible alias.
pub fn find_grok_bin(override_path: Option<&str>) -> Result<PathBuf> {
    find_agent_bin(AgentKind::Grok, override_path)
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
///
/// When `tag` is `Some("batch2")`, writes `last-agent-prompt-batch2.md` (and still
/// refreshes the untagged `last-agent-prompt.md` for muscle memory). Mass-batcher
/// launches use tags so concurrent agent windows do not clobber each other.
pub fn write_prompt_file(prompt: &str) -> Result<PathBuf> {
    write_prompt_file_tagged(prompt, None)
}

/// Like [`write_prompt_file`], with an optional filename tag for multi-batch launches.
pub fn write_prompt_file_tagged(prompt: &str, tag: Option<&str>) -> Result<PathBuf> {
    let home = chaos_home();
    fs::create_dir_all(&home).with_context(|| format!("mkdir {}", home.display()))?;
    let path = match tag.map(str::trim).filter(|s| !s.is_empty()) {
        Some(t) => home.join(format!("last-agent-prompt-{t}.md")),
        None => home.join("last-agent-prompt.md"),
    };
    fs::write(&path, prompt).with_context(|| format!("write {}", path.display()))?;
    // Always refresh the untagged handoff + legacy name (last launch wins).
    let _ = fs::write(home.join("last-agent-prompt.md"), prompt);
    let _ = fs::write(home.join("last-grok-prompt.md"), prompt);
    Ok(path)
}

/// Short bootstrap so interactive TUIs load the full prompt from disk.
pub fn bootstrap_message(prompt_path: &Path) -> String {
    format!(
        "Read the full match task at {} and execute it completely \
(verify, bank, log attempts). Work only inside the LOCAL DECOMP REPOSITORY \
path stated at the top of that file. Do not ask me to paste the prompt.",
        prompt_path.display()
    )
}

/// Prepend explicit on-disk repo location so the agent never has to guess.
pub fn with_repo_preamble(prompt: &str, repo: Option<&Path>) -> String {
    match repo {
        Some(p) => {
            let abs = p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
            format!(
                "LOCAL DECOMP REPOSITORY (on this machine — work only inside this tree):\n\
                 {abs}\n\
                 Use this as the working directory for all tools (match, bank, …) \
                 and relative paths (src/, tools/, config/).\n\
                 Do not invent another clone path.\n\
                 \n\
                 ======================================================================\n\
                 \n\
                 {prompt}",
                abs = abs.display(),
                prompt = prompt
            )
        }
        None => format!(
            "WARNING: No local decomp path is configured for this project.\n\
             Set `local_repo` in projects.toml (or grok_default_repo in config.toml) \
             so tools run in the right tree. Paths like src/ and tools/ may not resolve.\n\
             \n\
             ======================================================================\n\
             \n\
             {prompt}"
        ),
    }
}

/// Resolve local decomp directory for Grok.
///
/// Order: explicit profile `local_repo` → config `grok_default_repo` →
/// heuristic from atlas load path.
pub fn resolve_repo_cwd(
    profile_local_repo: Option<&str>,
    config_default_repo: Option<&str>,
    load_input: Option<&str>,
    source_path: Option<&Path>,
) -> Option<PathBuf> {
    for cand in [profile_local_repo, config_default_repo] {
        if let Some(raw) = cand.map(str::trim).filter(|s| !s.is_empty()) {
            let p = PathBuf::from(raw);
            let expanded = expand_tilde(&p);
            if expanded.is_dir() {
                return Some(expanded);
            }
        }
    }
    cwd_from_load_input(load_input, source_path)
}

fn expand_tilde(p: &Path) -> PathBuf {
    let s = p.to_string_lossy();
    if let Some(rest) = s.strip_prefix("~/") {
        if let Some(home) = dirs_home() {
            return home.join(rest);
        }
    }
    if s == "~" {
        if let Some(home) = dirs_home() {
            return home;
        }
    }
    p.to_path_buf()
}

/// Best-effort local project directory from atlas load path only.
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
    if raw.contains("://") || raw.starts_with("git@") {
        return None;
    }
    let p = expand_tilde(Path::new(raw));
    if p.is_dir() {
        return Some(p);
    }
    if p.is_file() {
        return p.parent().map(|x| x.to_path_buf());
    }
    None
}

fn shell_single_quote(s: &str) -> String {
    // Safe for POSIX single-quoted strings: 'foo'\''bar'
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Escape a string for an AppleScript double-quoted literal.
fn applescript_string(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Build the shell line executed inside the new terminal tab/window.
pub fn agent_shell_command(
    agent: AgentKind,
    bin: &Path,
    prompt_path: &Path,
    cwd: Option<&Path>,
    extra_args: &[String],
    // Only applies to AgentKind::Grok.
    grok_mode: GrokLaunchMode,
) -> String {
    let mut work: Vec<String> = Vec::new();
    if let Some(c) = cwd {
        work.push(format!(
            "cd {}",
            shell_single_quote(&c.display().to_string())
        ));
    }
    let boot = bootstrap_message(prompt_path);
    let bin_q = shell_single_quote(&bin.display().to_string());
    let mut cmd = bin_q;

    match agent {
        AgentKind::Grok => {
            if let Some(c) = cwd {
                cmd.push_str(" --cwd ");
                cmd.push_str(&shell_single_quote(&c.display().to_string()));
            }
            for a in extra_args {
                cmd.push(' ');
                cmd.push_str(&shell_single_quote(a));
            }
            match grok_mode {
                GrokLaunchMode::Run => {
                    cmd.push_str(" --prompt-file ");
                    cmd.push_str(&shell_single_quote(&prompt_path.display().to_string()));
                    cmd.push_str(" --verbatim");
                    work.push(cmd);
                    return format!(
                        "{work}; status=$?; echo; echo '[chaos] agent exited '$status' — press return to close'; read _",
                        work = work.join(" && ")
                    );
                }
                GrokLaunchMode::Interactive => {
                    cmd.push_str(" --fullscreen --verbatim ");
                    cmd.push_str(&shell_single_quote(&boot));
                }
            }
        }
        AgentKind::Codex => {
            // codex -C <dir> [PROMPT] → interactive TUI
            if let Some(c) = cwd {
                cmd.push_str(" -C ");
                cmd.push_str(&shell_single_quote(&c.display().to_string()));
            }
            for a in extra_args {
                cmd.push(' ');
                cmd.push_str(&shell_single_quote(a));
            }
            cmd.push(' ');
            cmd.push_str(&shell_single_quote(&boot));
        }
        AgentKind::Claude => {
            // claude [prompt] — interactive session; cwd via shell `cd`
            for a in extra_args {
                cmd.push(' ');
                cmd.push_str(&shell_single_quote(a));
            }
            if let Some(c) = cwd {
                cmd.push_str(" --add-dir ");
                cmd.push_str(&shell_single_quote(&c.display().to_string()));
            }
            cmd.push(' ');
            cmd.push_str(&shell_single_quote(&boot));
        }
        AgentKind::Antigravity => {
            // agy -i / --prompt-interactive starts interactive with initial prompt
            for a in extra_args {
                cmd.push(' ');
                cmd.push_str(&shell_single_quote(a));
            }
            if let Some(c) = cwd {
                cmd.push_str(" --add-dir ");
                cmd.push_str(&shell_single_quote(&c.display().to_string()));
            }
            cmd.push_str(" --prompt-interactive ");
            cmd.push_str(&shell_single_quote(&boot));
        }
    }

    // `exec` replaces the shell so the window *is* the agent TUI.
    work.push(format!("exec {cmd}"));
    work.join(" && ")
}

/// Backward-compatible wrapper (Grok only).
pub fn grok_shell_command(
    bin: &Path,
    mode: GrokLaunchMode,
    prompt_path: &Path,
    cwd: Option<&Path>,
    extra_args: &[String],
) -> String {
    agent_shell_command(AgentKind::Grok, bin, prompt_path, cwd, extra_args, mode)
}

/// Open a new terminal and run `shell_cmd` there. Chaos does not wait.
pub fn spawn_in_new_terminal(host: TerminalHost, shell_cmd: &str) -> Result<String> {
    spawn_in_new_terminal_tagged(host, shell_cmd, None)
}

pub fn spawn_in_new_terminal_tagged(
    host: TerminalHost,
    shell_cmd: &str,
    handoff_tag: Option<&str>,
) -> Result<String> {
    let host = match host {
        TerminalHost::Auto => detect_host(),
        other => other,
    };
    match host {
        TerminalHost::MacTerminal => spawn_macos_terminal(shell_cmd, handoff_tag),
        TerminalHost::ITerm => spawn_iterm(shell_cmd, handoff_tag),
        TerminalHost::Linux => spawn_linux_terminal(shell_cmd),
        TerminalHost::Windows => spawn_windows_cmd(shell_cmd),
        TerminalHost::Auto => unreachable!(),
    }
}

fn detect_host() -> TerminalHost {
    if cfg!(target_os = "macos") {
        // Prefer iTerm if running / installed.
        if Path::new("/Applications/iTerm.app").exists()
            || dirs_home()
                .map(|h| h.join("Applications/iTerm.app").exists())
                .unwrap_or(false)
        {
            // Still default to Terminal.app for wider availability unless iTerm is preferred via config.
            TerminalHost::MacTerminal
        } else {
            TerminalHost::MacTerminal
        }
    } else if cfg!(target_os = "windows") {
        TerminalHost::Windows
    } else {
        TerminalHost::Linux
    }
}

fn run_osascript(script: &str, label: &str) -> Result<()> {
    let output = Command::new("osascript")
        .arg("-e")
        .arg(script)
        .stdin(Stdio::null())
        .output()
        .with_context(|| format!("spawn osascript ({label})"))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let detail = if !stderr.is_empty() {
        stderr
    } else if !stdout.is_empty() {
        stdout
    } else {
        format!("exit {}", output.status)
    };
    bail!("osascript ({label}) failed: {detail}")
}

/// Write an executable launcher script under CHAOS_HOME (avoids AppleScript
/// length/quoting limits; `.command` is opened by Terminal on macOS).
pub fn write_run_script(shell_cmd: &str) -> Result<PathBuf> {
    write_run_script_tagged(shell_cmd, None)
}

/// Like [`write_run_script`], with an optional filename tag for multi-batch launches.
pub fn write_run_script_tagged(shell_cmd: &str, tag: Option<&str>) -> Result<PathBuf> {
    let home = chaos_home();
    fs::create_dir_all(&home).with_context(|| format!("mkdir {}", home.display()))?;
    // `.command` → macOS Launch Services runs it in Terminal when `open`ed.
    let path = match tag.map(str::trim).filter(|s| !s.is_empty()) {
        Some(t) => home.join(format!("last-agent-run-{t}.command")),
        None => home.join("last-agent-run.command"),
    };
    let body = format!(
        "#!/bin/zsh\n\
         # Generated by chaos-viewer-cli — agent launch handoff\n\
         {shell_cmd}\n"
    );
    fs::write(&path, &body).with_context(|| format!("write {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&path)
            .with_context(|| format!("stat {}", path.display()))?
            .permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&path, perms).with_context(|| format!("chmod {}", path.display()))?;
    }
    // Keep untagged path as a convenience pointer to the last script.
    if tag.is_some() {
        let untagged = home.join("last-agent-run.command");
        let _ = fs::write(&untagged, &body);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(meta) = fs::metadata(&untagged) {
                let mut perms = meta.permissions();
                perms.set_mode(0o755);
                let _ = fs::set_permissions(&untagged, perms);
            }
        }
    }
    Ok(path)
}

fn command_output_err(label: &str, output: &std::process::Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if !stderr.is_empty() {
        format!("{label}: {stderr}")
    } else if !stdout.is_empty() {
        format!("{label}: {stdout}")
    } else {
        format!("{label}: exit {}", output.status)
    }
}

fn spawn_macos_terminal(shell_cmd: &str, handoff_tag: Option<&str>) -> Result<String> {
    // Reliable path: write a .command script and `open` it. This uses Launch
    // Services (not AppleScript automation), so it works even when the parent
    // app lacks Automation permission, and usually opens a *new* Terminal window
    // instead of a hidden tab behind the chaos alternate screen.
    let script_path = write_run_script_tagged(shell_cmd, handoff_tag)?;
    let script_disp = script_path.display().to_string();

    let mut errors: Vec<String> = Vec::new();

    // 1) `open path.command` — preferred (Launch Services)
    match Command::new("open")
        .arg(&script_path)
        .stdin(Stdio::null())
        .output()
    {
        Ok(out) if out.status.success() => {
            // Bring Terminal forward (chaos often runs full-screen / alternate buffer).
            let _ = Command::new("osascript")
                .args(["-e", "tell application \"Terminal\" to activate"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
            return Ok(format!("Terminal (`open` {script_disp})"));
        }
        Ok(out) => errors.push(command_output_err("open", &out)),
        Err(e) => errors.push(format!("open: {e}")),
    }

    // 2) `open -a Terminal path.command` (+ activate)
    match Command::new("open")
        .args(["-a", "Terminal"])
        .arg(&script_path)
        .stdin(Stdio::null())
        .output()
    {
        Ok(out) if out.status.success() => {
            let _ = Command::new("osascript")
                .args(["-e", "tell application \"Terminal\" to activate"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
            return Ok(format!("Terminal.app (`open -a` {script_disp})"));
        }
        Ok(out) => errors.push(command_output_err("open -a Terminal", &out)),
        Err(e) => errors.push(format!("open -a Terminal: {e}")),
    }

    // 3) AppleScript — always a *new window* (not a tab in the chaos window)
    let run_line = format!("exec zsh {}", shell_single_quote(&script_disp));
    let cmd = applescript_string(&run_line);
    let ascript = format!(
        "tell application \"Terminal\"\n\
         activate\n\
         do script {cmd}\n\
         end tell"
    );
    match run_osascript(&ascript, "Terminal") {
        Ok(()) => return Ok(format!("Terminal.app (new window, {script_disp})")),
        Err(e) => errors.push(e.to_string()),
    }

    bail!(
        "could not open Terminal to run the agent. Tried open / open -a Terminal / AppleScript.\n\
         Script left at: {script_disp}\n\
         You can run it manually: open {script_disp}\n\
         Errors: {}",
        errors.join(" | ")
    )
}

fn spawn_iterm(shell_cmd: &str, handoff_tag: Option<&str>) -> Result<String> {
    // Prefer writing the same launcher script, then tell iTerm to run it.
    let script_path = write_run_script_tagged(shell_cmd, handoff_tag)?;
    let script_disp = script_path.display().to_string();
    let run_line = format!("exec zsh {}", shell_single_quote(&script_disp));
    let cmd = applescript_string(&run_line);
    let script = format!(
        "tell application \"iTerm\"\n\
         activate\n\
         try\n\
           create window with default profile\n\
           tell current session of current window\n\
             write text {cmd}\n\
           end tell\n\
         on error\n\
           tell current window\n\
             create tab with default profile\n\
             tell current session\n\
               write text {cmd}\n\
             end tell\n\
           end tell\n\
         end try\n\
         end tell"
    );
    run_osascript(&script, "iTerm")?;
    Ok(format!("iTerm2 (new window, {script_disp})"))
}

fn spawn_linux_terminal(shell_cmd: &str) -> Result<String> {
    // Prefer $TERMINAL, then common emulators.
    let candidates: Vec<(String, Vec<String>)> = {
        let mut v = Vec::new();
        if let Ok(t) = std::env::var("TERMINAL") {
            v.push((
                t,
                vec!["-e".into(), "bash".into(), "-lc".into(), shell_cmd.into()],
            ));
        }
        for name in [
            "ghostty",
            "kitty",
            "alacritty",
            "wezterm",
            "gnome-terminal",
            "konsole",
            "xfce4-terminal",
            "xterm",
        ] {
            v.push((
                name.into(),
                match name {
                    "gnome-terminal" | "xfce4-terminal" => {
                        vec!["--".into(), "bash".into(), "-lc".into(), shell_cmd.into()]
                    }
                    "konsole" => vec!["-e".into(), "bash".into(), "-lc".into(), shell_cmd.into()],
                    "wezterm" => {
                        vec![
                            "start".into(),
                            "--".into(),
                            "bash".into(),
                            "-lc".into(),
                            shell_cmd.into(),
                        ]
                    }
                    "alacritty" | "kitty" | "ghostty" | "xterm" => {
                        vec!["-e".into(), "bash".into(), "-lc".into(), shell_cmd.into()]
                    }
                    _ => vec!["-e".into(), "bash".into(), "-lc".into(), shell_cmd.into()],
                },
            ));
        }
        v
    };
    for (bin, args) in candidates {
        if which_cmd(&bin).is_none() && !Path::new(&bin).is_file() {
            continue;
        }
        let mut cmd = Command::new(&bin);
        cmd.args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        match cmd.spawn() {
            Ok(_) => return Ok(format!("{bin} (new window)")),
            Err(_) => continue,
        }
    }
    bail!(
        "no terminal emulator found — set $TERMINAL or grok_terminal in config.toml \
(or install gnome-terminal / kitty / …)"
    )
}

fn spawn_windows_cmd(shell_cmd: &str) -> Result<String> {
    // Run via bash if Git Bash exists; else cmd. Prefer Windows Terminal.
    let bash_cmd = format!("bash -lc {}", shell_single_quote(shell_cmd));
    if which_cmd("wt").is_some() {
        Command::new("wt")
            .args(["new-tab", "cmd", "/k", &bash_cmd])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .context("wt")?;
        return Ok("Windows Terminal (new tab)".into());
    }
    Command::new("cmd")
        .args(["/c", "start", "cmd", "/k", &bash_cmd])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("cmd start")?;
    Ok("cmd (new window)".into())
}

/// Prepare prompt file + open an agent in an external terminal. Does not block chaos.
pub fn launch_agent(
    agent: AgentKind,
    prompt: &str,
    bin_override: Option<&str>,
    repo_cwd: Option<PathBuf>,
    extra_args: &[String],
    terminal: TerminalHost,
    grok_mode: GrokLaunchMode,
) -> Result<LaunchReport> {
    launch_agent_tagged(
        agent,
        prompt,
        bin_override,
        repo_cwd,
        extra_args,
        terminal,
        grok_mode,
        None,
    )
}

/// Like [`launch_agent`], with an optional handoff tag so mass-batcher can open
/// several agent windows without overwriting each other's prompt/run files.
#[allow(clippy::too_many_arguments)]
pub fn launch_agent_tagged(
    agent: AgentKind,
    prompt: &str,
    bin_override: Option<&str>,
    repo_cwd: Option<PathBuf>,
    extra_args: &[String],
    terminal: TerminalHost,
    grok_mode: GrokLaunchMode,
    handoff_tag: Option<&str>,
) -> Result<LaunchReport> {
    if prompt.trim().is_empty() {
        bail!("prompt is empty");
    }
    let bin = find_agent_bin(agent, bin_override)?;
    let body = with_repo_preamble(prompt, repo_cwd.as_deref());
    let prompt_path = write_prompt_file_tagged(&body, handoff_tag)?;
    let shell_cmd = agent_shell_command(
        agent,
        &bin,
        &prompt_path,
        repo_cwd.as_deref(),
        extra_args,
        grok_mode,
    );
    let host_label = spawn_in_new_terminal_tagged(terminal, &shell_cmd, handoff_tag)?;
    Ok(LaunchReport {
        agent,
        grok_mode,
        prompt_path,
        repo_cwd,
        terminal: host_label,
        bin,
    })
}

/// Backward-compatible Grok-only launcher.
pub fn launch_external(
    prompt: &str,
    mode: GrokLaunchMode,
    bin_override: Option<&str>,
    repo_cwd: Option<PathBuf>,
    extra_args: &[String],
    terminal: TerminalHost,
) -> Result<LaunchReport> {
    launch_agent(
        AgentKind::Grok,
        prompt,
        bin_override,
        repo_cwd,
        extra_args,
        terminal,
        mode,
    )
}

#[derive(Debug, Clone)]
pub struct LaunchReport {
    pub agent: AgentKind,
    pub grok_mode: GrokLaunchMode,
    pub prompt_path: PathBuf,
    pub repo_cwd: Option<PathBuf>,
    pub terminal: String,
    pub bin: PathBuf,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Serializes tests that mutate process-wide `CHAOS_HOME`.
    static CHAOS_HOME_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn mode_parse() {
        assert_eq!(GrokLaunchMode::parse("run"), Some(GrokLaunchMode::Run));
        assert_eq!(
            GrokLaunchMode::parse("interactive"),
            Some(GrokLaunchMode::Interactive)
        );
        assert_eq!(GrokLaunchMode::default(), GrokLaunchMode::Interactive);
    }

    #[test]
    fn agent_kind_parse() {
        assert_eq!(AgentKind::parse("grok"), Some(AgentKind::Grok));
        assert_eq!(AgentKind::parse("codex"), Some(AgentKind::Codex));
        assert_eq!(AgentKind::parse("claude-code"), Some(AgentKind::Claude));
        assert_eq!(AgentKind::parse("agy"), Some(AgentKind::Antigravity));
        assert_eq!(AgentKind::default(), AgentKind::Grok);
    }

    #[test]
    fn preamble_includes_repo() {
        let t = with_repo_preamble("BODY", Some(Path::new("/tmp/my-decomp")));
        assert!(t.contains("LOCAL DECOMP REPOSITORY"));
        assert!(t.contains("my-decomp") || t.contains("/tmp"));
        assert!(t.contains("BODY"));
    }

    #[test]
    fn shell_command_run_is_headless_prompt_file() {
        let cmd = grok_shell_command(
            Path::new("/usr/bin/grok"),
            GrokLaunchMode::Run,
            Path::new("/tmp/p.md"),
            Some(Path::new("/repo")),
            &[],
        );
        assert!(cmd.contains("cd '/repo'"));
        assert!(cmd.contains("--prompt-file"));
        assert!(cmd.contains("/tmp/p.md"));
        assert!(cmd.contains("--cwd"));
        assert!(cmd.contains("press return to close"));
        assert!(!cmd.contains("exec "));
    }

    #[test]
    fn shell_command_interactive_is_tui_not_prompt_file() {
        let cmd = grok_shell_command(
            Path::new("/usr/bin/grok"),
            GrokLaunchMode::Interactive,
            Path::new("/tmp/p.md"),
            Some(Path::new("/repo")),
            &[],
        );
        assert!(cmd.contains("cd '/repo'"));
        assert!(cmd.contains("--cwd"));
        assert!(cmd.contains("--fullscreen"));
        assert!(cmd.contains("exec "));
        assert!(cmd.contains("/tmp/p.md"));
        // Must NOT use headless single-turn flags.
        assert!(!cmd.contains("--prompt-file"));
        assert!(!cmd.contains("press return to close"));
    }

    #[test]
    fn shell_command_codex_claude_agy() {
        let p = Path::new("/tmp/p.md");
        let cwd = Path::new("/repo");
        let codex = agent_shell_command(
            AgentKind::Codex,
            Path::new("/usr/bin/codex"),
            p,
            Some(cwd),
            &[],
            GrokLaunchMode::Interactive,
        );
        assert!(codex.contains("exec "));
        assert!(codex.contains("-C '/repo'"));
        assert!(!codex.contains("--prompt-file"));

        let claude = agent_shell_command(
            AgentKind::Claude,
            Path::new("/usr/bin/claude"),
            p,
            Some(cwd),
            &[],
            GrokLaunchMode::Interactive,
        );
        assert!(claude.contains("exec "));
        assert!(claude.contains("--add-dir '/repo'"));

        let agy = agent_shell_command(
            AgentKind::Antigravity,
            Path::new("/usr/bin/agy"),
            p,
            Some(cwd),
            &[],
            GrokLaunchMode::Interactive,
        );
        assert!(agy.contains("--prompt-interactive"));
        assert!(agy.contains("exec "));
    }

    #[test]
    fn applescript_string_escapes_quotes() {
        let s = applescript_string(r#"say "hi""#);
        assert_eq!(s, r#""say \"hi\"""#);
    }

    #[test]
    fn shell_command_quotes_paths_with_spaces() {
        let cmd = grok_shell_command(
            Path::new("/usr/bin/grok"),
            GrokLaunchMode::Run,
            Path::new("/tmp/my prompt.md"),
            Some(Path::new("/Users/me/my decomp")),
            &[],
        );
        assert!(cmd.contains("cd '/Users/me/my decomp'"));
        assert!(cmd.contains("--cwd '/Users/me/my decomp'"));
        assert!(cmd.contains("--prompt-file '/tmp/my prompt.md'"));
        // Safe to embed in AppleScript double quotes (no raw " left in work part).
        let as_lit = applescript_string(&cmd);
        assert!(as_lit.starts_with('"') && as_lit.ends_with('"'));
    }

    #[test]
    fn write_run_script_is_executable() {
        let _guard = CHAOS_HOME_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("CHAOS_HOME", dir.path());
        let path = write_run_script("echo hello").unwrap();
        assert!(path.ends_with("last-agent-run.command"));
        let body = fs::read_to_string(&path).unwrap();
        assert!(body.starts_with("#!/bin/zsh"));
        assert!(body.contains("echo hello"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o111, 0o111, "script should be executable");
        }
        std::env::remove_var("CHAOS_HOME");
    }

    #[test]
    fn resolve_prefers_profile_local_repo() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("decomp");
        fs::create_dir_all(&repo).unwrap();
        let got = resolve_repo_cwd(
            Some(repo.to_str().unwrap()),
            Some("/nonexistent"),
            Some("https://github.com/x/y"),
            None,
        );
        assert_eq!(got.as_deref(), Some(repo.as_path()));
    }

    #[test]
    fn write_prompt_roundtrip() {
        let _guard = CHAOS_HOME_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("CHAOS_HOME", dir.path());
        let p = write_prompt_file("hello match").unwrap();
        assert_eq!(fs::read_to_string(&p).unwrap(), "hello match");
        std::env::remove_var("CHAOS_HOME");
    }

    #[test]
    fn write_prompt_tagged_does_not_clobber_other_batch() {
        let _guard = CHAOS_HOME_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("CHAOS_HOME", dir.path());
        let p1 = write_prompt_file_tagged("batch one", Some("batch1")).unwrap();
        let p2 = write_prompt_file_tagged("batch two", Some("batch2")).unwrap();
        assert!(p1.ends_with("last-agent-prompt-batch1.md"));
        assert!(p2.ends_with("last-agent-prompt-batch2.md"));
        assert_eq!(fs::read_to_string(&p1).unwrap(), "batch one");
        assert_eq!(fs::read_to_string(&p2).unwrap(), "batch two");
        assert_eq!(
            fs::read_to_string(dir.path().join("last-agent-prompt.md")).unwrap(),
            "batch two"
        );
        std::env::remove_var("CHAOS_HOME");
    }
}
