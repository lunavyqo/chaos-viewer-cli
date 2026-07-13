//! Interactive full-screen TUI.

mod theme;

use std::collections::HashMap;
use std::io::{self, stdout};
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use ratatui::{Frame, Terminal};
use reqwest::Client;

use crate::claims::{load_claims, merge_locked_map, ClaimsSession};
use crate::clipboard::copy_text;
use crate::conventions::Convention;
use crate::discover::sources_equivalent;
use crate::grok_launch::{cwd_from_load_input, GrokLaunch, GrokLaunchMode};
use crate::load::{
    details_base_from_source, load_chaos_db, load_function_detail, DataSource, DetailCache,
};
use crate::prioritize::{priority_rows, PriorityMode};
use crate::projects::{ProjectProfile, ProjectStore};
use crate::prompt::{batch_max, PromptOptions};
use crate::schema::{format_pct, ChaosDb, ChaosFunction, FunctionDetail, ProjectConfig};
use crate::templates::{TemplateStore, BUILTIN_EXPERIMENTAL_ID, BUILTIN_ID};
use crate::treemap::{layout_treemap, TreemapLeaf};
use theme::Theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Screen {
    Setup,
    Overview,
    Heatmap,
    Priorities,
    Prompt,
    Claims,
}

/// Overview function list: which match states to show.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum MatchFilter {
    #[default]
    All,
    /// Hide matched — only still-open work.
    UnmatchedOnly,
    /// Hide unmatched — only finished work.
    MatchedOnly,
}

/// Module list sort (parity with chaos-viewer modes; best/worst use open work left).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum ModuleSort {
    #[default]
    Name,
    /// Most unmatched functions left first (worst / most open work).
    OpenDesc,
    /// Fewest unmatched left first (best; fully matched modules sit at the top).
    OpenAsc,
    /// Largest modules by function count.
    Count,
    /// Largest modules by byte size.
    Bytes,
}

impl ModuleSort {
    fn cycle(self) -> Self {
        match self {
            Self::Name => Self::OpenDesc,
            Self::OpenDesc => Self::OpenAsc,
            Self::OpenAsc => Self::Count,
            Self::Count => Self::Bytes,
            Self::Bytes => Self::Name,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Name => "name (a–z)",
            Self::OpenDesc => "worst first (most unmatched left)",
            Self::OpenAsc => "best first (fewest unmatched left)",
            Self::Count => "most functions",
            Self::Bytes => "most bytes",
        }
    }

    fn short(self) -> &'static str {
        match self {
            Self::Name => "name",
            Self::OpenDesc => "worst",
            Self::OpenAsc => "best",
            Self::Count => "count",
            Self::Bytes => "bytes",
        }
    }
}

/// Per-module aggregates used for list filtering and sort.
#[derive(Debug, Clone, Copy, Default)]
struct ModuleAgg {
    matched: usize,
    total: usize,
    bytes: u64,
}

impl ModuleAgg {
    fn open(self) -> usize {
        self.total.saturating_sub(self.matched)
    }
}

/// Sort module names in place using precomputed aggregates.
fn sort_modules(mods: &mut [String], stats: &HashMap<String, ModuleAgg>, mode: ModuleSort) {
    let name_cmp = |a: &String, b: &String| a.cmp(b);
    let open = |name: &String| stats.get(name).map(|s| s.open()).unwrap_or(0);
    match mode {
        ModuleSort::Name => mods.sort_by(name_cmp),
        // Worst: biggest pile of unmatched work first.
        ModuleSort::OpenDesc => mods.sort_by(|a, b| open(b).cmp(&open(a)).then_with(|| a.cmp(b))),
        // Best: least left (all matched / 0 open) first.
        ModuleSort::OpenAsc => mods.sort_by(|a, b| open(a).cmp(&open(b)).then_with(|| a.cmp(b))),
        ModuleSort::Count => mods.sort_by(|a, b| {
            let ta = stats.get(a).map(|s| s.total).unwrap_or(0);
            let tb = stats.get(b).map(|s| s.total).unwrap_or(0);
            tb.cmp(&ta).then_with(|| a.cmp(b))
        }),
        ModuleSort::Bytes => mods.sort_by(|a, b| {
            let ba = stats.get(a).map(|s| s.bytes).unwrap_or(0);
            let bb = stats.get(b).map(|s| s.bytes).unwrap_or(0);
            bb.cmp(&ba).then_with(|| a.cmp(b))
        }),
    }
}

impl MatchFilter {
    fn cycle(self) -> Self {
        match self {
            Self::All => Self::UnmatchedOnly,
            Self::UnmatchedOnly => Self::MatchedOnly,
            Self::MatchedOnly => Self::All,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::UnmatchedOnly => "unmatched",
            Self::MatchedOnly => "matched",
        }
    }

    fn allows(self, matched: bool) -> bool {
        match self {
            Self::All => true,
            Self::UnmatchedOnly => !matched,
            Self::MatchedOnly => matched,
        }
    }

    /// Whether a module belongs in the left list under this filter.
    ///
    /// - **unmatched**: only modules with remaining open (unmatched) work
    /// - **matched**: modules that have at least one match
    /// - **all**: every module
    fn keeps_module(self, matched: usize, total: usize) -> bool {
        match self {
            Self::All => total > 0,
            // Hide 100% matched modules — nothing left to do there.
            Self::UnmatchedOnly => total > 0 && matched < total,
            Self::MatchedOnly => matched > 0,
        }
    }
}

impl Screen {
    fn all_loaded() -> &'static [Screen] {
        &[
            Screen::Overview,
            Screen::Heatmap,
            Screen::Priorities,
            Screen::Prompt,
            Screen::Claims,
        ]
    }

    /// Short page name (no number).
    fn name(self) -> &'static str {
        match self {
            Screen::Setup => "Setup",
            Screen::Overview => "Overview",
            Screen::Heatmap => "Heatmap",
            Screen::Priorities => "Priorities",
            Screen::Prompt => "Prompt",
            Screen::Claims => "Claims",
        }
    }

    /// Hotkey digit for loaded pages (1–5).
    fn hotkey(self) -> Option<char> {
        match self {
            Screen::Overview => Some('1'),
            Screen::Heatmap => Some('2'),
            Screen::Priorities => Some('3'),
            Screen::Prompt => Some('4'),
            Screen::Claims => Some('5'),
            Screen::Setup => None,
        }
    }

    /// Tab label with hotkey so navigation is discoverable.
    fn tab_label(self) -> String {
        match self.hotkey() {
            Some(k) => format!("{k} {}", self.name()),
            None => self.name().into(),
        }
    }
}

/// One visible key binding for the chrome / help overlay.
struct KeyHint {
    key: &'static str,
    action: &'static str,
}

/// Full style replace on an explicit background.
///
/// Important: never use `Color::Reset` for list row backgrounds. Ratatui List
/// applies `highlight_style` as a post-pass patch; combined with Reset, visited
/// rows (and sometimes rows below the cursor) keep the wrong tint. Always paint
/// solid `theme.bg` / `theme.panel` instead, and bake selection into the row.
fn paint_on(fg: Color, bg: Color) -> Style {
    Style::reset().fg(fg).bg(bg)
}

fn paint_bold_on(fg: Color, bg: Color) -> Style {
    Style::reset().fg(fg).bg(bg).add_modifier(Modifier::BOLD)
}

fn paint_list_base(theme: &Theme) -> Style {
    Style::reset().fg(theme.text).bg(theme.bg)
}

/// Fill a pane with solid theme colours (never terminal default/white paper).
fn fill_pane(f: &mut Frame, area: Rect, theme: &Theme, bg: Color) {
    f.render_widget(Clear, area);
    f.render_widget(Block::default().style(paint_on(theme.text, bg)), area);
}

/// Standard content block: dark bg + border.
fn content_block<'a>(
    title: impl Into<ratatui::text::Line<'a>>,
    theme: &Theme,
    border: Color,
) -> Block<'a> {
    Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(paint_on(border, theme.bg))
        .style(paint_on(theme.text, theme.bg))
}

fn key_line(theme: &Theme, hints: &[KeyHint], bg: Color) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    for (i, h) in hints.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled("  ", paint_on(theme.muted, bg)));
        }
        spans.push(Span::styled(
            h.key.to_string(),
            paint_bold_on(theme.key, bg),
        ));
        spans.push(Span::styled(
            format!(" {}", h.action),
            paint_on(theme.muted, bg),
        ));
    }
    Line::from(spans)
}

struct App {
    theme: Theme,
    screen: Screen,
    setup_input: String,
    /// Original load key (GitHub repo URL / path as typed) — never the discovered raw JSON URL.
    load_input: Option<String>,
    /// Saved multi-repo profiles.
    project_store: ProjectStore,
    project_sel: usize,
    /// Setup: true = project list focused, false = freeform source input.
    setup_list_focus: bool,
    /// Saving a new profile: type id then Enter.
    saving_project: bool,
    project_id_input: String,
    /// Pending delete: project id awaiting y/n confirmation.
    pending_delete_id: Option<String>,
    /// Active data-tracking convention (from loaded project, else Default).
    convention: Convention,
    status: String,
    error: Option<String>,
    db: Option<ChaosDb>,
    source: Option<DataSource>,
    client: Client,
    detail_cache: Option<DetailCache>,
    locked_by: HashMap<String, String>,
    claims_status: String,
    claims_count: usize,
    search: String,
    searching: bool,
    module_list: Vec<String>,
    /// Parallel to `module_list`: (matched_count, total_count) — precomputed once per load.
    module_counts: Vec<(usize, usize)>,
    module_sel: usize,
    module_offset: usize,
    fn_list: Vec<usize>,
    fn_sel: usize,
    fn_offset: usize,
    /// `function.id` → index in `db.functions` (avoids O(n) scans on every key).
    id_index: HashMap<String, usize>,
    match_filter: MatchFilter,
    module_sort: ModuleSort,
    priority_mode: PriorityMode,
    priority_list: Vec<usize>,
    priority_sel: usize,
    priority_offset: usize,
    selected_id: Option<String>,
    detail: Option<FunctionDetail>,
    batch: Vec<String>,
    prompt_scroll: u16,
    /// Scroll offset for the Overview detail pane (lines from top).
    detail_scroll: u16,
    /// Inner height of the detail pane from the last draw (for scroll clamp).
    detail_view_h: u16,
    /// Cached detail-pane text; rebuilt only when selection/detail/batch changes.
    detail_lines_cache: Vec<String>,
    detail_lines_key: String,
    prompt_text: String,
    /// Loaded prompt templates (builtin + ~/.config/chaos/templates).
    template_store: TemplateStore,
    /// Active template id for Prompt / copy.
    prompt_template_id: String,
    /// When true, `template_name_input` is editing a new template id.
    naming_template: bool,
    template_name_input: String,
    /// After leaving the TUI briefly, open this path in $EDITOR / nano.
    pending_edit: Option<std::path::PathBuf>,
    /// After leaving the TUI briefly, run Grok Build with the batch prompt.
    pending_grok: Option<GrokLaunch>,
    claims_session: Option<ClaimsSession>,
    show_help: bool,
    should_quit: bool,
}

impl App {
    fn new(claims_session: Option<ClaimsSession>) -> Result<Self> {
        let client = Client::builder()
            .user_agent("chaos-viewer-cli/0.1")
            .timeout(Duration::from_secs(30))
            .build()?;
        let project_store = ProjectStore::load();
        let project_sel = project_store
            .active_id
            .as_ref()
            .and_then(|id| project_store.index_of(id))
            .unwrap_or(0);
        let initial_convention = project_store
            .active_id
            .as_ref()
            .and_then(|id| project_store.get(id))
            .map(|p| p.convention)
            .unwrap_or_default();
        let mut app = Self {
            theme: Theme::default(),
            screen: Screen::Setup,
            setup_input: String::new(),
            load_input: None,
            project_store,
            project_sel,
            // Default to saved project list; Tab (or start typing) for source input.
            setup_list_focus: true,
            saving_project: false,
            project_id_input: String::new(),
            pending_delete_id: None,
            convention: initial_convention,
            status:
                "Project list · j/k enter · v convention · Tab/type URL · Shift+s save · d delete"
                    .into(),
            error: None,
            db: None,
            source: None,
            client,
            detail_cache: None,
            locked_by: HashMap::new(),
            claims_status: "idle".into(),
            claims_count: 0,
            search: String::new(),
            searching: false,
            module_list: Vec::new(),
            module_counts: Vec::new(),
            module_sel: 0,
            module_offset: 0,
            fn_list: Vec::new(),
            fn_sel: 0,
            fn_offset: 0,
            id_index: HashMap::new(),
            match_filter: MatchFilter::All,
            module_sort: ModuleSort::Name,
            priority_mode: PriorityMode::Nearly,
            priority_list: Vec::new(),
            priority_sel: 0,
            priority_offset: 0,
            selected_id: None,
            detail: None,
            batch: Vec::new(),
            prompt_scroll: 0,
            detail_scroll: 0,
            detail_view_h: 8,
            detail_lines_cache: Vec::new(),
            detail_lines_key: String::new(),
            prompt_text: String::new(),
            template_store: TemplateStore::load(),
            prompt_template_id: String::new(),
            naming_template: false,
            template_name_input: String::new(),
            pending_edit: None,
            pending_grok: None,
            claims_session,
            show_help: false,
            should_quit: false,
        };
        app.prompt_template_id = app.template_store.default_id().to_string();
        app.sync_template_to_convention();
        Ok(app)
    }

