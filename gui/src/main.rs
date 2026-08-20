//! `everything-gui` — native egui frontend over the core engine.
//!
//! Everything-style semantics:
//! - name mode (default): glob patterns or regex against basename/full path,
//! - content mode (toggle): the query is a regex searched inside files.
//!
//! UX: system fonts, dark/light themes (follows the system initially),
//! search history (↑/↓ + dropdown, persisted), file-type icons, a virtualized
//! results table with column sorting, full keyboard navigation, and a file
//! preview pane.

use eframe::egui;
use egui_extras::{Column, TableBuilder};
use everything_core::{
    ContentIndexStatus, Engine, Query, ResultRow, SearchResponse, State, Status,
};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};

const DEBOUNCE_MS: u128 = 120;
const HISTORY_CAP: usize = 20;
const ACCENT: egui::Color32 = egui::Color32::from_rgb(108, 148, 255);
const BG_PANEL: egui::Color32 = egui::Color32::from_rgb(22, 24, 30);
const BG_WIDGET_DARK: egui::Color32 = egui::Color32::from_rgb(31, 34, 42);
const BG_WIDGET_HOVER_DARK: egui::Color32 = egui::Color32::from_rgb(38, 42, 52);
const BG_WIDGET_LIGHT: egui::Color32 = egui::Color32::from_rgb(232, 234, 240);
const BG_WIDGET_HOVER_LIGHT: egui::Color32 = egui::Color32::from_rgb(220, 224, 234);
const DIM_DARK: egui::Color32 = egui::Color32::from_rgb(140, 148, 165);
const DIM_LIGHT: egui::Color32 = egui::Color32::from_rgb(100, 108, 124);

/// Font families to try for the UI (first one found on the system wins).
const UI_FONT_PREFERENCE: &[&str] = &[
    "Noto Sans",
    "Cantarell",
    "Ubuntu",
    "DejaVu Sans",
    "Liberation Sans",
    "Roboto",
    "Fira Sans",
];
const MONO_FONT_PREFERENCE: &[&str] = &[
    "JetBrains Mono",
    "Fira Code",
    "Fira Mono",
    "DejaVu Sans Mono",
    "Liberation Mono",
    "Noto Sans Mono",
    "Ubuntu Mono",
];

enum UiMsg {
    Search(Query),
}

enum OutMsg {
    Done(Result<SearchResponse, String>),
}

