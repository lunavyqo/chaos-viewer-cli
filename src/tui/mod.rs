//! Interactive full-screen TUI.

mod theme;

use std::collections::HashMap;
use std::io::{self, stdout};
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Tabs, Wrap};
use ratatui::{Frame, Terminal};
use reqwest::Client;

use crate::claims::{load_claims, merge_locked_map, ClaimsSession};
use crate::clipboard::copy_text;
use crate::load::{
    details_base_from_source, load_chaos_db, load_function_detail, DataSource, DetailCache,
};
use crate::prioritize::{priority_rows, PriorityMode};
use crate::prompt::{batch_max, build_prompt, PromptOptions};
use crate::schema::{format_pct, ChaosDb, ChaosFunction, FunctionDetail, ProjectConfig};
use theme::Theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Screen {
    Setup,
    Overview,
    Priorities,
    Detail,
    Prompt,
    Claims,
}

impl Screen {
    fn all_loaded() -> &'static [Screen] {
        &[
            Screen::Overview,
            Screen::Priorities,
            Screen::Detail,
            Screen::Prompt,
            Screen::Claims,
        ]
    }

    /// Tab label with hotkey so navigation is discoverable.
    fn tab_label(self) -> String {
        match self {
            Screen::Setup => "Setup".into(),
            Screen::Overview => "1 Overview".into(),
            Screen::Priorities => "2 Priorities".into(),
            Screen::Detail => "3 Detail".into(),
            Screen::Prompt => "4 Prompt".into(),
            Screen::Claims => "5 Claims".into(),
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
    module_sel: usize,
    module_offset: usize,
    fn_list: Vec<usize>,
    fn_sel: usize,
    fn_offset: usize,
    priority_mode: PriorityMode,
    priority_list: Vec<usize>,
    priority_sel: usize,
    priority_offset: usize,
    selected_id: Option<String>,
    detail: Option<FunctionDetail>,
    batch: Vec<String>,
    prompt_scroll: u16,
    prompt_text: String,
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
        Ok(Self {
            theme: Theme::default(),
            screen: Screen::Setup,
            setup_input: String::new(),
            status: "Enter a path, JSON URL, or GitHub repo URL, then press Enter".into(),
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
            module_sel: 0,
            module_offset: 0,
            fn_list: Vec::new(),
            fn_sel: 0,
            fn_offset: 0,
            priority_mode: PriorityMode::Nearly,
            priority_list: Vec::new(),
            priority_sel: 0,
            priority_offset: 0,
            selected_id: None,
            detail: None,
            batch: Vec::new(),
            prompt_scroll: 0,
            prompt_text: String::new(),
            claims_session,
            show_help: false,
            should_quit: false,
        })
    }

    fn global_hints(&self) -> Vec<KeyHint> {
        if self.screen == Screen::Setup {
            return vec![
                KeyHint {
                    key: "enter",
                    action: "load",
                },
                KeyHint {
                    key: "?",
                    action: "help",
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
        vec![
            KeyHint {
                key: "tab/1-5",
                action: "screens",
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
                    key: "enter",
                    action: "open detail",
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
                    key: "c",
                    action: "copy prompt",
                },
            ],
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
                    action: "open detail",
                },
                KeyHint {
                    key: "b",
                    action: "batch",
                },
                KeyHint {
                    key: "c",
                    action: "copy prompt",
                },
            ],
            Screen::Detail => vec![
                KeyHint {
                    key: "b",
                    action: "toggle batch",
                },
                KeyHint {
                    key: "c",
                    action: "copy prompt",
                },
                KeyHint {
                    key: "4",
                    action: "prompt view",
                },
            ],
            Screen::Prompt => vec![
                KeyHint {
                    key: "j/k",
                    action: "scroll",
                },
                KeyHint {
                    key: "pgup/pgdn",
                    action: "scroll page",
                },
                KeyHint {
                    key: "c",
                    action: "copy",
                },
                KeyHint {
                    key: "b",
                    action: "toggle batch",
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
  1 2 3 4 5   Overview · Priorities · Detail · Prompt · Claims
  r           refresh claims
  c           copy current prompt to clipboard
  b           add/remove selected function from batch (max 16)
              batched rows show violet [B1] [B2] … badges in lists

OVERVIEW
  j / k       next / previous function  (also ↑ ↓)
  h / l       previous / next module    (also ← →)
  enter       open function detail
  /           search (filter by name, module, id)
  esc         leave search

PRIORITIES
  n           cycle Nearly / Scaffolded / Biggest
  j / k       move in ranked list
  enter       open selected function

DETAIL / PROMPT
  j / k       scroll prompt text
  pgup/pgdn   scroll prompt by page

SETUP
  type path, raw JSON URL, or GitHub repo URL
  enter       load atlas

Press ? or esc to close help."#
            .to_string()
    }

    async fn load_from(&mut self, input: &str) -> Result<()> {
        self.status = format!("Loading {input}…");
        self.error = None;
        let input = input.trim();
        let (db, source) = if input.contains("github.com/")
            && !input.contains("raw.githubusercontent.com")
            && !input.ends_with(".json")
        {
            load_chaos_db(&self.client, None, Some(input), None).await?
        } else {
            load_chaos_db(&self.client, Some(input), None, None).await?
        };
        let base = details_base_from_source(&source);
        self.detail_cache = Some(DetailCache::new(base));
        self.source = Some(source);
        self.apply_db(db).await;
        Ok(())
    }

    async fn apply_db(&mut self, db: ChaosDb) {
        self.refresh_claims(&db).await;
        self.rebuild_modules(&db);
        self.db = Some(db);
        self.screen = Screen::Overview;
        self.rebuild_functions();
        self.rebuild_priorities();
        self.rebuild_prompt();
        if let Some(db) = &self.db {
            self.status = format!(
                "Loaded {} · {}/{} fn ({:.2}%) · tab or 1-5 screens · ? help · q quit",
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
        let mut mods: Vec<String> = db.functions.iter().map(|f| f.module.clone()).collect();
        mods.sort();
        mods.dedup();
        self.module_list = mods;
        self.module_sel = 0;
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
        self.fn_list = db
            .functions
            .iter()
            .enumerate()
            .filter(|(_, f)| module.map(|m| f.module == m).unwrap_or(true))
            .filter(|(_, f)| {
                q.is_empty()
                    || f.name.to_ascii_lowercase().contains(&q)
                    || f.module.to_ascii_lowercase().contains(&q)
                    || f.id.to_ascii_lowercase().contains(&q)
            })
            .map(|(i, _)| i)
            .collect();
        self.fn_sel = 0;
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
            self.selected_id = Some(db.functions[idx].id.clone());
        }
    }

    fn selected_function(&self) -> Option<&ChaosFunction> {
        let db = self.db.as_ref()?;
        let id = self.selected_id.as_ref()?;
        db.find_by_id(id)
    }

    async fn load_selected_detail(&mut self) {
        let (module, name) = {
            let Some(f) = self.selected_function() else {
                self.detail = None;
                return;
            };
            (f.module.clone(), f.name.clone())
        };
        let Some(cache) = &self.detail_cache else {
            self.detail = None;
            return;
        };
        match load_function_detail(&self.client, cache, &module, &name).await {
            Ok(d) => self.detail = d,
            Err(_) => self.detail = None,
        }
        self.rebuild_prompt();
    }

    fn project(&self) -> ProjectConfig {
        self.db
            .as_ref()
            .and_then(|d| d.project.clone())
            .unwrap_or_default()
    }

    fn rebuild_prompt(&mut self) {
        let project = self.project();
        let opts = PromptOptions {
            claims_session: self.claims_session.clone(),
        };
        let Some(db) = &self.db else {
            self.prompt_text.clear();
            return;
        };
        let mut items: Vec<(ChaosFunction, Option<FunctionDetail>)> = Vec::new();
        if self.batch.is_empty() {
            if let Some(f) = self.selected_function() {
                items.push((f.clone(), self.detail.clone()));
            }
        } else {
            for id in &self.batch {
                if let Some(f) = db.find_by_id(id) {
                    items.push((f.clone(), None));
                }
            }
        }
        if items.is_empty() {
            self.prompt_text = "Select a function or add items to the batch.".into();
        } else {
            self.prompt_text = build_prompt(&project, &items, &opts);
        }
        self.prompt_scroll = 0;
    }

    fn toggle_batch_selected(&mut self) {
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
        self.rebuild_prompt();
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
        match copy_text(&self.prompt_text) {
            Ok(()) => self.status = "Prompt copied to clipboard".into(),
            Err(e) => {
                self.error = Some(format!("clipboard: {e}"));
                self.status = "Copy failed".into();
            }
        }
    }

    async fn on_key(&mut self, key: KeyCode, mods: KeyModifiers) {
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
                KeyCode::Backspace => {
                    self.search.pop();
                    self.rebuild_functions();
                }
                KeyCode::Char(c) if !mods.contains(KeyModifiers::CONTROL) => {
                    self.search.push(c);
                    self.rebuild_functions();
                }
                _ => {}
            }
            return;
        }

        // ? always available
        if key == KeyCode::Char('?') {
            self.show_help = true;
            self.error = None;
            return;
        }

        if self.screen == Screen::Setup {
            match key {
                KeyCode::Char('q') => self.should_quit = true,
                KeyCode::Esc => {
                    // do not silent-quit on esc in setup; show hint
                    self.status = "Press q to quit · enter to load".into();
                }
                KeyCode::Enter => {
                    let input = self.setup_input.clone();
                    if input.trim().is_empty() {
                        self.error = Some("Enter a path, URL, or GitHub repo".into());
                    } else if let Err(e) = self.load_from(&input).await {
                        self.error = Some(format!("{e:#}"));
                        self.status = "Load failed".into();
                    } else {
                        self.error = None;
                    }
                }
                KeyCode::Backspace => {
                    self.setup_input.pop();
                }
                KeyCode::Char(c) if !mods.contains(KeyModifiers::CONTROL) => {
                    self.setup_input.push(c);
                    self.error = None;
                }
                _ => {}
            }
            return;
        }

        match key {
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Esc => {
                // Soft escape: clear error / go overview, do not quit
                if self.error.take().is_some() {
                    self.status = "Error dismissed".into();
                } else if self.screen != Screen::Overview {
                    self.screen = Screen::Overview;
                    self.status = "Overview · press q to quit".into();
                } else {
                    self.status = "Press q to quit · ? for help".into();
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
                self.screen = Screen::Detail;
                self.load_selected_detail().await;
                self.status = "Detail".into();
            }
            KeyCode::Char('4') => {
                self.rebuild_prompt();
                self.screen = Screen::Prompt;
                self.status = "Prompt · press c to copy".into();
            }
            KeyCode::Char('5') => {
                self.screen = Screen::Claims;
                self.status = format!("Claims · {}", self.claims_status);
            }
            KeyCode::Char('/') => {
                self.searching = true;
                self.screen = Screen::Overview;
                self.status = "Search mode · type to filter functions".into();
            }
            KeyCode::Char('c') => self.copy_prompt(),
            KeyCode::Char('b') => self.toggle_batch_selected(),
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
            KeyCode::Up | KeyCode::Char('k') => self.move_sel(-1).await,
            KeyCode::Down | KeyCode::Char('j') => self.move_sel(1).await,
            KeyCode::Left | KeyCode::Char('h') if self.screen == Screen::Overview => {
                self.move_module(-1);
                self.rebuild_functions();
            }
            KeyCode::Right | KeyCode::Char('l') if self.screen == Screen::Overview => {
                self.move_module(1);
                self.rebuild_functions();
            }
            KeyCode::Enter => {
                if self.screen == Screen::Priorities {
                    if let Some(&idx) = self.priority_list.get(self.priority_sel) {
                        if let Some(db) = &self.db {
                            self.selected_id = Some(db.functions[idx].id.clone());
                            self.screen = Screen::Detail;
                            self.load_selected_detail().await;
                            self.status = "Opened from priorities".into();
                        }
                    }
                } else if self.screen == Screen::Overview {
                    self.sync_selection_from_fn();
                    self.screen = Screen::Detail;
                    self.load_selected_detail().await;
                    self.status = "Opened function detail".into();
                }
            }
            KeyCode::PageUp if self.screen == Screen::Prompt => {
                self.prompt_scroll = self.prompt_scroll.saturating_sub(5);
            }
            KeyCode::PageDown if self.screen == Screen::Prompt => {
                self.prompt_scroll = self.prompt_scroll.saturating_add(5);
            }
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
        match self.screen {
            Screen::Detail => {
                self.load_selected_detail().await;
                self.status = "Detail · b batch · c copy · ? help".into();
            }
            Screen::Prompt => {
                self.rebuild_prompt();
                self.status = "Prompt · c copy · j/k scroll · ? help".into();
            }
            Screen::Priorities => {
                self.rebuild_priorities();
                self.status = format!(
                    "Priorities · {} · n cycle · ? help",
                    self.priority_mode.label()
                );
            }
            Screen::Claims => {
                self.status = format!("Claims · {} · r refresh · ? help", self.claims_status);
            }
            Screen::Overview => {
                self.status = "Overview · j/k functions · h/l modules · enter open · ? help".into();
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

    fn draw(&mut self, f: &mut Frame) {
        let area = f.area();
        // Entire frame solid dark first — no white terminal paper anywhere.
        fill_pane(f, area, &self.theme, self.theme.bg);

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Min(5),
                Constraint::Length(5), // status + two key rows always visible
            ])
            .split(area);

        self.draw_header(f, chunks[0]);
        match self.screen {
            Screen::Setup => self.draw_setup(f, chunks[1]),
            Screen::Overview => self.draw_overview(f, chunks[1]),
            Screen::Priorities => self.draw_priorities(f, chunks[1]),
            Screen::Detail => self.draw_detail(f, chunks[1]),
            Screen::Prompt => self.draw_prompt(f, chunks[1]),
            Screen::Claims => self.draw_claims(f, chunks[1]),
        }
        self.draw_footer(f, chunks[2]);

        if self.show_help {
            self.draw_help_overlay(f, area);
        }
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
            format!(
                " chaos  ·  {}  ·  {}/{} fn ({}%)  ·  {batch_bit}  ·  gen {gen}",
                db.project_name(),
                db.stats.matched_functions,
                db.stats.total_functions,
                format_pct(db.stats.matched_functions, db.stats.total_functions),
            )
        } else {
            " chaos  ·  Chaos Viewer CLI  ·  press ? for help".into()
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

        let tabs = Screen::all_loaded();
        let titles: Vec<Line> = tabs
            .iter()
            .map(|s| Line::from(Span::styled(s.tab_label(), paint_on(self.theme.muted, bg))))
            .collect();
        let selected = tabs.iter().position(|s| *s == self.screen).unwrap_or(0);
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
            Tabs::new(titles)
                .select(selected)
                .divider(" │ ")
                .style(paint_on(self.theme.muted, bg))
                .highlight_style(paint_bold_on(self.theme.accent, bg)),
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
        let block = content_block(" Setup ", &self.theme, self.theme.border);
        let inner = block.inner(area);
        f.render_widget(block, area);
        fill_pane(f, inner, &self.theme, bg);

        let body = format!(
            "Point chaos at any decomp project that publishes chaos-db.json.\n\n\
             Path, raw JSON URL, or GitHub repo:\n\n  > {}_\n\n\
             Keys:  enter = load   ? = help   q = quit\n\n\
             Examples:\n  ./data/chaos-db.json\n  https://raw.githubusercontent.com/org/repo/chaos-data/chaos-db.json\n  https://github.com/org/repo",
            self.setup_input
        );
        f.render_widget(
            Paragraph::new(body)
                .style(paint_on(self.theme.text, bg))
                .wrap(Wrap { trim: false }),
            inner,
        );
    }

    fn draw_overview(&mut self, f: &mut Frame, area: Rect) {
        let Some(db) = &self.db else { return };
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(28), Constraint::Percentage(72)])
            .split(area);

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
                let total = db.functions.iter().filter(|f| f.module == *m).count();
                let matched = db
                    .functions
                    .iter()
                    .filter(|f| f.module == *m && f.matched)
                    .count();
                let selected = i == self.module_sel;
                let bg = if selected {
                    self.theme.panel
                } else {
                    self.theme.bg
                };
                let fg = if selected {
                    self.theme.accent
                } else {
                    self.theme.text
                };
                let mark = if selected { "› " } else { "  " };
                let style = if selected {
                    paint_bold_on(fg, bg)
                } else {
                    paint_on(fg, bg)
                };
                Line::from(Span::styled(format!("{mark}{m}  {matched}/{total}"), style))
            })
            .collect();
        Self::draw_line_list(
            f,
            cols[0],
            " Modules  (h/l) ".into(),
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
        let title = if self.search.is_empty() {
            format!(
                " Functions ({})  ·  batch {} ({} here)  ·  j/k enter / b ",
                self.fn_list.len(),
                self.batch_summary(),
                batched_visible
            )
        } else {
            format!(
                " Functions ({}) · /{}  ·  batch {}  ·  enter done ",
                self.fn_list.len(),
                self.search,
                self.batch_summary()
            )
        };
        Self::draw_line_list(f, cols[1], title, &self.theme, &fn_lines, self.fn_offset);
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

    fn draw_detail(&self, f: &mut Frame, area: Rect) {
        let bg = self.theme.bg;
        let Some(fn_) = self.selected_function() else {
            let block = content_block(" Function detail ", &self.theme, self.theme.border);
            let inner = block.inner(area);
            f.render_widget(block, area);
            fill_pane(f, inner, &self.theme, bg);
            f.render_widget(
                Paragraph::new("No function selected. Pick one in Overview or Priorities.")
                    .style(paint_on(self.theme.muted, bg)),
                inner,
            );
            return;
        };

        let title = if let Some(n) = self.batch_index(&fn_.id) {
            format!(" Function detail  ·  BATCHED [B{n}] ")
        } else {
            " Function detail ".into()
        };
        let border = if self.batch_index(&fn_.id).is_some() {
            self.theme.batch
        } else {
            self.theme.border
        };
        let block = content_block(title, &self.theme, border);
        let inner = block.inner(area);
        f.render_widget(block, area);
        fill_pane(f, inner, &self.theme, bg);

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
                for l in draft.lines().take(12) {
                    lines.push(l.to_string());
                }
            }
            if let Some(dis) = &det.disasm {
                lines.push(String::new());
                lines.push(format!("disasm ({} lines):", dis.len()));
                for l in dis.iter().take(20) {
                    lines.push(l.clone());
                }
            }
        } else {
            lines.push(String::new());
            lines.push("(no detail chunk loaded for this module)".into());
        }
        lines.push(String::new());
        if let Some(n) = self.batch_index(&fn_.id) {
            lines.push(format!(
                "BATCHED  [B{n}]  ·  position {n}/{}  ·  press b to remove  ·  c copy",
                self.batch.len()
            ));
        } else {
            lines.push(format!(
                "not in batch  ·  press b to add ({})  ·  c copy prompt",
                self.batch_summary()
            ));
        }

        f.render_widget(
            Paragraph::new(lines.join("\n"))
                .style(paint_on(self.theme.text, bg))
                .wrap(Wrap { trim: false }),
            inner,
        );
    }

    fn draw_prompt(&self, f: &mut Frame, area: Rect) {
        let bg = self.theme.bg;
        let roster: String = if self.batch.is_empty() {
            "batch empty — select functions and press b (or copy single selection)".into()
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

        let title = format!(
            " Prompt  ·  batch {}  ·  c copy · j/k scroll ",
            self.batch_summary()
        );
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
) -> Result<()> {
    let claims_session = ClaimsSession::from_env();
    let mut app = App::new(claims_session)?;

    if let Some(repo) = repo {
        let (db, source) = load_chaos_db(&app.client, None, Some(&repo), branch.as_deref()).await?;
        let base = details_base_from_source(&source);
        app.detail_cache = Some(DetailCache::new(base));
        app.source = Some(source);
        app.apply_db(db).await;
    } else if let Some(input) = input {
        app.load_from(&input).await?;
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
        if event::poll(Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    app.on_key(key.code, key.modifiers).await;
                }
            }
        }
        if app.should_quit {
            break;
        }
    }
    Ok(())
}