    fn global_hints(&self) -> Vec<KeyHint> {
        if self.screen == Screen::Setup {
            if self.pending_delete_id.is_some() {
                return vec![
                    KeyHint {
                        key: "y",
                        action: "confirm delete",
                    },
                    KeyHint {
                        key: "n/esc",
                        action: "cancel",
                    },
                ];
            }
            if self.saving_project {
                return vec![
                    KeyHint {
                        key: "type",
                        action: "project id",
                    },
                    KeyHint {
                        key: "enter",
                        action: "save",
                    },
                    KeyHint {
                        key: "esc",
                        action: "cancel",
                    },
                ];
            }
            return vec![
                KeyHint {
                    key: "type",
                    action: "source URL",
                },
                KeyHint {
                    key: "tab",
                    action: "list/input",
                },
                KeyHint {
                    key: "enter",
                    action: "load",
                },
                KeyHint {
                    key: "v",
                    action: "convention",
                },
                KeyHint {
                    key: "S-s",
                    action: "save profile",
                },
                KeyHint {
                    key: "d",
                    action: "delete",
                },
                KeyHint {
                    key: "q",
                    action: "quit",
                },
            ];
        }
        if self.searching {
            return vec![
                KeyHint {
                    key: "type",
                    action: "filter",
                },
                KeyHint {
                    key: "enter",
                    action: "apply",
                },
                KeyHint {
                    key: "esc",
                    action: "cancel search",
                },
            ];
        }
        if self.naming_template {
            return vec![
                KeyHint {
                    key: "type",
                    action: "template id",
                },
                KeyHint {
                    key: "enter",
                    action: "create + edit",
                },
                KeyHint {
                    key: "esc",
                    action: "cancel",
                },
            ];
        }
        vec![
            KeyHint {
                key: "tab/1-5",
                action: "screens",
            },
            KeyHint {
                key: "p",
                action: "projects",
            },
            KeyHint {
                key: "u",
                action: "update progress",
            },
            KeyHint {
                key: "?",
                action: "help",
            },
            KeyHint {
                key: "q",
                action: "quit",
            },
        ]
    }

    fn context_hints(&self) -> Vec<KeyHint> {
        if self.screen == Screen::Setup || self.searching {
            return Vec::new();
        }
        match self.screen {
            Screen::Overview => vec![
                KeyHint {
                    key: "j/k",
                    action: "functions",
                },
                KeyHint {
                    key: "h/l",
                    action: "modules",
                },
                KeyHint {
                    key: "pgup/pgdn",
                    action: "detail scroll",
                },
                KeyHint {
                    key: "m",
                    action: "match filter",
                },
                KeyHint {
                    key: "s",
                    action: "module sort",
                },
                KeyHint {
                    key: "/",
                    action: "search",
                },
                KeyHint {
                    key: "b",
                    action: "batch",
                },
                KeyHint {
                    key: "S-b",
                    action: "clear batch",
                },
            ],
            Screen::Heatmap => Vec::new(),
            Screen::Priorities => vec![
                KeyHint {
                    key: "j/k",
                    action: "move",
                },
                KeyHint {
                    key: "n",
                    action: "cycle list",
                },
                KeyHint {
                    key: "enter",
                    action: "show in overview",
                },
                KeyHint {
                    key: "b",
                    action: "batch",
                },
                KeyHint {
                    key: "S-b",
                    action: "clear batch",
                },
            ],
            Screen::Prompt => vec![
                KeyHint {
                    key: "j/k",
                    action: "scroll",
                },
                KeyHint {
                    key: "t",
                    action: "next template",
                },
                KeyHint {
                    key: "n",
                    action: "new template",
                },
                KeyHint {
                    key: "e",
                    action: "edit template",
                },
                KeyHint {
                    key: "S-t",
                    action: "set default",
                },
                KeyHint {
                    key: "c",
                    action: "copy",
                },
                KeyHint {
                    key: "g",
                    action: "grok build",
                },
                KeyHint {
                    key: "S-b",
                    action: "clear batch",
                },
            ],
            Screen::Claims => vec![KeyHint {
                key: "r",
                action: "refresh claims",
            }],
            Screen::Setup => Vec::new(),
        }
    }

    fn help_text(&self) -> String {
        r#"chaos — keyboard reference

GLOBAL
  ?           toggle this help
  q           quit
  tab / S-tab next / previous screen
  1 2 3 4 5   Overview · Heatmap · Priorities · Prompt · Claims
  p           projects hub (switch / add / remove saved repos)
  u           update progress (re-fetch chaos-db; matches can land mid-session)
  r           refresh claims only
  c           copy batch prompt to clipboard (no-op if batch empty)
  g           launch Grok Build with the batch prompt (Prompt page; needs `grok`)
  b           add/remove selected function from batch (max 16)
  Shift+b     clear entire batch (unselect all)
              Prompt page uses the batch only (not the Overview cursor)
              batched rows show violet [B1] [B2] … badges in lists

OVERVIEW
  top: modules (h/l) · functions (j/k) · m match filter · s module sort · / search
              unmatched filter hides fully matched modules
              sort: name · worst/best by unmatched left · most fns · most bytes
  bottom: detail pane for the selected function (loads as you move)
  pgup/pgdn   scroll the detail pane (j/k still move the function list)
  [ / ]       scroll detail one line
  b           toggle batch for selected function
  Shift+b     clear entire batch

HEATMAP
  view-only byte map (select a function on Overview / Priorities first)
  green = matched · grey = unmatched · yellow = claimed · cyan = selected

PRIORITIES
  n           cycle Nearly / Scaffolded / Biggest
  j / k       move in ranked list
  enter       jump to Overview with that function selected

PROMPT
  j / k       scroll prompt text
  pgup/pgdn   scroll prompt by page
  t           next prompt template (builtins + ~/.config/chaos/templates)
              builtins: chaos-viewer (default) · chaos-experimental (provenance)
  n           new template (copy of chaos-viewer → editor)
  e           edit current user template in $EDITOR / nano
  Shift+t     set current template as default
  c           copy batch prompt
  g           open Grok Build with this prompt (run headless, or interactive)
              writes ~/.config/chaos/last-grok-prompt.md · needs `grok` on PATH
  Shift+b     clear entire batch
  experimental projects auto-select chaos-experimental when on chaos-viewer

SETUP / PROJECTS
  type        source path / URL / GitHub (always works; focuses the input)
  tab         focus project list ↔ source input
  j / k       select saved project (when list focused)
  enter       load typed source, or selected project if list focused
  Shift+s     save current source as a named project (then type id)
  d           delete selected project (asks y/n first; list focused)
  p           open this hub from any loaded screen
  v           cycle data-tracking convention for selected project
              default = current / sm64ds-compatible · experimental = fork for
              future tracking experiments (same as default for now)

Press ? or esc to close help."#
            .to_string()
    }

    async fn load_from(&mut self, input: &str) -> Result<()> {
        self.load_from_with_branch(input, None).await
    }

    async fn load_from_with_branch(&mut self, input: &str, branch: Option<&str>) -> Result<()> {
        self.status = format!("Loading {input}…");
        self.error = None;
        let input = input.trim();
        let (db, source) = if input.contains("github.com/")
            && !input.contains("raw.githubusercontent.com")
            && !input.ends_with(".json")
        {
            load_chaos_db(&self.client, None, Some(input), branch).await?
        } else {
            load_chaos_db(&self.client, Some(input), None, None).await?
        };
        let base = details_base_from_source(&source);
        self.detail_cache = Some(DetailCache::new(base));
        // Keep the *user* source for save/resume; discovery may set source to a raw JSON URL.
        self.load_input = Some(input.to_string());
        self.setup_input = input.to_string();
        self.source = Some(source);
        // Switching repo clears batch / selection noise.
        self.batch.clear();
        self.selected_id = None;
        self.detail = None;
        self.invalidate_detail_lines();
        // Align active profile with what we actually loaded (or clear a stale one).
        self.sync_active_project_to_load_input();
        self.apply_db(db, true).await;
        Ok(())
    }

    /// Prefer matching a saved profile to `load_input`; otherwise clear active.
    fn sync_active_project_to_load_input(&mut self) {
        let Some(key) = self.load_input.clone() else {
            return;
        };
        if let Some(p) = self
            .project_store
            .projects
            .iter()
            .find(|p| sources_equivalent(&p.source, &key))
            .cloned()
        {
            let _ = self.project_store.set_active(Some(&p.id));
            if let Some(i) = self.project_store.index_of(&p.id) {
                self.project_sel = i;
            }
            self.convention = p.convention;
            self.sync_template_to_convention();
        } else {
            // Don't keep showing e.g. electroplankton as active after freeform-loading sm64ds.
            let _ = self.project_store.set_active(None);
            self.convention = Convention::Default;
            self.sync_template_to_convention();
        }
    }

    async fn load_project_by_id(&mut self, id: &str) -> Result<()> {
        let profile = self
            .project_store
            .get(id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("unknown project '{id}'"))?;
        // set_active after successful load only — via load_from_with_branch sync
        self.load_from_with_branch(&profile.source, profile.branch.as_deref())
            .await?;
        // Force this profile active even if source matching failed (e.g. local path forms).
        self.project_store.set_active(Some(&profile.id))?;
        if let Some(i) = self.project_store.index_of(&profile.id) {
            self.project_sel = i;
        }
        self.convention = profile.convention;
        self.sync_template_to_convention();
        self.status = format!(
            "Loaded project {} ({}) · [{}] · {}",
            profile.name,
            profile.id,
            profile.convention.label(),
            profile.source
        );
        Ok(())
    }

    /// Cycle the selected saved profile's convention and persist it.
    fn cycle_selected_convention(&mut self) -> Result<()> {
        let Some(p) = self.project_store.projects.get(self.project_sel).cloned() else {
            anyhow::bail!("no project selected");
        };
        let next = p.convention.cycle();
        let mut updated = p;
        let id = updated.id.clone();
        updated.convention = next;
        self.project_store.upsert(updated)?;
        // If this profile is active / currently loaded, keep session in sync.
        if self.project_store.active_id.as_deref() == Some(id.as_str()) {
            self.convention = next;
            self.sync_template_to_convention();
        }
        self.status = format!("Project '{id}' convention → {}", next.label());
        Ok(())
    }

    /// Prefer stock experimental prompt when on experimental convention (and vice versa).
    /// Does not override a user-selected custom template.
    fn sync_template_to_convention(&mut self) {
        match self.convention {
            Convention::Experimental => {
                if self.prompt_template_id == BUILTIN_ID
                    && self.template_store.get(BUILTIN_EXPERIMENTAL_ID).is_some()
                {
                    self.prompt_template_id = BUILTIN_EXPERIMENTAL_ID.to_string();
                }
            }
            Convention::Default => {
                if self.prompt_template_id == BUILTIN_EXPERIMENTAL_ID {
                    self.prompt_template_id = BUILTIN_ID.to_string();
                }
            }
        }
    }

    /// Source string to persist for a profile — original GitHub/path, never discovered raw atlas URL.
    fn profile_source_to_save(&self) -> Result<String> {
        if let Some(key) = self
            .load_input
            .as_ref()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
        {
            return Ok(key.to_string());
        }
        let typed = self.setup_input.trim();
        if !typed.is_empty() {
            return Ok(typed.to_string());
        }
        // Last resort: local path only (not raw.githubusercontent — that is an artifact of discovery).
        if let Some(DataSource::Path(p)) = &self.source {
            return Ok(p.display().to_string());
        }
        anyhow::bail!("nothing to save — type a path/URL or load a project first")
    }

    fn save_current_source_as_project(&mut self, id: &str) -> Result<()> {
        let source = self.profile_source_to_save()?;
        let id = crate::projects::sanitize_project_id(id)?;
        let name = id.clone();
        self.project_store.upsert(ProjectProfile {
            id: id.clone(),
            name,
            source: source.clone(),
            branch: None,
            // New saves keep the session convention (default unless cycling first).
            convention: self.convention,
        })?;
        self.project_store.set_active(Some(&id))?;
        self.project_sel = self.project_store.index_of(&id).unwrap_or(0);
        self.load_input = Some(source);
        Ok(())
    }

    fn active_project_label(&self) -> String {
        self.project_store
            .active_id
            .as_ref()
            .and_then(|id| self.project_store.get(id))
            .map(|p| format!("{} ({}) [{}]", p.name, p.id, p.convention.label()))
            .or_else(|| {
                self.source
                    .as_ref()
                    .map(|s| format!("{} [{}]", s.display(), self.convention.label()))
            })
            .unwrap_or_else(|| "no project".into())
    }

