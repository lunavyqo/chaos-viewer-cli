//! Chaos Viewer CLI — terminal decomp progress atlas.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use chaos_viewer_cli::claims::{load_claims, merge_locked_map, ClaimsSession};
use chaos_viewer_cli::clipboard::copy_text;
use chaos_viewer_cli::load::{
    details_base_from_source, load_chaos_db, load_function_detail, DetailCache,
};
use chaos_viewer_cli::prioritize::{priority_rows, PriorityMode};
use chaos_viewer_cli::prompt::{build_prompt, PromptOptions};
use chaos_viewer_cli::schema::format_pct;
use chaos_viewer_cli::tui;
use clap::{Parser, Subcommand};
use reqwest::Client;

#[derive(Debug, Parser)]
#[command(
    name = "chaos",
    version,
    about = "Chaos Viewer CLI — decomp progress atlas in the terminal",
    long_about = "Browse matching-decomp progress data, rank next targets, and build AI prompts.\n\
                  Schema-compatible with tangosdev/chaos-viewer."
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
        /// Write prompt to file instead of stdout
        #[arg(long)]
        out: Option<PathBuf>,
        /// Also copy to clipboard
        #[arg(long)]
        copy: bool,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let client = Client::builder()
        .user_agent("chaos-viewer-cli/0.1")
        .timeout(Duration::from_secs(30))
        .build()?;

    match cli.command.unwrap_or(Commands::Tui) {
        Commands::Tui => {
            tui::run(cli.input, cli.repo, cli.branch).await?;
        }
        Commands::Stats => {
            let (db, source) = load_chaos_db(
                &client,
                cli.input.as_deref(),
                cli.repo.as_deref(),
                cli.branch.as_deref(),
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
        }
        Commands::List { priority, limit } => {
            let (db, _) = load_chaos_db(
                &client,
                cli.input.as_deref(),
                cli.repo.as_deref(),
                cli.branch.as_deref(),
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
        Commands::Prompt { id, out, copy } => {
            let (db, source) = load_chaos_db(
                &client,
                cli.input.as_deref(),
                cli.repo.as_deref(),
                cli.branch.as_deref(),
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
            let text = build_prompt(&project, &[(fn_, detail)], &opts);
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
