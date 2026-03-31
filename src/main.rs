use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph},
    Frame, Terminal,
};
use serde::{Deserialize, Serialize};
use std::{
    error::Error,
    fs,
    io,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

// ─── Data model ───────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Clone, PartialEq)]
enum Direct {
    Mario,
    Martins,
}

impl Direct {
    fn name(&self) -> &'static str {
        match self {
            Direct::Mario => "Mario",
            Direct::Martins => "Martins",
        }
    }
}

#[derive(Serialize, Deserialize, Clone, PartialEq)]
enum Polarity {
    Positive,
    Negative,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(tag = "type")]
enum ItemKind {
    Feedback { polarity: Polarity, output: Option<String> },
    Topic { output: Option<String> },
}

#[derive(Serialize, Deserialize, Clone)]
struct Entry {
    text: String,
    kind: ItemKind,
}

/// One O3 meeting: belongs to a direct, has an optional date, and owns its entries.
#[derive(Serialize, Deserialize, Clone)]
struct O3 {
    id: u64,
    direct: Direct,
    date_days: Option<i64>,
    entries: Vec<Entry>,
}

#[derive(Serialize, Deserialize, Default)]
struct Store {
    o3s: Vec<O3>,
    next_id: u64,
}

// ─── Entry wizard ─────────────────────────────────────────────────────────────

#[derive(Clone, PartialEq)]
enum EntryStep {
    SelectItemType,
    SelectPolarity,
    EnterText,
}

#[derive(Clone)]
struct EntryWizard {
    step: EntryStep,
    is_topic: bool,
    polarity: Option<Polarity>,
    text: String,
    cursor: usize,
}

impl EntryWizard {
    fn new() -> Self {
        Self {
            step: EntryStep::SelectItemType,
            is_topic: false,
            polarity: None,
            text: String::new(),
            cursor: 0,
        }
    }
}

// ─── App ──────────────────────────────────────────────────────────────────────

enum Mode {
    O3List,
    O3Detail { o3_idx: usize, prev_list_selected: usize },
    CreatingO3,
    SettingDate { o3_idx: usize },
    AddingEntry { o3_idx: usize, wizard: EntryWizard },
    EditingOutput { o3_idx: usize, entry_idx: usize, text: String, cursor: usize },
}

struct App {
    o3s: Vec<O3>,
    next_id: u64,
    selected: usize,
    mode: Mode,
    storage_path: PathBuf,
}

impl App {
    fn new() -> Result<Self, Box<dyn Error>> {
        let storage_path = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".o3_manager.json");

        let store: Store = if storage_path.exists() {
            serde_json::from_str(&fs::read_to_string(&storage_path)?).unwrap_or_default()
        } else {
            Store::default()
        };