    /// Re-fetch atlas data from the current source (local file or URL).
    /// Matches / stats can change while you work; this picks up the latest publish.
    async fn update_progress(&mut self) -> Result<()> {
        // Prefer original GitHub/path key so we re-discover; not a stashed raw JSON URL only.
        let input = self
            .load_input
            .clone()
            .or_else(|| self.source.as_ref().map(|s| s.display()))
            .ok_or_else(|| anyhow::anyhow!("no data loaded yet"))?;
        self.status = "Updating progress…".into();
        self.error = None;

        let prev_module = self.selected_module().map(str::to_string);
        let prev_id = self.selected_id.clone();
        let prev_batch = self.batch.clone();
        let prev_screen = self.screen;
        let prev_priority_mode = self.priority_mode;

        let (db, source) = if input.contains("github.com/")
            && !input.contains("raw.githubusercontent.com")
            && !input.ends_with(".json")
        {
            load_chaos_db(&self.client, None, Some(&input), None).await?
        } else {
            load_chaos_db(&self.client, Some(&input), None, None).await?
        };
        let base = details_base_from_source(&source);
        self.detail_cache = Some(DetailCache::new(base));
        self.load_input = Some(input);
        self.source = Some(source);

        self.priority_mode = prev_priority_mode;
        self.apply_db(db, false).await;

        // Restore navigation context when possible.
        if let Some(m) = prev_module {
            if let Some(i) = self.module_list.iter().position(|x| x == &m) {
                self.module_sel = i;
            }
        }
        self.rebuild_functions();

        if let Some(id) = prev_id {
            if let Some(list_i) = self.fn_list.iter().enumerate().find_map(|(i, &idx)| {
                self.db
                    .as_ref()
                    .filter(|d| d.functions.get(idx).map(|f| f.id == id).unwrap_or(false))
                    .map(|_| i)
            }) {
                self.fn_sel = list_i;
                self.selected_id = Some(id);
            } else {
                self.sync_selection_from_fn();
            }
        }

        // Keep batch entries that still exist (matched/removed drop out).
        if let Some(db) = &self.db {
            self.batch = prev_batch
                .into_iter()
                .filter(|id| db.find_by_id(id).is_some())
                .collect();
        }

        self.rebuild_priorities();
        self.detail = None;
        if prev_screen != Screen::Setup {
            self.screen = prev_screen;
        }
        // Refresh detail for the selected function; rebuild prompt only here
        // (not on every list move).
        self.ensure_selected_detail().await;
        self.rebuild_prompt().await;
        self.invalidate_detail_lines();

        if let Some(db) = &self.db {
            self.status = format!(
                "Updated · {}/{} fn ({:.2}%) · batch {}/{}",
                db.stats.matched_functions,
                db.stats.total_functions,
                db.match_pct_functions(),
                self.batch.len(),
                crate::prompt::batch_max(),
            );
        }
        Ok(())
    }

    async fn apply_db(&mut self, db: ChaosDb, reset_to_overview: bool) {
        self.refresh_claims(&db).await;
        self.rebuild_modules(&db);
        self.id_index = db
            .functions
            .iter()
            .enumerate()
            .map(|(i, f)| (f.id.clone(), i))
            .collect();
        self.db = Some(db);
        if reset_to_overview {
            self.screen = Screen::Overview;
        }
        self.rebuild_functions();
        self.rebuild_priorities();
        self.ensure_selected_detail().await;
        self.rebuild_prompt().await;
        self.invalidate_detail_lines();
        if let Some(db) = &self.db {
            self.status = format!(
                "Loaded {} · {}/{} fn ({:.2}%)",
                db.project_name(),
                db.stats.matched_functions,
                db.stats.total_functions,
                db.match_pct_functions(),
            );
        }
    }

    async fn refresh_claims(&mut self, db: &ChaosDb) {
        let api = db.project.as_ref().and_then(|p| p.claims_api.as_deref());
        let gh = db.project.as_ref().and_then(|p| {
            if p.github.is_empty() {
                None
            } else {
                Some(p.github.as_str())
            }
        });
        match load_claims(&self.client, api, gh).await {
            Ok((claims, live)) => {
                self.claims_count = claims.len();
                self.locked_by = merge_locked_map(&db.functions, &claims);
                self.claims_status = if live {
                    format!(
                        "live · {} ranges · {} locked functions",
                        claims.len(),
                        self.locked_by.len()
                    )
                } else if api.is_none() && gh.is_none() {
                    "no claims source in project config".into()
                } else {
                    "CLAIMS.md / API not found (optional)".into()
                };
            }
            Err(e) => {
                self.claims_status = format!("claims fetch failed: {e}");
                self.locked_by.clear();
                self.claims_count = 0;
            }
        }
    }

    fn rebuild_modules(&mut self, db: &ChaosDb) {
        // One linear pass for counts instead of O(modules × functions) every frame.
        let mut totals: HashMap<String, ModuleAgg> = HashMap::new();
        for f in &db.functions {
            let e = totals.entry(f.module.clone()).or_default();
            e.total += 1;
            e.bytes = e.bytes.saturating_add(f.size);
            if f.matched {
                e.matched += 1;
            }
        }
        let mut mods: Vec<String> = totals.keys().cloned().collect();
        // Apply match filter to the module list (matched view drops never-touched modules).
        let filter = self.match_filter;
        mods.retain(|m| {
            let s = totals.get(m).copied().unwrap_or_default();
            filter.keeps_module(s.matched, s.total)
        });
        sort_modules(&mut mods, &totals, self.module_sort);
        let prev = self.selected_module().map(str::to_string);
        self.module_counts = mods
            .iter()
            .map(|m| {
                let s = totals.get(m).copied().unwrap_or_default();
                (s.matched, s.total)
            })
            .collect();
        self.module_list = mods;
        self.module_sel = prev
            .and_then(|m| self.module_list.iter().position(|x| x == &m))
            .unwrap_or(0);
        self.module_offset = 0;
    }

    fn selected_module(&self) -> Option<&str> {
        self.module_list.get(self.module_sel).map(String::as_str)
    }

    /// Keep `sel` visible inside a viewport of `height` rows starting at `offset`.
    fn clamp_scroll(sel: usize, offset: &mut usize, len: usize, height: usize) {
        if len == 0 || height == 0 {
            *offset = 0;
            return;
        }
        let sel = sel.min(len - 1);
        if sel < *offset {
            *offset = sel;
        } else if sel >= *offset + height {
            *offset = sel + 1 - height;
        }
        let max_off = len.saturating_sub(height);
        if *offset > max_off {
            *offset = max_off;
        }
    }