fn main() -> eframe::Result {
    let config = everything_core::Config::load();
    let mut engine = Engine::new(config);
    engine.start();
    let engine = Arc::new(engine);

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Everything for Linux")
            // Wayland-first: the app id is what the compositor uses for
            // desktop integration (window icon, taskbar grouping). X11 is the
            // fallback backend — winit picks it automatically when no Wayland
            // session is available (or with WINIT_UNIX_BACKEND=x11).
            .with_app_id("everything-linux")
            .with_inner_size([1120.0, 720.0])
            .with_min_inner_size([560.0, 360.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Everything for Linux",
        options,
        Box::new(move |cc| Ok(Box::new(App::new(cc, engine)))),
    )
}

/// Persistent GUI preferences (`~/.config/everything-linux/gui.json`).
#[derive(Serialize, Deserialize, Default)]
struct GuiPrefs {
    /// None = follow the system theme.
    dark: Option<bool>,
    show_preview: bool,
    /// Most recent first.
    history: Vec<String>,
}

impl GuiPrefs {
    fn path() -> PathBuf {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                std::env::var_os("HOME")
                    .map(|h| PathBuf::from(h).join(".config"))
                    .unwrap_or_else(|| PathBuf::from("."))
            });
        base.join("everything-linux").join("gui.json")
    }

    fn load() -> GuiPrefs {
        std::fs::read_to_string(Self::path())
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    fn save(&self) {
        if let Ok(text) = serde_json::to_string_pretty(self) {
            let p = Self::path();
            if let Some(dir) = p.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = std::fs::write(p, text);
        }
    }

    /// Remember a query (dedup, most-recent-first, capped).
    fn commit_query(&mut self, q: &str) {
        let q = q.trim();
        if q.is_empty() {
            return;
        }
        self.history.retain(|h| h != q);
        self.history.insert(0, q.to_string());
        self.history.truncate(HISTORY_CAP);
        self.save();
    }

    fn clear_history(&mut self) {
        self.history.clear();
        self.save();
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Sort {
    Name(bool), // asc
    Size(bool),
    Mtime(bool),
}

struct Preview {
    path: PathBuf,
    is_dir: bool,
    size: u64,
    mtime: i64,
    text: String,
}

struct App {
    engine: Arc<Engine>,
    prefs: GuiPrefs,
    query: String,
    regex_mode: bool,
    content_mode: bool,
    case_sensitive: bool,
    hidden: bool,
    full_path: bool,
    limit: usize,
    results: Vec<ResultRow>,
    truncated: bool,
    error: Option<String>,
    elapsed_ms: u64,
    status: Status,
    last_sent: String,
    pending: bool,
    last_edit: Instant,
    query_tx: mpsc::Sender<UiMsg>,
    result_rx: mpsc::Receiver<OutMsg>,
    selected: usize,
    scroll_to: Option<usize>,
    sort: Option<Sort>,
    preview: Option<Preview>,
    dark: bool,
    /// Position in the search-history cycle (None = not browsing).
    history_idx: Option<usize>,
    search_was_focused: bool,
    ui_font: Option<Vec<u8>>,
    mono_font: Option<Vec<u8>>,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>, engine: Arc<Engine>) -> App {
        let (query_tx, query_rx) = mpsc::channel::<UiMsg>();
        let (result_tx, result_rx) = mpsc::channel::<OutMsg>();
        let engine_worker = Arc::clone(&engine);

        // Dedicated search thread: queries never block the UI.
        std::thread::Builder::new()
            .name("search".into())
            .spawn(move || {
                while let Ok(UiMsg::Search(q)) = query_rx.recv() {
                    let res = engine_worker.search(&q);
                    let _ = result_tx.send(OutMsg::Done(res));
                }
            })
            .expect("failed to spawn search thread");

        let prefs = GuiPrefs::load();
        let dark = match prefs.dark {
            Some(d) => d,
            None => !matches!(dark_light::detect(), dark_light::Mode::Light),
        };
        let (ui_font, mono_font) = load_system_fonts();
        let status_snapshot = engine.status_snapshot();

        let mut app = App {
            engine,
            prefs,
            query: String::new(),
            regex_mode: false,
            content_mode: false,
            case_sensitive: false,
            hidden: false,
            full_path: false,
            limit: 500,
            results: Vec::new(),
            truncated: false,
            error: None,
            elapsed_ms: 0,
            status: status_snapshot,
            last_sent: String::new(),
            pending: false,
            last_edit: Instant::now(),
            query_tx,
            result_rx,
            selected: 0,
            scroll_to: None,
            sort: None,
            preview: None,
            dark,
            history_idx: None,
            search_was_focused: false,
            ui_font,
            mono_font,
        };
        app.apply_style(&cc.egui_ctx);
        app.send_query();
        app
    }

    fn apply_style(&self, ctx: &egui::Context) {
        // --- visuals (dark / light) ---
        let mut visuals = if self.dark {
            egui::Visuals::dark()
        } else {
            egui::Visuals::light()
        };
        visuals.selection.bg_fill = ACCENT;
        visuals.selection.stroke = egui::Stroke::new(1.0_f32, ACCENT);
        visuals.hyperlink_color = ACCENT;
        visuals.widgets.noninteractive.fg_stroke.color = if self.dark {
            egui::Color32::from_rgb(216, 220, 228)
        } else {
            egui::Color32::from_rgb(40, 44, 54)
        };
        let (w, wh) = if self.dark {
            (BG_WIDGET_DARK, BG_WIDGET_HOVER_DARK)
        } else {
            (BG_WIDGET_LIGHT, BG_WIDGET_HOVER_LIGHT)
        };
        visuals.widgets.inactive.weak_bg_fill = w;
        visuals.widgets.hovered.weak_bg_fill = wh;
        visuals.widgets.active.weak_bg_fill = wh;
        visuals.widgets.inactive.corner_radius = egui::CornerRadius::same(6);
        visuals.widgets.hovered.corner_radius = egui::CornerRadius::same(6);
        visuals.widgets.active.corner_radius = egui::CornerRadius::same(6);
        visuals.window_corner_radius = egui::CornerRadius::same(10);
        if self.dark {
            visuals.panel_fill = BG_PANEL;
        }
        ctx.set_visuals(visuals);

        // --- fonts (system UI + system mono, egui defaults as fallback) ---
        let mut fonts = egui::FontDefinitions::default();
        if let Some(bytes) = &self.ui_font {
            fonts.font_data.insert(
                "system-ui".to_owned(),
                Arc::new(egui::FontData::from_owned(bytes.clone())),
            );
            fonts
                .families
                .entry(egui::FontFamily::Proportional)
                .or_default()
                .insert(0, "system-ui".to_owned());
        }
        if let Some(bytes) = &self.mono_font {
            fonts.font_data.insert(
                "system-mono".to_owned(),
                Arc::new(egui::FontData::from_owned(bytes.clone())),
            );
            fonts
                .families
                .entry(egui::FontFamily::Monospace)
                .or_default()
                .insert(0, "system-mono".to_owned());
        }
        ctx.set_fonts(fonts);

        // --- sizes ---
        let mut style = (*ctx.style()).clone();
        style.spacing.item_spacing = egui::vec2(8.0, 6.0);
        style.spacing.button_padding = egui::vec2(10.0, 5.0);
        style.text_styles.insert(
            egui::TextStyle::Heading,
            egui::FontId::new(22.0, egui::FontFamily::Proportional),
        );
        style.text_styles.insert(
            egui::TextStyle::Body,
            egui::FontId::new(16.0, egui::FontFamily::Proportional),
        );
        style.text_styles.insert(
            egui::TextStyle::Button,
            egui::FontId::new(15.0, egui::FontFamily::Proportional),
        );
        style.text_styles.insert(
            egui::TextStyle::Small,
            egui::FontId::new(13.0, egui::FontFamily::Proportional),
        );
        style.text_styles.insert(
            egui::TextStyle::Monospace,
            egui::FontId::new(14.0, egui::FontFamily::Monospace),
        );
        ctx.set_style(style);
    }

    fn fg_dim(&self) -> egui::Color32 {
        if self.dark {
            DIM_DARK
        } else {
            DIM_LIGHT
        }
    }

    fn send_query(&mut self) {
        let name = if self.content_mode {
            String::new()
        } else {
            self.query.clone()
        };
        let content = if self.content_mode && !self.query.is_empty() {
            Some(self.query.clone())
        } else {
            None
        };
        let q = Query {
            name,
            regex_mode: self.regex_mode,
            case_sensitive: self.case_sensitive,
            include_hidden: self.hidden,
            full_path: self.full_path,
            content,
            limit: self.limit,
        };
        let _ = self.query_tx.send(UiMsg::Search(q));
        self.last_sent = self.query.clone();
        self.pending = true;
        self.selected = 0;
        self.scroll_to = None;
    }

    fn open(path: &Path) {
        let _ = Command::new("xdg-open").arg(path).spawn();
    }

    fn open_folder(path: &Path) {
        if let Some(parent) = path.parent() {
            let _ = Command::new("xdg-open").arg(parent).spawn();
        }
    }

    fn refresh_preview(&mut self) {
        let Some(row) = self.results.get(self.selected) else {
            self.preview = None;
            return;
        };
        if self.preview.as_ref().is_some_and(|p| p.path == row.path) {
            return;
        }
        let mut text = String::new();
        if !row.is_dir && row.size < 512 * 1024 {
            if let Ok(file) = std::fs::File::open(&row.path) {
                use std::io::Read;
                let mut buf = Vec::new();
                let _ = file.take(64 * 1024).read_to_end(&mut buf);
                text = String::from_utf8_lossy(&buf).into_owned();
            }
        }
        self.preview = Some(Preview {
            path: row.path.clone(),
            is_dir: row.is_dir,
            size: row.size,
            mtime: row.mtime,
            text,
        });
    }

    fn select(&mut self, idx: usize, ctx: &egui::Context) {
        let len = self.results.len();
        if len == 0 {
            return;
        }
        self.selected = idx.min(len - 1);
        self.scroll_to = Some(self.selected);
        ctx.request_repaint();
    }
}

fn search_id() -> egui::Id {
    egui::Id::new("search_input")
}

/// Find system font faces (UI + monospace) and return their raw bytes.
fn load_system_fonts() -> (Option<Vec<u8>>, Option<Vec<u8>>) {
    let mut db = fontdb::Database::new();
    db.load_system_fonts();
    let ui = first_available_face(&db, UI_FONT_PREFERENCE);
    let mono = first_available_face(&db, MONO_FONT_PREFERENCE);
    (ui, mono)
}

fn first_available_face(db: &fontdb::Database, families: &[&str]) -> Option<Vec<u8>> {
    for family in families {
        let query = fontdb::Query {
            families: &[fontdb::Family::Name(family)],
            weight: fontdb::Weight::NORMAL,
            stretch: fontdb::Stretch::Normal,
            style: fontdb::Style::Normal,
        };
        if let Some(id) = db.query(&query) {
            let mut bytes: Option<Vec<u8>> = None;
            db.with_face_data(id, |data, _face| bytes = Some(data.to_vec()));
            if bytes.is_some() {
                return bytes;
            }
        }
    }
    None
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Drain completed searches.
        while let Ok(OutMsg::Done(res)) = self.result_rx.try_recv() {
            match res {
                Ok(r) => {
                    self.results = r.results;
                    self.truncated = r.truncated;
                    self.elapsed_ms = r.elapsed_ms;
                    self.error = None;
                    self.pending = false;
                    if let Some(sort) = self.sort {
                        sort_results(&mut self.results, sort);
                    }
                }
                Err(e) => {
                    self.error = Some(e);
                    self.pending = false;
                }
            }
        }
        self.status = self.engine.status_snapshot();

        // Debounced live search.
        if self.query != self.last_sent {
            if self.pending {
                self.last_sent = self.query.clone();
                self.pending = false;
                self.send_query();
            } else if self.last_edit.elapsed() >= Duration::from_millis(DEBOUNCE_MS as u64) {
                self.send_query();
            } else {
                ctx.request_repaint_after(Duration::from_millis(DEBOUNCE_MS as u64));
            }
        }

        let search_focused = ctx.memory(|m| m.has_focus(search_id()));

        // Commit a finished query to history when the search box loses focus.
        if self.search_was_focused && !search_focused && !self.query.is_empty() && !self.pending {
            let q = self.query.clone();
            self.prefs.commit_query(&q);
        }
        self.search_was_focused = search_focused;

        // Keyboard shortcuts.
        if search_focused && self.query.is_empty() {
            // ↑ / ↓ cycle search history.
            if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp)) {
                let n = self.prefs.history.len();
                if n > 0 {
                    let idx = self.history_idx.map_or(0, |i| (i + 1).min(n - 1));
                    self.history_idx = Some(idx);
                    self.query = self.prefs.history[idx].clone();
                    self.last_edit = Instant::now();
                }
            }
            if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown)) {
                match self.history_idx {
                    Some(0) => {
                        self.history_idx = None;
                        self.query.clear();
                    }
                    Some(i) => {
                        self.history_idx = Some(i - 1);
                        self.query = self.prefs.history[i - 1].clone();
                        self.last_edit = Instant::now();
                    }
                    None => {}
                }
            }
        } else if !search_focused {
            // Result navigation.
            if ctx.input(|i| i.key_pressed(egui::Key::ArrowDown)) {
                self.select(self.selected + 1, ctx);
            }
            if ctx.input(|i| i.key_pressed(egui::Key::ArrowUp)) {
                self.select(self.selected.saturating_sub(1), ctx);
            }
            if ctx.input(|i| i.key_pressed(egui::Key::PageDown)) {
                self.select(self.selected + 20, ctx);
            }
            if ctx.input(|i| i.key_pressed(egui::Key::PageUp)) {
                self.select(self.selected.saturating_sub(20), ctx);
            }
            if ctx.input(|i| i.key_pressed(egui::Key::Enter)) {
                if let Some(row) = self.results.get(self.selected) {
                    App::open(&row.path);
                }
            }
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            if !self.query.is_empty() {
                self.query.clear();
                self.history_idx = None;
                self.send_query();
            }
        }
        if ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::F)) {
            ctx.memory_mut(|m| m.request_focus(search_id()));
        }

        // Keep the status bar live while indexing.
        ctx.request_repaint_after(Duration::from_millis(250));

        self.top_panel(ctx);
        if self.prefs.show_preview {
            self.refresh_preview();
            egui::SidePanel::right("preview")
                .resizable(true)
                .default_width(360.0)
                .min_width(240.0)
                .show(ctx, |ui| self.preview_panel(ui));
        }
        egui::TopBottomPanel::bottom("status").show(ctx, |ui| self.status_bar(ui));
        egui::CentralPanel::default().show(ctx, |ui| self.results_table(ui));
    }
}