        Ok(App {
            o3s: store.o3s,
            next_id: store.next_id,
            selected: 0,
            mode: Mode::O3List,
            storage_path,
        })
    }

    fn save(&self) {
        let store = Store { o3s: self.o3s.clone(), next_id: self.next_id };
        if let Ok(json) = serde_json::to_string_pretty(&store) {
            let _ = fs::write(&self.storage_path, json);
        }
    }

    /// O3s for display: upcoming ascending, undated after upcoming, past most-recent-first.
    fn sorted_o3_indices(&self) -> Vec<usize> {
        let today = today_days();
        let mut indices: Vec<usize> = (0..self.o3s.len()).collect();
        indices.sort_by_key(|&i| match self.o3s[i].date_days {
            Some(d) if d >= today => (0i32, d),
            None => (1, 0),
            Some(d) => (2, -d),
        });
        indices
    }

    fn create_o3(&mut self, direct: Direct) -> usize {
        let id = self.next_id;
        self.next_id += 1;
        self.o3s.push(O3 { id, direct, date_days: None, entries: Vec::new() });
        self.save();
        self.o3s.len() - 1
    }

    fn delete_o3(&mut self, o3_idx: usize) {
        if o3_idx < self.o3s.len() {
            self.o3s.remove(o3_idx);
            self.save();
        }
    }

    fn set_o3_date(&mut self, o3_idx: usize, date_days: Option<i64>) {
        if let Some(o3) = self.o3s.get_mut(o3_idx) {
            o3.date_days = date_days;
            self.save();
        }
    }

    fn add_entry(&mut self, o3_idx: usize, wizard: &EntryWizard) {
        if wizard.text.trim().is_empty() {
            return;
        }
        let kind = if wizard.is_topic {
            ItemKind::Topic { output: None }
        } else {
            ItemKind::Feedback {
                polarity: wizard.polarity.clone().unwrap_or(Polarity::Positive),
                output: None,
            }
        };
        if let Some(o3) = self.o3s.get_mut(o3_idx) {
            o3.entries.push(Entry { text: wizard.text.trim().to_string(), kind });
            self.save();
        }
    }

    fn delete_entry(&mut self, o3_idx: usize, entry_idx: usize) {
        if let Some(o3) = self.o3s.get_mut(o3_idx) {
            if entry_idx < o3.entries.len() {
                o3.entries.remove(entry_idx);
                self.save();
            }
        }
    }

    fn commit_output(&mut self, o3_idx: usize, entry_idx: usize, text: &str) {
        let output = if text.trim().is_empty() { None } else { Some(text.trim().to_string()) };
        if let Some(o3) = self.o3s.get_mut(o3_idx) {
            if let Some(e) = o3.entries.get_mut(entry_idx) {
                let slot = match &mut e.kind {
                    ItemKind::Topic { output } => output,
                    ItemKind::Feedback { output, .. } => output,
                };
                *slot = output;
                self.save();
            }
        }
    }
}

// ─── Date helpers ─────────────────────────────────────────────────────────────

const MONTH_NAMES: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun",
    "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

fn today_days() -> i64 {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    (secs / 86400) as i64
}

fn days_to_ymd(d: i64) -> (i32, u32, u32) {
    let z = d + 719468;
    let era = (if z >= 0 { z } else { z - 146096 }) / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    let year = (if month <= 2 { y + 1 } else { y }) as i32;
    (year, month, day)
}

fn o3_label(date_days: i64) -> String {
    let today = today_days();
    let (_, month, day) = days_to_ymd(date_days);
    let mon = MONTH_NAMES[(month - 1) as usize];
    let suffix = if date_days < today { " (past)" } else if date_days == today { " (Today!)" } else { "" };
    format!("Thu {} {}{}", mon, day, suffix)
}

/// 8 Thursdays centred on the current week: 2 past + this/next 6.
fn thursday_options() -> Vec<i64> {
    let today = today_days();
    let dow = today % 7; // 0 = Thursday
    let last_thu = today - dow;
    (-2..6_i64).map(|i| last_thu + i * 7).collect()
}

// ─── Event handlers ───────────────────────────────────────────────────────────

fn handle_o3_list(app: &mut App, code: KeyCode) {
    match code {
        KeyCode::Char('a') => {
            app.mode = Mode::CreatingO3;
        }
        KeyCode::Char('j') | KeyCode::Down => {
            let n = app.o3s.len();
            if n > 0 && app.selected < n - 1 {
                app.selected += 1;
            }
        }
        KeyCode::Char('k') | KeyCode::Up => {
            if app.selected > 0 {
                app.selected -= 1;
            }
        }
        KeyCode::Enter => {
            let o3_idx = app.sorted_o3_indices().get(app.selected).copied();
            if let Some(idx) = o3_idx {
                let prev = app.selected;
                app.selected = 0;
                app.mode = Mode::O3Detail { o3_idx: idx, prev_list_selected: prev };
            }
        }
        KeyCode::Char('d') => {
            let o3_idx = app.sorted_o3_indices().get(app.selected).copied();
            if let Some(idx) = o3_idx {
                app.delete_o3(idx);
                let n = app.o3s.len();
                if n == 0 {
                    app.selected = 0;
                } else if app.selected >= n {
                    app.selected = n - 1;
                }
            }
        }
        _ => {}
    }
}