    /// Draw a bordered pane of pre-built lines with manual scrolling (no List widget).
    ///
    /// Every cell in the viewport is written with an explicit fg/bg so macOS
    /// Terminal cannot keep a previous SGR colour for empty/short rows.
    fn draw_line_list(
        f: &mut Frame,
        area: Rect,
        title: String,
        theme: &Theme,
        lines: &[Line<'static>],
        offset: usize,
    ) {
        let block = Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(paint_on(theme.border, theme.bg))
            .style(paint_list_base(theme));
        let inner = block.inner(area);
        f.render_widget(block, area);
        f.render_widget(Clear, inner);

        let height = inner.height as usize;
        let width = inner.width as usize;
        let base = paint_list_base(theme);
        let buf = f.buffer_mut();

        for row in 0..height {
            let y = inner.y + row as u16;
            // Fill the full row first so no cell inherits a previous colour.
            for col in 0..width {
                let cell = &mut buf[(inner.x + col as u16, y)];
                cell.set_symbol(" ");
                cell.set_style(base);
            }
            let idx = offset + row;
            if let Some(line) = lines.get(idx) {
                buf.set_line(inner.x, y, line, inner.width);
            }
        }
    }

    fn rebuild_functions(&mut self) {
        let Some(db) = &self.db else {
            self.fn_list.clear();
            return;
        };
        let module = self.selected_module();
        let q = self.search.to_ascii_lowercase();
        let filter = self.match_filter;
        let prev_id = self.selected_id.clone();
        self.fn_list = db
            .functions
            .iter()
            .enumerate()
            .filter(|(_, f)| module.map(|m| f.module == m).unwrap_or(true))
            .filter(|(_, f)| filter.allows(f.matched))
            .filter(|(_, f)| {
                q.is_empty()
                    || f.name.to_ascii_lowercase().contains(&q)
                    || f.module.to_ascii_lowercase().contains(&q)
                    || f.id.to_ascii_lowercase().contains(&q)
            })
            .map(|(i, _)| i)
            .collect();
        // Keep the same function selected if it is still visible under the filter.
        if let Some(id) = prev_id {
            if let Some(list_i) = self
                .fn_list
                .iter()
                .enumerate()
                .find_map(|(i, &idx)| db.functions.get(idx).filter(|f| f.id == id).map(|_| i))
            {
                self.fn_sel = list_i;
            } else {
                self.fn_sel = 0;
            }
        } else {
            self.fn_sel = 0;
        }
        self.fn_offset = 0;
        self.sync_selection_from_fn();
    }

    fn rebuild_priorities(&mut self) {
        let Some(db) = &self.db else {
            self.priority_list.clear();
            return;
        };
        let rows = priority_rows(&db.functions, &self.locked_by, self.priority_mode);
        self.priority_list = rows
            .into_iter()
            .filter_map(|f| db.functions.iter().position(|x| x.id == f.id))
            .collect();
        self.priority_sel = 0;
        self.priority_offset = 0;
    }

    fn sync_selection_from_fn(&mut self) {
        let Some(db) = &self.db else { return };
        if let Some(&idx) = self.fn_list.get(self.fn_sel) {
            let new_id = db.functions[idx].id.clone();
            if self.selected_id.as_deref() != Some(new_id.as_str()) {
                self.detail_scroll = 0;
            }
            self.selected_id = Some(new_id);
        }
    }

    fn invalidate_detail_lines(&mut self) {
        self.detail_lines_key.clear();
        self.detail_lines_cache.clear();
    }

    fn detail_cache_key(&self) -> String {
        let id = self.selected_id.as_deref().unwrap_or("");
        let has_det = self.detail.is_some();
        let batch_n = self.batch_index(id).unwrap_or(0);
        let batch_len = self.batch.len();
        let locked = self.locked_by.get(id).map(|s| s.as_str()).unwrap_or("");
        format!("{id}|d={has_det}|b={batch_n}/{batch_len}|L={locked}")
    }

    /// Cached until selection / detail / batch membership changes.
    fn detail_pane_lines(&mut self) -> &Vec<String> {
        let key = self.detail_cache_key();
        if self.detail_lines_key == key && !self.detail_lines_cache.is_empty() {
            return &self.detail_lines_cache;
        }
        let lines = self.build_detail_pane_lines();
        self.detail_lines_key = key;
        self.detail_lines_cache = lines;
        &self.detail_lines_cache
    }

    fn build_detail_pane_lines(&self) -> Vec<String> {
        let Some(fn_) = self.selected_function() else {
            return vec!["No function selected. Move with j/k in the list above.".into()];
        };
        let locked = self
            .locked_by
            .get(&fn_.id)
            .map(|h| format!("CLAIMED by {h}"))
            .unwrap_or_else(|| "unlocked".into());
        let mut lines = vec![
            format!(
                "{}  [{}]",
                fn_.name,
                if fn_.matched { "MATCHED" } else { "UNMATCHED" }
            ),
            format!(
                "module {}  addr 0x{:x}  size {}  id {}",
                fn_.module, fn_.addr, fn_.size, fn_.id
            ),
            locked,
        ];
        if let Some(d) = fn_.div {
            lines.push(format!("near-miss div={d}  cat={:?}", fn_.cat));
        }
        if let Some(s) = fn_.sim {
            lines.push(format!("similarity {s:.3}  sibling={:?}", fn_.sibling));
        }
        if let Some(floor) = &fn_.floor {
            lines.push(format!("floor: {floor}"));
        }
        if let Some(a) = &fn_.author {
            lines.push(format!("author: {a}"));
        }
        // Experimental: show how this function was matched (model/harness or human).
        for line in crate::conventions::Tracking::provenance_detail_lines(self.convention, fn_) {
            lines.push(line);
        }
        if let Some(det) = &self.detail {
            if let Some(c) = &det.callees {
                lines.push(format!("callees: {}", c.join(", ")));
            }
            if let Some(c) = &det.called_by {
                lines.push(format!("called by: {}", c.join(", ")));
            }
            if let Some(draft) = &det.draft {
                lines.push(String::new());
                lines.push(format!("draft (div={:?}):", det.draft_div));
                for l in draft.lines() {
                    lines.push(l.to_string());
                }
            }
            if let Some(dis) = &det.disasm {
                lines.push(String::new());
                lines.push(format!("disasm ({} lines):", dis.len()));
                for l in dis {
                    lines.push(l.clone());
                }
            }
            if let Some(pool) = &det.pool {
                if !pool.is_empty() {
                    lines.push(String::new());
                    lines.push(format!("pool ({} entries):", pool.len()));
                    for p in pool {
                        lines.push(format!("  {p}"));
                    }
                }
            }
        } else {
            lines.push(String::new());
            lines.push("(detail loading… or no chunk for this module)".into());
        }
        lines
    }

    /// Sticky footer for the detail pane (always visible; not part of scroll text).
    fn detail_batch_footer(&self) -> String {
        let Some(fn_) = self.selected_function() else {
            return " select a function above · b adds to batch ".into();
        };
        if let Some(n) = self.batch_index(&fn_.id) {
            format!(
                " BATCHED [B{n}]  ·  {n}/{}  ·  b remove  ·  S-b clear  ·  c copy ",
                self.batch.len()
            )
        } else if self.batch.is_empty() {
            format!(
                " not in batch  ·  b to add ({})  ·  Prompt uses batch only ",
                self.batch_summary()
            )
        } else {
            format!(
                " not in batch  ·  b to add ({})  ·  S-b clear batch ",
                self.batch_summary()
            )
        }
    }

    /// Max scroll so the last page still fills the viewport (not past the last line).
    fn detail_max_scroll(&mut self) -> u16 {
        let total = self.detail_pane_lines().len();
        let view = self.detail_view_h.max(1) as usize;
        total.saturating_sub(view) as u16
    }

    fn scroll_detail(&mut self, delta: i32) {
        let max = self.detail_max_scroll();
        if delta < 0 {
            self.detail_scroll = self.detail_scroll.saturating_sub((-delta) as u16);
        } else {
            self.detail_scroll = self.detail_scroll.saturating_add(delta as u16).min(max);
        }
        let max = self.detail_max_scroll();
        self.detail_scroll = self.detail_scroll.min(max);
    }

    fn selected_function(&self) -> Option<&ChaosFunction> {
        let db = self.db.as_ref()?;
        let id = self.selected_id.as_ref()?;
        if let Some(&idx) = self.id_index.get(id) {
            return db.functions.get(idx);
        }
        db.find_by_id(id)
    }

    /// Fast path: use in-memory module chunk if present. No network.
    fn apply_detail_from_cache(&mut self) -> bool {
        let (module, name) = {
            let Some(f) = self.selected_function() else {
                self.detail = None;
                self.invalidate_detail_lines();
                return true;
            };
            (f.module.clone(), f.name.clone())
        };
        let Some(cache) = &self.detail_cache else {
            self.detail = None;
            self.invalidate_detail_lines();
            return true;
        };
        if let Some(det) = cache.get_if_module_loaded(&module, &name) {
            self.detail = det;
            self.invalidate_detail_lines();
            return true;
        }
        false
    }

    /// Load detail for the current selection. Uses cache when possible (no await I/O).
    /// Does **not** rebuild the batch prompt (that made j/k laggy).
    async fn ensure_selected_detail(&mut self) {
        if self.apply_detail_from_cache() {
            return;
        }
        self.load_selected_detail().await;
    }

    async fn load_selected_detail(&mut self) {
        let (module, name) = {
            let Some(f) = self.selected_function() else {
                self.detail = None;
                self.invalidate_detail_lines();
                return;
            };
            (f.module.clone(), f.name.clone())
        };
        let Some(cache) = &self.detail_cache else {
            self.detail = None;
            self.invalidate_detail_lines();
            return;
        };
        match load_function_detail(&self.client, cache, &module, &name).await {
            Ok(d) => self.detail = d,
            Err(_) => self.detail = None,
        }
        self.invalidate_detail_lines();
    }

    fn project(&self) -> ProjectConfig {
        self.db
            .as_ref()
            .and_then(|d| d.project.clone())
            .unwrap_or_default()
    }

    /// Rebuild the Prompt page from the **batch only**.
    ///
    /// An empty batch never falls back to the Overview cursor — that was
    /// surprising and made it look like a random function was "in the prompt".
    /// Add with `b` on Overview/Priorities/Detail first.
    async fn rebuild_prompt(&mut self) {
        let project = self.project();
        let opts = PromptOptions {
            claims_session: self.claims_session.clone(),
        };
        let Some(db) = &self.db else {
            self.prompt_text.clear();
            return;
        };

        let targets: Vec<ChaosFunction> = self
            .batch
            .iter()
            .filter_map(|id| {
                self.id_index
                    .get(id)
                    .and_then(|&i| db.functions.get(i))
                    .or_else(|| db.find_by_id(id))
                    .cloned()
            })
            .collect();

        if targets.is_empty() {
            self.prompt_text = "Batch is empty.\n\n\
Add functions with b on Overview or Priorities \
(max 16), then open Prompt (4) or press c to copy."
                .into();
            self.prompt_scroll = 0;
            return;
        }

        let mut items: Vec<(ChaosFunction, Option<FunctionDetail>)> = Vec::new();
        for f in targets {
            let det = if let Some(cache) = &self.detail_cache {
                load_function_detail(&self.client, cache, &f.module, &f.name)
                    .await
                    .ok()
                    .flatten()
            } else {
                None
            };
            // Keep selected-function detail cache in sync for the Detail view.
            if self.selected_id.as_deref() == Some(f.id.as_str()) {
                self.detail = det.clone();
            }
            items.push((f, det));
        }

        let id = if self.prompt_template_id.is_empty() {
            self.template_store.default_id().to_string()
        } else {
            self.prompt_template_id.clone()
        };
        self.prompt_template_id = id.clone();
        match self.template_store.render(&id, &project, &items, &opts) {
            Ok(text) => self.prompt_text = text,
            Err(e) => {
                self.prompt_text = format!("Template error ({id}): {e:#}");
            }
        }
        self.prompt_scroll = 0;
    }

    fn cycle_prompt_template(&mut self, delta: isize) {
        self.prompt_template_id = self
            .template_store
            .cycle_id(&self.prompt_template_id, delta);
    }

    fn set_prompt_template_default(&mut self) {
        match self.template_store.set_default(&self.prompt_template_id) {
            Ok(()) => {
                self.status = format!(
                    "Default template → {} ({})",
                    self.prompt_template_id,
                    self.template_store
                        .get(&self.prompt_template_id)
                        .map(|e| e.name.as_str())
                        .unwrap_or("?")
                );
            }
            Err(e) => {
                self.error = Some(format!("set default template: {e:#}"));
            }
        }
    }

    fn prompt_template_label(&self) -> String {
        let id = &self.prompt_template_id;
        let name = self
            .template_store
            .get(id)
            .map(|e| e.name.as_str())
            .unwrap_or(id.as_str());
        let def = self.template_store.default_id();
        if id == def {
            format!("{name} ★")
        } else {
            name.to_string()
        }
    }

    async fn toggle_batch_selected(&mut self) {
        let Some(id) = self.selected_id.clone() else {
            self.status = "Nothing selected to batch · pick a function first".into();
            return;
        };
        let name = self
            .selected_function()
            .map(|f| f.name.clone())
            .unwrap_or_else(|| id.clone());
        if let Some(pos) = self.batch.iter().position(|x| x == &id) {
            self.batch.remove(pos);
            self.status = format!(
                "Removed {name} from batch · now {}/{}",
                self.batch.len(),
                batch_max()
            );
        } else if self.batch.len() < batch_max() {
            self.batch.push(id);
            self.status = format!(
                "Batched {name} · {}/{}  (B badge in lists)",
                self.batch.len(),
                batch_max()
            );
        } else {
            self.status = format!("Batch full ({}/{})", self.batch.len(), batch_max());
        }
        self.invalidate_detail_lines();
        self.rebuild_prompt().await;
    }

    /// Remove every function from the prompt batch (web “clear”).
    async fn clear_batch(&mut self) {
        if self.batch.is_empty() {
            self.status = "Batch already empty".into();
            return;
        }
        let n = self.batch.len();
        self.batch.clear();
        self.invalidate_detail_lines();
        self.rebuild_prompt().await;
        self.status = format!("Cleared batch · removed {n} function(s)");
    }

    /// 1-based position in the prompt batch, if present.
    fn batch_index(&self, id: &str) -> Option<usize> {
        self.batch.iter().position(|x| x == id).map(|i| i + 1)
    }

    fn batch_badge_spans(&self, id: &str, bg: Color) -> Vec<Span<'static>> {
        if let Some(n) = self.batch_index(id) {
            vec![Span::styled(
                format!("[B{n}] "),
                paint_bold_on(self.theme.batch, bg),
            )]
        } else {
            Vec::new()
        }
    }

    fn batch_summary(&self) -> String {
        format!("{}/{}", self.batch.len(), batch_max())
    }

    fn copy_prompt(&mut self) {
        if self.batch.is_empty() {
            self.status = "Nothing to copy · batch is empty (press b to add functions)".into();
            return;
        }
        match copy_text(&self.prompt_text) {
            Ok(()) => {
                self.status = format!(
                    "Prompt copied · {} function(s) from batch",
                    self.batch.len()
                );
            }
            Err(e) => {
                self.error = Some(format!("clipboard: {e}"));
                self.status = "Copy failed".into();
            }
        }
    }

    /// Queue a Grok Build launch after this frame (suspend TUI like the editor).
    fn queue_grok_launch(&mut self) {
        if self.batch.is_empty() {
            self.status = "Nothing to send · batch empty (b to add, then g on Prompt)".into();
            return;
        }
        if self.prompt_text.trim().is_empty() {
            self.status = "Prompt empty · open Prompt (4) first so it can rebuild".into();
            return;
        }
        let cfg = &self.template_store.config;
        let mode = cfg
            .grok_mode
            .as_deref()
            .and_then(GrokLaunchMode::parse)
            .unwrap_or_default();
        let cwd = cwd_from_load_input(
            self.load_input.as_deref(),
            self.source.as_ref().and_then(|s| match s {
                DataSource::Path(p) => Some(p.as_path()),
                DataSource::Url(_) => None,
            }),
        );
        // Also copy so the user can paste if launch fails.
        let _ = copy_text(&self.prompt_text);
        match GrokLaunch::prepare(
            &self.prompt_text,
            mode,
            cfg.grok_bin.as_deref(),
            cwd,
            &cfg.grok_extra_args,
        ) {
            Ok(launch) => {
                self.status = format!(
                    "Launching Grok Build ({}) · {} fn · esc after it exits",
                    mode.as_str(),
                    self.batch.len()
                );
                self.pending_grok = Some(launch);
            }
            Err(e) => {
                self.error = Some(format!("{e:#}"));
                self.status = "Grok launch failed (prompt still copied if clipboard ok)".into();
            }
        }
    }

    async fn on_key(&mut self, ev: KeyEvent) {
        let mods = ev.modifiers;
        // Keep original case for typing; lower-case for command keys.
        let typed = match ev.code {
            KeyCode::Char(c) => Some(c),
            _ => None,
        };
        let key = match ev.code {
            KeyCode::Char(c) => KeyCode::Char(c.to_ascii_lowercase()),
            other => other,
        };

        // Help overlay: dismiss only (second q after close will quit)
        if self.show_help {
            match key {
                KeyCode::Char('?') | KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') => {
                    self.show_help = false;
                    self.status = if key == KeyCode::Char('q') {
                        "Help closed · press q again to quit".into()
                    } else {
                        "Help closed".into()
                    };
                }
                _ => {}
            }
            return;
        }

        if self.searching {
            match key {
                KeyCode::Esc => {
                    self.searching = false;
                    self.search.clear();
                    self.rebuild_functions();
                    self.status = "Search cleared".into();
                }
                KeyCode::Enter => {
                    self.searching = false;
                    self.rebuild_functions();
                    self.status = if self.search.is_empty() {
                        "Search closed".into()
                    } else {
                        format!("Filter: {} ({} matches)", self.search, self.fn_list.len())
                    };
                }
                KeyCode::Backspace | KeyCode::Delete => {
                    self.search.pop();
                    self.rebuild_functions();
                }
                KeyCode::Char(_) if !mods.contains(KeyModifiers::CONTROL) => {
                    if let Some(c) = typed {
                        self.search.push(c);
                        self.rebuild_functions();
                    }
                }
                _ => {}
            }
            return;
        }

