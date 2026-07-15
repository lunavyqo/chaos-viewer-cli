//! Chaos Viewer CLI — terminal decomp progress atlas.

use anyhow::{bail, Context, Result};
use chaos_viewer_cli::claims::{load_claims, merge_locked_map, ClaimsClient, ClaimsSession};
use chaos_viewer_cli::clipboard::copy_text;
use chaos_viewer_cli::conventions::Convention;
use chaos_viewer_cli::load::{
    details_base_from_source, load_chaos_db, load_function_detail, DetailCache,
};
use chaos_viewer_cli::prioritize::{priority_rows, PriorityMode};
use chaos_viewer_cli::projects::{ProjectProfile, ProjectStore};
use chaos_viewer_cli::prompt::PromptOptions;
use chaos_viewer_cli::schema::format_pct;
use chaos_viewer_cli::templates::TemplateStore;
use chaos_viewer_cli::tui;
use clap::{Parser, Subcommand};
use reqwest::Client;
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(
    name = "chaos",
    version,
    about = "Chaos Viewer CLI — decomp progress atlas in the terminal",
    long_about = "Browse matching-decomp progress data, rank next targets, and build AI prompts.\n\
                  Schema-compatible with tangosdev/chaos-viewer.\n\
                  Claims coordinators are project-configured (any host via project.claimsApi)."
)]
struct Cli {
    /// Local path or raw URL to chaos-db.json
    #[arg(short, long, global = true)]
    input: Option<String>,

    /// GitHub repo URL to discover chaos-db.json
    #[arg(long, global = true)]
    repo: Option<String>,

    /// Branch override for --repo discovery
    #[arg(long, global = true)]
    branch: Option<String>,

    /// Saved project profile id (`chaos projects list`)
    #[arg(long, global = true, env = "CHAOS_PROJECT")]
    project: Option<String>,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Open the interactive TUI (default)
    Tui,
    /// Print match stats
    Stats,
    /// List functions (optionally by priority mode)
    List {
        /// nearly | scaffolded | biggest (default: all unmatched sample)
        #[arg(long)]
        priority: Option<String>,
        #[arg(long, default_value_t = 25)]
        limit: usize,
    },
    /// Build a matching prompt for a function id
    Prompt {
        /// Function id (e.g. module:0x02000000)
        #[arg(long)]
        id: String,
        /// Template id (default: config default, usually chaos-viewer)
        #[arg(long)]
        template: Option<String>,
        /// Write prompt to file instead of stdout
        #[arg(long)]
        out: Option<PathBuf>,
        /// Also copy to clipboard
        #[arg(long)]
        copy: bool,
    },
    /// List / manage prompt templates (~/.config/chaos/templates)
    Templates {
        #[command(subcommand)]
        cmd: TemplatesCmd,
    },
    /// Saved multi-repo profiles (~/.config/chaos/projects.toml)
    Projects {
        #[command(subcommand)]
        cmd: ProjectsCmd,
    },
    /// Talk to the project's claims coordinator (any compatible API)
    Claims {
        #[command(subcommand)]
        cmd: ClaimsCmd,
    },
}