impl App {
    fn top_panel(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("search").show(ctx, |ui| {
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("🔍").size(20.0));
                let dim = self.fg_dim();
                let hint = if self.content_mode {
                    "Search file contents (regex)…"
                } else if self.regex_mode {
                    "Search filenames (regex)…"
                } else {
                    "Search — e.g. *.pdf · invoice 2026 · !draft"
                };
                let edit = egui::TextEdit::singleline(&mut self.query)
                    .id(search_id())
                    .hint_text(egui::RichText::new(hint).color(dim))
                    .font(egui::TextStyle::Heading)
                    .desired_width(f32::INFINITY)
                    .margin(egui::vec2(12.0, 9.0));
                let resp = ui.add(edit);
                if resp.changed() {
                    self.last_edit = Instant::now();
                    self.history_idx = None;
                }
                // Enter commits the query to history and opens the top hit.
                if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    let q = self.query.clone();
                    self.prefs.commit_query(&q);
                    self.history_idx = None;
                    if let Some(first) = self.results.first() {
                        App::open(&first.path);
                    }
                }
                // History dropdown.
                let history = self.prefs.history.clone();
                ui.menu_button("🕘", |ui| {
                    ui.set_min_width(260.0);
                    if history.is_empty() {
                        ui.weak("No recent searches");
                    }
                    for q in &history {
                        if ui.button(q).clicked() {
                            self.query = q.clone();
                            self.last_edit = Instant::now();
                            self.history_idx = None;
                            ui.close_menu();
                        }
                    }
                    if !history.is_empty() {
                        ui.separator();
                        if ui.button("Clear history").clicked() {
                            self.prefs.clear_history();
                            ui.close_menu();
                        }
                    }
                });
                if !self.query.is_empty() && ui.button("✕").on_hover_text("Clear").clicked() {
                    self.query.clear();
                    self.history_idx = None;
                    self.send_query();
                }
                if self.pending {
                    ui.spinner();
                }
            });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.checkbox(&mut self.content_mode, "Match contents")
                    .on_hover_text("Search inside files (regex)");
                ui.checkbox(&mut self.regex_mode, ".* Regex")
                    .on_hover_text("Treat query terms as regex");
                ui.checkbox(&mut self.case_sensitive, "Aa")
                    .on_hover_text("Case-sensitive");
                ui.checkbox(&mut self.hidden, "Hidden")
                    .on_hover_text("Include hidden files & dirs");
                ui.checkbox(&mut self.full_path, "Full path")
                    .on_hover_text("Match the whole path, not just the name");
                ui.separator();
                if ui
                    .checkbox(&mut self.prefs.show_preview, "👁 Preview")
                    .on_hover_text("Show a preview of the selected file")
                    .changed()
                {
                    self.prefs.save();
                }
                if ui
                    .button(if self.dark { "☀️" } else { "🌙" })
                    .on_hover_text("Toggle dark / light theme")
                    .clicked()
                {
                    self.dark = !self.dark;
                    self.prefs.dark = Some(self.dark);
                    self.prefs.save();
                    self.apply_style(ctx);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let mut label = format!(
                        "{} result{}",
                        self.results.len(),
                        if self.results.len() == 1 { "" } else { "s" }
                    );
                    if self.truncated {
                        label.push('+');
                    }
                    if self.elapsed_ms > 0 {
                        label.push_str(&format!(" · {} ms", self.elapsed_ms));
                    }
                    ui.label(egui::RichText::new(label).color(self.fg_dim()).small());
                });
            });
            ui.add_space(8.0);
        });
    }

    fn results_table(&mut self, ui: &mut egui::Ui) {
        if let Some(err) = &self.error {
            ui.add_space(24.0);
            ui.vertical_centered(|ui| {
                ui.colored_label(ui.visuals().error_fg_color, format!("⚠ Query error: {err}"));
            });
            return;
        }

        if self.results.is_empty() {
            ui.add_space(60.0);
            ui.vertical_centered(|ui| {
                if !self.query.is_empty() && !self.pending {
                    ui.label(egui::RichText::new("🔎").size(52.0));
                    ui.add_space(8.0);
                    ui.label(egui::RichText::new("No results").size(22.0).strong());
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new(
                            "Try fewer terms, a different pattern, or tick “Match contents”",
                        )
                        .color(self.fg_dim()),
                    );
                } else if self.query.is_empty() && self.status.state != State::Live {
                    ui.spinner();
                    ui.add_space(8.0);
                    ui.label(
                        egui::RichText::new(format!("Indexing… {} files", self.engine.counts().0))
                            .color(self.fg_dim()),
                    );
                } else {
                    ui.label(egui::RichText::new("🔍").size(52.0));
                    ui.add_space(8.0);
                    ui.label(
                        egui::RichText::new(format!(
                            "{} files indexed — start typing to search",
                            self.engine.counts().0
                        ))
                        .color(self.fg_dim()),
                    );
                }
            });
            return;
        }

        let mut table = TableBuilder::new(ui)
            .striped(true)
            .resizable(true)
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::remainder().clip(true))
            .column(Column::exact(92.0).at_least(70.0).clip(true))
            .column(Column::exact(150.0).at_least(110.0).clip(true));
        if let Some(target) = self.scroll_to.take() {
            table = table.scroll_to_row(target, Some(egui::Align::Center));
        }

        table
            .header(30.0, |mut header| {
                header.col(|ui| {
                    if sort_button(ui, "Name", self.sort, |s| matches!(s, Sort::Name(_))).clicked()
                    {
                        self.sort = cycle_sort(self.sort, Sort::Name(true));
                    }
                });
                header.col(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if sort_button(ui, "Size", self.sort, |s| matches!(s, Sort::Size(_)))
                            .clicked()
                        {
                            self.sort = cycle_sort(self.sort, Sort::Size(true));
                        }
                    });
                });
                header.col(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if sort_button(ui, "Modified", self.sort, |s| matches!(s, Sort::Mtime(_)))
                            .clicked()
                        {
                            self.sort = cycle_sort(self.sort, Sort::Mtime(true));
                        }
                    });
                });
            })
            .body(|body| {
                let rows = self.results.len();
                let selected = self.selected;
                body.rows(42.0, rows, |mut row| {
                    let i = row.index();
                    row.set_selected(i == selected);
                    let r = &self.results[i];

                    row.col(|ui| {
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new(icon_for(&r.path, r.is_dir)).size(17.0));
                            ui.vertical(|ui| {
                                let name = r
                                    .path
                                    .file_name()
                                    .map(|n| n.to_string_lossy().into_owned())
                                    .unwrap_or_else(|| r.path.display().to_string());
                                ui.label(egui::RichText::new(name).strong());
                                ui.label(
                                    egui::RichText::new(r.path.display().to_string())
                                        .small()
                                        .monospace()
                                        .color(self.fg_dim()),
                                );
                            });
                        });
                    });
                    row.col(|ui| {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(
                                egui::RichText::new(if r.is_dir {
                                    "—".to_string()
                                } else {
                                    human_size(r.size)
                                })
                                .color(self.fg_dim()),
                            );
                        });
                    });
                    row.col(|ui| {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(egui::RichText::new(human_time(r.mtime)).color(self.fg_dim()));
                        });
                    });

                    let resp = row.response().clone();
                    if resp.double_clicked() {
                        App::open(&r.path);
                    }
                    if resp.clicked() {
                        self.selected = i;
                    }
                    resp.context_menu(|ui| {
                        if ui.button("Open").clicked() {
                            App::open(&r.path);
                            ui.close_menu();
                        }
                        if ui.button("Open containing folder").clicked() {
                            App::open_folder(&r.path);
                            ui.close_menu();
                        }
                        if ui.button("Copy path").clicked() {
                            ui.ctx().copy_text(r.path.display().to_string());
                            ui.close_menu();
                        }
                    });
                });
            });
    }

    fn preview_panel(&mut self, ui: &mut egui::Ui) {
        let Some(pv) = &self.preview else {
            ui.add_space(40.0);
            ui.vertical_centered(|ui| {
                ui.label(egui::RichText::new("👁").size(36.0));
                ui.add_space(6.0);
                ui.label(egui::RichText::new("Select a file to preview").color(self.fg_dim()));
            });
            return;
        };

        let name = pv
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        ui.add_space(8.0);
        ui.label(
            egui::RichText::new(format!("{} {}", icon_for(&pv.path, pv.is_dir), name)).strong(),
        );
        ui.label(
            egui::RichText::new(format!(
                "{} · {}",
                pv.path.display(),
                if pv.is_dir {
                    "directory".to_string()
                } else {
                    human_size(pv.size)
                }
            ))
            .small()
            .monospace()
            .color(self.fg_dim()),
        );
        ui.label(
            egui::RichText::new(human_time(pv.mtime))
                .small()
                .color(self.fg_dim()),
        );
        ui.separator();

        if pv.is_dir {
            ui.add_space(20.0);
            ui.vertical_centered(|ui| {
                ui.label(egui::RichText::new("📁 Directory").size(30.0));
                ui.add_space(8.0);
                if ui.button("Open").clicked() {
                    App::open(&pv.path);
                }
            });
            return;
        }
        if pv.text.is_empty() {
            ui.label(egui::RichText::new("No preview (binary or too large)").color(self.fg_dim()));
            return;
        }
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .stick_to_bottom(false)
            .show(ui, |ui| {
                ui.label(
                    egui::RichText::new(&pv.text)
                        .monospace()
                        .color(if self.dark {
                            egui::Color32::from_rgb(200, 208, 218)
                        } else {
                            egui::Color32::from_rgb(30, 34, 42)
                        }),
                );
            });
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let (files, dirs) = self.engine.counts();
            let mut parts = Vec::new();
            match self.status.state {
                State::Starting => parts.push("starting…".to_string()),
                State::Indexing => parts.push(format!("indexing… {files} files")),
                State::Live => parts.push(format!("{files} files · {dirs} dirs")),
            }
            parts.push(if self.status.base_entries > 0 {
                "mmap index".to_string()
            } else {
                "ram index".to_string()
            });
            if self.status.degraded {
                parts.push("⚠ degraded (periodic rebuild)".to_string());
            }
            if self.status.overlay_pending > 0 {
                parts.push(format!("{} pending", self.status.overlay_pending));
            }
            match &self.status.content_index {
                ContentIndexStatus::Enabled { entries, bytes, pending } => {
                    parts.push(format!(
                        "content cache: {entries} files · {} MiB{}",
                        bytes / (1024 * 1024),
                        if *pending > 0 { format!(" · {pending} pending") } else { String::new() }
                    ));
                }
                ContentIndexStatus::Disabled => {}
            }
            ui.label(egui::RichText::new(parts.join(" · ")).color(self.fg_dim()).small());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    egui::RichText::new(
                        "↑↓ results · ↑↓ in empty search = history · Enter open · Esc clear · Ctrl+F search",
                    )
                    .color(self.fg_dim())
                    .small(),
                );
            });
        });
    }
}

