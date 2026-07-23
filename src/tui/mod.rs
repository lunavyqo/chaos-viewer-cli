//! Interactive full-screen TUI.

mod theme;

use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{self, stdout};
use std::sync::{Arc, Mutex};
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

use crate::claims::{
    clear_saved_auth, load_claims, load_my_claims, load_saved_auth, merge_locked_map,
    open_in_browser, save_auth, session_from_gh_cli, ClaimsClient, ClaimsSession, MyClaimRecord,
};
use crate::clipboard::copy_text;
use crate::conventions::Convention;
use crate::discover::sources_equivalent;
use crate::grok_launch::{
    launch_agent_tagged, resolve_repo_cwd, AgentKind, GrokLaunchMode, TerminalHost,
};
use crate::load::{
    details_base_from_source, ensure_module_chunk, load_chaos_db_opts, load_function_detail,
    DataSource, DetailCache, DETAIL_PREWARM_CONCURRENCY,
};
use crate::prioritize::{priority_rows, PriorityMode};
use crate::projects::{ProjectProfile, ProjectStore};
use crate::prompt::{batch_max, PromptOptions};
use crate::schema::{format_pct, ChaosDb, ChaosFunction, FunctionDetail, ProjectConfig};
use crate::templates::{TemplateStore, BUILTIN_EXPERIMENTAL_ID, BUILTIN_ID, PROVENANCE_MODELS};
use crate::tools_catalog::{
    filtered_indices, tool_found_path, tool_present, ToolCategory, TOOL_CARDS,
};
use theme::Theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Screen {
    Setup,
    Overview,
    Priorities,
    Prompt,
    Claims,
    /// Decomp instrument catalog (cards).
    Tools,
}

/// Filter for the Tools page: all categories or one bucket.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum ToolsFilter {
    #[default]
    All,
    Category(ToolCategory),
}

impl ToolsFilter {
    fn label(self) -> String {
        match self {
            Self::All => "all".into(),
            Self::Category(c) => c.label().into(),
        }
    }

    fn cycle(self) -> Self {
        match self {
            Self::All => Self::Category(ToolCategory::Core),
            Self::Category(c) => {
                let next = c.cycle();
                // After full cycle of categories, back to All.
                if next == ToolCategory::Core && c == ToolCategory::Optional {
                    Self::All
                } else {
                    Self::Category(next)
                }
            }
        }
    }