fn handle_o3_detail(app: &mut App, code: KeyCode, o3_idx: usize, prev_list_selected: usize) {
    match code {
        KeyCode::Esc => {
            app.selected = prev_list_selected;
            app.mode = Mode::O3List;
        }
        KeyCode::Char('a') => {
            app.mode = Mode::AddingEntry { o3_idx, wizard: EntryWizard::new() };
        }
        KeyCode::Char('t') => {
            app.mode = Mode::SettingDate { o3_idx };
        }
        KeyCode::Char('j') | KeyCode::Down => {
            let n = app.o3s.get(o3_idx).map_or(0, |o| o.entries.len());
            if n > 0 && app.selected < n - 1 {
                app.selected += 1;
            }
        }
        KeyCode::Char('k') | KeyCode::Up => {
            if app.selected > 0 {
                app.selected -= 1;
            }
        }
        KeyCode::Char('o') => {
            if let Some(o3) = app.o3s.get(o3_idx) {
                if let Some(e) = o3.entries.get(app.selected) {
                    let existing = match &e.kind {
                        ItemKind::Topic { output } => Some(output.clone().unwrap_or_default()),
                        ItemKind::Feedback { output, .. } => Some(output.clone().unwrap_or_default()),
                    };
                    if let Some(text) = existing {
                        let cursor = text.len();
                        let entry_idx = app.selected;
                        app.mode = Mode::EditingOutput { o3_idx, entry_idx, text, cursor };
                    }
                }
            }
        }
        KeyCode::Char('d') => {
            let sel = app.selected;
            app.delete_entry(o3_idx, sel);
            let n = app.o3s.get(o3_idx).map_or(0, |o| o.entries.len());
            if n == 0 {
                app.selected = 0;
            } else if app.selected >= n {
                app.selected = n - 1;
            }
        }
        _ => {}
    }
}

fn handle_creating_o3(app: &mut App, code: KeyCode) {
    match code {
        KeyCode::Esc => {
            app.mode = Mode::O3List;
        }
        KeyCode::Char('1') | KeyCode::Char('m') => {
            let o3_idx = app.create_o3(Direct::Mario);
            app.selected = 0;
            app.mode = Mode::O3Detail { o3_idx, prev_list_selected: app.selected };
        }
        KeyCode::Char('2') | KeyCode::Char('n') => {
            let o3_idx = app.create_o3(Direct::Martins);
            app.selected = 0;
            app.mode = Mode::O3Detail { o3_idx, prev_list_selected: app.selected };
        }
        _ => {}
    }
}

fn handle_setting_date(app: &mut App, code: KeyCode, o3_idx: usize) {
    let prev = app.selected;
    match code {
        KeyCode::Esc => {
            app.mode = Mode::O3Detail { o3_idx, prev_list_selected: prev };
        }
        KeyCode::Char('0') => {
            app.set_o3_date(o3_idx, None);
            app.mode = Mode::O3Detail { o3_idx, prev_list_selected: prev };
        }
        KeyCode::Char(c) if c.is_ascii_digit() => {
            let idx = (c as u8 - b'0') as usize;
            let options = thursday_options();
            if idx >= 1 && idx <= options.len() {
                app.set_o3_date(o3_idx, Some(options[idx - 1]));
            }
            app.mode = Mode::O3Detail { o3_idx, prev_list_selected: prev };
        }
        _ => {}
    }
}