        if self.naming_template {
            match key {
                KeyCode::Esc => {
                    self.naming_template = false;
                    self.template_name_input.clear();
                    self.status = "New template cancelled".into();
                }
                KeyCode::Enter => {
                    let id = self.template_name_input.clone();
                    self.naming_template = false;
                    self.template_name_input.clear();
                    match self.template_store.create_from_builtin(&id, None) {
                        Ok(path) => {
                            self.prompt_template_id = id;
                            self.pending_edit = Some(path.clone());
                            self.status = format!(
                                "Created {} · opening {}…",
                                path.display(),
                                crate::templates::preferred_editor()
                            );
                        }
                        Err(e) => {
                            self.error = Some(format!("new template: {e:#}"));
                            self.status = "Create failed".into();
                        }
                    }
                }
                KeyCode::Backspace | KeyCode::Delete => {
                    self.template_name_input.pop();
                }
                KeyCode::Char(_) if !mods.contains(KeyModifiers::CONTROL) => {
                    if let Some(c) = typed {
                        if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                            self.template_name_input.push(c);
                        }
                    }
                }
                _ => {}
            }
            return;
        }

        // ? always available (Shift+/ on US keyboards)
        if key == KeyCode::Char('?') || matches!(typed, Some('?')) {
            self.show_help = true;
            self.error = None;
            return;
        }

        if self.screen == Screen::Setup {
            if let Some(id) = self.pending_delete_id.clone() {
                match key {
                    KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                        self.pending_delete_id = None;
                        match self.project_store.remove(&id) {
                            Ok(true) => {
                                self.project_sel = self
                                    .project_sel
                                    .min(self.project_store.projects.len().saturating_sub(1));
                                self.status = format!("Removed project '{id}'");
                                self.error = None;
                            }
                            Ok(false) => self.status = "Nothing to remove".into(),
                            Err(e) => self.error = Some(format!("{e:#}")),
                        }
                    }
                    KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc | KeyCode::Backspace => {
                        self.pending_delete_id = None;
                        self.status = "Delete cancelled".into();
                    }
                    _ => {
                        self.status = format!("Delete '{id}'?  y = yes · n/esc = cancel");
                    }
                }
                return;
            }
            if self.saving_project {
                match key {
                    KeyCode::Esc => {
                        self.saving_project = false;
                        self.project_id_input.clear();
                        self.status = "Save cancelled".into();
                    }
                    KeyCode::Enter => {
                        let id = self.project_id_input.clone();
                        self.saving_project = false;
                        self.project_id_input.clear();
                        match self.save_current_source_as_project(&id) {
                            Ok(()) => {
                                self.status = format!("Saved project profile '{id}'");
                                self.error = None;
                            }
                            Err(e) => {
                                self.error = Some(format!("{e:#}"));
                                self.status = "Save failed".into();
                            }
                        }
                    }
                    KeyCode::Backspace | KeyCode::Delete => {
                        self.project_id_input.pop();
                    }
                    KeyCode::Char(_) if !mods.contains(KeyModifiers::CONTROL) => {
                        if let Some(c) = typed {
                            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                                self.project_id_input.push(c);
                            }
                        }
                    }
                    _ => {}
                }
                return;
            }
            match key {
                KeyCode::Esc => {
                    if self.db.is_some() {
                        self.screen = Screen::Overview;
                        self.status = "Overview".into();
                    } else {
                        self.status =
                            "type URL · enter load · Tab list · v convention · Shift+s save · q quit"
                                .into();
                    }
                }
                KeyCode::Tab => {
                    self.setup_list_focus = !self.setup_list_focus;
                    self.status = if self.setup_list_focus {
                        "Focus: project list (j/k enter · v convention · d delete · Shift+s save)"
                    } else {
                        "Focus: source input (type URL · enter load · Shift+s save)"
                    }
                    .into();
                }
                KeyCode::Up | KeyCode::Char('k') if self.setup_list_focus => {
                    if !self.project_store.projects.is_empty() {
                        if self.project_sel == 0 {
                            self.project_sel = self.project_store.projects.len() - 1;
                        } else {
                            self.project_sel -= 1;
                        }
                    }
                }
                KeyCode::Down | KeyCode::Char('j') if self.setup_list_focus => {
                    if !self.project_store.projects.is_empty() {
                        self.project_sel =
                            (self.project_sel + 1) % self.project_store.projects.len();
                    }
                }
                KeyCode::Char('v') if self.setup_list_focus => {
                    if let Err(e) = self.cycle_selected_convention() {
                        self.error = Some(format!("{e:#}"));
                    }
                }
                KeyCode::Char('d') if self.setup_list_focus => {
                    if let Some(p) = self.project_store.projects.get(self.project_sel) {
                        self.pending_delete_id = Some(p.id.clone());
                        self.status = format!(
                            "Delete project '{}' ({})?  y = yes · n/esc = cancel",
                            p.id, p.source
                        );
                    }
                }
                // Shift+s = save (plain `s` must type into https://…)
                KeyCode::Char('s') if mods.contains(KeyModifiers::SHIFT) => {
                    let suggest = if let Ok(src) = self.profile_source_to_save() {
                        ProjectStore::suggest_id(&src)
                    } else if let Some(p) = self.project_store.projects.get(self.project_sel) {
                        p.id.clone()
                    } else {
                        "my-project".into()
                    };
                    self.saving_project = true;
                    self.project_id_input = suggest;
                    self.status =
                        "Project id (letters/digits/-/_) · enter save · esc cancel".into();
                }
                KeyCode::Enter => {
                    if self.setup_list_focus && !self.project_store.projects.is_empty() {
                        if let Some(p) = self.project_store.projects.get(self.project_sel) {
                            let id = p.id.clone();
                            if let Err(e) = self.load_project_by_id(&id).await {
                                self.error = Some(format!("{e:#}"));
                                self.status = "Load failed".into();
                            } else {
                                self.error = None;
                            }
                        }
                    } else {
                        let input = self.setup_input.clone();
                        if input.trim().is_empty() {
                            self.error = Some("Enter a path, URL, or GitHub repo".into());
                        } else if let Err(e) = self.load_from(&input).await {
                            self.error = Some(format!("{e:#}"));
                            self.status = "Load failed".into();
                        } else {
                            self.error = None;
                            self.status = "Loaded · Shift+s to save as a named project".into();
                        }
                    }
                }
                KeyCode::Backspace | KeyCode::Delete => {
                    self.setup_list_focus = false;
                    self.setup_input.pop();
                }
                KeyCode::Char(_) if !mods.contains(KeyModifiers::CONTROL) => {
                    if let Some(c) = typed {
                        if matches!(c, 'q' | 'Q')
                            && self.setup_input.is_empty()
                            && !mods.contains(KeyModifiers::SHIFT)
                        {
                            self.should_quit = true;
                        } else if self.setup_list_focus
                            && matches!(c, 'j' | 'k' | 'd' | 'v' | 'J' | 'K' | 'D' | 'V')
                        {
                            // list shortcuts handled above
                        } else {
                            // Any other character goes to the source URL field.
                            self.setup_list_focus = false;
                            self.setup_input.push(c);
                            self.error = None;
                        }
                    }
                }
                _ => {}
            }
            return;
        }

        match key {
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Char('p') => {
                self.screen = Screen::Setup;
                self.setup_list_focus = true;
                self.project_store.reload();
                if let Some(id) = self.project_store.active_id.clone() {
                    if let Some(i) = self.project_store.index_of(&id) {
                        self.project_sel = i;
                    }
                }
                self.status =
                    "Project list · j/k enter · v convention · Tab/type URL · Shift+s save · d delete"
                        .into();
            }
            KeyCode::Esc => {
                // Soft escape: clear error / go overview, do not quit
                if self.error.take().is_some() {
                    self.status = "Error dismissed".into();
                } else if self.screen != Screen::Overview {
                    self.screen = Screen::Overview;
                    self.status = "Overview".into();
                } else {
                    self.status = "Overview".into();
                }
            }
            KeyCode::Tab => {
                self.next_screen().await;
            }
            KeyCode::BackTab => {
                self.prev_screen().await;
            }
            KeyCode::Char('1') => {
                self.screen = Screen::Overview;
                self.status = "Overview".into();
            }
            KeyCode::Char('2') => {
                self.screen = Screen::Heatmap;
                self.status = "Heatmap".into();
            }
            KeyCode::Char('3') => {
                self.screen = Screen::Priorities;
                self.rebuild_priorities();
                self.status = format!("Priorities · {}", self.priority_mode.label());
            }
            KeyCode::Char('4') => {
                self.screen = Screen::Prompt;
                self.rebuild_prompt().await;
                self.status = "Prompt".into();
            }
            KeyCode::Char('5') => {
                self.screen = Screen::Claims;
                self.status = format!("Claims · {}", self.claims_status);
            }
            KeyCode::Char('/') => {
                self.searching = true;
                self.screen = Screen::Overview;
                self.status = "Search".into();
            }
            KeyCode::Char('c') => {
                // Ensure disasm/draft are loaded before copy (web always has detail).
                self.rebuild_prompt().await;
                self.copy_prompt();
            }
            // g = hand batch prompt to Grok Build (Prompt page; needs batch).
            KeyCode::Char('g') if self.screen == Screen::Prompt => {
                self.rebuild_prompt().await;
                self.queue_grok_launch();
            }
            // Shift+b = clear entire batch (plain b toggles selected).
            KeyCode::Char('b') if mods.contains(KeyModifiers::SHIFT) => {
                self.clear_batch().await;
            }
            KeyCode::Char('b') => self.toggle_batch_selected().await,
            KeyCode::Char('u') => {
                if let Err(e) = self.update_progress().await {
                    self.error = Some(format!("{e:#}"));
                    self.status = "Update failed".into();
                }
            }
            KeyCode::Char('r') => {
                if let Some(db) = self.db.clone() {
                    self.refresh_claims(&db).await;
                    self.rebuild_priorities();
                    self.status = format!("Claims refreshed · {}", self.claims_status);
                }
            }
            KeyCode::Char('n') if self.screen == Screen::Priorities => {
                self.priority_mode = match self.priority_mode {
                    PriorityMode::Nearly => PriorityMode::Scaffolded,
                    PriorityMode::Scaffolded => PriorityMode::Biggest,
                    PriorityMode::Biggest => PriorityMode::Nearly,
                };
                self.rebuild_priorities();
                self.status = format!(
                    "Priority mode: {} ({} rows)",
                    self.priority_mode.label(),
                    self.priority_list.len()
                );
            }
            KeyCode::Char('m') if self.screen == Screen::Overview => {
                self.match_filter = self.match_filter.cycle();
                if let Some(db) = self.db.clone() {
                    self.rebuild_modules(&db);
                }
                self.rebuild_functions();
                self.ensure_selected_detail().await;
                self.status = format!(
                    "Overview filter: {} ({} modules · {} functions)",
                    self.match_filter.label(),
                    self.module_list.len(),
                    self.fn_list.len()
                );
            }
            KeyCode::Char('s') if self.screen == Screen::Overview => {
                self.module_sort = self.module_sort.cycle();
                if let Some(db) = self.db.clone() {
                    self.rebuild_modules(&db);
                }
                // Keep the selected module if possible; functions stay for that module.
                self.rebuild_functions();
                self.ensure_selected_detail().await;
                self.status = format!(
                    "Module sort: {} ({} modules)",
                    self.module_sort.label(),
                    self.module_list.len()
                );
            }
            KeyCode::Up | KeyCode::Char('k') => self.move_sel(-1).await,
            KeyCode::Down | KeyCode::Char('j') => self.move_sel(1).await,
            KeyCode::Left | KeyCode::Char('h') if self.screen == Screen::Overview => {
                self.move_module(-1);
                self.rebuild_functions();
                self.ensure_selected_detail().await;
            }
            KeyCode::Right | KeyCode::Char('l') if self.screen == Screen::Overview => {
                self.move_module(1);
                self.rebuild_functions();
                self.ensure_selected_detail().await;
            }
            KeyCode::Enter => {
                if self.screen == Screen::Priorities {
                    if let Some(&idx) = self.priority_list.get(self.priority_sel) {
                        if let Some(db) = &self.db {
                            let id = db.functions[idx].id.clone();
                            let module = db.functions[idx].module.clone();
                            self.selected_id = Some(id);
                            self.detail_scroll = 0;
                            // Jump to Overview so the bottom detail pane is visible.
                            if let Some(i) = self.module_list.iter().position(|m| m == &module) {
                                self.module_sel = i;
                            }
                            self.rebuild_functions();
                            self.screen = Screen::Overview;
                            self.ensure_selected_detail().await;
                            self.status = "Overview · from priorities".into();
                        }
                    }
                }
            }
            KeyCode::Char('t') if self.screen == Screen::Prompt => {
                if mods.contains(KeyModifiers::SHIFT) {
                    self.set_prompt_template_default();
                } else {
                    self.cycle_prompt_template(1);
                    self.rebuild_prompt().await;
                    self.status = format!(
                        "Template: {}  (Shift+t = set default · n = new · {})",
                        self.prompt_template_label(),
                        self.template_store.templates_dir.display()
                    );
                }
            }
            KeyCode::Char('n') if self.screen == Screen::Prompt => {
                self.naming_template = true;
                self.template_name_input = "my-template".into();
                self.status =
                    "New template id (letters/digits/-/_) · enter create & edit · esc cancel"
                        .into();
            }
            KeyCode::Char('e') if self.screen == Screen::Prompt => {
                match self.template_store.editable_path(&self.prompt_template_id) {
                    Ok(path) => {
                        self.pending_edit = Some(path.clone());
                        self.status = format!(
                            "Opening {} in {}…",
                            path.display(),
                            crate::templates::preferred_editor()
                        );
                    }
                    Err(e) => {
                        self.error = Some(format!("{e:#}"));
                        self.status = "Cannot edit this template".into();
                    }
                }
            }
            KeyCode::PageUp if self.screen == Screen::Prompt => {
                self.prompt_scroll = self.prompt_scroll.saturating_sub(5);
            }
            KeyCode::PageDown if self.screen == Screen::Prompt => {
                self.prompt_scroll = self.prompt_scroll.saturating_add(5);
            }
            KeyCode::PageUp if self.screen == Screen::Overview => {
                self.scroll_detail(-8);
            }
            KeyCode::PageDown if self.screen == Screen::Overview => {
                self.scroll_detail(8);
            }
            KeyCode::Char('[') if self.screen == Screen::Overview => {
                self.scroll_detail(-1);
            }
            KeyCode::Char(']') if self.screen == Screen::Overview => {
                self.scroll_detail(1);
            }
            // Arrow keys always move even when screen guards above don't match
            _ => {}
        }
    }

    async fn next_screen(&mut self) {
        let tabs = Screen::all_loaded();
        let i = tabs.iter().position(|s| *s == self.screen).unwrap_or(0);
        self.screen = tabs[(i + 1) % tabs.len()];
        self.on_screen_enter().await;
    }

    async fn prev_screen(&mut self) {
        let tabs = Screen::all_loaded();
        let i = tabs.iter().position(|s| *s == self.screen).unwrap_or(0);
        self.screen = tabs[(i + tabs.len() - 1) % tabs.len()];
        self.on_screen_enter().await;
    }

    async fn on_screen_enter(&mut self) {
        // Short status only — key hints live in the controls bar below.
        match self.screen {
            Screen::Prompt => {
                self.rebuild_prompt().await;
                self.status = "Prompt".into();
            }
            Screen::Priorities => {
                self.rebuild_priorities();
                self.status = format!("Priorities · {}", self.priority_mode.label());
            }
            Screen::Heatmap => {
                self.status = "Heatmap".into();
            }
            Screen::Claims => {
                self.status = format!("Claims · {}", self.claims_status);
            }
            Screen::Overview => {
                self.ensure_selected_detail().await;
                self.status = "Overview".into();
            }
            Screen::Setup => {}
        }
    }

    fn move_module(&mut self, delta: isize) {
        if self.module_list.is_empty() {
            return;
        }
        let n = self.module_list.len() as isize;
        let i = self.module_sel as isize + delta;
        let i = ((i % n) + n) % n;
        self.module_sel = i as usize;
    }

    async fn move_sel(&mut self, delta: isize) {
        match self.screen {
            Screen::Overview => {
                if self.fn_list.is_empty() {
                    return;
                }
                let n = self.fn_list.len() as isize;
                let i = self.fn_sel as isize + delta;
                let i = ((i % n) + n) % n;
                self.fn_sel = i as usize;
                self.sync_selection_from_fn();
                self.ensure_selected_detail().await;
            }
            Screen::Priorities => {
                if self.priority_list.is_empty() {
                    return;
                }
                let n = self.priority_list.len() as isize;
                let i = self.priority_sel as isize + delta;
                let i = ((i % n) + n) % n;
                self.priority_sel = i as usize;
            }
            Screen::Prompt => {
                if delta < 0 {
                    self.prompt_scroll = self.prompt_scroll.saturating_sub(1);
                } else {
                    self.prompt_scroll = self.prompt_scroll.saturating_add(1);
                }
            }
            _ => {}
        }
    }

    fn draw_heatmap(&mut self, f: &mut Frame, area: Rect) {
        if self.db.is_none() {
            return;
        }

        let title = " Heatmap ";
        let block = content_block(title, &self.theme, self.theme.border);
        let inner = block.inner(area);
        f.render_widget(block, area);
        fill_pane(f, inner, &self.theme, self.theme.bg);

        if inner.width < 4 || inner.height < 2 {
            return;
        }

        // Map body + one-line legend for whatever is selected on other pages.
        let map_h = inner.height.saturating_sub(1).max(1);
        let map = Rect {
            x: inner.x,
            y: inner.y,
            width: inner.width,
            height: map_h,
        };
        let legend = Rect {
            x: inner.x,
            y: inner.y + map_h,
            width: inner.width,
            height: 1,
        };

        let leaves: Vec<TreemapLeaf> = self
            .db
            .as_ref()
            .map(|db| db.functions.iter().map(TreemapLeaf::from).collect())
            .unwrap_or_default();
        let rects = layout_treemap(&leaves, map.width as f64, map.height as f64, None);

        let selected_id = self.selected_id.clone();
        let locked_ids: std::collections::HashSet<String> =
            self.locked_by.keys().cloned().collect();
        let batch = self.batch.clone();
        let theme_bg = self.theme.bg;
        let theme_panel = self.theme.panel;
        let theme_text = self.theme.text;
        let theme_muted = self.theme.muted;
        let theme_accent = self.theme.accent;
        let theme_matched = self.theme.matched;
        let theme_unmatched = self.theme.unmatched;
        let theme_claim = self.theme.claim;
        let theme_batch = self.theme.batch;

        let legend_text = if let Some(f) = selected_id
            .as_ref()
            .and_then(|id| self.db.as_ref().and_then(|db| db.find_by_id(id)))
        {
            let state = if f.matched {
                "matched"
            } else if locked_ids.contains(&f.id) {
                "claimed"
            } else {
                "unmatched"
            };
            let batch_s = batch
                .iter()
                .position(|x| x == &f.id)
                .map(|n| format!(" · [B{}]", n + 1))
                .unwrap_or_default();
            format!(
                " {}  {}  0x{:x}  {}B  {state}{batch_s} ",
                f.module, f.name, f.addr, f.size
            )
        } else {
            " (select a function on Overview or Priorities) ".into()
        };

        let buf = f.buffer_mut();
        let base = paint_on(theme_muted, theme_bg);
        for row in 0..map.height {
            for col in 0..map.width {
                let cell = &mut buf[(map.x + col, map.y + row)];
                cell.set_symbol(" ");
                cell.set_style(base);
            }
        }

        // Module chrome (panel fill + label).
        for r in rects.iter().filter(|r| r.is_module) {
            if let Some((cx, cy, cw, ch)) = r.cell_bounds(map.width, map.height) {
                let style = paint_on(theme_muted, theme_panel);
                for row in 0..ch {
                    for col in 0..cw {
                        let cell = &mut buf[(map.x + cx + col, map.y + cy + row)];
                        cell.set_symbol(" ");
                        cell.set_style(style);
                    }
                }
                if let Some(label) = &r.module_label {
                    if ch >= 1 && cw >= 4 {
                        let text: String = label.chars().take(cw as usize).collect();
                        let line =
                            Line::from(Span::styled(text, paint_bold_on(theme_text, theme_panel)));
                        buf.set_line(map.x + cx, map.y + cy, &line, cw);
                    }
                }
            }
        }

        // Functions: block glyphs from the first heatmap (not braille).
        for r in rects.iter().filter(|r| !r.is_module) {
            let Some((cx, cy, cw, ch)) = r.cell_bounds(map.width, map.height) else {
                continue;
            };
            let is_sel = selected_id.as_deref() == Some(r.id.as_str());
            let is_locked = locked_ids.contains(&r.id);
            let is_batch = batch.iter().any(|x| x == &r.id);
            let (fg, bg) = if is_sel {
                (theme_bg, theme_accent)
            } else if is_locked {
                (theme_bg, theme_claim)
            } else if is_batch {
                (theme_text, theme_batch)
            } else if r.matched {
                (theme_bg, theme_matched)
            } else {
                (theme_text, theme_unmatched)
            };
            let style = if is_sel {
                paint_bold_on(fg, bg)
            } else {
                paint_on(fg, bg)
            };
            let sym = if is_sel {
                "█"
            } else if r.matched {
                "▓"
            } else if is_locked {
                "▒"
            } else {
                "░"
            };
            for row in 0..ch {
                for col in 0..cw {
                    let cell = &mut buf[(map.x + cx + col, map.y + cy + row)];
                    cell.set_symbol(sym);
                    cell.set_style(style);
                }
            }
        }

        let leg_style = paint_on(theme_text, theme_panel);
        for col in 0..legend.width {
            let cell = &mut buf[(legend.x + col, legend.y)];
            cell.set_symbol(" ");
            cell.set_style(leg_style);
        }
        let line = Line::from(Span::styled(
            legend_text
                .chars()
                .take(legend.width as usize)
                .collect::<String>(),
            paint_on(theme_text, theme_panel),
        ));
        buf.set_line(legend.x, legend.y, &line, legend.width);
    }

    fn draw(&mut self, f: &mut Frame) {
        let area = f.area();
        // Entire frame solid dark first — no white terminal paper anywhere.
        fill_pane(f, area, &self.theme, self.theme.bg);

        // Header: 2 content rows (title + pages) when loaded → height 4 with borders.
        let header_h = if self.db.is_some() && self.screen != Screen::Setup {
            4
        } else {
            3
        };
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(header_h),
                Constraint::Min(5),
                Constraint::Length(5), // status + two key rows always visible
            ])
            .split(area);

        self.draw_header(f, chunks[0]);
        match self.screen {
            Screen::Setup => self.draw_setup(f, chunks[1]),
            Screen::Overview => self.draw_overview(f, chunks[1]),
            Screen::Heatmap => self.draw_heatmap(f, chunks[1]),
            Screen::Priorities => self.draw_priorities(f, chunks[1]),
            Screen::Prompt => self.draw_prompt(f, chunks[1]),
            Screen::Claims => self.draw_claims(f, chunks[1]),
        }
        self.draw_footer(f, chunks[2]);

        if self.show_help {
            self.draw_help_overlay(f, area);
        }
    }

    /// All loaded pages on one line; current page marked as a reversed chip `[name]`.
    fn pages_line(&self, bg: Color) -> Line<'static> {
        let mut spans: Vec<Span<'static>> = Vec::new();
        spans.push(Span::styled(" ", paint_on(self.theme.muted, bg)));
        for (i, screen) in Screen::all_loaded().iter().enumerate() {
            if i > 0 {
                spans.push(Span::styled(" ", paint_on(self.theme.muted, bg)));
            }
            let active = self.screen == *screen;
            let label = screen.tab_label();
            if active {
                spans.push(Span::styled(
                    format!("[{label}]"),
                    paint_bold_on(self.theme.bg, self.theme.accent),
                ));
            } else {
                spans.push(Span::styled(
                    format!(" {label} "),
                    paint_on(self.theme.muted, bg),
                ));
            }
        }
        Line::from(spans)
    }

    fn draw_header(&self, f: &mut Frame, area: Rect) {
        let bg = self.theme.bg;
        let title = if let Some(db) = &self.db {
            let gen = if db.generated_at.is_empty() {
                "—"
            } else {
                db.generated_at.as_str()
            };
            let batch_bit = if self.batch.is_empty() {
                format!("batch {}", self.batch_summary())
            } else {
                format!("batch {} ★", self.batch_summary())
            };
            // Prefer atlas project name (truth of loaded data); append profile id if set.
            let atlas = db.project_name();
            let conv = self.convention.label();
            let proj = match self.project_store.active_id.as_deref() {
                Some(id) if id != atlas => format!("{atlas} [{id}/{conv}]"),
                Some(id) => format!("{id} [{conv}]"),
                None => format!("{atlas} [{conv}]"),
            };
            format!(
                " chaos  ·  {proj}  ·  {}/{} fn ({}%)  ·  {batch_bit}  ·  gen {gen}  ·  p projects",
                db.stats.matched_functions,
                db.stats.total_functions,
                format_pct(db.stats.matched_functions, db.stats.total_functions),
            )
        } else {
            " chaos  ·  Chaos Viewer CLI  ·  projects hub  ·  press ? for help".into()
        };

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(paint_on(self.theme.border, bg))
            .style(paint_on(self.theme.text, bg));
        let inner = block.inner(area);
        f.render_widget(block, area);
        fill_pane(f, inner, &self.theme, bg);

        if self.screen == Screen::Setup || self.db.is_none() {
            f.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    title,
                    paint_bold_on(self.theme.accent, bg),
                )))
                .style(paint_on(self.theme.text, bg)),
                inner,
            );
            return;
        }

        // Row 1: project stats · Row 2: all pages with current marked
        let header_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Length(1)])
            .split(inner);
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                title,
                paint_bold_on(self.theme.accent, bg),
            )))
            .style(paint_on(self.theme.text, bg)),
            header_chunks[0],
        );
        f.render_widget(
            Paragraph::new(self.pages_line(bg)).style(paint_on(self.theme.text, bg)),
            header_chunks[1],
        );
    }

    fn draw_footer(&self, f: &mut Frame, area: Rect) {
        let bg = self.theme.panel;
        let (status_text, status_style) = if let Some(err) = &self.error {
            (
                format!("error · {err}"),
                paint_bold_on(self.theme.error, bg),
            )
        } else if self.searching {
            (
                format!(
                    "search · {}_  ·  {} matches",
                    self.search,
                    self.fn_list.len()
                ),
                paint_on(self.theme.accent, bg),
            )
        } else {
            (self.status.clone(), paint_on(self.theme.text, bg))
        };

        let block = Block::default()
            .title(" controls ")
            .borders(Borders::ALL)
            .border_style(paint_on(self.theme.border, bg))
            .style(paint_on(self.theme.text, bg));
        let inner = block.inner(area);
        f.render_widget(block, area);
        // Solid fill so Reset/default never shows as white terminal paper.
        f.render_widget(Clear, inner);
        f.render_widget(Block::default().style(paint_on(self.theme.text, bg)), inner);

        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(1),
            ])
            .split(inner);

        f.render_widget(
            Paragraph::new(Line::from(Span::styled(status_text, status_style)))
                .style(paint_on(self.theme.text, bg)),
            rows[0],
        );
        f.render_widget(
            Paragraph::new(key_line(&self.theme, &self.global_hints(), bg))
                .style(paint_on(self.theme.text, bg)),
            rows[1],
        );
        let ctx = self.context_hints();
        if ctx.is_empty() {
            f.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    "this screen · (see ? for full map)",
                    paint_on(self.theme.muted, bg),
                )))
                .style(paint_on(self.theme.text, bg)),
                rows[2],
            );
        } else {
            f.render_widget(
                Paragraph::new(key_line(&self.theme, &ctx, bg))
                    .style(paint_on(self.theme.text, bg)),
                rows[2],
            );
        }
    }

    fn draw_help_overlay(&self, f: &mut Frame, area: Rect) {
        let w = area.width.saturating_sub(8).min(78);
        let h = area.height.saturating_sub(4).min(28);
        let x = area.x + (area.width.saturating_sub(w)) / 2;
        let y = area.y + (area.height.saturating_sub(h)) / 2;
        let rect = Rect::new(x, y, w, h);
        let bg = self.theme.panel;

        let block = Block::default()
            .title(" help  ·  ? or esc to close ")
            .borders(Borders::ALL)
            .border_style(paint_on(self.theme.accent, bg))
            .style(paint_on(self.theme.text, bg));
        let inner = block.inner(rect);
        f.render_widget(block, rect);
        fill_pane(f, inner, &self.theme, bg);
        f.render_widget(
            Paragraph::new(self.help_text())
                .style(paint_on(self.theme.text, bg))
                .wrap(Wrap { trim: false }),
            inner,
        );
    }

    fn draw_setup(&self, f: &mut Frame, area: Rect) {
        let bg = self.theme.bg;
        let title = if let Some(id) = &self.pending_delete_id {
            format!(" Delete '{id}'?  y confirm · n/esc cancel ")
        } else if self.saving_project {
            format!(" Save project as: {}_ ", self.project_id_input)
        } else {
            format!(
                " Projects  ·  active: {}  ·  p anytime ",
                self.active_project_label()
            )
        };
        let block = content_block(title, &self.theme, self.theme.border);
        let inner = block.inner(area);
        f.render_widget(block, area);
        fill_pane(f, inner, &self.theme, bg);

        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(5),
                Constraint::Length(6),
                Constraint::Length(4),
            ])
            .split(inner);

        // Project list
        let list_title = if self.setup_list_focus && !self.saving_project {
            " Saved projects  [focused]  j/k enter v=convention d "
        } else {
            " Saved projects "
        };
        let list_block = content_block(list_title, &self.theme, self.theme.border);
        let list_inner = list_block.inner(rows[0]);
        f.render_widget(list_block, rows[0]);
        fill_pane(f, list_inner, &self.theme, bg);

        let mut list_lines: Vec<Line<'static>> = Vec::new();
        if self.project_store.projects.is_empty() {
            list_lines.push(Line::from(Span::styled(
                "  (none yet — type a source below, enter to load, s to save)",
                paint_on(self.theme.muted, bg),
            )));
        } else {
            for (i, p) in self.project_store.projects.iter().enumerate() {
                let selected = i == self.project_sel;
                let active = self.project_store.active_id.as_deref() == Some(p.id.as_str());
                let row_bg = if selected { self.theme.panel } else { bg };
                let mark = if selected { "› " } else { "  " };
                let star = if active { "★ " } else { "  " };
                let style = if selected {
                    paint_bold_on(self.theme.accent, row_bg)
                } else {
                    paint_on(self.theme.text, row_bg)
                };
                list_lines.push(Line::from(Span::styled(
                    format!(
                        "{mark}{star}{:<14}  [{:<12}]  {}",
                        p.id,
                        p.convention.label(),
                        p.source
                    ),
                    style,
                )));
            }
        }
        let height = list_inner.height as usize;
        let offset = self
            .project_sel
            .saturating_sub(height.saturating_sub(1))
            .min(self.project_store.projects.len().saturating_sub(height));
        for (row, line) in list_lines.iter().skip(offset).take(height).enumerate() {
            f.buffer_mut().set_line(
                list_inner.x,
                list_inner.y + row as u16,
                line,
                list_inner.width,
            );
        }

        // Source input
        let input_title = if !self.setup_list_focus && !self.saving_project {
            " Source  [focused]  path · URL · GitHub "
        } else {
            " Source  path · URL · GitHub "
        };
        let input_block = content_block(input_title, &self.theme, self.theme.border);
        let input_inner = input_block.inner(rows[1]);
        f.render_widget(input_block, rows[1]);
        fill_pane(f, input_inner, &self.theme, bg);
        let input_line = format!("> {}_", self.setup_input);
        f.render_widget(
            Paragraph::new(input_line)
                .style(paint_on(self.theme.text, bg))
                .wrap(Wrap { trim: false }),
            input_inner,
        );

        // Help strip
        let help = if self.pending_delete_id.is_some() {
            "y = permanently remove this saved profile · n or esc = keep it"
        } else if self.saving_project {
            "Enter id · enter save · esc cancel"
        } else {
            "j/k list · enter load · v convention (default|experimental) · Tab/type URL · Shift+s save · d delete · q quit"
        };
        f.render_widget(
            Paragraph::new(help)
                .style(paint_on(self.theme.muted, bg))
                .wrap(Wrap { trim: true }),
            rows[2],
        );
    }

    fn draw_overview(&mut self, f: &mut Frame, area: Rect) {
        let Some(db) = &self.db else { return };

        // Top: modules | functions. Bottom: detail spanning full width.
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
            .split(area);
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(28), Constraint::Percentage(72)])
            .split(rows[0]);

        // Module lines
        let mod_height = cols[0].height.saturating_sub(2) as usize;
        Self::clamp_scroll(
            self.module_sel,
            &mut self.module_offset,
            self.module_list.len(),
            mod_height,
        );
        let mod_lines: Vec<Line<'static>> = self
            .module_list
            .iter()
            .enumerate()
            .map(|(i, m)| {
                let (matched, total) = self.module_counts.get(i).copied().unwrap_or((0, 0));
                let selected = i == self.module_sel;
                let bg = if selected {
                    self.theme.panel
                } else {
                    self.theme.bg
                };
                let done = total > 0 && matched == total;
                let fg = if selected {
                    self.theme.accent
                } else if done {
                    self.theme.matched
                } else {
                    self.theme.text
                };
                let mark = if selected { "› " } else { "  " };
                let style = if selected {
                    paint_bold_on(fg, bg)
                } else {
                    paint_on(fg, bg)
                };
                let counts = format!("{matched}/{total}");
                Line::from(Span::styled(format!("{mark}{m}  {counts}"), style))
            })
            .collect();
        let mod_title = match self.match_filter {
            MatchFilter::MatchedOnly => {
                format!(
                    " Modules  (matches · {} · s/h/l) ",
                    self.module_sort.short()
                )
            }
            MatchFilter::All | MatchFilter::UnmatchedOnly => {
                format!(" Modules  ({} · s/h/l) ", self.module_sort.short())
            }
        };
        Self::draw_line_list(
            f,
            cols[0],
            mod_title,
            &self.theme,
            &mod_lines,
            self.module_offset,
        );

        // Function lines
        let fn_height = cols[1].height.saturating_sub(2) as usize;
        Self::clamp_scroll(
            self.fn_sel,
            &mut self.fn_offset,
            self.fn_list.len(),
            fn_height,
        );
        let fn_lines: Vec<Line<'static>> = self
            .fn_list
            .iter()
            .enumerate()
            .map(|(list_i, &idx)| {
                let f = &db.functions[idx];
                let selected = list_i == self.fn_sel;
                let bg = if selected {
                    self.theme.panel
                } else {
                    self.theme.bg
                };
                let badge = if f.matched {
                    "M"
                } else if self.locked_by.contains_key(&f.id) {
                    "L"
                } else if f.div.is_some() {
                    "N"
                } else {
                    "U"
                };
                let badge_color = if f.matched {
                    self.theme.matched
                } else if self.locked_by.contains_key(&f.id) {
                    self.theme.claim
                } else if f.div.is_some() {
                    self.theme.key
                } else {
                    self.theme.unmatched
                };
                let in_batch = self.batch_index(&f.id).is_some();
                let name_fg = if selected {
                    self.theme.accent
                } else if in_batch {
                    self.theme.batch
                } else {
                    self.theme.text
                };
                let name_style = if selected || in_batch {
                    paint_bold_on(name_fg, bg)
                } else {
                    paint_on(name_fg, bg)
                };
                let mark = if selected { "› " } else { "  " };
                let mut spans = vec![
                    Span::styled(mark.to_string(), paint_on(self.theme.accent, bg)),
                    Span::styled(format!("[{badge}] "), paint_on(badge_color, bg)),
                ];
                spans.extend(self.batch_badge_spans(&f.id, bg));
                spans.push(Span::styled(
                    format!("{}  0x{:x}  {}B", f.name, f.addr, f.size),
                    name_style,
                ));
                Line::from(spans)
            })
            .collect();
        let batched_visible = self
            .fn_list
            .iter()
            .filter(|&&idx| self.batch_index(&db.functions[idx].id).is_some())
            .count();
        let filter = self.match_filter.label();
        let title = if self.search.is_empty() {
            format!(
                " Functions ({}) · {filter} · batch {} ({} here) · m filter · j/k / b ",
                self.fn_list.len(),
                self.batch_summary(),
                batched_visible
            )
        } else {
            format!(
                " Functions ({}) · {filter} · /{} · batch {} · m · esc done ",
                self.fn_list.len(),
                self.search,
                self.batch_summary()
            )
        };
        Self::draw_line_list(f, cols[1], title, &self.theme, &fn_lines, self.fn_offset);

        // Detail strip under both lists.
        self.draw_detail_pane(f, rows[1]);
    }

    fn draw_priorities(&mut self, f: &mut Frame, area: Rect) {
        let Some(db) = &self.db else { return };
        let title = format!(
            " {}  ·  {} rows  ·  batch {}  ·  n cycle · enter · b ",
            self.priority_mode.label(),
            self.priority_list.len(),
            self.batch_summary()
        );
        let height = area.height.saturating_sub(2) as usize;
        Self::clamp_scroll(
            self.priority_sel,
            &mut self.priority_offset,
            self.priority_list.len(),
            height,
        );
        let lines: Vec<Line<'static>> = self
            .priority_list
            .iter()
            .enumerate()
            .map(|(list_i, &idx)| {
                let f = &db.functions[idx];
                let selected = list_i == self.priority_sel;
                let bg = if selected {
                    self.theme.panel
                } else {
                    self.theme.bg
                };
                let extra = match self.priority_mode {
                    PriorityMode::Nearly => format!("div={}", f.div.unwrap_or(0)),
                    PriorityMode::Scaffolded => format!("sim={:.2}", f.sim.unwrap_or(0.0)),
                    PriorityMode::Biggest => format!("{}B", f.size),
                };
                let in_batch = self.batch_index(&f.id).is_some();
                let name_fg = if selected {
                    self.theme.accent
                } else if in_batch {
                    self.theme.batch
                } else {
                    self.theme.text
                };
                let name_style = if selected || in_batch {
                    paint_bold_on(name_fg, bg)
                } else {
                    paint_on(name_fg, bg)
                };
                let mark = if selected { "› " } else { "  " };
                let mut spans = vec![Span::styled(
                    mark.to_string(),
                    paint_on(self.theme.accent, bg),
                )];
                spans.extend(self.batch_badge_spans(&f.id, bg));
                spans.push(Span::styled(
                    format!("{}  {}  0x{:x}  {extra}", f.module, f.name, f.addr),
                    name_style,
                ));
                Line::from(spans)
            })
            .collect();
        Self::draw_line_list(f, area, title, &self.theme, &lines, self.priority_offset);
    }

    /// Detail panel used under Overview (modules + functions).
    fn draw_detail_pane(&mut self, f: &mut Frame, area: Rect) {
        let bg = self.theme.bg;
        let panel = self.theme.panel;
        let has_fn = self.selected_function().is_some();
        let batched = self
            .selected_function()
            .and_then(|f| self.batch_index(&f.id));
        let footer = self.detail_batch_footer();

        let body = self.detail_pane_lines().join("\n");
        let total = self.detail_lines_cache.len();

        // Border + sticky footer row; scroll uses the remaining height only.
        let inner_h = area.height.saturating_sub(2); // border
        let footer_h: u16 = if inner_h >= 2 { 1 } else { 0 };
        let body_h = inner_h.saturating_sub(footer_h).max(1);
        self.detail_view_h = body_h;
        let max_scroll = total.saturating_sub(body_h as usize);
        if self.detail_scroll as usize > max_scroll {
            self.detail_scroll = max_scroll as u16;
        }
        let scroll = self.detail_scroll as usize;
        let body_h_usize = body_h as usize;

        let title = if !has_fn {
            " Detail ".into()
        } else {
            format!(
                " Detail  ·  lines {}–{}/{}  ·  pgup/pgdn · [ ] ",
                scroll + 1,
                (scroll + body_h_usize).min(total).max(scroll + 1),
                total
            )
        };
        let border = if batched.is_some() {
            self.theme.batch
        } else {
            self.theme.border
        };
        let block = content_block(title, &self.theme, border);
        let inner = block.inner(area);
        f.render_widget(block, area);
        fill_pane(f, inner, &self.theme, bg);

        let chunks = if footer_h > 0 {
            Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Min(1), Constraint::Length(footer_h)])
                .split(inner)
        } else {
            Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Min(1)])
                .split(inner)
        };

        let style = if has_fn {
            paint_on(self.theme.text, bg)
        } else {
            paint_on(self.theme.muted, bg)
        };
        f.render_widget(
            Paragraph::new(body)
                .style(style)
                .wrap(Wrap { trim: false })
                .scroll((scroll as u16, 0)),
            chunks[0],
        );

        if footer_h > 0 && chunks.len() > 1 {
            let foot_area = chunks[1];
            let foot_fg = if batched.is_some() {
                self.theme.batch
            } else {
                self.theme.muted
            };
            let foot_style = paint_bold_on(foot_fg, panel);
            let buf = f.buffer_mut();
            for col in 0..foot_area.width {
                let cell = &mut buf[(foot_area.x + col, foot_area.y)];
                cell.set_symbol(" ");
                cell.set_style(paint_on(foot_fg, panel));
            }
            let text: String = footer.chars().take(foot_area.width as usize).collect();
            buf.set_line(
                foot_area.x,
                foot_area.y,
                &Line::from(Span::styled(text, foot_style)),
                foot_area.width,
            );
        }
    }

    fn draw_prompt(&self, f: &mut Frame, area: Rect) {
        let bg = self.theme.bg;
        let roster: String = if self.batch.is_empty() {
            "batch empty — press b on Overview or Priorities to add functions".into()
        } else if let Some(db) = &self.db {
            self.batch
                .iter()
                .enumerate()
                .filter_map(|(i, id)| {
                    db.find_by_id(id)
                        .map(|f| format!("[B{}] {}", i + 1, f.name))
                })
                .collect::<Vec<_>>()
                .join("  ·  ")
        } else {
            format!("batch {}", self.batch_summary())
        };

        let title = if self.naming_template {
            format!(
                " Prompt  ·  new template id: {}_  ·  enter create · esc ",
                self.template_name_input
            )
        } else {
            format!(
                " Prompt  ·  {}  ·  batch {}  ·  t · n new · e edit · S-t default · c ",
                self.prompt_template_label(),
                self.batch_summary()
            )
        };
        let border = if self.batch.is_empty() {
            self.theme.border
        } else {
            self.theme.batch
        };
        let block = content_block(title, &self.theme, border);
        let inner = block.inner(area);
        f.render_widget(block, area);
        fill_pane(f, inner, &self.theme, bg);

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(2), Constraint::Min(1)])
            .split(inner);

        let roster_fg = if self.batch.is_empty() {
            self.theme.muted
        } else {
            self.theme.batch
        };
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                roster,
                paint_bold_on(roster_fg, bg),
            )))
            .style(paint_on(self.theme.text, bg))
            .wrap(Wrap { trim: true }),
            chunks[0],
        );
        f.render_widget(
            Paragraph::new(self.prompt_text.as_str())
                .style(paint_on(self.theme.text, bg))
                .wrap(Wrap { trim: false })
                .scroll((self.prompt_scroll, 0)),
            chunks[1],
        );
    }

    fn draw_claims(&self, f: &mut Frame, area: Rect) {
        let bg = self.theme.bg;
        let block = content_block(" Claims (read-only) ", &self.theme, self.theme.border);
        let inner = block.inner(area);
        f.render_widget(block, area);
        fill_pane(f, inner, &self.theme, bg);

        let mut lines = vec![
            format!("status: {}", self.claims_status),
            format!("locked functions: {}", self.locked_by.len()),
            String::new(),
            "Keys: r refresh · 1-5 screens · ? help · q quit".into(),
            String::new(),
        ];
        let mut entries: Vec<_> = self.locked_by.iter().collect();
        entries.sort_by(|a, b| a.0.cmp(b.0));
        for (id, handle) in entries.into_iter().take(40) {
            lines.push(format!("{handle:16}  {id}"));
        }
        if self.locked_by.is_empty() {
            lines.push("No active locks right now.".into());
            lines.push("Claims are optional: they appear when project.claimsApi is set,".into());
            lines.push("or when CLAIMS.md on the repo has active rows.".into());
            lines.push("Empty / placeholder tables are normal and not an error.".into());
        }
        f.render_widget(
            Paragraph::new(lines.join("\n")).style(paint_on(self.theme.text, bg)),
            inner,
        );
    }
}

