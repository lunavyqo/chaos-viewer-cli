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
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Tabs, Wrap};
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

    fn title(self) -> &'static str {
        match self {
            Screen::Setup => "Setup",
            Screen::Overview => "Overview",
            Screen::Priorities => "Priorities",
            Screen::Detail => "Detail",
            Screen::Prompt => "Prompt",
            Screen::Claims => "Claims",
        }
    }
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
    module_state: ListState,
    fn_list: Vec<usize>,
    fn_state: ListState,
    priority_mode: PriorityMode,
    priority_list: Vec<usize>,
    priority_state: ListState,
    selected_id: Option<String>,
    detail: Option<FunctionDetail>,
    batch: Vec<String>,
    prompt_scroll: u16,
    prompt_text: String,
    claims_session: Option<ClaimsSession>,
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
            module_state: ListState::default(),
            fn_list: Vec::new(),
            fn_state: ListState::default(),
            priority_mode: PriorityMode::Nearly,
            priority_list: Vec::new(),
            priority_state: ListState::default(),
            selected_id: None,
            detail: None,
            batch: Vec::new(),
            prompt_scroll: 0,
            prompt_text: String::new(),
            claims_session,
            should_quit: false,
        })
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
                "Loaded {} · {} / {} fn ({:.2}%) · {}",
                db.project_name(),
                db.stats.matched_functions,
                db.stats.total_functions,
                db.match_pct_functions(),
                self.source
                    .as_ref()
                    .map(|s| s.display())
                    .unwrap_or_default()
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
                        "live · {} ranges · {} locked fn",
                        claims.len(),
                        self.locked_by.len()
                    )
                } else {
                    "unavailable".into()
                };
            }
            Err(e) => {
                self.claims_status = format!("error: {e}");
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
        if !self.module_list.is_empty() {
            self.module_state.select(Some(0));
        } else {
            self.module_state.select(None);
        }
    }

    fn selected_module(&self) -> Option<&str> {
        self.module_state
            .selected()
            .and_then(|i| self.module_list.get(i))
            .map(String::as_str)
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
        if self.fn_list.is_empty() {
            self.fn_state.select(None);
        } else {
            self.fn_state.select(Some(0));
            self.sync_selection_from_fn();
        }
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
        if self.priority_list.is_empty() {
            self.priority_state.select(None);
        } else {
            self.priority_state.select(Some(0));
        }
    }

    fn sync_selection_from_fn(&mut self) {
        let Some(db) = &self.db else { return };
        if let Some(sel) = self.fn_state.selected() {
            if let Some(&idx) = self.fn_list.get(sel) {
                self.selected_id = Some(db.functions[idx].id.clone());
            }
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
            return;
        };
        if let Some(pos) = self.batch.iter().position(|x| x == &id) {
            self.batch.remove(pos);
            self.status = "Removed from batch".into();
        } else if self.batch.len() < batch_max() {
            self.batch.push(id);
            self.status = format!("Batch {}/{}", self.batch.len(), batch_max());
        } else {
            self.status = format!("Batch full ({})", batch_max());
        }
        self.rebuild_prompt();
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
        if self.searching {
            match key {
                KeyCode::Esc => {
                    self.searching = false;
                }
                KeyCode::Enter => {
                    self.searching = false;
                    self.rebuild_functions();
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

        if self.screen == Screen::Setup {
            match key {
                KeyCode::Char('q') if mods.contains(KeyModifiers::CONTROL) => {
                    self.should_quit = true;
                }
                KeyCode::Esc | KeyCode::Char('q') => self.should_quit = true,
                KeyCode::Enter => {
                    let input = self.setup_input.clone();
                    if input.trim().is_empty() {
                        self.error = Some("Enter a path, URL, or GitHub repo".into());
                    } else if let Err(e) = self.load_from(&input).await {
                        self.error = Some(format!("{e:#}"));
                        self.status = "Load failed".into();
                    }
                }
                KeyCode::Backspace => {
                    self.setup_input.pop();
                }
                KeyCode::Char(c) if !mods.contains(KeyModifiers::CONTROL) => {
                    self.setup_input.push(c);
                }
                _ => {}
            }
            return;
        }

        match key {
            KeyCode::Char('q') | KeyCode::Esc => self.should_quit = true,
            KeyCode::Tab => self.next_screen(),
            KeyCode::BackTab => self.prev_screen(),
            KeyCode::Char('1') => self.screen = Screen::Overview,
            KeyCode::Char('2') => self.screen = Screen::Priorities,
            KeyCode::Char('3') => {
                self.screen = Screen::Detail;
                self.load_selected_detail().await;
            }
            KeyCode::Char('4') => {
                self.rebuild_prompt();
                self.screen = Screen::Prompt;
            }
            KeyCode::Char('5') => self.screen = Screen::Claims,
            KeyCode::Char('/') => {
                self.searching = true;
                self.screen = Screen::Overview;
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
                    if let Some(sel) = self.priority_state.selected() {
                        if let Some(&idx) = self.priority_list.get(sel) {
                            if let Some(db) = &self.db {
                                self.selected_id = Some(db.functions[idx].id.clone());
                                self.screen = Screen::Detail;
                                self.load_selected_detail().await;
                            }
                        }
                    }
                } else if self.screen == Screen::Overview {
                    self.sync_selection_from_fn();
                    self.screen = Screen::Detail;
                    self.load_selected_detail().await;
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

    fn next_screen(&mut self) {
        let tabs = Screen::all_loaded();
        let i = tabs.iter().position(|s| *s == self.screen).unwrap_or(0);
        self.screen = tabs[(i + 1) % tabs.len()];
    }

    fn prev_screen(&mut self) {
        let tabs = Screen::all_loaded();
        let i = tabs.iter().position(|s| *s == self.screen).unwrap_or(0);
        self.screen = tabs[(i + tabs.len() - 1) % tabs.len()];
    }

    fn move_module(&mut self, delta: isize) {
        if self.module_list.is_empty() {
            return;
        }
        let i = self.module_state.selected().unwrap_or(0) as isize + delta;
        let n = self.module_list.len() as isize;
        let i = ((i % n) + n) % n;
        self.module_state.select(Some(i as usize));
    }

    async fn move_sel(&mut self, delta: isize) {
        match self.screen {
            Screen::Overview => {
                if self.fn_list.is_empty() {
                    return;
                }
                let i = self.fn_state.selected().unwrap_or(0) as isize + delta;
                let n = self.fn_list.len() as isize;
                let i = ((i % n) + n) % n;
                self.fn_state.select(Some(i as usize));
                self.sync_selection_from_fn();
            }
            Screen::Priorities => {
                if self.priority_list.is_empty() {
                    return;
                }
                let i = self.priority_state.selected().unwrap_or(0) as isize + delta;
                let n = self.priority_list.len() as isize;
                let i = ((i % n) + n) % n;
                self.priority_state.select(Some(i as usize));
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
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Min(5),
                Constraint::Length(3),
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
    }

    fn draw_header(&self, f: &mut Frame, area: Rect) {
        let title = if let Some(db) = &self.db {
            format!(
                " chaos  ·  {}  ·  {}/{} fn ({}%)  ·  gen {}",
                db.project_name(),
                db.stats.matched_functions,
                db.stats.total_functions,
                format_pct(db.stats.matched_functions, db.stats.total_functions),
                if db.generated_at.is_empty() {
                    "?"
                } else {
                    db.generated_at.as_str()
                }
            )
        } else {
            " chaos  ·  Chaos Viewer CLI".into()
        };

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(self.theme.border))
            .style(Style::default().bg(self.theme.bg).fg(self.theme.text));
        let inner = block.inner(area);
        f.render_widget(block, area);

        if self.screen == Screen::Setup {
            f.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    title,
                    Style::default()
                        .fg(self.theme.accent)
                        .add_modifier(Modifier::BOLD),
                ))),
                inner,
            );
            return;
        }

        let tabs = Screen::all_loaded();
        let titles: Vec<Line> = tabs.iter().map(|s| Line::from(s.title())).collect();
        let selected = tabs.iter().position(|s| *s == self.screen).unwrap_or(0);
        let header_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Length(1)])
            .split(inner);
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                title,
                Style::default()
                    .fg(self.theme.accent)
                    .add_modifier(Modifier::BOLD),
            ))),
            header_chunks[0],
        );
        f.render_widget(
            Tabs::new(titles)
                .select(selected)
                .style(Style::default().fg(self.theme.muted))
                .highlight_style(
                    Style::default()
                        .fg(self.theme.accent)
                        .add_modifier(Modifier::BOLD),
                ),
            header_chunks[1],
        );
    }

    fn draw_footer(&self, f: &mut Frame, area: Rect) {
        let hints = if self.screen == Screen::Setup {
            "enter load  ·  q quit"
        } else if self.searching {
            "type to filter  ·  enter/esc done"
        } else {
            "tab screens  ·  j/k move  ·  / search  ·  b batch  ·  c copy  ·  r claims  ·  q quit"
        };
        let status = if let Some(err) = &self.error {
            format!("error: {err}")
        } else if self.searching {
            format!("search: {}_", self.search)
        } else {
            self.status.clone()
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(self.theme.border))
            .style(Style::default().bg(self.theme.panel));
        let inner = block.inner(area);
        f.render_widget(block, area);
        let lines = vec![
            Line::from(Span::styled(status, Style::default().fg(self.theme.text))),
            Line::from(Span::styled(hints, Style::default().fg(self.theme.muted))),
        ];
        f.render_widget(Paragraph::new(lines), inner);
    }

    fn draw_setup(&self, f: &mut Frame, area: Rect) {
        let block = Block::default()
            .title(" Setup ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(self.theme.border))
            .style(Style::default().bg(self.theme.bg).fg(self.theme.text));
        let inner = block.inner(area);
        f.render_widget(block, area);

        let body = format!(
            "Point chaos at any decomp project that publishes chaos-db.json.\n\n\
             Path, raw JSON URL, or GitHub repo:\n\n  > {}_\n\n\
             Examples:\n  ./data/chaos-db.json\n  https://raw.githubusercontent.com/org/repo/chaos-data/chaos-db.json\n  https://github.com/org/repo",
            self.setup_input
        );
        f.render_widget(
            Paragraph::new(body)
                .style(Style::default().fg(self.theme.text))
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

        let mod_items: Vec<ListItem> = self
            .module_list
            .iter()
            .map(|m| {
                let total = db.functions.iter().filter(|f| f.module == *m).count();
                let matched = db
                    .functions
                    .iter()
                    .filter(|f| f.module == *m && f.matched)
                    .count();
                ListItem::new(format!("{m}  {matched}/{total}"))
            })
            .collect();
        let mods = List::new(mod_items)
            .block(
                Block::default()
                    .title(" Modules ")
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(self.theme.border)),
            )
            .highlight_style(
                Style::default()
                    .bg(self.theme.panel)
                    .fg(self.theme.accent)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("› ");
        f.render_stateful_widget(mods, cols[0], &mut self.module_state);

        let fn_items: Vec<ListItem> = self
            .fn_list
            .iter()
            .map(|&idx| {
                let f = &db.functions[idx];
                let badge = if f.matched {
                    "M"
                } else if self.locked_by.contains_key(&f.id) {
                    "L"
                } else if f.div.is_some() {
                    "N"
                } else {
                    "U"
                };
                let color = if f.matched {
                    self.theme.matched
                } else if self.locked_by.contains_key(&f.id) {
                    self.theme.claim
                } else {
                    self.theme.unmatched
                };
                ListItem::new(Line::from(vec![
                    Span::styled(format!("[{badge}] "), Style::default().fg(color)),
                    Span::raw(format!("{}  0x{:x}  {}B", f.name, f.addr, f.size)),
                ]))
            })
            .collect();
        let title = if self.search.is_empty() {
            format!(" Functions ({}) ", self.fn_list.len())
        } else {
            format!(" Functions ({}) · /{} ", self.fn_list.len(), self.search)
        };
        let fns = List::new(fn_items)
            .block(
                Block::default()
                    .title(title)
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(self.theme.border)),
            )
            .highlight_style(
                Style::default()
                    .bg(self.theme.panel)
                    .fg(self.theme.accent)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("› ");
        f.render_stateful_widget(fns, cols[1], &mut self.fn_state);
    }

    fn draw_priorities(&mut self, f: &mut Frame, area: Rect) {
        let Some(db) = &self.db else { return };
        let title = format!(
            " {}  (n cycle)  ·  {} rows ",
            self.priority_mode.label(),
            self.priority_list.len()
        );
        let items: Vec<ListItem> = self
            .priority_list
            .iter()
            .map(|&idx| {
                let f = &db.functions[idx];
                let extra = match self.priority_mode {
                    PriorityMode::Nearly => format!("div={}", f.div.unwrap_or(0)),
                    PriorityMode::Scaffolded => format!("sim={:.2}", f.sim.unwrap_or(0.0)),
                    PriorityMode::Biggest => format!("{}B", f.size),
                };
                ListItem::new(format!("{}  {}  0x{:x}  {extra}", f.module, f.name, f.addr))
            })
            .collect();
        let list = List::new(items)
            .block(
                Block::default()
                    .title(title)
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(self.theme.border)),
            )
            .highlight_style(
                Style::default()
                    .bg(self.theme.panel)
                    .fg(self.theme.accent)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("› ");
        f.render_stateful_widget(list, area, &mut self.priority_state);
    }

    fn draw_detail(&self, f: &mut Frame, area: Rect) {
        let block = Block::default()
            .title(" Function detail ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(self.theme.border));
        let inner = block.inner(area);
        f.render_widget(block, area);

        let Some(fn_) = self.selected_function() else {
            f.render_widget(
                Paragraph::new("No function selected. Pick one in Overview or Priorities.")
                    .style(Style::default().fg(self.theme.muted)),
                inner,
            );
            return;
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
        let in_batch = self.batch.iter().any(|id| id == &fn_.id);
        lines.push(String::new());
        lines.push(format!(
            "batch: {}  ·  press b to toggle  ·  c copy prompt",
            if in_batch { "yes" } else { "no" }
        ));

        f.render_widget(
            Paragraph::new(lines.join("\n"))
                .style(Style::default().fg(self.theme.text))
                .wrap(Wrap { trim: false }),
            inner,
        );
    }

    fn draw_prompt(&self, f: &mut Frame, area: Rect) {
        let title = format!(
            " Prompt  ·  batch {}/{}  ·  PgUp/PgDn scroll  ·  c copy ",
            self.batch.len(),
            batch_max()
        );
        let block = Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(Style::default().fg(self.theme.border));
        let inner = block.inner(area);
        f.render_widget(block, area);
        f.render_widget(
            Paragraph::new(self.prompt_text.as_str())
                .style(Style::default().fg(self.theme.text))
                .wrap(Wrap { trim: false })
                .scroll((self.prompt_scroll, 0)),
            inner,
        );
    }

    fn draw_claims(&self, f: &mut Frame, area: Rect) {
        let block = Block::default()
            .title(" Claims (read-only) ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(self.theme.border));
        let inner = block.inner(area);
        f.render_widget(block, area);

        let mut lines = vec![
            format!("status: {}", self.claims_status),
            format!("locked functions: {}", self.locked_by.len()),
            String::new(),
        ];
        let mut entries: Vec<_> = self.locked_by.iter().collect();
        entries.sort_by(|a, b| a.0.cmp(b.0));
        for (id, handle) in entries.into_iter().take(40) {
            lines.push(format!("{handle:16}  {id}"));
        }
        if self.locked_by.is_empty() {
            lines.push("No active locks (or claims source unavailable).".into());
            lines.push("Claims appear when project.claimsApi is set or CLAIMS.md exists.".into());
        }
        f.render_widget(
            Paragraph::new(lines.join("\n")).style(Style::default().fg(self.theme.text)),
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