fn handle_adding_entry(app: &mut App, code: KeyCode, o3_idx: usize) {
    let wizard = match &app.mode {
        Mode::AddingEntry { wizard, .. } => wizard.clone(),
        _ => return,
    };

    let mut w = wizard;
    let mut commit = false;
    let mut cancel = false;

    match w.step {
        EntryStep::SelectItemType => match code {
            KeyCode::Char('f') => {
                w.is_topic = false;
                w.step = EntryStep::SelectPolarity;
            }
            KeyCode::Char('t') => {
                w.is_topic = true;
                w.step = EntryStep::EnterText;
            }
            KeyCode::Esc => cancel = true,
            _ => {}
        },
        EntryStep::SelectPolarity => match code {
            KeyCode::Char('+') | KeyCode::Char('p') => {
                w.polarity = Some(Polarity::Positive);
                w.step = EntryStep::EnterText;
            }
            KeyCode::Char('-') | KeyCode::Char('n') => {
                w.polarity = Some(Polarity::Negative);
                w.step = EntryStep::EnterText;
            }
            KeyCode::Esc => cancel = true,
            _ => {}
        },
        EntryStep::EnterText => match code {
            KeyCode::Enter => commit = true,
            KeyCode::Esc => cancel = true,
            KeyCode::Char(c) => {
                w.text.insert(w.cursor, c);
                w.cursor += 1;
            }
            KeyCode::Backspace => {
                if w.cursor > 0 {
                    w.text.remove(w.cursor - 1);
                    w.cursor -= 1;
                }
            }
            KeyCode::Delete => {
                if w.cursor < w.text.len() {
                    w.text.remove(w.cursor);
                }
            }
            KeyCode::Left => {
                if w.cursor > 0 {
                    w.cursor -= 1;
                }
            }
            KeyCode::Right => {
                if w.cursor < w.text.len() {
                    w.cursor += 1;
                }
            }
            KeyCode::Home => w.cursor = 0,
            KeyCode::End => w.cursor = w.text.len(),
            _ => {}
        },
    }

    let prev = app.selected;
    if cancel {
        app.mode = Mode::O3Detail { o3_idx, prev_list_selected: prev };
    } else if commit {
        app.add_entry(o3_idx, &w);
        app.mode = Mode::O3Detail { o3_idx, prev_list_selected: prev };
    } else {
        app.mode = Mode::AddingEntry { o3_idx, wizard: w };
    }
}

fn handle_editing_output(app: &mut App, code: KeyCode, o3_idx: usize, entry_idx: usize) {
    let (mut text, mut cursor) = match &app.mode {
        Mode::EditingOutput { text, cursor, .. } => (text.clone(), *cursor),
        _ => return,
    };

    let mut commit = false;
    let mut cancel = false;

    match code {
        KeyCode::Enter => commit = true,
        KeyCode::Esc => cancel = true,
        KeyCode::Char(c) => {
            text.insert(cursor, c);
            cursor += 1;
        }
        KeyCode::Backspace => {
            if cursor > 0 {
                text.remove(cursor - 1);
                cursor -= 1;
            }
        }
        KeyCode::Delete => {
            if cursor < text.len() {
                text.remove(cursor);
            }
        }
        KeyCode::Left => {
            if cursor > 0 {
                cursor -= 1;
            }
        }
        KeyCode::Right => {
            if cursor < text.len() {
                cursor += 1;
            }
        }
        KeyCode::Home => cursor = 0,
        KeyCode::End => cursor = text.len(),
        _ => {}
    }

    let prev = app.selected;
    if commit {
        app.commit_output(o3_idx, entry_idx, &text);
        app.mode = Mode::O3Detail { o3_idx, prev_list_selected: prev };
    } else if cancel {
        app.mode = Mode::O3Detail { o3_idx, prev_list_selected: prev };
    } else {
        app.mode = Mode::EditingOutput { o3_idx, entry_idx, text, cursor };
    }
}

// ─── Main / run loop ──────────────────────────────────────────────────────────

fn main() -> Result<(), Box<dyn Error>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new()?;
    let res = run_app(&mut terminal, &mut app);

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    if let Err(err) = res {
        println!("Error: {:?}", err);
    }
    Ok(())
}