/// Run the interactive TUI. Optional initial input loads immediately.
pub async fn run(
    input: Option<String>,
    repo: Option<String>,
    branch: Option<String>,
    project: Option<String>,
) -> Result<()> {
    let claims_session = ClaimsSession::from_env();
    let mut app = App::new(claims_session)?;

    if let Some(project) = project {
        if let Err(e) = app.load_project_by_id(&project).await {
            // Fall through to setup with error if profile missing.
            app.error = Some(format!("{e:#}"));
            app.status = "Project load failed · pick another".into();
        }
    } else if let Some(repo) = repo {
        if let Err(e) = app.load_from_with_branch(&repo, branch.as_deref()).await {
            app.error = Some(format!("{e:#}"));
            app.status = "Repo load failed".into();
        }
    } else if let Some(input) = input {
        app.load_from(&input).await?;
    } else if let Some(id) = app.project_store.active_id.clone() {
        // Resume last project silently; stay on Setup if it fails.
        if let Err(e) = app.load_project_by_id(&id).await {
            app.error = Some(format!("last project '{id}': {e:#}"));
            app.status = "Could not load last project · choose or enter a source".into();
        }
    }

    enable_raw_mode()?;
    let mut stdout = stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let mut terminal = Terminal::new(ratatui::backend::CrosstermBackend::new(stdout))?;

    let result = run_loop(&mut terminal, &mut app).await;

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    result
}