/// Header sort button showing ▲/▼ for the active sort column.
fn sort_button(
    ui: &mut egui::Ui,
    label: &str,
    current: Option<Sort>,
    is_active: impl Fn(Sort) -> bool,
) -> egui::Response {
    let mark = match current {
        Some(s) if is_active(s) => match s {
            Sort::Name(asc) | Sort::Size(asc) | Sort::Mtime(asc) => {
                if asc {
                    " ▲"
                } else {
                    " ▼"
                }
            }
        },
        _ => "",
    };
    ui.button(format!("{label}{mark}"))
}

fn cycle_sort(current: Option<Sort>, prefer: Sort) -> Option<Sort> {
    match current {
        None => Some(prefer),
        Some(s) if same_key(s, prefer) => match s {
            Sort::Name(true) => Some(Sort::Name(false)),
            Sort::Size(true) => Some(Sort::Size(false)),
            Sort::Mtime(true) => Some(Sort::Mtime(false)),
            _ => None,
        },
        Some(_) => Some(prefer),
    }
}

fn same_key(a: Sort, b: Sort) -> bool {
    matches!(
        (a, b),
        (Sort::Name(_), Sort::Name(_))
            | (Sort::Size(_), Sort::Size(_))
            | (Sort::Mtime(_), Sort::Mtime(_))
    )
}