fn run_app<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
) -> io::Result<()> {
    loop {
        terminal.draw(|f| ui(f, app))?;

        if let Event::Key(key) = event::read()? {
            if key.kind != KeyEventKind::Press {
                continue;
            }

            // Extract mode info (all Copy) before releasing the borrow.
            enum ModeTag {
                O3List,
                O3Detail(usize, usize),
                CreatingO3,
                SettingDate(usize),
                AddingEntry(usize),
                EditingOutput(usize, usize),
            }
            let tag = match &app.mode {
                Mode::O3List => ModeTag::O3List,
                Mode::O3Detail { o3_idx, prev_list_selected } => {
                    ModeTag::O3Detail(*o3_idx, *prev_list_selected)
                }
                Mode::CreatingO3 => ModeTag::CreatingO3,
                Mode::SettingDate { o3_idx } => ModeTag::SettingDate(*o3_idx),
                Mode::AddingEntry { o3_idx, .. } => ModeTag::AddingEntry(*o3_idx),
                Mode::EditingOutput { o3_idx, entry_idx, .. } => {
                    ModeTag::EditingOutput(*o3_idx, *entry_idx)
                }
            };

            match tag {
                ModeTag::O3List => {
                    if key.code == KeyCode::Char('q') {
                        return Ok(());
                    }
                    handle_o3_list(app, key.code);
                }
                ModeTag::O3Detail(o3_idx, prev) => {
                    if key.code == KeyCode::Char('q') {
                        return Ok(());
                    }
                    handle_o3_detail(app, key.code, o3_idx, prev);
                }
                ModeTag::CreatingO3 => handle_creating_o3(app, key.code),
                ModeTag::SettingDate(o3_idx) => handle_setting_date(app, key.code, o3_idx),
                ModeTag::AddingEntry(o3_idx) => handle_adding_entry(app, key.code, o3_idx),
                ModeTag::EditingOutput(o3_idx, entry_idx) => {
                    handle_editing_output(app, key.code, o3_idx, entry_idx)
                }
            }
        }
    }
}

// ─── UI helpers ───────────────────────────────────────────────────────────────

/// Mirrors Mode without borrowing app, so we can pass app immutably to renderers.
enum UiState {
    O3List,
    O3Detail(usize),
    CreatingO3,
    SettingDate(usize),
    AddingEntry(usize, EntryWizard),
    EditingOutput(usize, String, usize),
}

fn ui(f: &mut Frame, app: &App) {
    let area = f.area();

    let state = match &app.mode {
        Mode::O3List => UiState::O3List,
        Mode::O3Detail { o3_idx, .. } => UiState::O3Detail(*o3_idx),
        Mode::CreatingO3 => UiState::CreatingO3,
        Mode::SettingDate { o3_idx } => UiState::SettingDate(*o3_idx),
        Mode::AddingEntry { o3_idx, wizard } => UiState::AddingEntry(*o3_idx, wizard.clone()),
        Mode::EditingOutput { o3_idx, text, cursor, .. } => {
            UiState::EditingOutput(*o3_idx, text.clone(), *cursor)
        }
    };

    match state {
        UiState::O3List => render_o3_list(f, app, area),
        UiState::O3Detail(idx) => render_o3_detail(f, app, idx, area),
        UiState::CreatingO3 => {
            render_o3_list(f, app, area);
            render_creating_o3_popup(f, area);
        }
        UiState::SettingDate(idx) => {
            render_o3_detail(f, app, idx, area);
            render_setting_date_popup(f, app, idx, area);
        }
        UiState::AddingEntry(idx, wizard) => {
            render_o3_detail(f, app, idx, area);
            render_adding_entry_popup(f, &wizard, area);
        }
        UiState::EditingOutput(idx, text, cursor) => {
            render_o3_detail(f, app, idx, area);
            render_text_popup(
                f,
                &text,
                cursor,
                " Output (Enter:save  Esc:cancel) ",
                Color::Cyan,
                area,
            );
        }
    }
}

// ─── Screen: O3 list ──────────────────────────────────────────────────────────