async fn run_loop(
    terminal: &mut Terminal<ratatui::backend::CrosstermBackend<io::Stdout>>,
    app: &mut App,
) -> Result<()> {
    loop {
        terminal.draw(|f| app.draw(f))?;
        if event::poll(Duration::from_millis(16))? {
            // Drain the queue. Coalesce rapid j/k/↑/↓ on list screens so holding
            // a key only pays one selection update (+ one detail load) per frame.
            let mut nav_delta: isize = 0;
            let mut other_keys: Vec<KeyEvent> = Vec::new();
            while event::poll(Duration::from_millis(0))? {
                match event::read()? {
                    Event::Key(key) => {
                        if key.kind == KeyEventKind::Release {
                            continue;
                        }
                        let code = match key.code {
                            KeyCode::Char(c) => KeyCode::Char(c.to_ascii_lowercase()),
                            other => other,
                        };
                        let coalescable = matches!(
                            app.screen,
                            Screen::Overview | Screen::Priorities | Screen::Prompt
                        ) && !app.searching
                            && !app.naming_template
                            && !app.show_help
                            && matches!(
                                code,
                                KeyCode::Char('j')
                                    | KeyCode::Char('k')
                                    | KeyCode::Up
                                    | KeyCode::Down
                            );
                        if coalescable {
                            match code {
                                KeyCode::Char('j') | KeyCode::Down => nav_delta += 1,
                                KeyCode::Char('k') | KeyCode::Up => nav_delta -= 1,
                                _ => {}
                            }
                        } else {
                            // Flush pending nav before other keys so order stays sane.
                            if nav_delta != 0 {
                                app.move_sel(nav_delta).await;
                                nav_delta = 0;
                            }
                            other_keys.push(key);
                        }
                    }
                    Event::Resize(_, _) => {}
                    _ => {}
                }
            }
            if nav_delta != 0 {
                app.move_sel(nav_delta).await;
            }
            for key in other_keys {
                app.on_key(key).await;
            }
        }

        // Suspend TUI, open external editor for a new/edited template, resume.
        if let Some(path) = app.pending_edit.take() {
            disable_raw_mode()?;
            execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
            terminal.show_cursor()?;
            let edit_result = crate::templates::open_in_editor(&path);
            execute!(terminal.backend_mut(), EnterAlternateScreen)?;
            enable_raw_mode()?;
            terminal.hide_cursor()?;
            terminal.clear()?;
            app.template_store.reload();
            if app.template_store.get(&app.prompt_template_id).is_none() {
                app.prompt_template_id = app.template_store.default_id().to_string();
            }
            app.rebuild_prompt().await;
            match edit_result {
                Ok(()) => {
                    app.status = format!(
                        "Template ready · {} · t to cycle",
                        path.file_stem().and_then(|s| s.to_str()).unwrap_or("?")
                    );
                }
                Err(e) => {
                    app.error = Some(format!("editor: {e:#}"));
                    app.status = format!("File left at {}", path.display());
                }
            }
        }

        // Suspend TUI, run Grok Build with the batch prompt, resume.
        if let Some(launch) = app.pending_grok.take() {
            disable_raw_mode()?;
            execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
            terminal.show_cursor()?;
            eprintln!(
                "chaos → Grok Build ({}) · prompt {}\n",
                launch.mode.as_str(),
                launch.prompt_path.display()
            );
            let result = launch.run_foreground();
            execute!(terminal.backend_mut(), EnterAlternateScreen)?;
            enable_raw_mode()?;
            terminal.hide_cursor()?;
            terminal.clear()?;
            match result {
                Ok(()) => {
                    app.status = format!(
                        "Grok finished · prompt kept at {}",
                        launch.prompt_path.display()
                    );
                }
                Err(e) => {
                    app.error = Some(format!("grok: {e:#}"));
                    app.status = format!(
                        "Grok failed · prompt at {} (also on clipboard)",
                        launch.prompt_path.display()
                    );
                }
            }
        }

        if app.should_quit {
            break;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stats(pairs: &[(&str, usize, usize, u64)]) -> HashMap<String, ModuleAgg> {
        pairs
            .iter()
            .map(|(name, matched, total, bytes)| {
                (
                    (*name).to_string(),
                    ModuleAgg {
                        matched: *matched,
                        total: *total,
                        bytes: *bytes,
                    },
                )
            })
            .collect()
    }

    #[test]
    fn module_sort_best_worst_by_unmatched_left() {
        // open: a=1, b=98, c=0 — worst is b (most left); best is c (all matched).
        let st = stats(&[("a", 1, 2, 10), ("b", 2, 100, 20), ("c", 5, 5, 5)]);
        let mut mods = vec!["a".into(), "b".into(), "c".into()];

        sort_modules(&mut mods, &st, ModuleSort::OpenDesc);
        assert_eq!(mods, vec!["b", "a", "c"]); // 98, 1, 0 unmatched

        sort_modules(&mut mods, &st, ModuleSort::OpenAsc);
        assert_eq!(mods, vec!["c", "a", "b"]); // 0, 1, 98 unmatched
    }

    #[test]
    fn module_sort_name_count_bytes() {
        let st = stats(&[("mid", 1, 10, 50), ("big", 1, 30, 10), ("tiny", 1, 2, 100)]);
        let mut mods = vec!["mid".into(), "big".into(), "tiny".into()];

        sort_modules(&mut mods, &st, ModuleSort::Name);
        assert_eq!(mods, vec!["big", "mid", "tiny"]);

        sort_modules(&mut mods, &st, ModuleSort::Count);
        assert_eq!(mods, vec!["big", "mid", "tiny"]); // 30, 10, 2

        sort_modules(&mut mods, &st, ModuleSort::Bytes);
        assert_eq!(mods, vec!["tiny", "mid", "big"]); // 100, 50, 10
    }

    #[test]
    fn module_sort_cycles() {
        let mut m = ModuleSort::Name;
        for expected in [
            ModuleSort::OpenDesc,
            ModuleSort::OpenAsc,
            ModuleSort::Count,
            ModuleSort::Bytes,
            ModuleSort::Name,
        ] {
            m = m.cycle();
            assert_eq!(m, expected);
        }
    }
}