    fn as_category(self) -> Option<ToolCategory> {
        match self {
            Self::All => None,
            Self::Category(c) => Some(c),
        }
    }
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
            Screen::Priorities,
            Screen::Prompt,
            Screen::Claims,
            Screen::Tools,
        ]
    }

    /// Short page name (no number).
    fn name(self) -> &'static str {
        match self {
            Screen::Setup => "Setup",
            Screen::Overview => "Overview",
            Screen::Priorities => "Priorities",
            Screen::Prompt => "Prompt",
            Screen::Claims => "Claims",
            Screen::Tools => "Tools",
        }
    }

    /// Hotkey digit for loaded pages (1–5).
    fn hotkey(self) -> Option<char> {
        match self {
            Screen::Overview => Some('1'),
            Screen::Priorities => Some('2'),
            Screen::Prompt => Some('3'),
            Screen::Claims => Some('4'),
            Screen::Tools => Some('5'),
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

/// ASCII case-insensitive substring check without allocating a lowercased haystack.
///
/// `needle` must already be lowercased (e.g. from `search.to_ascii_lowercase()`).
fn contains_ignore_ascii_case(haystack: &str, needle_lower: &str) -> bool {
    if needle_lower.is_empty() {
        return true;
    }
    let h = haystack.as_bytes();
    let n = needle_lower.as_bytes();
    if n.len() > h.len() {
        return false;
    }
    h.windows(n.len()).any(|window| {
        window
            .iter()
            .zip(n.iter())
            .all(|(&a, &b)| a.to_ascii_lowercase() == b)
    })
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

/// Build a key-hint line that stops before overflowing `max_w` columns.
/// Remaining hints collapse to a short `…?` so important early keys stay visible.
fn key_line_fit(theme: &Theme, hints: &[KeyHint], bg: Color, max_w: u16) -> Line<'static> {
    let max = max_w as usize;
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut used = 0usize;
    let ellipsis = "  …?";
    let ellipsis_w = ellipsis.chars().count();

    for (i, h) in hints.iter().enumerate() {
        let sep_w = if i > 0 { 2 } else { 0 };
        let piece_w = sep_w + h.key.chars().count() + 1 + h.action.chars().count();
        let more_after = i + 1 < hints.len();
        let budget = if more_after {
            max.saturating_sub(ellipsis_w)
        } else {
            max
        };
        if used + piece_w > budget && i > 0 {
            spans.push(Span::styled(
                ellipsis.to_string(),
                paint_on(theme.muted, bg),
            ));
            break;
        }
        if i > 0 {
            spans.push(Span::styled("  ", paint_on(theme.muted, bg)));
            used += 2;
        }
        spans.push(Span::styled(
            h.key.to_string(),
            paint_bold_on(theme.key, bg),
        ));
        spans.push(Span::styled(
            format!(" {}", h.action),
            paint_on(theme.muted, bg),
        ));
        used += h.key.chars().count() + 1 + h.action.chars().count();
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
    /// Editing selected profile's local decomp path (for Grok `--cwd`).
    editing_local_repo: bool,
    local_repo_input: String,
    /// Pending delete: project id awaiting y/n confirmation.
    pending_delete_id: Option<String>,
    /// Active data-tracking convention (from loaded project, else Default).
    convention: Convention,
    status: String,
    error: Option<String>,
    db: Option<ChaosDb>,
    source: Option<DataSource>,
    client: Client,
    /// Shared with background prewarm tasks (module detail chunks).
    detail_cache: Option<Arc<DetailCache>>,
    /// Modules currently being fetched/parsed in `tokio::spawn` workers.
    detail_in_flight: Arc<Mutex<HashSet<String>>>,
    /// Remaining modules to warm in the background (front = highest priority).
    prewarm_queue: VecDeque<String>,
    /// How many modules were queued for this atlas (progress denominator).
    prewarm_total: usize,
    /// True while the selected function's module chunk is still loading.
    waiting_for_detail: bool,
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
    /// `module` → indices into `db.functions` (module switch without full atlas scan).
    module_index: HashMap<String, Vec<usize>>,
    match_filter: MatchFilter,
    module_sort: ModuleSort,
    priority_mode: PriorityMode,
    priority_list: Vec<usize>,
    priority_sel: usize,
    priority_offset: usize,
    selected_id: Option<String>,
    detail: Option<FunctionDetail>,
    /// Mass batcher: one or more slots of ≤[`batch_max`] function ids.
    /// Active slot drives Prompt / copy; `,` / `.` switch; overflow creates a new slot.
    batches: Vec<Vec<String>>,
    /// Index into [`Self::batches`] (always valid while `batches` is non-empty).
    active_batch: usize,
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
    /// Attach stored near-miss / NONMATCHING C drafts to prompts (from details).
    include_near_miss_draft: bool,
    /// Attach Ghidra decompiler C to prompts when available (local ghidra_out / detail).
    include_ghidra_draft: bool,
    /// When true, `template_name_input` is editing a new template id.
    naming_template: bool,
    template_name_input: String,
    /// After leaving the TUI briefly, open this path in $EDITOR / nano.
    pending_edit: Option<std::path::PathBuf>,
    claims_session: Option<ClaimsSession>,
    /// Locks we acquired (renew / release), persisted with the session.
    my_claims: Vec<MyClaimRecord>,
    /// Claims page: paste API key / session token after `i` fails auto sign-in.
    claims_paste_open: bool,
    claims_paste_buf: String,
    show_help: bool,
    /// Centered agent picker (Prompt · Shift+g).
    agent_picker_open: bool,
    agent_picker_sel: usize,
    /// Centered model picker (Prompt · m) for MATCH_RESULT prefill.
    model_picker_open: bool,
    model_picker_sel: usize,
    /// Tools page: filtered catalog indices into TOOL_CARDS.
    tools_filter: ToolsFilter,
    tools_indices: Vec<usize>,
    tools_sel: usize,
    /// Scroll offset in card rows (each row = up to 2 cards).
    tools_row_offset: usize,
    should_quit: bool,
}

impl App {
    fn new(claims_session: Option<ClaimsSession>) -> Result<Self> {
        let client = crate::http::build_client()?;
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
            editing_local_repo: false,
            local_repo_input: String::new(),
            pending_delete_id: None,
            convention: initial_convention,
            status:
                "Project list · j/k enter · v convention · r local path · Shift+s save · d delete"
                    .into(),
            error: None,
            db: None,
            source: None,
            client,
            detail_cache: None,
            detail_in_flight: Arc::new(Mutex::new(HashSet::new())),
            prewarm_queue: VecDeque::new(),
            prewarm_total: 0,
            waiting_for_detail: false,
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
            module_index: HashMap::new(),
            match_filter: MatchFilter::All,
            module_sort: ModuleSort::Name,
            priority_mode: PriorityMode::Nearly,
            priority_list: Vec::new(),
            priority_sel: 0,
            priority_offset: 0,
            selected_id: None,
            detail: None,
            batches: vec![Vec::new()],
            active_batch: 0,
            prompt_scroll: 0,
            detail_scroll: 0,
            detail_view_h: 8,
            detail_lines_cache: Vec::new(),
            detail_lines_key: String::new(),
            prompt_text: String::new(),
            template_store: TemplateStore::load(),
            prompt_template_id: String::new(),
            include_near_miss_draft: true,
            include_ghidra_draft: true,
            naming_template: false,
            template_name_input: String::new(),
            pending_edit: None,
            claims_session,
            my_claims: load_my_claims(),
            claims_paste_open: false,
            claims_paste_buf: String::new(),
            show_help: false,
            agent_picker_open: false,
            agent_picker_sel: 0,
            model_picker_open: false,
            model_picker_sel: 0,
            tools_filter: ToolsFilter::All,
            tools_indices: filtered_indices(None),
            tools_sel: 0,
            tools_row_offset: 0,
            should_quit: false,
        };
        app.prompt_template_id = app.template_store.default_id().to_string();
        app.sync_template_to_convention();
        Ok(app)
    }

    fn global_hints(&self) -> Vec<KeyHint> {
        if self.agent_picker_open {
            return vec![
                KeyHint {
                    key: "j/k",
                    action: "select agent",
                },
                KeyHint {
                    key: "enter",
                    action: "launch",
                },
                KeyHint {
                    key: "d",
                    action: "set default",
                },
                KeyHint {
                    key: "esc",
                    action: "close",
                },
            ];
        }
        if self.model_picker_open {
            return vec![
                KeyHint {
                    key: "j/k",
                    action: "select model",
                },
                KeyHint {
                    key: "enter",
                    action: "use model",
                },
                KeyHint {
                    key: "esc",
                    action: "close",
                },
            ];
        }
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
            if self.editing_local_repo {
                return vec![
                    KeyHint {
                        key: "type",
                        action: "local path",
                    },
                    KeyHint {
                        key: "enter",
                        action: "save path",
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
                    key: "r",
                    action: "local repo",
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
        // Keep this row short: one terminal line. Prefer high-traffic keys first;
        // rarer ones live in `?` help. Batch clear keys stay near the front of
        // the batch cluster so they do not fall off the right edge.
        match self.screen {
            Screen::Overview => vec![
                KeyHint {
                    key: "j/k",
                    action: "fn",
                },
                KeyHint {
                    key: "h/l",
                    action: "mod",
                },
                KeyHint {
                    key: "m/s",
                    action: "filter/sort",
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
                    key: ",/.+",
                    action: "slots",
                },
                KeyHint {
                    key: "S-b",
                    action: "clear-all",
                },
            ],
            Screen::Priorities => vec![
                KeyHint {
                    key: "j/k",
                    action: "move",
                },
                KeyHint {
                    key: "n",
                    action: "list",
                },
                KeyHint {
                    key: "enter",
                    action: "overview",
                },
                KeyHint {
                    key: "b",
                    action: "batch",
                },
                KeyHint {
                    key: ",/.+",
                    action: "slots",
                },
                KeyHint {
                    key: "S-b",
                    action: "clear-all",
                },
            ],
            Screen::Prompt => vec![
                KeyHint {
                    key: "j/k",
                    action: "scroll",
                },
                KeyHint {
                    key: "t",
                    action: "template",
                },
                KeyHint {
                    key: "m",
                    action: "model",
                },
                KeyHint {
                    key: "d/h",
                    action: "drafts",
                },
                KeyHint {
                    key: "c",
                    action: "copy",
                },
                KeyHint {
                    key: "g",
                    action: "launch",
                },
                KeyHint {
                    key: "S-g",
                    action: "agents",
                },
                KeyHint {
                    key: ",/.+",
                    action: "slots",
                },
                KeyHint {
                    key: "S-b",
                    action: "clear-all",
                },
            ],
            Screen::Claims => vec![
                KeyHint {
                    key: "i/o",
                    action: "sign in/out",
                },
                KeyHint {
                    key: "L",
                    action: "claim sel",
                },
                KeyHint {
                    key: "A",
                    action: "claim all",
                },
                KeyHint {
                    key: "y/x",
                    action: "renew/release",
                },
                KeyHint {
                    key: "r",
                    action: "refresh",
                },
            ],
            Screen::Tools => vec![
                KeyHint {
                    key: "j/k",
                    action: "cards",
                },
                KeyHint {
                    key: "h/l",
                    action: "column",
                },
                KeyHint {
                    key: "n",
                    action: "filter",
                },
                KeyHint {
                    key: "r",
                    action: "rescan",
                },
            ],
            Screen::Setup => Vec::new(),
        }
    }

    fn help_text(&self) -> String {
        r#"chaos — keyboard reference

GLOBAL
  ?           toggle this help
  q           quit
  tab / S-tab next / previous screen
  1 2 3 4 5   Overview · Priorities · Prompt · Claims · Tools
  p           projects hub (switch / add / remove saved repos)
  u           update progress (re-fetch chaos-db; matches can land mid-session)
  r           refresh claims only
  L           claim selected function on project.claimsApi (e.g. tangos.dev)
  A           claim every function in ALL mass-batcher slots (not only active)
  c           copy active-batch prompt to clipboard (no-op if active empty)
  g           launch default agent for ALL non-empty batches (Prompt)
              · each batch opens a separate terminal window · Shift+g agent picker
  b           add/remove selected function (per-batch max 16; overflow opens a new batch)
  , / .       previous / next batch slot (mass batcher · also < / >)
  + / =       open a new empty batch after the active slot and switch to it
  Shift+b     clear ALL mass-batcher slots (every badge gone · one empty slot)
  Ctrl+b      same as Shift+b (alias; may be eaten by tmux — prefer Shift+b)
  Shift+Del   clear active slot only
              Prompt page uses the active batch only (not the Overview cursor)
              badges: [B3] in a single batch, or [2:3] = batch 2 slot 3 when multi

OVERVIEW
  top: modules (h/l) · functions (j/k) · m match filter · s module sort · / search
              unmatched filter hides fully matched modules
              sort: name · worst/best by unmatched left · most fns · most bytes
  bottom: detail pane for the selected function (loads as you move)
  pgup/pgdn   scroll the detail pane (j/k still move the function list)
  [ / ]       scroll detail one line
  b           toggle batch for selected function
  , / .       switch mass-batcher slot
  + / =       new empty batch
  Shift+b     clear ALL batches
  Shift+Del   clear active slot only

PRIORITIES
  n           cycle Nearly / Scaffolded / Biggest / Smallest
  j / k       move in ranked list
  enter       jump to Overview with that function selected
  , / .       switch mass-batcher slot
  + / =       new empty batch

PROMPT
  j / k       scroll prompt text
  pgup/pgdn   scroll prompt by page
  t           next prompt template (builtins + ~/.config/chaos/templates)
              builtins: chaos-viewer (match + provenance) · chaos-experimental (alias)
  n           new template (copy of chaos-viewer → editor)
  e           edit current user template in $EDITOR / nano
  Shift+t     set current template as default
  m           model picker (fixed list · j/k · enter select · esc close)
  y           cycle reasoning / thinking level (high · medium · low · none)
  w           cycle harness preset (grok-build · cursor-agent · claude-code · …)
              model / reasoning / harness are saved in config.toml and prefilled
              into MATCH_RESULT so you do not retype them each try
  d           toggle stored near-miss / NONMATCHING drafts in prompt (off = disasm only)
  h           toggle Ghidra C draft in prompt (from local_repo/ghidra_out or details)
  , / .       previous / next batch (Prompt rebuilds for the active slot)
  + / =       new empty batch after active
  c           copy active-batch prompt
  g           launch default agent — one window per non-empty batch
  Shift+g     agent picker · enter launch all · d set default · esc close
  Shift+b     clear ALL batches
  Shift+Del   clear active slot only
  stock prompts always include provenance / attempt tree (experimental merged)

CLAIMS (page 4 — live locks via project.claimsApi, e.g. https://tangos.dev/api/claims)
  r           refresh locks (API + CLAIMS.md)
  i           sign in: try `gh auth token` exchange, else paste API key / session
  o           sign out (clears saved ~/.config/chaos/claims-session.toml)
  L           claim the Overview/Priorities selected function
  A           claim all functions in every mass-batcher slot
  y           renew every lock we hold (my claims)
  x           release every lock we hold
  Session is required for write; Discord key or GitHub session both work as X-Api-Key.
  Same contract as the web Chaos Viewer claim buttons.

TOOLS
  j / k       move selection among instrument cards
  h / l       previous / next card (columns)
  n           filter category (all · core · atlas · experimental · …)
  r           rescan local_repo for which tools are present
  Cards show purpose + what the tool changes (outputs / ledgers).
  Green ★ = found under project local_repo · muted = not detected

SETUP / PROJECTS
  type        source path / URL / GitHub (always works; focuses the input)
  tab         focus project list ↔ source input
  j / k       select saved project (when list focused)
  enter       load typed source, or selected project if list focused
  Shift+s     save current source as a named project (then type id)
  r           set local decomp path for selected project (Grok cwd; list focused)
              enter empty path to clear · ~/… expanded on save
  d           delete selected project (asks y/n first; list focused)
  p           open this hub from any loaded screen
  v           cycle data-tracking convention for selected project
              default = full tracking · experimental = alias of default

Press ? or esc to close help."#
            .to_string()
    }

    async fn load_from(&mut self, input: &str) -> Result<()> {
        self.load_from_with_branch(input, None).await
    }

    async fn load_from_with_branch(&mut self, input: &str, branch: Option<&str>) -> Result<()> {
        self.load_from_with_branch_opts(input, branch, None, false)
            .await
    }

    async fn load_from_with_branch_opts(
        &mut self,
        input: &str,
        branch: Option<&str>,
        preferred_atlas_url: Option<&str>,
        fresh: bool,
    ) -> Result<()> {
        self.status = format!("Loading {input}…");
        self.error = None;
        let input = input.trim();
        let (db, source) = if input.contains("github.com/")
            && !input.contains("raw.githubusercontent.com")
            && !input.ends_with(".json")
        {
            load_chaos_db_opts(
                &self.client,
                None,
                Some(input),
                branch,
                preferred_atlas_url,
                fresh,
            )
            .await?
        } else {
            load_chaos_db_opts(
                &self.client,
                Some(input),
                None,
                None,
                preferred_atlas_url,
                fresh,
            )
            .await?
        };
        let base = details_base_from_source(&source);
        self.replace_detail_cache(base);
        // Keep the *user* source for save/resume; discovery may set source to a raw JSON URL.
        self.load_input = Some(input.to_string());
        self.setup_input = input.to_string();
        self.source = Some(source);
        // Switching repo clears batch / selection noise (not on soft refresh).
        if !fresh {
            self.batches = vec![Vec::new()];
            self.active_batch = 0;
            self.selected_id = None;
            self.detail = None;
            self.invalidate_detail_lines();
        }
        // Align active profile with what we actually loaded (or clear a stale one).
        self.sync_active_project_to_load_input();
        // Remember raw atlas URL on the active profile for fast reopen.
        self.remember_atlas_url_on_active();
        self.apply_db(db, !fresh).await;
        Ok(())
    }

    /// Persist last raw atlas URL onto the active saved project (if any).
    fn remember_atlas_url_on_active(&mut self) {
        let Some(id) = self.project_store.active_id.clone() else {
            return;
        };
        let Some(DataSource::Url(url)) = &self.source else {
            return;
        };
        // Only cache raw atlas endpoints, never a github.com HTML repo page.
        if !url.contains("raw.githubusercontent.com")
            && !url.contains("github.io/")
            && !url.ends_with(".json")
        {
            return;
        }
        let Some(mut p) = self.project_store.get(&id).cloned() else {
            return;
        };
        if p.atlas_url.as_deref() == Some(url.as_str()) {
            return;
        }
        p.atlas_url = Some(url.clone());
        let _ = self.project_store.upsert(p);
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
        // Prefer last raw atlas URL (one GET) over full multi-probe discovery.
        self.load_from_with_branch_opts(
            &profile.source,
            profile.branch.as_deref(),
            profile.atlas_url.as_deref(),
            false,
        )
        .await?;
        // Force this profile active even if source matching failed (e.g. local path forms).
        self.project_store.set_active(Some(&profile.id))?;
        if let Some(i) = self.project_store.index_of(&profile.id) {
            self.project_sel = i;
        }
        self.convention = profile.convention;
        self.sync_template_to_convention();
        // Ensure atlas_url is stored under this id (active may have changed during load).
        self.remember_atlas_url_on_active();
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

    /// Start typing a local decomp path for the selected saved project.
    fn begin_edit_local_repo(&mut self) {
        let Some(p) = self.project_store.projects.get(self.project_sel) else {
            self.status = "No project selected · save one with Shift+s first".into();
            return;
        };
        self.editing_local_repo = true;
        self.local_repo_input = p.local_repo.clone().unwrap_or_default();
        self.status = format!(
            "Local decomp path for '{}' · enter save · empty clears · esc cancel",
            p.id
        );
    }

    /// Persist `local_repo_input` onto the selected project (empty = clear).
    fn commit_local_repo_edit(&mut self) -> Result<()> {
        let Some(p) = self.project_store.projects.get(self.project_sel).cloned() else {
            anyhow::bail!("no project selected");
        };
        let id = p.id.clone();
        let raw = self.local_repo_input.trim();
        let mut updated = p;
        if raw.is_empty() {
            updated.local_repo = None;
            self.project_store.upsert(updated)?;
            self.status = format!("Project '{id}' local_repo cleared");
            return Ok(());
        }
        let expanded = expand_user_path_tui(raw);
        if !expanded.is_dir() {
            anyhow::bail!("not a directory: {raw} (expanded: {})", expanded.display());
        }
        let display = expanded.display().to_string();
        updated.local_repo = Some(display.clone());
        self.project_store.upsert(updated)?;
        self.status = format!("Project '{id}' local_repo → {display}");
        Ok(())
    }

    /// Stock prompts are the same for default and experimental (merged).
    /// Prefer the canonical `chaos-viewer` id when on either stock builtin.
    /// Does not override a user-selected custom template.
    fn sync_template_to_convention(&mut self) {
        let _ = self.convention;
        if self.prompt_template_id == BUILTIN_EXPERIMENTAL_ID {
            self.prompt_template_id = BUILTIN_ID.to_string();
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
        // Keep any previously configured local_repo when re-saving the same id.
        let existing_local = self
            .project_store
            .get(&id)
            .and_then(|p| p.local_repo.clone());
        // If source is a local path and no local_repo yet, use it as the decomp root.
        let local_repo = existing_local.or_else(|| {
            let path = std::path::Path::new(source.trim());
            if path.is_dir() {
                Some(path.display().to_string())
            } else if path.is_file() {
                path.parent().map(|x| x.display().to_string())
            } else {
                None
            }
        });
        let existing_atlas = self
            .project_store
            .get(&id)
            .and_then(|p| p.atlas_url.clone())
            .or_else(|| {
                self.source.as_ref().and_then(|s| match s {
                    DataSource::Url(u) => Some(u.clone()),
                    DataSource::Path(_) => None,
                })
            });
        self.project_store.upsert(ProjectProfile {
            id: id.clone(),
            name,
            source: source.clone(),
            branch: None,
            // New saves keep the session convention (default unless cycling first).
            convention: self.convention,
            local_repo,
            atlas_url: existing_atlas,
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
        let prev_batches = self.batches.clone();
        let prev_active_batch = self.active_batch;
        let prev_screen = self.screen;
        let prev_priority_mode = self.priority_mode;

        // Soft update: cache-bust remote GETs; keep preferred atlas URL when we have one.
        let preferred = self.source.as_ref().and_then(|s| match s {
            DataSource::Url(u) => Some(u.as_str()),
            DataSource::Path(_) => None,
        });
        let (db, source) = if input.contains("github.com/")
            && !input.contains("raw.githubusercontent.com")
            && !input.ends_with(".json")
        {
            load_chaos_db_opts(&self.client, None, Some(&input), None, preferred, true).await?
        } else {
            load_chaos_db_opts(&self.client, Some(&input), None, None, preferred, true).await?
        };
        let base = details_base_from_source(&source);
        self.replace_detail_cache(base);
        self.load_input = Some(input);
        self.source = Some(source);
        self.remember_atlas_url_on_active();

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
        self.batches = prev_batches
            .into_iter()
            .map(|slot| {
                slot.into_iter()
                    .filter(|id| self.id_index.contains_key(id))
                    .collect::<Vec<_>>()
            })
            .collect();
        self.active_batch = prev_active_batch;
        self.normalize_batches();

        self.rebuild_priorities();
        self.detail = None;
        if prev_screen != Screen::Setup {
            self.screen = prev_screen;
        }
        // Refresh detail for the selected function; rebuild prompt only here
        // (not on every list move). Detail load is non-blocking (background).
        self.ensure_selected_detail();
        self.rebuild_prompt().await;
        self.invalidate_detail_lines();

        if let Some(db) = &self.db {
            self.status = format!(
                "Updated · {}/{} fn ({:.2}%) · {}/{} B ({:.2}%) · batch {} · warming details…",
                db.stats.matched_functions,
                db.stats.total_functions,
                db.match_pct_functions(),
                db.stats.matched_bytes,
                db.stats.total_bytes,
                db.match_pct_bytes(),
                self.batch_summary(),
            );
        }
        Ok(())
    }

    async fn apply_db(&mut self, db: ChaosDb, reset_to_overview: bool) {
        self.refresh_claims(&db).await;
        self.rebuild_lookup_indexes(&db);
        self.rebuild_modules(&db);
        self.db = Some(db);
        if reset_to_overview {
            self.screen = Screen::Overview;
        }
        self.rebuild_functions();
        self.rebuild_priorities();
        // Queue every module for gentle background prewarm (selected first).
        self.queue_detail_prewarm();
        self.ensure_selected_detail();
        self.kick_detail_prewarm();
        self.rebuild_prompt().await;
        self.invalidate_detail_lines();
        if let Some(db) = &self.db {
            self.status = format!(
                "Loaded {} · {}/{} fn ({:.2}%) · {}/{} B ({:.2}%) · warming details…",
                db.project_name(),
                db.stats.matched_functions,
                db.stats.total_functions,
                db.match_pct_functions(),
                db.stats.matched_bytes,
                db.stats.total_bytes,
                db.match_pct_bytes(),
            );
        }
    }

    /// Drop any previous cache / in-flight set (atlas replaced).
    fn replace_detail_cache(&mut self, base: String) {
        self.detail_cache = Some(Arc::new(DetailCache::new(base)));
        self.detail_in_flight = Arc::new(Mutex::new(HashSet::new()));
        self.prewarm_queue.clear();
        self.prewarm_total = 0;
        self.waiting_for_detail = false;
    }

    /// Enqueue all known modules for background detail warm (selected + neighbors first).
    fn queue_detail_prewarm(&mut self) {
        self.prewarm_queue.clear();
        let mut seen = HashSet::new();
        let push = |m: &str, q: &mut VecDeque<String>, seen: &mut HashSet<String>| {
            if m.is_empty() || !seen.insert(m.to_string()) {
                return;
            }
            q.push_back(m.to_string());
        };

        // 1) Currently selected module (and nearby list neighbors).
        if let Some(sel) = self.selected_module().map(str::to_string) {
            push(&sel, &mut self.prewarm_queue, &mut seen);
            if let Some(i) = self.module_list.iter().position(|m| m == &sel) {
                if i > 0 {
                    push(&self.module_list[i - 1], &mut self.prewarm_queue, &mut seen);
                }
                if i + 1 < self.module_list.len() {
                    push(&self.module_list[i + 1], &mut self.prewarm_queue, &mut seen);
                }
            }
        }
        // 2) Rest of the module list in display order.
        for m in &self.module_list {
            push(m, &mut self.prewarm_queue, &mut seen);
        }
        // 3) Any module that appears in the atlas but was filtered out of the list.
        if let Some(db) = &self.db {
            for f in &db.functions {
                push(&f.module, &mut self.prewarm_queue, &mut seen);
            }
        }
        self.prewarm_total = self.prewarm_queue.len();
    }

    /// Move `module` to the front of the prewarm queue (user just opened it).
    fn prioritize_detail_module(&mut self, module: &str) {
        if module.is_empty() {
            return;
        }
        self.prewarm_queue.retain(|m| m != module);
        self.prewarm_queue.push_front(module.to_string());
        if self.prewarm_total == 0 {
            self.prewarm_total = 1;
        }
    }

    /// Spawn background workers for pending module detail chunks (rate-limited).
    ///
    /// Does **not** block the UI. Cap concurrency so we never reintroduce the
    /// old “download every details/*.json at once” storm.
    fn kick_detail_prewarm(&mut self) {
        let Some(cache) = self.detail_cache.clone() else {
            return;
        };
        let in_flight = self.detail_in_flight.clone();
        let client = self.client.clone();

        loop {
            let inflight_n = in_flight.lock().expect("in_flight").len();
            if inflight_n >= DETAIL_PREWARM_CONCURRENCY {
                break;
            }

            // Pop next module that still needs work.
            let mut next: Option<String> = None;
            while let Some(m) = self.prewarm_queue.pop_front() {
                if cache.is_module_loaded(&m) {
                    continue;
                }
                let mut guard = in_flight.lock().expect("in_flight");
                if guard.contains(&m) {
                    continue;
                }
                guard.insert(m.clone());
                next = Some(m);
                break;
            }
            let Some(module) = next else {
                break;
            };

            let cache_bg = Arc::clone(&cache);
            let in_flight_bg = Arc::clone(&in_flight);
            let client_bg = client.clone();
            tokio::spawn(async move {
                let _ = ensure_module_chunk(&client_bg, &cache_bg, &module).await;
                if let Ok(mut g) = in_flight_bg.lock() {
                    g.remove(&module);
                }
            });
        }
    }

    /// If a background prewarm finished for the current selection, apply it.
    /// Returns true when the UI should repaint.
    fn poll_detail_prewarm(&mut self) -> bool {
        let mut dirty = false;

        // Selected function's module became ready?
        if self.waiting_for_detail && self.apply_detail_from_cache() {
            self.waiting_for_detail = false;
            dirty = true;
            if let Some(m) = self.selected_function().map(|f| f.module.clone()) {
                self.status = format!("Details ready · {m}");
            }
        } else if !self.waiting_for_detail {
            // Even when not "waiting", j/k might have landed on a warm module —
            // apply_detail_from_cache already handles hits; nothing to do.
        }

        // Keep the pipeline fed.
        let before = self.prewarm_queue.len();
        self.kick_detail_prewarm();

        // Mild status while warming in the background (don't stomp active work msgs).
        if let Some(cache) = &self.detail_cache {
            let loaded = cache.loaded_module_count();
            let total = self.prewarm_total.max(loaded);
            let inflight = self.detail_in_flight.lock().map(|g| g.len()).unwrap_or(0);
            if loaded < total || inflight > 0 || before > 0 {
                // Only rewrite status when we're not mid user action message that
                // already mentions batch/copy — keep it short for warming.
                if self.waiting_for_detail {
                    if let Some(m) = self.selected_function().map(|f| f.module.clone()) {
                        self.status = format!("Loading details · {m}  (warmed {loaded}/{total})");
                        dirty = true;
                    }
                } else if self.status.contains("warming details")
                    || self.status.starts_with("Details warm")
                    || self.status.starts_with("Loaded ")
                    || self.status.starts_with("Updated ")
                {
                    self.status = format!("Details warm {loaded}/{total}");
                    dirty = true;
                }
            } else if self.status.starts_with("Details warm")
                || self.status.contains("warming details")
            {
                self.status = format!("Details ready · {loaded} modules cached");
                dirty = true;
            }
        }

        dirty
    }

    /// One linear pass: id → index and module → [function indices].
    fn rebuild_lookup_indexes(&mut self, db: &ChaosDb) {
        let mut id_index = HashMap::with_capacity(db.functions.len());
        let mut module_index: HashMap<String, Vec<usize>> = HashMap::new();
        for (i, f) in db.functions.iter().enumerate() {
            id_index.insert(f.id.clone(), i);
            module_index.entry(f.module.clone()).or_default().push(i);
        }
        self.id_index = id_index;
        self.module_index = module_index;
    }

    fn claims_api_base(&self) -> Option<String> {
        self.db
            .as_ref()
            .and_then(|d| d.project.as_ref())
            .and_then(|p| p.claims_api.as_ref())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    }

    fn claims_auth_url(&self) -> Option<String> {
        self.db
            .as_ref()
            .and_then(|d| d.project.as_ref())
            .and_then(|p| p.claims_auth_url.as_ref())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    }

    fn claims_client(&self) -> Option<ClaimsClient> {
        self.claims_api_base()
            .map(|api| ClaimsClient::new(self.client.clone(), &api))
    }

    fn persist_claims_auth(&self) {
        if let Some(session) = &self.claims_session {
            let api = self.claims_api_base();
            if let Err(e) = save_auth(session, api.as_deref(), &self.my_claims) {
                // Non-fatal: still usable this session.
                let _ = e;
            }
        }
    }

    fn claims_session_label(&self) -> String {
        match &self.claims_session {
            Some(s) if s.is_ready() => format!("signed in as {}", s.handle),
            _ => "not signed in".into(),
        }
    }

    /// Sign in for claim writes: env already loaded; try gh exchange, else paste.
    async fn claims_sign_in(&mut self) {
        let Some(cc) = self.claims_client() else {
            self.status =
                "No project.claimsApi — atlas must publish claimsApi (e.g. tangos.dev)".into();
            return;
        };
        // Prefer GitHub CLI token exchange (desktop path from coordinator docs).
        match session_from_gh_cli(&cc).await {
            Ok(session) => {
                self.claims_session = Some(session);
                self.persist_claims_auth();
                self.claims_paste_open = false;
                self.status = format!("Claims signed in via gh · {}", self.claims_session_label());
                self.error = None;
                return;
            }
            Err(e) => {
                self.status = format!(
                    "gh exchange failed ({e:#}) · paste API key (Discord / session) · enter"
                );
            }
        }
        // Optional: open browser OAuth start (allowlist may reject non-web redirects).
        if let Some(auth) = self.claims_auth_url() {
            let _ = open_in_browser(&auth);
            self.status = "Opened sign-in · if browser finishes, paste session token here · \
or Discord: DM bot `key` · enter when ready"
                .into();
        }
        self.claims_paste_open = true;
        self.claims_paste_buf.clear();
        self.screen = Screen::Claims;
    }

    fn claims_sign_out(&mut self) {
        self.claims_session = None;
        self.my_claims.clear();
        self.claims_paste_open = false;
        self.claims_paste_buf.clear();
        let _ = clear_saved_auth();
        self.status = "Claims signed out · local session cleared".into();
        self.error = None;
    }

    fn apply_claims_paste(&mut self) {
        let raw = self.claims_paste_buf.trim().to_string();
        if raw.is_empty() {
            self.status = "Paste cancelled · empty token".into();
            self.claims_paste_open = false;
            return;
        }
        // Accept "token" or "token handle" or "token\thandle"
        let mut parts = raw.split_whitespace();
        let token = parts.next().unwrap_or("").to_string();
        let handle = parts
            .next()
            .map(str::to_string)
            .or_else(|| std::env::var("CHAOS_CLAIMS_HANDLE").ok())
            .filter(|s| !s.is_empty())
            .or_else(|| {
                load_saved_auth()
                    .map(|a| a.handle)
                    .filter(|s| !s.is_empty())
            })
            .unwrap_or_else(|| "chaos-viewer-user".into());
        if token.is_empty() {
            self.status = "Need an API key / session token".into();
            return;
        }
        self.claims_session = Some(ClaimsSession { token, handle });
        self.persist_claims_auth();
        self.claims_paste_open = false;
        self.claims_paste_buf.clear();
        self.status = format!("Claims key saved · {}", self.claims_session_label());
        self.error = None;
    }

    async fn ensure_claims_session(&mut self) -> bool {
        if self
            .claims_session
            .as_ref()
            .map(|s| s.is_ready())
            .unwrap_or(false)
        {
            return true;
        }
        // Reload disk/env in case user set env after launch.
        if let Some(s) = ClaimsSession::load() {
            self.claims_session = Some(s);
            return true;
        }
        self.claims_sign_in().await;
        self.claims_session
            .as_ref()
            .map(|s| s.is_ready())
            .unwrap_or(false)
    }

    async fn claim_functions(&mut self, fns: Vec<ChaosFunction>) {
        if fns.is_empty() {
            self.status = "Nothing to claim".into();
            return;
        }
        let Some(api) = self.claims_api_base() else {
            self.status = "No claimsApi on this project".into();
            return;
        };
        if !self.ensure_claims_session().await {
            self.status = "Sign in required to claim · i on Claims, or paste key".into();
            return;
        }
        let session = self.claims_session.clone().unwrap();
        let cc = ClaimsClient::new(self.client.clone(), &api);
        let mut locked = 0usize;
        let mut last_err: Option<String> = None;
        for f in &fns {
            if f.matched {
                continue;
            }
            if self.locked_by.contains_key(&f.id) {
                continue;
            }
            let end = f.addr.saturating_add(f.size);
            let note = format!("via chaos-viewer-cli: {}", f.name);
            match cc
                .try_lock(&session, &f.module, f.addr, end, Some(&note))
                .await
            {
                Ok(resp) => {
                    if let Some(id) = resp.claim.as_ref().and_then(|c| c.id.clone()) {
                        self.my_claims.retain(|c| c.id != id);
                        self.my_claims.push(MyClaimRecord {
                            id,
                            module: f.module.clone(),
                            start: f.addr,
                            end,
                            name: f.name.clone(),
                        });
                    }
                    locked += 1;
                }
                Err(e) => {
                    last_err = Some(format!("{}: {e:#}", f.name));
                    break;
                }
            }
        }
        self.persist_claims_auth();
        if let Some(db) = self.db.clone() {
            self.refresh_claims(&db).await;
        }
        if locked == 0 {
            self.status = last_err.unwrap_or_else(|| "No functions claimed".into());
            if self.status.contains("401") || self.status.contains("unauthorized") {
                self.claims_session = None;
            }
        } else if let Some(err) = last_err {
            self.status = format!("Claimed {locked}, then stopped: {err}");
            self.error = Some(err);
        } else {
            self.status = format!(
                "Locked {locked} function(s) as {} · y renew · x release",
                session.handle
            );
            self.error = None;
        }
    }

    async fn claim_selected_function(&mut self) {
        let Some(f) = self.selected_function().cloned() else {
            self.status = "Nothing selected to claim".into();
            return;
        };
        self.claim_functions(vec![f]).await;
    }

    /// Claim every function across **all** mass-batcher slots (not only active).
    async fn claim_all_batches(&mut self) {
        if self.total_batched() == 0 {
            self.status = "All batches empty · b to add, then A to claim".into();
            return;
        }
        let Some(db) = &self.db else {
            return;
        };
        // Preserve batch order; dedupe if the same id somehow appears twice.
        let mut seen = HashSet::new();
        let mut fns: Vec<ChaosFunction> = Vec::new();
        for slot in &self.batches {
            for id in slot {
                if !seen.insert(id.clone()) {
                    continue;
                }
                if let Some(f) = self
                    .id_index
                    .get(id)
                    .and_then(|&i| db.functions.get(i))
                    .cloned()
                {
                    fns.push(f);
                }
            }
        }
        let n_slots = self.batches.iter().filter(|b| !b.is_empty()).count();
        if fns.is_empty() {
            self.status = "No claimable functions in batches".into();
            return;
        }
        self.status = format!(
            "Claiming {} function(s) across {n_slots} batch(es)…",
            fns.len()
        );
        self.claim_functions(fns).await;
    }

    async fn renew_my_claims(&mut self) {
        if self.my_claims.is_empty() {
            self.status = "No local my-claims to renew · L / A to claim first".into();
            return;
        }
        let Some(api) = self.claims_api_base() else {
            self.status = "No claimsApi".into();
            return;
        };
        if !self.ensure_claims_session().await {
            return;
        }
        let session = self.claims_session.clone().unwrap();
        let cc = ClaimsClient::new(self.client.clone(), &api);
        let mut ok = 0usize;
        let mut err: Option<String> = None;
        for c in &self.my_claims {
            match cc.renew(&session, &c.id).await {
                Ok(_) => ok += 1,
                Err(e) => {
                    err = Some(format!("{}: {e:#}", c.name));
                    break;
                }
            }
        }
        if let Some(db) = self.db.clone() {
            self.refresh_claims(&db).await;
        }
        self.status = if let Some(e) = err {
            format!("Renewed {ok}, then: {e}")
        } else {
            format!("Renewed {ok} claim(s) as {}", session.handle)
        };
    }

    async fn release_my_claims(&mut self) {
        if self.my_claims.is_empty() {
            self.status = "No local my-claims to release".into();
            return;
        }
        let Some(api) = self.claims_api_base() else {
            self.status = "No claimsApi".into();
            return;
        };
        if !self.ensure_claims_session().await {
            return;
        }
        let session = self.claims_session.clone().unwrap();
        let cc = ClaimsClient::new(self.client.clone(), &api);
        let mut keep = Vec::new();
        let mut released = 0usize;
        for c in self.my_claims.drain(..) {
            match cc.release(&session, &c.id).await {
                Ok(_) => released += 1,
                Err(_) => keep.push(c),
            }
        }
        self.my_claims = keep;
        self.persist_claims_auth();
        if let Some(db) = self.db.clone() {
            self.refresh_claims(&db).await;
        }
        self.status = format!(
            "Released {released} · {} still held locally",
            self.my_claims.len()
        );
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
        let module = self.selected_module().map(str::to_string);
        let q = self.search.to_ascii_lowercase();
        let filter = self.match_filter;
        let prev_id = self.selected_id.clone();

        // Prefer per-module index (O(module size)) over scanning the whole atlas.
        // Search: only lowercases the needle once; haystack uses ASCII case fold
        // without allocating three full lowercase copies per function.
        let matches = |f: &ChaosFunction| {
            if !filter.allows(f.matched) {
                return false;
            }
            if q.is_empty() {
                return true;
            }
            contains_ignore_ascii_case(&f.name, &q)
                || contains_ignore_ascii_case(&f.module, &q)
                || contains_ignore_ascii_case(&f.id, &q)
        };
        self.fn_list = if let Some(ref m) = module {
            self.module_index
                .get(m)
                .map(|cands| {
                    cands
                        .iter()
                        .copied()
                        .filter(|&i| matches(&db.functions[i]))
                        .collect()
                })
                .unwrap_or_default()
        } else {
            db.functions
                .iter()
                .enumerate()
                .filter(|(_, f)| matches(f))
                .map(|(i, _)| i)
                .collect()
        };

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
            .filter_map(|f| self.id_index.get(&f.id).copied())
            .collect();
        self.priority_sel = 0;
        self.priority_offset = 0;
        // Keep selected_id in lockstep with the priorities cursor so `b` batches
        // the highlighted row (not the Overview selection).
        self.sync_selection_from_priority();
    }

    fn sync_selection_from_priority(&mut self) {
        let Some(db) = &self.db else { return };
        if let Some(&idx) = self.priority_list.get(self.priority_sel) {
            let new_id = db.functions[idx].id.clone();
            if self.selected_id.as_deref() != Some(new_id.as_str()) {
                self.detail_scroll = 0;
            }
            self.selected_id = Some(new_id);
        }
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
        let (bi, pos) = self.batch_membership(id).unwrap_or((0, 0));
        let n_batches = self.batches.len();
        let active_len = self.active_batch_ids().len();
        let locked = self.locked_by.get(id).map(|s| s.as_str()).unwrap_or("");
        format!("{id}|d={has_det}|b={bi}:{pos}/{n_batches}a{active_len}|L={locked}")
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
            lines.push("(loading module details in background… list stays navigable)".into());
        }
        lines
    }

    /// Sticky footer for the detail pane (always visible; not part of scroll text).
    fn detail_batch_footer(&self) -> String {
        let Some(fn_) = self.selected_function() else {
            return " select a function above · b adds to batch ".into();
        };
        if let Some((bi, pos)) = self.batch_membership(&fn_.id) {
            let slot_len = self.batches.get(bi - 1).map(|b| b.len()).unwrap_or(0);
            format!(
                " BATCHED [{bi}:{pos}]  ·  batch {bi}/{}  ·  {pos}/{slot_len}  ·  b remove  ·  S-b clear-all ",
                self.batches.len()
            )
        } else if self.total_batched() == 0 {
            format!(
                " not in batch  ·  b to add ({})  ·  Prompt uses active batch only ",
                self.batch_summary()
            )
        } else {
            format!(
                " not in batch  ·  b to add ({})  ·  ,/. switch  ·  S-b clear-all ",
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
    ///
    /// Returns `true` when the selection is fully handled (including “no function”
    /// / “no cache”). Returns `false` when the module chunk is not loaded yet.
    fn apply_detail_from_cache(&mut self) -> bool {
        let (module, name) = {
            let Some(f) = self.selected_function() else {
                self.detail = None;
                self.invalidate_detail_lines();
                self.waiting_for_detail = false;
                return true;
            };
            (f.module.clone(), f.name.clone())
        };
        let Some(cache) = &self.detail_cache else {
            self.detail = None;
            self.invalidate_detail_lines();
            self.waiting_for_detail = false;
            return true;
        };
        if let Some(det) = cache.get_if_module_loaded(&module, &name) {
            self.detail = det;
            self.invalidate_detail_lines();
            self.waiting_for_detail = false;
            return true;
        }
        false
    }

    /// Resolve detail for the current selection **without blocking the UI**.
    ///
    /// On a cache miss, prioritizes that module in the background prewarm queue
    /// and shows a loading state. The event loop's idle tick applies the detail
    /// once the chunk lands (see [`Self::poll_detail_prewarm`]).
    ///
    /// Does **not** rebuild the batch prompt (that made j/k laggy).
    fn ensure_selected_detail(&mut self) {
        if self.apply_detail_from_cache() {
            return;
        }
        let module = self
            .selected_function()
            .map(|f| f.module.clone())
            .unwrap_or_default();
        self.detail = None;
        self.invalidate_detail_lines();
        self.waiting_for_detail = true;
        if !module.is_empty() {
            self.prioritize_detail_module(&module);
            // Neighbors: pre-warm next/prev so h/l after this stays snappy.
            if let Some(i) = self.module_list.iter().position(|m| m == &module) {
                if i > 0 {
                    self.prioritize_detail_module(&self.module_list[i - 1].clone());
                    // keep current at front
                    self.prioritize_detail_module(&module);
                }
                if i + 1 < self.module_list.len() {
                    let n = self.module_list[i + 1].clone();
                    self.prioritize_detail_module(&n);
                    self.prioritize_detail_module(&module);
                }
            }
            self.status = format!("Loading details · {module}");
        }
        self.kick_detail_prewarm();
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
    /// Prefer active project's `local_repo/ghidra_out`, then env, then `./ghidra_out`.
    fn resolve_ghidra_dir(&self) -> Option<std::path::PathBuf> {
        if let Ok(p) = std::env::var("CHAOS_GHIDRA_DIR") {
            let pb = std::path::PathBuf::from(p);
            if pb.is_dir() {
                return Some(pb);
            }
        }
        if let Some(id) = self.project_store.active_id.as_ref() {
            if let Some(prof) = self.project_store.get(id) {
                if let Some(repo) = prof.local_repo.as_ref() {
                    let d = expand_user_path_tui(repo).join("ghidra_out");
                    if d.is_dir() {
                        return Some(d);
                    }
                }
            }
        }
        let local = std::path::PathBuf::from("ghidra_out");
        if local.is_dir() {
            return Some(local);
        }
        None
    }

    fn resolve_local_repo(&self) -> Option<std::path::PathBuf> {
        if let Ok(p) = std::env::var("CHAOS_LOCAL_REPO") {
            let pb = expand_user_path_tui(&p);
            if pb.is_dir() {
                return Some(pb);
            }
        }
        if let Some(id) = self.project_store.active_id.as_ref() {
            if let Some(prof) = self.project_store.get(id) {
                if let Some(repo) = prof.local_repo.as_ref() {
                    let pb = expand_user_path_tui(repo);
                    if pb.is_dir() {
                        return Some(pb);
                    }
                }
            }
        }
        None
    }

    fn prompt_opts(&self) -> PromptOptions {
        PromptOptions {
            claims_session: self.claims_session.clone(),
            include_near_miss_draft: self.include_near_miss_draft,
            include_ghidra_draft: self.include_ghidra_draft,
            ghidra_dir: self.resolve_ghidra_dir(),
            local_repo: self.resolve_local_repo(),
            provenance_model: Some(self.template_store.provenance_model().to_string()),
            provenance_reasoning: Some(self.template_store.provenance_reasoning().to_string()),
            provenance_harness: Some(self.template_store.provenance_harness().to_string()),
        }
    }

    /// Render a prompt for an arbitrary batch slot (used by mass-batcher launch).
    async fn render_batch_prompt(&mut self, batch_ids: &[String]) -> String {
        let project = self.project();
        let opts = self.prompt_opts();
        let Some(db) = &self.db else {
            return String::new();
        };

        let targets: Vec<ChaosFunction> = batch_ids
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
            return "Batch is empty.\n\n\
Add functions with b on Overview or Priorities \
(max 16 per batch; overflow opens batch 2, 3, …), then open Prompt (3) or press c to copy."
                .into();
        }

        let mut items: Vec<(ChaosFunction, Option<FunctionDetail>)> = Vec::new();
        for f in targets {
            let det = if let Some(cache) = self.detail_cache.clone() {
                // Prompt/copy needs real detail — block here (batch ≤16).
                load_function_detail(&self.client, cache.as_ref(), &f.module, &f.name)
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
            Ok(text) => text,
            Err(e) => format!("Template error ({id}): {e:#}"),
        }
    }

    /// Rebuild the Prompt page from the **active batch only**.
    async fn rebuild_prompt(&mut self) {
        let ids = self.active_batch_ids().to_vec();
        self.prompt_text = self.render_batch_prompt(&ids).await;
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

    /// Ids in the active mass-batcher slot.
    fn active_batch_ids(&self) -> &[String] {
        self.batches
            .get(self.active_batch)
            .map(|b| b.as_slice())
            .unwrap_or(&[])
    }

    fn total_batched(&self) -> usize {
        self.batches.iter().map(|b| b.len()).sum()
    }

    /// Ensure at least one batch slot and a valid `active_batch` index.
    /// Drops empty slots except when every slot is empty (then leave a single empty).
    fn normalize_batches(&mut self) {
        if self.batches.is_empty() {
            self.batches.push(Vec::new());
            self.active_batch = 0;
            return;
        }
        let any_nonempty = self.batches.iter().any(|b| !b.is_empty());
        if any_nonempty {
            // Keep empty slots the user is still looking at; only prune empties
            // that are not the sole remaining slot when mixed with content is messy.
            // Policy: keep all non-empty; if active is empty but others exist, keep it
            // so switching still works; drop other empties.
            let active = self.active_batch.min(self.batches.len() - 1);
            let mut next = Vec::new();
            let mut new_active = 0;
            for (i, slot) in self.batches.drain(..).enumerate() {
                if !slot.is_empty() || i == active {
                    if i == active {
                        new_active = next.len();
                    }
                    next.push(slot);
                }
            }
            if next.is_empty() {
                next.push(Vec::new());
                new_active = 0;
            }
            self.batches = next;
            self.active_batch = new_active.min(self.batches.len() - 1);
        } else {
            self.batches = vec![Vec::new()];
            self.active_batch = 0;
        }
    }

    /// `(batch_num 1-based, position 1-based)` if `id` is in any slot.
    fn batch_membership(&self, id: &str) -> Option<(usize, usize)> {
        for (bi, slot) in self.batches.iter().enumerate() {
            if let Some(pos) = slot.iter().position(|x| x == id) {
                return Some((bi + 1, pos + 1));
            }
        }
        None
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

        // Toggle off if present in any batch.
        if let Some((bi, pos)) = self.batch_membership(&id) {
            let slot_i = bi - 1;
            if let Some(slot) = self.batches.get_mut(slot_i) {
                if pos - 1 < slot.len() {
                    slot.remove(pos - 1);
                }
            }
            self.normalize_batches();
            self.status = format!(
                "Removed {name} from batch {bi} · now {}",
                self.batch_summary()
            );
            self.invalidate_detail_lines();
            self.rebuild_prompt().await;
            return;
        }

        // Prefer adding to the active slot; if full, spill into the next non-full
        // slot or create a new batch automatically (mass batcher).
        self.normalize_batches();
        let max = batch_max();
        let mut target = self.active_batch.min(self.batches.len() - 1);
        if self.batches[target].len() >= max {
            // Search forward (wrap) for a non-full slot.
            let n = self.batches.len();
            let mut found = None;
            for step in 1..=n {
                let i = (target + step) % n;
                if self.batches[i].len() < max {
                    found = Some(i);
                    break;
                }
            }
            if let Some(i) = found {
                target = i;
            } else {
                self.batches.push(Vec::new());
                target = self.batches.len() - 1;
            }
            self.active_batch = target;
        }

        // Note emptiness before the push so overflow can report "opened batch N".
        let was_empty_slot = self.batches[target].is_empty();
        self.batches[target].push(id);
        self.active_batch = target;
        let pos = self.batches[target].len();
        let bi = target + 1;
        self.status = if was_empty_slot && self.batches.len() > 1 && bi > 1 {
            format!("Batched {name} · opened batch {bi} · 1/{max} (overflow past {max})")
        } else if self.batches.len() > 1 {
            format!(
                "Batched {name} · batch {bi}/{} · {pos}/{max} · ,/. switch",
                self.batches.len()
            )
        } else {
            format!("Batched {name} · {pos}/{max}  (B badge in lists)")
        };
        self.invalidate_detail_lines();
        self.rebuild_prompt().await;
    }

    /// Clear the active mass-batcher slot (web “clear”).
    async fn clear_batch(&mut self) {
        self.normalize_batches();
        let n = self.active_batch_ids().len();
        if n == 0 && self.total_batched() == 0 {
            self.status = "Batch already empty".into();
            return;
        }
        if n == 0 {
            self.status = format!(
                "Active batch {} already empty · ,/. switch · total {} fn · S-b clear ALL",
                self.active_batch + 1,
                self.total_batched()
            );
            return;
        }
        let bi = self.active_batch + 1;
        if let Some(slot) = self.batches.get_mut(self.active_batch) {
            slot.clear();
        }
        self.normalize_batches();
        self.invalidate_detail_lines();
        self.rebuild_prompt().await;
        self.status = format!(
            "Cleared active batch {bi} · removed {n} · now {} · S-b clears ALL slots",
            self.batch_summary()
        );
    }

    /// Wipe every mass-batcher slot (back to a single empty batch).
    /// All `[B…]` / `[n:m]` badges disappear; active slot resets to 1.
    async fn clear_all_batches(&mut self) {
        let total = self.total_batched();
        let slots = self.batches.iter().filter(|s| !s.is_empty()).count().max(1);
        if total == 0 {
            self.batches = vec![Vec::new()];
            self.active_batch = 0;
            self.invalidate_detail_lines();
            self.rebuild_prompt().await;
            self.status = "All batches already empty".into();
            return;
        }
        // Drop every slot completely — not just the active one.
        self.batches.clear();
        self.batches.push(Vec::new());
        self.active_batch = 0;
        self.invalidate_detail_lines();
        self.rebuild_prompt().await;
        self.status = format!(
            "Cleared ALL batches · removed {total} function(s) from {slots} slot(s) · badges cleared"
        );
    }

    /// Switch mass-batcher slot (`delta` = ±1). Wraps around.
    async fn cycle_batch_slot(&mut self, delta: isize) {
        self.normalize_batches();
        let n = self.batches.len() as isize;
        if n <= 1 {
            self.status = format!(
                "Only one batch · {} · + new empty · overflow past {} auto-opens more",
                self.batch_summary(),
                batch_max()
            );
            return;
        }
        let cur = self.active_batch as isize;
        let next = (cur + delta).rem_euclid(n) as usize;
        self.active_batch = next;
        self.invalidate_detail_lines();
        self.rebuild_prompt().await;
        self.status = format!(
            "Active batch {}/{} · {}/{} · {} total fn · ,/. switch · + new",
            self.active_batch + 1,
            self.batches.len(),
            self.active_batch_ids().len(),
            batch_max(),
            self.total_batched()
        );
    }

    /// Manually open a new empty batch after the active slot and switch to it.
    ///
    /// No-op (with status) if the active slot is already empty — avoids a stack
    /// of empty slots that [`normalize_batches`] would prune anyway.
    async fn new_empty_batch(&mut self) {
        self.normalize_batches();
        if self.active_batch_ids().is_empty() {
            self.status = format!(
                "Already on empty batch {}/{} · b to fill · ,/. switch",
                self.active_batch + 1,
                self.batches.len()
            );
            return;
        }
        let insert_at = self.active_batch + 1;
        self.batches.insert(insert_at, Vec::new());
        self.active_batch = insert_at;
        self.invalidate_detail_lines();
        self.rebuild_prompt().await;
        self.status = format!(
            "New empty batch {}/{} · b to add · ,/. switch · S-b clear",
            self.active_batch + 1,
            self.batches.len()
        );
    }

    fn batch_badge_spans(&self, id: &str, bg: Color) -> Vec<Span<'static>> {
        if let Some((bi, pos)) = self.batch_membership(id) {
            let label = if self.batches.len() <= 1 {
                format!("[B{pos}] ")
            } else {
                format!("[{bi}:{pos}] ")
            };
            vec![Span::styled(label, paint_bold_on(self.theme.batch, bg))]
        } else {
            Vec::new()
        }
    }

    fn batch_summary(&self) -> String {
        let cur = self.active_batch_ids().len();
        let max = batch_max();
        if self.batches.len() <= 1 {
            format!("{cur}/{max}")
        } else {
            format!(
                "{}/{} · {}/{} ({} fn)",
                self.active_batch + 1,
                self.batches.len(),
                cur,
                max,
                self.total_batched()
            )
        }
    }

    fn copy_prompt(&mut self) {
        if self.active_batch_ids().is_empty() {
            self.status =
                "Nothing to copy · active batch empty (press b to add · ,/. switch)".into();
            return;
        }
        match copy_text(&self.prompt_text) {
            Ok(()) => {
                self.status = format!(
                    "Prompt copied · batch {} · {} function(s)",
                    self.active_batch + 1,
                    self.active_batch_ids().len()
                );
            }
            Err(e) => {
                self.error = Some(format!("clipboard: {e}"));
                self.status = "Copy failed".into();
            }
        }
    }

    fn default_agent(&self) -> AgentKind {
        self.template_store
            .config
            .default_agent
            .as_deref()
            .and_then(AgentKind::parse)
            .unwrap_or_default()
    }

    fn agent_bin_override(&self, agent: AgentKind) -> Option<&str> {
        let cfg = &self.template_store.config;
        match agent {
            AgentKind::Grok => cfg.grok_bin.as_deref(),
            AgentKind::Codex => cfg.codex_bin.as_deref(),
            AgentKind::Claude => cfg.claude_bin.as_deref(),
            AgentKind::Antigravity => cfg.antigravity_bin.as_deref(),
        }
        .map(str::trim)
        .filter(|s| !s.is_empty())
    }

    fn agent_extra_args(&self, agent: AgentKind) -> &[String] {
        let cfg = &self.template_store.config;
        match agent {
            AgentKind::Grok => cfg.grok_extra_args.as_slice(),
            AgentKind::Codex => cfg.codex_extra_args.as_slice(),
            AgentKind::Claude => cfg.claude_extra_args.as_slice(),
            AgentKind::Antigravity => cfg.antigravity_extra_args.as_slice(),
        }
    }

    fn open_agent_picker(&mut self) {
        if self.total_batched() == 0 {
            self.status =
                "Nothing to send · batch empty (b to add, then g / Shift+g on Prompt)".into();
            return;
        }
        let def = self.default_agent();
        self.agent_picker_sel = def.index();
        self.agent_picker_open = true;
        let n = self.batches.iter().filter(|b| !b.is_empty()).count();
        self.status =
            format!("Agent picker · j/k · enter launch {n} batch window(s) · d set default · esc");
    }

    fn open_model_picker(&mut self) {
        let cur = self.template_store.provenance_model();
        self.model_picker_sel = crate::templates::provenance_model_index(cur).unwrap_or(0);
        self.model_picker_open = true;
        self.status =
            "Model picker · j/k select · enter use · esc close · prefills MATCH_RESULT".into();
    }

    /// Open the preferred coding agent in a **separate terminal per non-empty batch**.
    async fn queue_agent_launch(&mut self, agent: AgentKind) {
        self.normalize_batches();
        let non_empty: Vec<usize> = self
            .batches
            .iter()
            .enumerate()
            .filter(|(_, b)| !b.is_empty())
            .map(|(i, _)| i)
            .collect();
        if non_empty.is_empty() {
            self.status =
                "Nothing to send · batch empty (b to add, then g / Shift+g on Prompt)".into();
            return;
        }

        let cfg = &self.template_store.config;
        let grok_mode = cfg
            .grok_mode
            .as_deref()
            .and_then(GrokLaunchMode::parse)
            .unwrap_or_default();
        let terminal = cfg
            .grok_terminal
            .as_deref()
            .and_then(TerminalHost::parse)
            .unwrap_or_default();
        let profile_local = self
            .project_store
            .active_id
            .as_ref()
            .and_then(|id| self.project_store.get(id))
            .and_then(|p| p.local_repo.as_deref());
        let repo_cwd = resolve_repo_cwd(
            profile_local,
            cfg.grok_default_repo.as_deref(),
            self.load_input.as_deref(),
            self.source.as_ref().and_then(|s| match s {
                DataSource::Path(p) => Some(p.as_path()),
                DataSource::Url(_) => None,
            }),
        );
        let bin_override = self.agent_bin_override(agent).map(str::to_string);
        let extra = self.agent_extra_args(agent).to_vec();
        let multi = non_empty.len() > 1;

        let mut opened = 0usize;
        let mut total_fn = 0usize;
        let mut last_terminal = String::new();
        let mut last_repo = repo_cwd.clone();
        let mut errors: Vec<String> = Vec::new();
        let mut first_prompt: Option<String> = None;

        for &bi in &non_empty {
            let ids = self.batches[bi].clone();
            let n_fn = ids.len();
            let prompt = self.render_batch_prompt(&ids).await;
            if prompt.trim().is_empty() || prompt.starts_with("Batch is empty") {
                errors.push(format!("batch {}: empty prompt", bi + 1));
                continue;
            }
            if first_prompt.is_none() {
                first_prompt = Some(prompt.clone());
            }
            let tag_owned = if multi {
                Some(format!("batch{}", bi + 1))
            } else {
                None
            };
            match launch_agent_tagged(
                agent,
                &prompt,
                bin_override.as_deref(),
                repo_cwd.clone(),
                &extra,
                terminal,
                grok_mode,
                tag_owned.as_deref(),
            ) {
                Ok(report) => {
                    opened += 1;
                    total_fn += n_fn;
                    last_terminal = report.terminal;
                    last_repo = report.repo_cwd;
                }
                Err(e) => {
                    errors.push(format!("batch {}: {e:#}", bi + 1));
                }
            }
        }

        // Clipboard fallback: active (or first) prompt.
        if let Some(p) = first_prompt {
            let _ = copy_text(&p);
        }
        // Keep the Prompt page on the active slot.
        self.rebuild_prompt().await;

        let repo = last_repo
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "no local_repo — set: chaos projects local-repo <id> <path>".into());

        if opened == 0 {
            self.error = Some(errors.join(" · "));
            self.status = format!(
                "{} launch failed for all {} batch(es) (prompt still copied if clipboard ok)",
                agent.label(),
                non_empty.len()
            );
            return;
        }

        if multi {
            self.status = format!(
                "{} · opened {opened}/{} windows via {last_terminal} · {total_fn} fn total · cwd {repo} \
· handoff last-agent-prompt-batchN.md",
                agent.label(),
                non_empty.len(),
            );
        } else {
            self.status = format!(
                "{} opened via {last_terminal} · {total_fn} fn · cwd {repo} · look for a NEW Terminal window \
(or: open ~/.config/chaos/last-agent-run.command)",
                agent.label(),
            );
        }
        if let Some(first_err) = errors.first() {
            self.error = Some(format!(
                "Some batches failed ({}/{} ok): {first_err}",
                opened,
                non_empty.len()
            ));
        } else if last_repo.is_none() {
            self.error = Some(
                "No local decomp path. Set per project: \
chaos projects local-repo <id> /path/to/decomp \
(or grok_default_repo in config.toml)."
                    .into(),
            );
        } else {
            self.error = None;
        }
        self.agent_picker_open = false;
    }

    fn set_default_agent_from_picker(&mut self) {
        let agent = AgentKind::ALL
            .get(self.agent_picker_sel)
            .copied()
            .unwrap_or_default();
        match self.template_store.set_default_agent(agent.id()) {
            Ok(()) => {
                self.status = format!(
                    "Default agent → {} (`g` launches this) · enter to open now · esc close",
                    agent.label()
                );
                self.error = None;
            }
            Err(e) => {
                self.error = Some(format!("{e:#}"));
                self.status = "Could not save default agent".into();
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

        // Agent picker modal (Prompt · Shift+g)
        if self.agent_picker_open {
            match key {
                KeyCode::Esc | KeyCode::Char('q') => {
                    self.agent_picker_open = false;
                    self.status = "Agent picker closed".into();
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    if self.agent_picker_sel == 0 {
                        self.agent_picker_sel = AgentKind::ALL.len() - 1;
                    } else {
                        self.agent_picker_sel -= 1;
                    }
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    self.agent_picker_sel = (self.agent_picker_sel + 1) % AgentKind::ALL.len();
                }
                KeyCode::Char('1') => self.agent_picker_sel = 0,
                KeyCode::Char('2') => self.agent_picker_sel = 1,
                KeyCode::Char('3') => self.agent_picker_sel = 2,
                KeyCode::Char('4') => self.agent_picker_sel = 3,
                KeyCode::Char('d') | KeyCode::Char('*') => {
                    self.set_default_agent_from_picker();
                }
                KeyCode::Enter => {
                    let agent = AgentKind::ALL
                        .get(self.agent_picker_sel)
                        .copied()
                        .unwrap_or_default();
                    self.queue_agent_launch(agent).await;
                }
                _ => {}
            }
            return;
        }

        // Model picker modal (Prompt · m)
        if self.model_picker_open {
            let n = PROVENANCE_MODELS.len();
            match key {
                KeyCode::Esc | KeyCode::Char('q') => {
                    self.model_picker_open = false;
                    self.status = "Model picker closed".into();
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    if self.model_picker_sel == 0 {
                        self.model_picker_sel = n - 1;
                    } else {
                        self.model_picker_sel -= 1;
                    }
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    self.model_picker_sel = (self.model_picker_sel + 1) % n;
                }
                KeyCode::PageUp => {
                    self.model_picker_sel = self.model_picker_sel.saturating_sub(8);
                }
                KeyCode::PageDown => {
                    self.model_picker_sel = (self.model_picker_sel + 8).min(n.saturating_sub(1));
                }
                KeyCode::Home => self.model_picker_sel = 0,
                KeyCode::End => self.model_picker_sel = n.saturating_sub(1),
                KeyCode::Enter => {
                    let slug = PROVENANCE_MODELS
                        .get(self.model_picker_sel)
                        .map(|m| m.slug)
                        .unwrap_or(PROVENANCE_MODELS[0].slug);
                    match self.template_store.set_provenance_model(slug) {
                        Ok(selected) => {
                            let selected = selected.to_string();
                            let label =
                                crate::templates::provenance_model_label(&selected).to_string();
                            self.model_picker_open = false;
                            self.rebuild_prompt().await;
                            self.status =
                                format!("Model → {label} ({selected}) · y reasoning · w harness");
                            self.error = None;
                        }
                        Err(e) => {
                            self.error = Some(format!("{e:#}"));
                            self.status = "Could not set model".into();
                        }
                    }
                }
                _ => {}
            }
            return;
        }

        // Claims paste modal (API key / session after sign-in)
        if self.claims_paste_open {
            match key {
                KeyCode::Esc => {
                    self.claims_paste_open = false;
                    self.claims_paste_buf.clear();
                    self.status = "Claims paste cancelled".into();
                }
                KeyCode::Enter => {
                    self.apply_claims_paste();
                }
                KeyCode::Backspace | KeyCode::Delete => {
                    self.claims_paste_buf.pop();
                }
                KeyCode::Char(_) if !mods.contains(KeyModifiers::CONTROL) => {
                    if let Some(c) = typed {
                        self.claims_paste_buf.push(c);
                    }
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
            if self.editing_local_repo {
                match key {
                    KeyCode::Esc => {
                        self.editing_local_repo = false;
                        self.local_repo_input.clear();
                        self.status = "Local path edit cancelled".into();
                    }
                    KeyCode::Enter => match self.commit_local_repo_edit() {
                        Ok(()) => {
                            self.editing_local_repo = false;
                            self.local_repo_input.clear();
                            self.error = None;
                        }
                        Err(e) => {
                            self.error = Some(format!("{e:#}"));
                            self.status = "Local path not saved".into();
                        }
                    },
                    KeyCode::Backspace | KeyCode::Delete => {
                        self.local_repo_input.pop();
                    }
                    KeyCode::Char(_) if !mods.contains(KeyModifiers::CONTROL) => {
                        if let Some(c) = typed {
                            // Paths: allow spaces, ~, /, ., etc.
                            if !c.is_control() {
                                self.local_repo_input.push(c);
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
                            "type URL · enter load · Tab · v convention · r path · Shift+s · q"
                                .into();
                    }
                }
                KeyCode::Tab => {
                    self.setup_list_focus = !self.setup_list_focus;
                    self.status = if self.setup_list_focus {
                        "Focus: project list (j/k enter · v convention · r path · d delete)"
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
                KeyCode::Char('r') if self.setup_list_focus => {
                    self.begin_edit_local_repo();
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
                            && matches!(
                                c,
                                'j' | 'k' | 'd' | 'v' | 'r' | 'J' | 'K' | 'D' | 'V' | 'R'
                            )
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
                    "Project list · j/k enter · v convention · r local path · Shift+s save · d"
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
                self.screen = Screen::Priorities;
                self.rebuild_priorities();
                self.status = format!("Priorities · {}", self.priority_mode.label());
            }
            KeyCode::Char('3') => {
                self.screen = Screen::Prompt;
                self.rebuild_prompt().await;
                self.status = "Prompt".into();
            }
            KeyCode::Char('4') => {
                self.screen = Screen::Claims;
                self.status = format!("Claims · {}", self.claims_status);
            }
            KeyCode::Char('5') => {
                self.screen = Screen::Tools;
                self.refresh_tools_list();
                self.status = self.tools_status_line();
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
            // g = default agent; Shift+g = agent picker (Prompt page; needs batch).
            KeyCode::Char('g') if self.screen == Screen::Prompt => {
                if mods.contains(KeyModifiers::SHIFT) {
                    self.open_agent_picker();
                } else {
                    let agent = self.default_agent();
                    self.queue_agent_launch(agent).await;
                }
            }
            // Mass batcher: , / . (and < / >) switch active batch slot.
            KeyCode::Char(',') | KeyCode::Char('<') => {
                self.cycle_batch_slot(-1).await;
            }
            KeyCode::Char('.') | KeyCode::Char('>') => {
                self.cycle_batch_slot(1).await;
            }
            // + / = open a new empty batch after the active slot.
            KeyCode::Char('+') | KeyCode::Char('=') => {
                self.new_empty_batch().await;
            }
            // L = claim selected; A = claim active batch (tangos.dev / claimsApi).
            KeyCode::Char('l') if mods.contains(KeyModifiers::SHIFT) => {
                self.claim_selected_function().await;
            }
            KeyCode::Char('a') if mods.contains(KeyModifiers::SHIFT) => {
                self.claim_all_batches().await;
            }
            // Clear-all must not rely only on Ctrl+b (tmux prefix / terminals eat it).
            // Shift+b and Ctrl+b both wipe every mass-batcher slot.
            // Shift+Backspace clears only the active slot.
            KeyCode::Char('b')
                if mods.contains(KeyModifiers::CONTROL) || mods.contains(KeyModifiers::SHIFT) =>
            {
                self.clear_all_batches().await;
            }
            KeyCode::Backspace | KeyCode::Delete if mods.contains(KeyModifiers::SHIFT) => {
                self.clear_batch().await;
            }
            KeyCode::Char('b') => self.toggle_batch_selected().await,
            KeyCode::Char('u') => {
                if let Err(e) = self.update_progress().await {
                    self.error = Some(format!("{e:#}"));
                    self.status = "Update failed".into();
                }
            }
            KeyCode::Char('r') if self.screen == Screen::Tools => {
                self.refresh_tools_list();
                self.status = format!("{} · rescan", self.tools_status_line());
            }
            KeyCode::Char('r') => {
                if let Some(db) = self.db.clone() {
                    self.refresh_claims(&db).await;
                    self.rebuild_priorities();
                    self.status = format!("Claims refreshed · {}", self.claims_status);
                }
            }
            // Claims page write controls (also L/A work globally above).
            KeyCode::Char('i') if self.screen == Screen::Claims => {
                self.claims_sign_in().await;
            }
            KeyCode::Char('o') if self.screen == Screen::Claims => {
                self.claims_sign_out();
            }
            KeyCode::Char('y') if self.screen == Screen::Claims => {
                self.renew_my_claims().await;
            }
            KeyCode::Char('x') if self.screen == Screen::Claims => {
                self.release_my_claims().await;
            }
            KeyCode::Char('n') if self.screen == Screen::Priorities => {
                self.priority_mode = match self.priority_mode {
                    PriorityMode::Nearly => PriorityMode::Scaffolded,
                    PriorityMode::Scaffolded => PriorityMode::Biggest,
                    PriorityMode::Biggest => PriorityMode::Smallest,
                    PriorityMode::Smallest => PriorityMode::Nearly,
                };
                self.rebuild_priorities();
                self.status = format!(
                    "Priority mode: {} ({} rows)",
                    self.priority_mode.label(),
                    self.priority_list.len()
                );
            }
            KeyCode::Char('n') if self.screen == Screen::Tools => {
                self.tools_filter = self.tools_filter.cycle();
                self.refresh_tools_list();
                self.status = self.tools_status_line();
            }
            KeyCode::Char('h') if self.screen == Screen::Tools => {
                self.move_tools_sel(-1);
            }
            KeyCode::Char('l') if self.screen == Screen::Tools => {
                self.move_tools_sel(1);
            }
            KeyCode::Left if self.screen == Screen::Tools => {
                self.move_tools_sel(-1);
            }
            KeyCode::Right if self.screen == Screen::Tools => {
                self.move_tools_sel(1);
            }
            KeyCode::PageUp if self.screen == Screen::Tools => {
                self.move_tools_sel(-4);
            }
            KeyCode::PageDown if self.screen == Screen::Tools => {
                self.move_tools_sel(4);
            }
            KeyCode::Char('m') if self.screen == Screen::Overview => {
                self.match_filter = self.match_filter.cycle();
                if let Some(db) = self.db.clone() {
                    self.rebuild_modules(&db);
                }
                self.rebuild_functions();
                self.ensure_selected_detail();
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
                self.ensure_selected_detail();
                self.status = format!(
                    "Module sort: {} ({} modules)",
                    self.module_sort.label(),
                    self.module_list.len()
                );
            }
            KeyCode::Up | KeyCode::Char('k') => self.move_sel(-1).await,
            KeyCode::Down | KeyCode::Char('j') => self.move_sel(1).await,
            KeyCode::Char('d') if self.screen == Screen::Prompt => {
                self.include_near_miss_draft = !self.include_near_miss_draft;
                self.rebuild_prompt().await;
                self.status = if self.include_near_miss_draft {
                    "Near-miss drafts ON · stored NONMATCHING C included when present".into()
                } else {
                    "Near-miss drafts OFF · matching from disasm (and Ghidra if h on) only".into()
                };
            }
            KeyCode::Char('h') if self.screen == Screen::Prompt => {
                self.include_ghidra_draft = !self.include_ghidra_draft;
                self.rebuild_prompt().await;
                let dir = self.resolve_ghidra_dir();
                self.status = if self.include_ghidra_draft {
                    match dir {
                        Some(p) => format!("Ghidra draft ON · {}", p.display()),
                        None => {
                            "Ghidra draft ON · no ghidra_out found (set local_repo or CHAOS_GHIDRA_DIR)"
                                .into()
                        }
                    }
                } else {
                    "Ghidra draft OFF".into()
                };
            }
            KeyCode::Char('m') if self.screen == Screen::Prompt => {
                self.open_model_picker();
            }
            KeyCode::Char('y') if self.screen == Screen::Prompt => {
                match self.template_store.cycle_provenance_reasoning(1) {
                    Ok(level) => {
                        let level = level.to_string();
                        self.rebuild_prompt().await;
                        self.status = format!(
                            "Reasoning → {level}  (high · medium · low · none · prefills MATCH_RESULT)"
                        );
                        self.error = None;
                    }
                    Err(e) => {
                        self.error = Some(format!("{e:#}"));
                        self.status = "Could not cycle reasoning".into();
                    }
                }
            }
            KeyCode::Char('w') if self.screen == Screen::Prompt => {
                match self.template_store.cycle_provenance_harness(1) {
                    Ok(harness) => {
                        let harness = harness.to_string();
                        self.rebuild_prompt().await;
                        self.status = format!(
                            "Harness → {harness}  (m model picker · y reasoning · prefills MATCH_RESULT)"
                        );
                        self.error = None;
                    }
                    Err(e) => {
                        self.error = Some(format!("{e:#}"));
                        self.status = "Could not cycle harness".into();
                    }
                }
            }
            KeyCode::Left | KeyCode::Char('h') if self.screen == Screen::Overview => {
                self.apply_module_delta(-1).await;
            }
            KeyCode::Right | KeyCode::Char('l') if self.screen == Screen::Overview => {
                self.apply_module_delta(1).await;
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
                            self.ensure_selected_detail();
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
            Screen::Claims => {
                self.status = format!("Claims · {}", self.claims_status);
            }
            Screen::Overview => {
                self.ensure_selected_detail();
                self.status = "Overview".into();
            }
            Screen::Tools => {
                self.refresh_tools_list();
                self.status = self.tools_status_line();
            }
            Screen::Setup => {}
        }
    }

    fn active_local_repo(&self) -> Option<std::path::PathBuf> {
        let id = self.project_store.active_id.as_ref()?;
        let prof = self.project_store.get(id)?;
        let repo = prof.local_repo.as_ref()?;
        let p = expand_user_path_tui(repo);
        if p.is_dir() {
            Some(p)
        } else {
            None
        }
    }

    fn refresh_tools_list(&mut self) {
        self.tools_indices = filtered_indices(self.tools_filter.as_category());
        if self.tools_sel >= self.tools_indices.len() {
            self.tools_sel = self.tools_indices.len().saturating_sub(1);
        }
        self.ensure_tools_scroll(6);
    }

    fn tools_status_line(&self) -> String {
        let n = self.tools_indices.len();
        let present = self
            .active_local_repo()
            .map(|repo| {
                self.tools_indices
                    .iter()
                    .filter(|&&i| tool_present(&repo, &TOOL_CARDS[i]))
                    .count()
            })
            .unwrap_or(0);
        let repo_bit = match self.active_local_repo() {
            Some(p) => format!("local_repo {}", p.display()),
            None => "no local_repo (set r on projects)".into(),
        };
        format!(
            "Tools · filter {} · {n} cards · {present} found · {repo_bit}",
            self.tools_filter.label()
        )
    }

    fn move_tools_sel(&mut self, delta: isize) {
        if self.tools_indices.is_empty() {
            return;
        }
        let n = self.tools_indices.len() as isize;
        let i = self.tools_sel as isize + delta;
        self.tools_sel = (((i % n) + n) % n) as usize;
        self.ensure_tools_scroll(6);
    }

    fn ensure_tools_scroll(&mut self, visible_rows: usize) {
        if self.tools_indices.is_empty() || visible_rows == 0 {
            self.tools_row_offset = 0;
            return;
        }
        let row = self.tools_sel / 2;
        if row < self.tools_row_offset {
            self.tools_row_offset = row;
        } else if row >= self.tools_row_offset + visible_rows {
            self.tools_row_offset = row + 1 - visible_rows;
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

    /// Apply coalesced module navigation (h/l). One rebuild + one detail load.
    async fn apply_module_delta(&mut self, delta: isize) {
        if delta == 0 || self.module_list.is_empty() {
            return;
        }
        self.move_module(delta);
        self.rebuild_functions();
        self.ensure_selected_detail();
    }

    async fn move_sel(&mut self, delta: isize) {
        match self.screen {
            Screen::Tools => {
                // j/k move by one card; two columns so Down goes to next row.
                let step = if delta > 0 { 2 } else { -2 };
                // Prefer vertical feel: j/k jump a row when possible.
                let n = self.tools_indices.len() as isize;
                if n == 0 {
                    return;
                }
                let mut next = self.tools_sel as isize + step;
                if next < 0 {
                    next = (n - 1) - ((n - 1) % 2); // last row left card-ish
                    if next >= n {
                        next = n - 1;
                    }
                } else if next >= n {
                    next = if delta > 0 { 0 } else { n - 1 };
                }
                // If stepped off the end of a short last row, clamp.
                if next >= n {
                    next = n - 1;
                }
                self.tools_sel = next as usize;
                self.ensure_tools_scroll(6);
            }
            Screen::Overview => {
                if self.fn_list.is_empty() {
                    return;
                }
                let n = self.fn_list.len() as isize;
                let i = self.fn_sel as isize + delta;
                let i = ((i % n) + n) % n;
                self.fn_sel = i as usize;
                self.sync_selection_from_fn();
                self.ensure_selected_detail();
            }
            Screen::Priorities => {
                if self.priority_list.is_empty() {
                    return;
                }
                let n = self.priority_list.len() as isize;
                let i = self.priority_sel as isize + delta;
                let i = ((i % n) + n) % n;
                self.priority_sel = i as usize;
                self.sync_selection_from_priority();
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
            Screen::Priorities => self.draw_priorities(f, chunks[1]),
            Screen::Prompt => self.draw_prompt(f, chunks[1]),
            Screen::Claims => self.draw_claims(f, chunks[1]),
            Screen::Tools => self.draw_tools(f, chunks[1]),
        }
        self.draw_footer(f, chunks[2]);

        if self.agent_picker_open {
            self.draw_agent_picker_overlay(f, area);
        }
        if self.model_picker_open {
            self.draw_model_picker_overlay(f, area);
        }
        if self.show_help {
            self.draw_help_overlay(f, area);
        }
    }

    fn draw_agent_picker_overlay(&self, f: &mut Frame, area: Rect) {
        let w = area.width.saturating_sub(10).min(56);
        let h = area.height.saturating_sub(6).min(14);
        let x = area.x + (area.width.saturating_sub(w)) / 2;
        let y = area.y + (area.height.saturating_sub(h)) / 2;
        let rect = Rect::new(x, y, w, h);
        let bg = self.theme.panel;
        let def = self.default_agent();

        // Dim the background slightly by clearing the modal region only.
        f.render_widget(Clear, rect);

        let title = format!(
            " coding agent  ·  default: {}  ·  d set · enter launch ",
            def.label()
        );
        let block = Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(paint_on(self.theme.accent, bg))
            .style(paint_on(self.theme.text, bg));
        let inner = block.inner(rect);
        f.render_widget(block, rect);
        fill_pane(f, inner, &self.theme, bg);

        let mut lines: Vec<Line<'static>> = Vec::new();
        lines.push(Line::from(Span::styled(
            "  j/k move · 1–4 jump · enter open · d default · esc",
            paint_on(self.theme.muted, bg),
        )));
        lines.push(Line::from(""));
        for (i, agent) in AgentKind::ALL.iter().enumerate() {
            let selected = i == self.agent_picker_sel;
            let is_def = *agent == def;
            let mark = if selected { "›" } else { " " };
            let star = if is_def { "★" } else { " " };
            let row = format!(
                " {mark} {star} {}. {:<14}  ({})",
                i + 1,
                agent.label(),
                agent.short_bin()
            );
            let style = if selected {
                paint_bold_on(self.theme.accent, bg)
            } else {
                paint_on(self.theme.text, bg)
            };
            lines.push(Line::from(Span::styled(row, style)));
        }
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "  needs local_repo · batch not empty",
            paint_on(self.theme.muted, bg),
        )));

        f.render_widget(
            Paragraph::new(lines)
                .style(paint_on(self.theme.text, bg))
                .wrap(Wrap { trim: false }),
            inner,
        );
    }

    fn draw_model_picker_overlay(&self, f: &mut Frame, area: Rect) {
        let n = PROVENANCE_MODELS.len();
        let w = area.width.saturating_sub(8).min(52);
        // Header + list rows + footer; cap so short terminals still fit.
        let h = area
            .height
            .saturating_sub(4)
            .min((n as u16).saturating_add(5).min(24));
        let x = area.x + (area.width.saturating_sub(w)) / 2;
        let y = area.y + (area.height.saturating_sub(h)) / 2;
        let rect = Rect::new(x, y, w, h);
        let bg = self.theme.panel;
        let current = self.template_store.provenance_model();

        f.render_widget(Clear, rect);

        let title = format!(
            " model  ·  current: {}  ·  enter select ",
            self.template_store.provenance_model_label()
        );
        let block = Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(paint_on(self.theme.accent, bg))
            .style(paint_on(self.theme.text, bg));
        let inner = block.inner(rect);
        f.render_widget(block, rect);
        fill_pane(f, inner, &self.theme, bg);

        // Visible window of models (scroll with selection).
        let header_lines: u16 = 2; // hint + blank
        let footer_lines: u16 = 2; // blank + note
        let list_h = inner
            .height
            .saturating_sub(header_lines + footer_lines)
            .max(1) as usize;
        let sel = self.model_picker_sel.min(n.saturating_sub(1));
        let mut start = sel.saturating_sub(list_h.saturating_sub(1) / 2);
        if start + list_h > n {
            start = n.saturating_sub(list_h);
        }

        let mut lines: Vec<Line<'static>> = Vec::new();
        lines.push(Line::from(Span::styled(
            "  j/k · pgup/pgdn · enter use · esc",
            paint_on(self.theme.muted, bg),
        )));
        lines.push(Line::from(""));
        for (i, model) in PROVENANCE_MODELS
            .iter()
            .enumerate()
            .skip(start)
            .take(list_h)
        {
            let selected = i == sel;
            let is_cur = model.slug == current;
            let mark = if selected { "›" } else { " " };
            let star = if is_cur { "★" } else { " " };
            let row = format!(" {mark} {star} {:<20}  {}", model.label, model.slug);
            let style = if selected {
                paint_bold_on(self.theme.accent, bg)
            } else {
                paint_on(self.theme.text, bg)
            };
            lines.push(Line::from(Span::styled(row, style)));
        }
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            format!("  prefills matchProvenance.model  ·  {}/{}", sel + 1, n),
            paint_on(self.theme.muted, bg),
        )));

        f.render_widget(
            Paragraph::new(lines)
                .style(paint_on(self.theme.text, bg))
                .wrap(Wrap { trim: false }),
            inner,
        );
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
            let batch_bit = if self.total_batched() == 0 {
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
                " chaos  ·  {proj}  ·  {}/{} fn ({}%)  ·  {}/{} B ({}%)  ·  {batch_bit}  ·  gen {gen}  ·  p projects",
                db.stats.matched_functions,
                db.stats.total_functions,
                format_pct(db.stats.matched_functions, db.stats.total_functions),
                db.stats.matched_bytes,
                db.stats.total_bytes,
                format_pct(db.stats.matched_bytes, db.stats.total_bytes),
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
        let key_w = rows[1].width.max(1);
        f.render_widget(
            Paragraph::new(key_line_fit(&self.theme, &self.global_hints(), bg, key_w))
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
            let ctx_w = rows[2].width.max(1);
            f.render_widget(
                Paragraph::new(key_line_fit(&self.theme, &ctx, bg, ctx_w))
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
        } else if self.editing_local_repo {
            let id = self
                .project_store
                .projects
                .get(self.project_sel)
                .map(|p| p.id.as_str())
                .unwrap_or("?");
            format!(" Local decomp path for '{id}': {}_ ", self.local_repo_input)
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
        let list_title =
            if self.setup_list_focus && !self.saving_project && !self.editing_local_repo {
                " Saved projects  [focused]  j/k enter v=convention r=path d "
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
                let local = p
                    .local_repo
                    .as_deref()
                    .map(|r| format!("  · local:{r}"))
                    .unwrap_or_else(|| "  · local:(unset)".into());
                list_lines.push(Line::from(Span::styled(
                    format!(
                        "{mark}{star}{:<14}  [{:<12}]  {}{local}",
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

        // Source input — or local path editor when r-mode is active.
        let (input_title, input_value) = if self.editing_local_repo {
            (
                " Local decomp path  [editing]  enter save · empty clears · esc cancel ",
                self.local_repo_input.as_str(),
            )
        } else if !self.setup_list_focus && !self.saving_project {
            (
                " Source  [focused]  path · URL · GitHub ",
                self.setup_input.as_str(),
            )
        } else {
            (" Source  path · URL · GitHub ", self.setup_input.as_str())
        };
        let input_block = content_block(input_title, &self.theme, self.theme.border);
        let input_inner = input_block.inner(rows[1]);
        f.render_widget(input_block, rows[1]);
        fill_pane(f, input_inner, &self.theme, bg);
        let input_line = format!("> {input_value}_");
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
        } else if self.editing_local_repo {
            "Type absolute or ~/ path · enter save (must exist) · empty + enter clears · esc cancel"
        } else {
            "j/k list · enter load · v convention · r local path (Grok) · Tab/type URL · Shift+s save · d delete · q quit"
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

        // Module lines — only the visible viewport (not the whole 70+ list every frame).
        let mod_height = cols[0].height.saturating_sub(2) as usize;
        Self::clamp_scroll(
            self.module_sel,
            &mut self.module_offset,
            self.module_list.len(),
            mod_height,
        );
        let mod_end = (self.module_offset + mod_height).min(self.module_list.len());
        let mod_lines: Vec<Line<'static>> = (self.module_offset..mod_end)
            .map(|i| {
                let m = &self.module_list[i];
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
        // offset 0: lines are already the visible window
        Self::draw_line_list(f, cols[0], mod_title, &self.theme, &mod_lines, 0);

        // Function lines — viewport only (arm9 alone can be ~3000 rows).
        let fn_height = cols[1].height.saturating_sub(2) as usize;
        Self::clamp_scroll(
            self.fn_sel,
            &mut self.fn_offset,
            self.fn_list.len(),
            fn_height,
        );
        let fn_end = (self.fn_offset + fn_height).min(self.fn_list.len());
        let fn_lines: Vec<Line<'static>> = (self.fn_offset..fn_end)
            .map(|list_i| {
                let idx = self.fn_list[list_i];
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
                let in_batch = self.batch_membership(&f.id).is_some();
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
        // Batch count in *this module* is O(batched), not O(all functions).
        let mod_name = self.selected_module();
        let batched_here = self
            .batches
            .iter()
            .flatten()
            .filter(|id| {
                self.id_index
                    .get(id.as_str())
                    .and_then(|&i| db.functions.get(i))
                    .is_some_and(|f| mod_name == Some(f.module.as_str()))
            })
            .count();
        let filter = self.match_filter.label();
        let title = if self.search.is_empty() {
            format!(
                " Functions ({}) · {filter} · batch {} ({} here) · m · b · ,/. ",
                self.fn_list.len(),
                self.batch_summary(),
                batched_here
            )
        } else {
            format!(
                " Functions ({}) · {filter} · /{} · batch {} · m · esc done ",
                self.fn_list.len(),
                self.search,
                self.batch_summary()
            )
        };
        Self::draw_line_list(f, cols[1], title, &self.theme, &fn_lines, 0);

        // Detail strip under both lists.
        self.draw_detail_pane(f, rows[1]);
    }

    fn draw_priorities(&mut self, f: &mut Frame, area: Rect) {
        let Some(db) = &self.db else { return };
        let title = format!(
            " {}  ·  {} rows  ·  batch {}  ·  n cycle · enter · b · ,/. ",
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
        let pri_end = (self.priority_offset + height).min(self.priority_list.len());
        let lines: Vec<Line<'static>> = (self.priority_offset..pri_end)
            .map(|list_i| {
                let idx = self.priority_list[list_i];
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
                    PriorityMode::Biggest | PriorityMode::Smallest => format!("{}B", f.size),
                };
                let in_batch = self.batch_membership(&f.id).is_some();
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
        Self::draw_line_list(f, area, title, &self.theme, &lines, 0);
    }

    /// Detail panel used under Overview (modules + functions).
    fn draw_detail_pane(&mut self, f: &mut Frame, area: Rect) {
        let bg = self.theme.bg;
        let panel = self.theme.panel;
        let has_fn = self.selected_function().is_some();
        let batched = self
            .selected_function()
            .and_then(|f| self.batch_membership(&f.id));
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
        let active_ids = self.active_batch_ids();
        let roster: String = if active_ids.is_empty() {
            if self.total_batched() == 0 {
                "batch empty — press b on Overview or Priorities · overflow past 16 opens batch 2+"
                    .into()
            } else {
                format!(
                    "active batch {} empty — ,/. switch · {} fn in other batch(es)",
                    self.active_batch + 1,
                    self.total_batched()
                )
            }
        } else if let Some(db) = &self.db {
            let bi = self.active_batch + 1;
            active_ids
                .iter()
                .enumerate()
                .filter_map(|(i, id)| {
                    db.find_by_id(id).map(|f| {
                        if self.batches.len() <= 1 {
                            format!("[B{}] {}", i + 1, f.name)
                        } else {
                            format!("[{bi}:{}] {}", i + 1, f.name)
                        }
                    })
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
                " Prompt  ·  {}  ·  batch {}  ·  {}/{}  {}  ·  drafts:{}  Ghidra:{}  ·  ,/. · + · g all · c ",
                self.prompt_template_label(),
                self.batch_summary(),
                self.template_store.provenance_model(),
                self.template_store.provenance_reasoning(),
                self.template_store.provenance_harness(),
                if self.include_near_miss_draft {
                    "on"
                } else {
                    "off"
                },
                if self.include_ghidra_draft {
                    "on"
                } else {
                    "off"
                },
            )
        };
        let border = if active_ids.is_empty() {
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

        let roster_fg = if active_ids.is_empty() {
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
        let api = self
            .claims_api_base()
            .unwrap_or_else(|| "(no claimsApi)".into());
        let title = if self.claims_paste_open {
            " Claims  ·  paste API key [/ handle]  ·  enter save · esc cancel "
        } else {
            " Claims  ·  live locks + try-lock  ·  i sign-in · L claim · A all batches "
        };
        let border = if self
            .claims_session
            .as_ref()
            .map(|s| s.is_ready())
            .unwrap_or(false)
        {
            self.theme.claim
        } else {
            self.theme.border
        };
        let block = content_block(title, &self.theme, border);
        let inner = block.inner(area);
        f.render_widget(block, area);
        fill_pane(f, inner, &self.theme, bg);

        let mut lines = vec![
            format!("coordinator: {api}"),
            format!("session:     {}", self.claims_session_label()),
            format!("status:      {}", self.claims_status),
            format!(
                "locked now:  {} functions · my claims: {}",
                self.locked_by.len(),
                self.my_claims.len()
            ),
            String::new(),
        ];
        if self.claims_paste_open {
            lines.push(format!("paste> {}_", self.claims_paste_buf));
            lines.push(
                "Discord: DM bot `key` · or GitHub session token · optional second word = handle"
                    .into(),
            );
            lines.push(String::new());
        } else {
            lines.push(
                "Keys: i sign-in · o sign-out · L claim selected · A claim ALL batches · y renew · x release · r refresh"
                    .into(),
            );
            lines.push(String::new());
        }

        if !self.my_claims.is_empty() {
            let who = self
                .claims_session
                .as_ref()
                .map(|s| s.handle.as_str())
                .filter(|h| !h.is_empty())
                .unwrap_or("you");
            lines.push(format!(
                "My claims (this machine · handle {who} · y renew · x release):"
            ));
            // Name + module first (human-readable). Claim API id last — only needed
            // for renew/release, not as the primary label.
            for c in self.my_claims.iter().take(16) {
                let label = if c.name.is_empty() {
                    format!("0x{:x}", c.start)
                } else {
                    c.name.clone()
                };
                lines.push(format!(
                    "  {who:16}  {mod_}  {label}  0x{start:x}-0x{end:x}  ({id})",
                    who = who,
                    mod_ = c.module,
                    label = label,
                    start = c.start,
                    end = c.end,
                    id = c.id,
                ));
            }
            if self.my_claims.len() > 16 {
                lines.push(format!("  … +{} more", self.my_claims.len() - 16));
            }
            lines.push(String::new());
        }

        lines.push("Active locks (sample · who → function id):".into());
        let mut entries: Vec<_> = self.locked_by.iter().collect();
        entries.sort_by(|a, b| a.0.cmp(b.0));
        for (fn_id, handle) in entries.into_iter().take(24) {
            lines.push(format!("  {handle:20}  {fn_id}"));
        }
        if self.locked_by.is_empty() {
            lines.push("  (none right now)".into());
            lines.push(
                "Claims appear when project.claimsApi is set (sm64ds → tangos.dev) or CLAIMS.md has rows."
                    .into(),
            );
        }
        f.render_widget(
            Paragraph::new(lines.join("\n")).style(paint_on(self.theme.text, bg)),
            inner,
        );
    }

    fn draw_tools(&self, f: &mut Frame, area: Rect) {
        let bg = self.theme.bg;
        let repo = self.active_local_repo();
        let title = format!(
            " Tools  ·  {}  ·  n filter  ·  ★ in local_repo ",
            self.tools_filter.label()
        );
        let block = content_block(title.as_str(), &self.theme, self.theme.border);
        let inner = block.inner(area);
        f.render_widget(block, area);
        fill_pane(f, inner, &self.theme, bg);

        if self.tools_indices.is_empty() {
            f.render_widget(
                Paragraph::new("No tools in this filter.").style(paint_on(self.theme.muted, bg)),
                inner,
            );
            return;
        }

        // Header strip: repo path
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(3)])
            .split(inner);
        let repo_line = match &repo {
            Some(p) => format!("local_repo: {}", p.display()),
            None => "local_repo: (not set — project hub r · cards still show catalog)".into(),
        };
        f.render_widget(
            Paragraph::new(repo_line).style(paint_on(self.theme.muted, bg)),
            chunks[0],
        );

        let grid = chunks[1];
        // Card size: 2 columns, ~7 rows tall each.
        const CARD_H: u16 = 7;
        const COLS: usize = 2;
        let rows_fit = (grid.height / CARD_H).max(1) as usize;
        // Keep scroll in range for this height (draw-only clamp).
        let total_rows = self.tools_indices.len().div_ceil(COLS);
        let max_off = total_rows.saturating_sub(rows_fit);
        let row_off = self.tools_row_offset.min(max_off);

        let start = row_off * COLS;
        let end = (start + rows_fit * COLS).min(self.tools_indices.len());
        let visible = &self.tools_indices[start..end];

        let n_vis_rows = visible.len().div_ceil(COLS).max(1);
        let row_constraints: Vec<Constraint> = (0..n_vis_rows)
            .map(|_| Constraint::Length(CARD_H))
            .collect();
        let row_layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints(row_constraints)
            .split(grid);

        for (r, row_area) in row_layout.iter().enumerate() {
            let left_i = r * COLS;
            let right_i = left_i + 1;
            let col_layout = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
                .split(*row_area);

            if let Some(&card_idx) = visible.get(left_i) {
                let sel = start + left_i == self.tools_sel;
                self.draw_tool_card(f, col_layout[0], card_idx, sel, repo.as_deref());
            }
            if let Some(&card_idx) = visible.get(right_i) {
                let sel = start + right_i == self.tools_sel;
                self.draw_tool_card(f, col_layout[1], card_idx, sel, repo.as_deref());
            }
        }
    }

    fn draw_tool_card(
        &self,
        f: &mut Frame,
        area: Rect,
        card_idx: usize,
        selected: bool,
        repo: Option<&std::path::Path>,
    ) {
        let card = &TOOL_CARDS[card_idx];
        let bg = self.theme.panel;
        let present = repo.is_some_and(|r| tool_present(r, card));
        let border = if selected {
            self.theme.accent
        } else if present {
            self.theme.matched
        } else {
            self.theme.border
        };
        let star = if present { "★" } else { " " };
        let title = format!(" {star} {} · {} ", card.name, card.category.label());
        let block = Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(paint_on(border, bg))
            .style(paint_on(self.theme.text, bg));
        let inner = block.inner(area);
        f.render_widget(block, area);
        fill_pane(f, inner, &self.theme, bg);

        let path_bit = repo
            .and_then(|r| tool_found_path(r, card))
            .map(|p| {
                // Prefer path relative to repo when possible.
                repo.and_then(|root| {
                    p.strip_prefix(root)
                        .ok()
                        .map(|rel| rel.display().to_string())
                })
                .unwrap_or_else(|| p.display().to_string())
            })
            .unwrap_or_else(|| card.detect.first().copied().unwrap_or("—").to_string());

        let lines = vec![
            Line::from(Span::styled(
                truncate_for_card(card.summary, inner.width),
                if selected {
                    paint_bold_on(self.theme.text, bg)
                } else {
                    paint_on(self.theme.text, bg)
                },
            )),
            Line::from(Span::styled(
                truncate_for_card(&format!("changes: {}", card.changes), inner.width),
                paint_on(self.theme.muted, bg),
            )),
            Line::from(Span::styled(
                truncate_for_card(
                    &format!(
                        "{}  {}",
                        if present { "found" } else { "missing" },
                        path_bit
                    ),
                    inner.width,
                ),
                paint_on(
                    if present {
                        self.theme.matched
                    } else {
                        self.theme.muted
                    },
                    bg,
                ),
            )),
        ];
        f.render_widget(
            Paragraph::new(lines)
                .style(paint_on(self.theme.text, bg))
                .wrap(Wrap { trim: true }),
            inner,
        );
    }
}

fn truncate_for_card(s: &str, width: u16) -> String {
    let w = width as usize;
    if w == 0 {
        return String::new();
    }
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i + 1 >= w {
            if w > 1 {
                out.push('…');
            }
            break;
        }
        out.push(ch);
    }
    out
}

/// Run the interactive TUI. Optional initial input loads immediately.
pub async fn run(
    input: Option<String>,
    repo: Option<String>,
    branch: Option<String>,
    project: Option<String>,
) -> Result<()> {
    let claims_session = ClaimsSession::load();
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

/// Expand `~/…` using `$HOME` / `%USERPROFILE%` (TUI local_repo editor).
fn expand_user_path_tui(raw: &str) -> std::path::PathBuf {
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

async fn run_loop(
    terminal: &mut Terminal<ratatui::backend::CrosstermBackend<io::Stdout>>,
    app: &mut App,
) -> Result<()> {
    // Event-driven redraw. Only paint after input/resize/state changes (no free-spin).
    //
    // Idle ticks (~200 ms) still run so background detail prewarm can apply
    // finished module chunks without blocking key handling.
    let mut dirty = true;
    loop {
        if dirty {
            terminal.draw(|f| app.draw(f))?;
            dirty = false;
        }

        // Block until input (or a short idle tick for prewarm progress).
        if !event::poll(Duration::from_millis(200))? {
            if app.should_quit {
                break;
            }
            // Background detail chunks finished? Apply + keep prewarm pipeline fed.
            if app.poll_detail_prewarm() {
                dirty = true;
            }
            // No events — stay idle aside from prewarm. Editor handoff below only
            // after keys set `pending_edit`.
            if app.pending_edit.is_none() {
                continue;
            }
        } else {
            dirty = true;
            // Drain the queue. Coalesce rapid list nav so holding a key only
            // pays one selection update (+ one detail load) per paint.
            let mut nav_delta: isize = 0;
            let mut mod_delta: isize = 0;
            let mut other_keys: Vec<KeyEvent> = Vec::new();
            // Process the event that made poll return true, then drain the rest.
            loop {
                match event::read()? {
                    Event::Key(key) => {
                        if key.kind == KeyEventKind::Release {
                            // fall through to drain check
                        } else {
                            let code = match key.code {
                                KeyCode::Char(c) => KeyCode::Char(c.to_ascii_lowercase()),
                                other => other,
                            };
                            let overview_idle = app.screen == Screen::Overview
                                && !app.searching
                                && !app.naming_template
                                && !app.show_help
                                && !app.agent_picker_open
                                && !app.model_picker_open;
                            let list_idle = matches!(
                                app.screen,
                                Screen::Overview | Screen::Priorities | Screen::Prompt
                            ) && !app.searching
                                && !app.naming_template
                                && !app.show_help
                                && !app.agent_picker_open
                                && !app.model_picker_open;
                            if list_idle
                                && matches!(
                                    code,
                                    KeyCode::Char('j')
                                        | KeyCode::Char('k')
                                        | KeyCode::Up
                                        | KeyCode::Down
                                )
                            {
                                match code {
                                    KeyCode::Char('j') | KeyCode::Down => nav_delta += 1,
                                    KeyCode::Char('k') | KeyCode::Up => nav_delta -= 1,
                                    _ => {}
                                }
                            } else if overview_idle
                                && matches!(
                                    code,
                                    KeyCode::Char('h')
                                        | KeyCode::Char('l')
                                        | KeyCode::Left
                                        | KeyCode::Right
                                )
                            {
                                // Module list: coalesce h/l so holding the key is not O(n) rebuilds.
                                match code {
                                    KeyCode::Char('l') | KeyCode::Right => mod_delta += 1,
                                    KeyCode::Char('h') | KeyCode::Left => mod_delta -= 1,
                                    _ => {}
                                }
                            } else {
                                // Flush pending nav before other keys so order stays sane.
                                if nav_delta != 0 {
                                    app.move_sel(nav_delta).await;
                                    nav_delta = 0;
                                }
                                if mod_delta != 0 {
                                    app.apply_module_delta(mod_delta).await;
                                    mod_delta = 0;
                                }
                                other_keys.push(key);
                            }
                        }
                    }
                    Event::Resize(_, _) => {
                        // Redraw with new size (dirty already set above).
                    }
                    _ => {}
                }
                if !event::poll(Duration::from_millis(0))? {
                    break;
                }
            }
            if nav_delta != 0 {
                app.move_sel(nav_delta).await;
            }
            if mod_delta != 0 {
                app.apply_module_delta(mod_delta).await;
            }
            for key in other_keys {
                app.on_key(key).await;
            }
            // Prewarm may have finished during key handling; also feed the queue.
            if app.poll_detail_prewarm() {
                dirty = true;
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
            dirty = true;
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

    #[test]
    fn contains_ignore_ascii_case_basic() {
        // needle must already be lowercased (search box lowercases once).
        assert!(contains_ignore_ascii_case("FooBar", "foo"));
        assert!(contains_ignore_ascii_case("FooBar", "obar"));
        assert!(contains_ignore_ascii_case("Arm9", "arm"));
        assert!(!contains_ignore_ascii_case("arm9", "arm10"));
        assert!(contains_ignore_ascii_case("x", ""));
    }

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