fn render_o3_list(f: &mut Frame, app: &App, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(3)])
        .split(area);

    let sorted = app.sorted_o3_indices();
    let mut items: Vec<ListItem> = Vec::new();

    if sorted.is_empty() {
        items.push(ListItem::new(Line::from(Span::styled(
            "  No O3s yet. Press 'a' to create one.",
            Style::default().fg(Color::DarkGray),
        ))));
    } else {
        for (list_i, &o3_idx) in sorted.iter().enumerate() {
            let o3 = &app.o3s[o3_idx];
            let selected = list_i == app.selected && matches!(app.mode, Mode::O3List);

            let date_str = match o3.date_days {
                Some(d) => o3_label(d),
                None => "No date set".to_string(),
            };

            let count = o3.entries.len();
            let count_str = match count {
                0 => "empty".to_string(),
                1 => "1 item".to_string(),
                n => format!("{} items", n),
            };

            let cursor_sym = if selected { "► " } else { "  " };

            let person_style = if selected {
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
            } else {
                match o3.direct {
                    Direct::Mario => Style::default().fg(Color::Green),
                    Direct::Martins => Style::default().fg(Color::Cyan),
                }
            };

            let date_style = if selected {
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
            } else if o3.date_days.map_or(true, |d| d < today_days()) {
                Style::default().fg(Color::DarkGray)
            } else {
                Style::default()
            };

            items.push(ListItem::new(Line::from(vec![
                Span::raw(cursor_sym),
                Span::styled(format!("{:<8}", o3.direct.name()), person_style),
                Span::raw("  "),
                Span::styled(format!("{:<28}", date_str), date_style),
                Span::styled(count_str, Style::default().fg(Color::DarkGray)),
            ])));
        }
    }

    let list =
        List::new(items).block(Block::default().borders(Borders::ALL).title(" O3 Manager "));
    f.render_widget(list, chunks[0]);

    let help = "a:new O3  Enter:open  d:delete  j/k:↑↓  q:quit";
    f.render_widget(help_bar(help), chunks[1]);
}

// ─── Screen: O3 detail ────────────────────────────────────────────────────────

fn render_o3_detail(f: &mut Frame, app: &App, o3_idx: usize, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(3)])
        .split(area);

    let Some(o3) = app.o3s.get(o3_idx) else { return };

    let date_str = match o3.date_days {
        Some(d) => o3_label(d),
        None => "No date  (press 't' to set)".to_string(),
    };
    let title = format!(" {} ─ {} ", o3.direct.name(), date_str);

    let mut items: Vec<ListItem> = Vec::new();

    if o3.entries.is_empty() {
        items.push(ListItem::new(Line::from(Span::styled(
            "  No items yet. Press 'a' to add feedback or a topic.",
            Style::default().fg(Color::DarkGray),
        ))));
    } else {
        for (i, entry) in o3.entries.iter().enumerate() {
            let selected = i == app.selected && matches!(app.mode, Mode::O3Detail { .. });

            let (prefix, accent) = match &entry.kind {
                ItemKind::Feedback { polarity: Polarity::Positive, .. } => ("[+]", Color::Green),
                ItemKind::Feedback { polarity: Polarity::Negative, .. } => ("[-]", Color::Red),
                ItemKind::Topic { .. } => ("[T]", Color::Cyan),
            };

            let cursor_sym = if selected { "► " } else { "  " };

            let pfx_style = if selected {
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(accent)
            };

            let txt_style = if selected {
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };

            items.push(ListItem::new(Line::from(vec![
                Span::raw(cursor_sym),
                Span::styled(prefix, pfx_style),
                Span::raw(" "),
                Span::styled(entry.text.as_str(), txt_style),
            ])));

            let out = match &entry.kind {
                ItemKind::Topic { output: Some(o) } => Some(o.as_str()),
                ItemKind::Feedback { output: Some(o), .. } => Some(o.as_str()),
                _ => None,
            };
            if let Some(out) = out {
                items.push(ListItem::new(Line::from(vec![
                    Span::raw("       └ "),
                    Span::styled(
                        out,
                        Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC),
                    ),
                ])));
            }
        }
    }

    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(title));
    f.render_widget(list, chunks[0]);

    let help = "a:add  t:set date  j/k:↑↓  o:output  d:delete  Esc:back  q:quit";
    f.render_widget(help_bar(help), chunks[1]);
}

// ─── Popups ───────────────────────────────────────────────────────────────────