#[derive(Debug, Subcommand)]
enum ProjectsCmd {
    /// List saved projects
    List,
    /// Show projects.toml path
    Dir,
    /// Add or update a project profile
    Add {
        /// Profile id (letters/digits/-/_)
        id: String,
        /// Path, raw JSON URL, or GitHub repo URL
        #[arg(long)]
        source: String,
        /// Display name (defaults to id)
        #[arg(long)]
        name: Option<String>,
        /// Branch for GitHub discovery
        #[arg(long)]
        branch: Option<String>,
        /// Data-tracking convention: default | experimental
        #[arg(long, default_value = "default")]
        convention: String,
        /// Local decomp checkout on this machine (for Grok `--cwd`)
        #[arg(long)]
        local_repo: Option<String>,
        /// Mark as active (resume on next `chaos` launch)
        #[arg(long)]
        use_now: bool,
    },
    /// Remove a saved project
    Remove { id: String },
    /// Set active project (loaded by default in the TUI)
    Use { id: String },
    /// Set a profile's data-tracking convention (default | experimental)
    Convention {
        /// Profile id
        id: String,
        /// Convention name: default or experimental
        convention: String,
    },
    /// Set (or clear) the local decomp path used by Grok launch (`g` in TUI)
    LocalRepo {
        /// Profile id
        id: String,
        /// Absolute or `~/…` path to the decomp checkout. Omit or pass `-` to clear.
        path: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
enum TemplatesCmd {
    /// List built-in and user templates
    List,
    /// Show config / templates directory
    Dir,
    /// Set the default template id
    Default {
        /// Template id (omit to print current default)
        id: Option<String>,
    },
    /// Create a new template (copy of chaos-viewer) and open it in $EDITOR / nano
    New {
        /// Template id (file stem), e.g. my-style
        id: String,
        /// Display name (defaults to id)
        #[arg(long)]
        name: Option<String>,
        /// Create the file but do not open an editor
        #[arg(long)]
        no_edit: bool,
    },
    /// Open a user template in $EDITOR / nano (not the built-in)
    Edit {
        /// Template id (default: current default template)
        id: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
enum ClaimsCmd {
    /// List active locks (API + CLAIMS.md)
    List {
        /// Override project.claimsApi (otherwise load atlas for config)
        #[arg(long)]
        api: Option<String>,
    },
    /// Print coordinator instructions (GET {claimsApi}/instructions)
    Instructions {
        #[arg(long)]
        api: Option<String>,
    },
    /// Acquire a lock on [start, end) in a module
    TryLock {
        #[arg(long)]
        module: String,
        /// Start address (hex, with or without 0x)
        #[arg(long)]
        start: String,
        /// End address (hex, half-open)
        #[arg(long)]
        end: String,
        #[arg(long)]
        note: Option<String>,
        #[arg(long)]
        api: Option<String>,
    },
    /// Renew a claim by id
    Renew {
        #[arg(long)]
        id: String,
        #[arg(long)]
        api: Option<String>,
    },
    /// Release a claim by id
    Release {
        #[arg(long)]
        id: String,
        #[arg(long)]
        api: Option<String>,
    },
    /// Exchange a GitHub access token for a session (if the coordinator supports it)
    GithubExchange {
        #[arg(long)]
        github_token: String,
        /// claimsApi base URL (required; no atlas load)
        #[arg(long)]
        api: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let mut cli = Cli::parse();
    let client = chaos_viewer_cli::http::build_client()?;

    let command = cli.command.take().unwrap_or(Commands::Tui);
    match command {
        Commands::Tui => {
            tui::run(
                cli.input.clone(),
                cli.repo.clone(),
                cli.branch.clone(),
                cli.project.clone(),
            )
            .await?;
        }
        Commands::Stats => {
            let (input, repo, branch) = resolve_atlas_args(&cli)?;
            let (db, source) = load_chaos_db(
                &client,
                input.as_deref(),
                repo.as_deref(),
                branch.as_deref(),
            )
            .await?;
            println!("source:          {}", source.display());
            println!("project:         {}", db.project_name());
            println!("generatedAt:     {}", db.generated_at);
            println!(
                "functions:       {} / {} ({}%)",
                db.stats.matched_functions,
                db.stats.total_functions,
                format_pct(db.stats.matched_functions, db.stats.total_functions)
            );
            println!(
                "bytes:           {} / {} ({}%)",
                db.stats.matched_bytes,
                db.stats.total_bytes,
                format_pct(db.stats.matched_bytes, db.stats.total_bytes)
            );
            println!("modules:         {}", db.stats.module_count);
            println!("functions array: {}", db.functions.len());
            if let Some(api) = db.project.as_ref().and_then(|p| p.claims_api.as_deref()) {
                println!("claimsApi:       {api}");
            } else {
                println!("claimsApi:       (none — CLAIMS.md only if present)");
            }
        }
        Commands::List { priority, limit } => {
            let (input, repo, branch) = resolve_atlas_args(&cli)?;
            let (db, _) = load_chaos_db(
                &client,
                input.as_deref(),
                repo.as_deref(),
                branch.as_deref(),
            )
            .await?;
            let (claims, _) = load_claims(
                &client,
                db.project.as_ref().and_then(|p| p.claims_api.as_deref()),
                db.project.as_ref().map(|p| p.github.as_str()),
            )
            .await
            .unwrap_or_else(|_| (Vec::new(), false));
            let locked = merge_locked_map(&db.functions, &claims);

            if let Some(p) = priority {
                let mode = PriorityMode::parse(&p)
                    .with_context(|| format!("unknown priority mode: {p}"))?;
                let rows = priority_rows(&db.functions, &locked, mode);
                for f in rows.into_iter().take(limit) {
                    print_fn_line(f, &locked);
                }
            } else {
                for f in db.functions.iter().filter(|f| !f.matched).take(limit) {
                    print_fn_line(f, &locked);
                }
            }
        }
        Commands::Prompt {
            id,
            template,
            out,
            copy,
        } => {
            let (input, repo, branch) = resolve_atlas_args(&cli)?;
            let (db, source) = load_chaos_db(
                &client,
                input.as_deref(),
                repo.as_deref(),
                branch.as_deref(),
            )
            .await?;
            let fn_ = db
                .find_by_id(&id)
                .with_context(|| format!("function id not found: {id}"))?
                .clone();
            let cache = DetailCache::new(details_base_from_source(&source));
            let detail = load_function_detail(&client, &cache, &fn_.module, &fn_.name)
                .await
                .ok()
                .flatten();
            let project = db.project.clone().unwrap_or_default();
            let opts = PromptOptions {
                claims_session: ClaimsSession::from_env(),
            };
            let store = TemplateStore::load();
            let tid = template
                .as_deref()
                .unwrap_or_else(|| store.default_id())
                .to_string();
            let text = store
                .render(&tid, &project, &[(fn_, detail)], &opts)
                .with_context(|| format!("render template '{tid}'"))?;
            if let Some(path) = out {
                std::fs::write(&path, &text)
                    .with_context(|| format!("write {}", path.display()))?;
                eprintln!("wrote {}", path.display());
            } else {
                println!("{text}");
            }
            if copy {
                copy_text(&text)?;
                eprintln!("copied to clipboard");
            }
        }
        Commands::Templates { cmd } => {
            let mut store = TemplateStore::load();
            match cmd {
                TemplatesCmd::List => {
                    let def = store.default_id().to_string();
                    println!("dir: {}", store.templates_dir.display());
                    println!("default: {def}");
                    println!();
                    for e in &store.entries {
                        let mark = if e.id == def { "*" } else { " " };
                        let src = e
                            .path
                            .as_ref()
                            .map(|p| p.display().to_string())
                            .unwrap_or_else(|| "builtin".into());
                        println!("{mark} {:<20}  {}  ({src})", e.id, e.name);
                        if !e.description.is_empty() {
                            println!("    {}", e.description);
                        }
                    }
                    if store.entries.len() == 1 {
                        println!();
                        println!(
                            "Add user templates as {{id}}.toml under {}",
                            store.templates_dir.display()
                        );
                        println!("See docs/prompt-templates.md");
                    }
                }
                TemplatesCmd::Dir => {
                    println!(
                        "home:      {}",
                        chaos_viewer_cli::templates::chaos_home().display()
                    );
                    println!("config:    {}", store.config_path.display());
                    println!("templates: {}", store.templates_dir.display());
                }
                TemplatesCmd::Default { id: None } => {
                    println!("{}", store.default_id());
                }
                TemplatesCmd::Default { id: Some(id) } => {
                    store.set_default(&id)?;
                    println!("default_template = {id}");
                    println!("wrote {}", store.config_path.display());
                }
                TemplatesCmd::New { id, name, no_edit } => {
                    let path = store.create_from_builtin(&id, name.as_deref())?;
                    println!("created {}", path.display());
                    println!(
                        "editor: {}",
                        chaos_viewer_cli::templates::preferred_editor()
                    );
                    if !no_edit {
                        chaos_viewer_cli::templates::open_in_editor(&path)?;
                        println!("saved — run `chaos templates list` to see it");
                    } else {
                        println!("skipped editor (--no-edit); open the file yourself when ready");
                    }
                }
                TemplatesCmd::Edit { id } => {
                    let tid = id
                        .as_deref()
                        .unwrap_or_else(|| store.default_id())
                        .to_string();
                    let path = store.editable_path(&tid)?;
                    println!("editing {}", path.display());
                    println!(
                        "editor: {}",
                        chaos_viewer_cli::templates::preferred_editor()
                    );
                    chaos_viewer_cli::templates::open_in_editor(&path)?;
                }
            }
        }
        Commands::Projects { cmd } => {
            let mut store = ProjectStore::load();
            match cmd {
                ProjectsCmd::List => {
                    println!("file: {}", store.path.display());
                    println!("active: {}", store.active_id.as_deref().unwrap_or("(none)"));
                    println!();
                    if store.projects.is_empty() {
                        println!("(no saved projects)");
                        println!("Add with: chaos projects add <id> --source <path|url|github>");
                        println!("Set decomp path: chaos projects local-repo <id> /path/to/decomp");
                    } else {
                        for p in &store.projects {
                            let mark = if store.active_id.as_deref() == Some(p.id.as_str()) {
                                "*"
                            } else {
                                " "
                            };
                            let branch = p
                                .branch
                                .as_ref()
                                .map(|b| format!("  branch={b}"))
                                .unwrap_or_default();
                            println!(
                                "{mark} {:<16}  [{:<12}]  {}{branch}",
                                p.id,
                                p.convention.label(),
                                p.source
                            );
                            if p.name != p.id {
                                println!("    name: {}", p.name);
                            }
                            match &p.local_repo {
                                Some(r) => println!("    local_repo: {r}"),
                                None => println!("    local_repo: (unset)"),
                            }
                        }
                    }
                }
                ProjectsCmd::Dir => {
                    println!("{}", store.path.display());
                }
                ProjectsCmd::Add {
                    id,
                    source,
                    name,
                    branch,
                    convention,
                    local_repo,
                    use_now,
                } => {
                    let name = name.unwrap_or_else(|| id.clone());
                    let convention = Convention::parse(&convention).with_context(|| {
                        format!("unknown convention '{convention}' (use default or experimental)")
                    })?;
                    // Preserve previous local_repo / atlas_url when re-adding without flags.
                    let prev = store.get(&id).cloned();
                    let local_repo =
                        local_repo.or_else(|| prev.as_ref().and_then(|p| p.local_repo.clone()));
                    let atlas_url = prev.and_then(|p| p.atlas_url);
                    store.upsert(ProjectProfile {
                        id: id.clone(),
                        name,
                        source,
                        branch,
                        convention,
                        local_repo: local_repo.clone(),
                        atlas_url,
                    })?;
                    if use_now {
                        store.set_active(Some(&id))?;
                        println!("active → {id}");
                    }
                    println!(
                        "saved {id} [{}] → {}",
                        convention.label(),
                        store.path.display()
                    );
                    if let Some(r) = local_repo {
                        println!("local_repo = {r}");
                    }
                }
                ProjectsCmd::Remove { id } => {
                    if store.remove(&id)? {
                        println!("removed {id}");
                    } else {
                        bail!("unknown project '{id}'");
                    }
                }
                ProjectsCmd::Use { id } => {
                    store.set_active(Some(&id))?;
                    println!("active_project = {id}");
                }
                ProjectsCmd::Convention { id, convention } => {
                    let convention = Convention::parse(&convention).with_context(|| {
                        format!("unknown convention '{convention}' (use default or experimental)")
                    })?;
                    let mut profile = store
                        .get(&id)
                        .cloned()
                        .with_context(|| format!("unknown project '{id}'"))?;
                    profile.convention = convention;
                    store.upsert(profile)?;
                    println!("{id} convention → {}", convention.label());
                }
                ProjectsCmd::LocalRepo { id, path } => {
                    let mut profile = store
                        .get(&id)
                        .cloned()
                        .with_context(|| format!("unknown project '{id}'"))?;
                    let clear = path
                        .as_deref()
                        .map(|s| s.trim().is_empty() || s.trim() == "-")
                        .unwrap_or(true);
                    if clear {
                        profile.local_repo = None;
                        store.upsert(profile)?;
                        println!("{id} local_repo cleared");
                    } else {
                        let raw = path.unwrap();
                        let expanded = expand_user_path(&raw);
                        if !expanded.is_dir() {
                            bail!(
                                "not a directory: {} (expanded: {})",
                                raw,
                                expanded.display()
                            );
                        }
                        let display = expanded.display().to_string();
                        profile.local_repo = Some(display.clone());
                        store.upsert(profile)?;
                        println!("{id} local_repo → {display}");
                    }
                }
            }
        }
        Commands::Claims { cmd } => {
            run_claims(&client, &cli, cmd).await?;
        }
    }

    Ok(())
}

/// Expand `~/…` using `$HOME` / `%USERPROFILE%`.
fn expand_user_path(raw: &str) -> std::path::PathBuf {
    let s = raw.trim();
    if let Some(rest) = s.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
            return std::path::PathBuf::from(home).join(rest);
        }
    }
    if s == "~" {
        if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
            return std::path::PathBuf::from(home);
        }
    }
    std::path::PathBuf::from(s)
}

/// Resolve --project profile into input/repo/branch, else use CLI flags.
fn resolve_atlas_args(cli: &Cli) -> Result<(Option<String>, Option<String>, Option<String>)> {
    if let Some(pid) = cli.project.as_deref() {
        let store = ProjectStore::load();
        let p = store
            .get(pid)
            .with_context(|| format!("unknown project '{pid}' (chaos projects list)"))?;
        let source = p.source.clone();
        let branch = p.branch.clone().or_else(|| cli.branch.clone());
        if source.contains("github.com/")
            && !source.contains("raw.githubusercontent.com")
            && !source.ends_with(".json")
        {
            return Ok((None, Some(source), branch));
        }
        return Ok((Some(source), None, None));
    }
    Ok((cli.input.clone(), cli.repo.clone(), cli.branch.clone()))
}

async fn resolve_claims_api(
    client: &Client,
    cli: &Cli,
    api_flag: Option<String>,
) -> Result<String> {
    if let Some(api) = api_flag.filter(|s| !s.is_empty()) {
        return Ok(api);
    }
    let (input, repo, branch) = resolve_atlas_args(cli)?;
    let (db, _) =
        load_chaos_db(client, input.as_deref(), repo.as_deref(), branch.as_deref()).await?;
    db.project
        .as_ref()
        .and_then(|p| p.claims_api.clone())
        .filter(|s| !s.is_empty())
        .context(
            "no claimsApi in project config; pass --api https://host/api/claims \
             or publish project.claimsApi in chaos-db.json",
        )
}

fn require_session() -> Result<ClaimsSession> {
    ClaimsSession::from_env().context(
        "set CHAOS_CLAIMS_API_KEY (or CHAOS_CLAIMS_SESSION / CHAOS_CLAIMS_KEY) \
         and optionally CHAOS_CLAIMS_HANDLE",
    )
}

fn parse_addr(s: &str) -> Result<u64> {
    let s = s.trim();
    if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        u64::from_str_radix(hex, 16).with_context(|| format!("bad hex address: {s}"))
    } else if s.chars().all(|c| c.is_ascii_hexdigit()) && s.len() >= 6 {
        u64::from_str_radix(s, 16).with_context(|| format!("bad hex address: {s}"))
    } else {
        s.parse::<u64>()
            .with_context(|| format!("bad address: {s}"))
    }
}

async fn run_claims(client: &Client, cli: &Cli, cmd: ClaimsCmd) -> Result<()> {
    match cmd {
        ClaimsCmd::List { api } => {
            let api = resolve_claims_api(client, cli, api).await.ok();
            let github = if cli.repo.is_some() || cli.input.is_some() {
                load_chaos_db(
                    client,
                    cli.input.as_deref(),
                    cli.repo.as_deref(),
                    cli.branch.as_deref(),
                )
                .await
                .ok()
                .and_then(|(db, _)| {
                    db.project.and_then(|p| {
                        if p.github.is_empty() {
                            None
                        } else {
                            Some(p.github)
                        }
                    })
                })
            } else {
                None
            };
            let (claims, live) = load_claims(client, api.as_deref(), github.as_deref()).await?;
            println!(
                "source: {} · {} claim(s)",
                if live { "live" } else { "unavailable" },
                claims.len()
            );
            if let Some(a) = &api {
                println!("claimsApi: {a}");
            }
            for c in &claims {
                println!(
                    "{:12}  0x{:08x}-0x{:08x}  {}  {}",
                    c.module,
                    c.start.to_u64(),
                    c.end.to_u64(),
                    c.handle.as_deref().unwrap_or("-"),
                    c.id.as_deref().unwrap_or("-"),
                );
            }
        }
        ClaimsCmd::Instructions { api } => {
            let api = resolve_claims_api(client, cli, api).await?;
            let cc = ClaimsClient::new(client.clone(), &api);
            println!("{}", cc.instructions().await?);
        }
        ClaimsCmd::TryLock {
            module,
            start,
            end,
            note,
            api,
        } => {
            let api = resolve_claims_api(client, cli, api).await?;
            let session = require_session()?;
            let start = parse_addr(&start)?;
            let end = parse_addr(&end)?;
            if end <= start {
                bail!("end must be > start (half-open range)");
            }
            let cc = ClaimsClient::new(client.clone(), &api);
            let resp = cc
                .try_lock(&session, &module, start, end, note.as_deref())
                .await?;
            println!("{}", serde_json::to_string_pretty(&resp.claim)?);
            if let Some(id) = resp.claim.as_ref().and_then(|c| c.id.as_ref()) {
                eprintln!("locked id={id} as {}", session.handle);
            }
        }
        ClaimsCmd::Renew { id, api } => {
            let api = resolve_claims_api(client, cli, api).await?;
            let session = require_session()?;
            let cc = ClaimsClient::new(client.clone(), &api);
            let resp = cc.renew(&session, &id).await?;
            println!("{}", serde_json::to_string_pretty(&resp)?);
            eprintln!("renewed {id}");
        }
        ClaimsCmd::Release { id, api } => {
            let api = resolve_claims_api(client, cli, api).await?;
            let session = require_session()?;
            let cc = ClaimsClient::new(client.clone(), &api);
            let resp = cc.release(&session, &id).await?;
            println!("{}", serde_json::to_string_pretty(&resp)?);
            eprintln!("released {id}");
        }
        ClaimsCmd::GithubExchange { github_token, api } => {
            let cc = ClaimsClient::new(client.clone(), &api);
            let session = cc.exchange_github_token(&github_token).await?;
            println!("export CHAOS_CLAIMS_SESSION='{}'", session.token);
            println!("export CHAOS_CLAIMS_HANDLE='{}'", session.handle);
            eprintln!(
                "session ready for handle={} (paste exports into your shell)",
                session.handle
            );
        }
    }
    Ok(())
}

fn print_fn_line(f: &chaos_viewer_cli::ChaosFunction, locked: &HashMap<String, String>) {
    let flag = if f.matched {
        "M"
    } else if locked.contains_key(&f.id) {
        "L"
    } else if f.div.is_some() {
        "N"
    } else {
        "U"
    };
    println!(
        "[{flag}] {:40}  {:>10}  0x{:08x}  {:>6}B  {}",
        f.name, f.module, f.addr, f.size, f.id
    );
}