fn sort_results(results: &mut [ResultRow], sort: Sort) {
    match sort {
        Sort::Name(asc) => {
            results.sort_by(|a, b| {
                let ka = a
                    .path
                    .file_name()
                    .unwrap_or(a.path.as_os_str())
                    .to_string_lossy()
                    .to_lowercase();
                let kb = b
                    .path
                    .file_name()
                    .unwrap_or(b.path.as_os_str())
                    .to_string_lossy()
                    .to_lowercase();
                if asc {
                    ka.cmp(&kb)
                } else {
                    kb.cmp(&ka)
                }
            });
        }
        Sort::Size(asc) => {
            results.sort_by(|a, b| {
                if asc {
                    a.size.cmp(&b.size)
                } else {
                    b.size.cmp(&a.size)
                }
            });
        }
        Sort::Mtime(asc) => {
            results.sort_by(|a, b| {
                if asc {
                    a.mtime.cmp(&b.mtime)
                } else {
                    b.mtime.cmp(&a.mtime)
                }
            });
        }
    }
}

fn icon_for(path: &Path, is_dir: bool) -> &'static str {
    if is_dir {
        return "📁";
    }
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase());
    match ext.as_deref() {
        Some("png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "bmp" | "tiff" | "ico") => "🖼️",
        Some("mp4" | "mkv" | "avi" | "mov" | "webm" | "flv" | "mpg" | "mpeg" | "wmv") => "🎬",
        Some("mp3" | "wav" | "flac" | "ogg" | "m4a" | "aac" | "opus") => "🎵",
        Some("zip" | "tar" | "gz" | "xz" | "bz2" | "7z" | "rar" | "zst" | "deb" | "rpm") => "📦",
        Some("pdf") => "📕",
        Some("doc" | "docx" | "odt" | "rtf") => "📝",
        Some("xls" | "xlsx" | "csv" | "ods") => "📊",
        Some("ppt" | "pptx" | "odp") => "📽️",
        Some(
            "rs" | "py" | "js" | "ts" | "go" | "c" | "cpp" | "h" | "hpp" | "java" | "rb" | "sh"
            | "toml" | "json" | "yaml" | "yml" | "html" | "css" | "sql" | "php" | "lua" | "zig"
            | "ex" | "exs" | "kt" | "swift",
        ) => "💻",
        Some("txt" | "md" | "log" | "conf" | "ini" | "cfg" | "env") => "📄",
        Some("exe" | "bin" | "so" | "appimage") => "⚙️",
        _ => "📄",
    }
}

fn human_size(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = bytes as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{bytes} B")
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

fn human_time(secs: i64) -> String {
    if secs <= 0 {
        return String::new();
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let diff = now - secs;
    if diff < 0 {
        return String::new();
    }
    let (n, unit) = if diff < 60 {
        (diff, "s")
    } else if diff < 3600 {
        (diff / 60, "min")
    } else if diff < 86400 {
        (diff / 3600, "h")
    } else if diff < 30 * 86400 {
        (diff / 86400, "d")
    } else if diff < 365 * 86400 {
        (diff / (30 * 86400), "mo")
    } else {
        (diff / (365 * 86400), "y")
    };
    format!("{n} {unit} ago")
}