fn render_creating_o3_popup(f: &mut Frame, area: Rect) {
    let popup = centered_rect(44, 55, area);
    let lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            "  Create O3 for:",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled("  [1]  Mario", Style::default().fg(Color::Green))),
        Line::from(Span::styled("  [2]  Martins", Style::default().fg(Color::Cyan))),
        Line::from(""),
        Line::from(Span::styled("  Esc  cancel", Style::default().fg(Color::DarkGray))),
    ];
    render_choice_popup(f, " New O3 ", lines, Color::Blue, popup);
}

fn render_setting_date_popup(f: &mut Frame, app: &App, o3_idx: usize, area: Rect) {
    let person = app.o3s.get(o3_idx).map(|o| o.direct.name()).unwrap_or("?");
    let options = thursday_options();
    let popup = centered_rect(46, 80, area);

    let mut lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            format!("  Set date for {}'s O3:", person),
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
    ];

    for (i, &d) in options.iter().enumerate() {
        lines.push(Line::from(Span::styled(
            format!("  [{}]  {}", i + 1, o3_label(d)),
            Style::default().fg(Color::Yellow),
        )));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  [0]  Clear date",
        Style::default().fg(Color::DarkGray),
    )));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  Esc  cancel",
        Style::default().fg(Color::DarkGray),
    )));

    let title = format!(" Set Date – {} ", person);
    render_choice_popup(f, &title, lines, Color::Blue, popup);
}

fn render_adding_entry_popup(f: &mut Frame, wizard: &EntryWizard, area: Rect) {
    match &wizard.step {
        EntryStep::SelectItemType => {
            let popup = centered_rect(44, 50, area);
            let lines = vec![
                Line::from(""),
                Line::from(Span::styled(
                    "  Item type:",
                    Style::default().add_modifier(Modifier::BOLD),
                )),
                Line::from(""),
                Line::from(Span::styled("  [f]  Feedback", Style::default().fg(Color::Yellow))),
                Line::from(Span::styled("  [t]  Topic", Style::default().fg(Color::Yellow))),
                Line::from(""),
                Line::from(Span::styled("  Esc  cancel", Style::default().fg(Color::DarkGray))),
            ];
            render_choice_popup(f, " Add Item ", lines, Color::Green, popup);
        }
        EntryStep::SelectPolarity => {
            let popup = centered_rect(44, 50, area);
            let lines = vec![
                Line::from(""),
                Line::from(Span::styled(
                    "  Feedback type:",
                    Style::default().add_modifier(Modifier::BOLD),
                )),
                Line::from(""),
                Line::from(Span::styled(
                    "  [+]  Positive",
                    Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
                )),
                Line::from(Span::styled(
                    "  [-]  Negative",
                    Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                )),
                Line::from(""),
                Line::from(Span::styled("  Esc  cancel", Style::default().fg(Color::DarkGray))),
            ];
            render_choice_popup(f, " Add Feedback ", lines, Color::Green, popup);
        }
        EntryStep::EnterText => {
            let label = if wizard.is_topic {
                " Topic text (Enter:save  Esc:cancel) "
            } else {
                " Feedback text (Enter:save  Esc:cancel) "
            };
            render_text_popup(f, &wizard.text, wizard.cursor, label, Color::Green, area);
        }
    }
}

fn render_choice_popup(f: &mut Frame, title: &str, lines: Vec<Line>, color: Color, popup: Rect) {
    let para = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .title(title)
            .style(Style::default().fg(color)),
    );
    f.render_widget(Clear, popup);
    f.render_widget(para, popup);
}

fn render_text_popup(
    f: &mut Frame,
    text: &str,
    cursor: usize,
    title: &str,
    color: Color,
    area: Rect,
) {
    let popup = centered_rect(65, 20, area);
    let para = Paragraph::new(text).block(
        Block::default()
            .borders(Borders::ALL)
            .title(title)
            .style(Style::default().fg(color)),
    );
    f.render_widget(Clear, popup);
    f.render_widget(para, popup);
    f.set_cursor_position((popup.x + 1 + cursor as u16, popup.y + 1));
}

fn help_bar(text: &str) -> Paragraph<'_> {
    Paragraph::new(text)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .style(Style::default().fg(Color::DarkGray)),
        )
        .alignment(Alignment::Center)
}

fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}
