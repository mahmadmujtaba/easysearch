//! `everything-gui` — native egui frontend over the core engine.
//!
//! Pro-Search layout (three panes): a category sidebar, a floating search bar
//! with in-bar toggles, a virtualized results table (two-line rows, badges,
//! breadcrumbs, hover actions), and a live preview pane (thumbnail + quick
//! actions). Tokyo Night palette; Wayland-first windowing.

use eframe::egui;
use egui_extras::{Column, TableBuilder};
use everything_core::{Category, ContentIndexStatus, Engine, Query, ResultRow, SearchResponse, State, Status};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};

const DEBOUNCE_MS: u128 = 120;
const HISTORY_CAP: usize = 20;

// Tokyo Night palette (dark).
const ACCENT: egui::Color32 = egui::Color32::from_rgb(122, 162, 247); // #7aa2f7
const BG: egui::Color32 = egui::Color32::from_rgb(26, 27, 38); // #1a1b26
const BG_PANEL: egui::Color32 = egui::Color32::from_rgb(22, 22, 30); // #16161e
const SIDEBAR_BG: egui::Color32 = egui::Color32::from_rgb(31, 35, 53); // #1f2335
const WIDGET_DARK: egui::Color32 = egui::Color32::from_rgb(41, 46, 66); // #292e42
const WIDGET_HOVER_DARK: egui::Color32 = egui::Color32::from_rgb(59, 66, 97); // #3b4261
const FG_DARK: egui::Color32 = egui::Color32::from_rgb(192, 202, 245); // #c0caf5
const DIM_DARK: egui::Color32 = egui::Color32::from_rgb(86, 95, 137); // #565f89
const WIDGET_LIGHT: egui::Color32 = egui::Color32::from_rgb(232, 234, 240);
const WIDGET_HOVER_LIGHT: egui::Color32 = egui::Color32::from_rgb(220, 224, 234);
const DIM_LIGHT: egui::Color32 = egui::Color32::from_rgb(100, 108, 124);
const OK_GREEN: egui::Color32 = egui::Color32::from_rgb(158, 206, 106); // #9ece6a
const WARN_ORANGE: egui::Color32 = egui::Color32::from_rgb(255, 158, 100); // #ff9e64
const IMG_BLUE: egui::Color32 = egui::Color32::from_rgb(122, 162, 247);
const ARCH_ORANGE: egui::Color32 = egui::Color32::from_rgb(224, 175, 104); // #e0af68
const AV_PURPLE: egui::Color32 = egui::Color32::from_rgb(187, 154, 247); // #bb9af7
const CODE_CYAN: egui::Color32 = egui::Color32::from_rgb(125, 207, 255); // #7dcfff

/// Font families to try for the UI (first one found on the system wins).
const UI_FONT_PREFERENCE: &[&str] = &[
    "Inter", "Noto Sans", "Cantarell", "Ubuntu", "DejaVu Sans", "Liberation Sans",
    "Roboto", "Fira Sans",
];
const MONO_FONT_PREFERENCE: &[&str] = &[
    "JetBrains Mono", "Fira Code", "Fira Mono", "DejaVu Sans Mono",
    "Liberation Mono", "Noto Sans Mono", "Ubuntu Mono",
];

const RECENT_AGE_SECS: i64 = 7 * 24 * 3600;
const LARGE_MIN_BYTES: u64 = 1024 * 1024 * 1024; // 1 GiB

const CATEGORIES: &[(&str, &str, Category)] = &[
    ("All", "🗂️", Category::All),
    ("Recent", "🕘", Category::Recent { max_age_secs: RECENT_AGE_SECS }),
    ("Images", "🖼️", Category::Images),
    ("Docs", "📄", Category::Docs),
    ("Code", "💻", Category::Code),
    ("Archives", "📦", Category::Archives),
    ("Audio", "🎵", Category::Audio),
    ("Video", "🎬", Category::Video),
    ("Large files > 1 GiB", "🐘", Category::Large { min_bytes: LARGE_MIN_BYTES }),
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
            .with_app_id("everything-linux")
            .with_inner_size([1240.0, 760.0])
            .with_min_inner_size([640.0, 400.0]),
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
    dark: Option<bool>,
    show_preview: bool,
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
    Name(bool),
    Size(bool),
    Mtime(bool),
}

struct Preview {
    path: PathBuf,
    is_dir: bool,
    size: u64,
    mtime: i64,
    text: String,
    image: Option<egui::TextureHandle>,
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
    category: Category,
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
            category: Category::All,
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
        let mut visuals = if self.dark { egui::Visuals::dark() } else { egui::Visuals::light() };
        if self.dark {
            visuals.panel_fill = BG_PANEL;
            visuals.window_fill = BG;
            visuals.override_text_color = Some(FG_DARK);
        }
        visuals.selection.bg_fill = ACCENT;
        visuals.selection.stroke = egui::Stroke::new(1.0_f32, ACCENT);
        visuals.hyperlink_color = ACCENT;
        let (w, wh) = if self.dark { (WIDGET_DARK, WIDGET_HOVER_DARK) } else { (WIDGET_LIGHT, WIDGET_HOVER_LIGHT) };
        visuals.widgets.inactive.weak_bg_fill = w;
        visuals.widgets.hovered.weak_bg_fill = wh;
        visuals.widgets.active.weak_bg_fill = wh;
        visuals.widgets.inactive.corner_radius = egui::CornerRadius::same(8);
        visuals.widgets.hovered.corner_radius = egui::CornerRadius::same(8);
        visuals.widgets.active.corner_radius = egui::CornerRadius::same(8);
        visuals.window_corner_radius = egui::CornerRadius::same(12);
        ctx.set_visuals(visuals);

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

    fn fg(&self) -> egui::Color32 {
        if self.dark { FG_DARK } else { egui::Color32::from_rgb(40, 44, 54) }
    }

    fn fg_dim(&self) -> egui::Color32 {
        if self.dark { DIM_DARK } else { DIM_LIGHT }
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
            category: self.category,
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

    fn open_terminal(path: &Path) {
        let dir = if path.is_dir() { path } else { path.parent().unwrap_or(path) };
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
        let run = format!("cd {} && exec {}", shell_quote(dir), shell);
        // (binary, fixed flags before the directory argument)
        for (bin, flags) in [
            ("kitty", &["--directory"][..]),
            ("konsole", &["--workdir"][..]),
            ("gnome-terminal", &["--working-directory"][..]),
            ("xfce4-terminal", &["--working-directory"][..]),
            ("alacritty", &["--working-directory"][..]),
            ("wezterm", &["start", "--cwd"][..]),
            ("xterm", &["-e", "sh", "-c"][..]),
        ] {
            let Some(full) = find_in_path(bin) else { continue };
            let mut cmd = Command::new(&full);
            cmd.args(flags);
            if bin == "xterm" {
                cmd.arg(&run);
            } else {
                cmd.arg(dir);
            }
            if cmd.spawn().is_ok() {
                return;
            }
        }
    }

    fn refresh_preview(&mut self, ctx: &egui::Context) {
        let Some(row) = self.results.get(self.selected) else {
            self.preview = None;
            return;
        };
        if self.preview.as_ref().is_some_and(|p| p.path == row.path) {
            return;
        }
        let mut text = String::new();
        let mut image = None;
        if !row.is_dir && row.size < 4 * 1024 * 1024 {
            if let Ok(bytes) = std::fs::read(&row.path) {
                if is_image_file(&row.path) {
                    if let Ok(decoded) = image::load_from_memory(&bytes) {
                        let thumb = decoded.thumbnail(280, 280);
                        let rgba = thumb.to_rgba8();
                        let (w, h) = (rgba.width() as usize, rgba.height() as usize);
                        let color = egui::ColorImage::from_rgba_unmultiplied([w, h], rgba.as_raw());
                        image = Some(ctx.load_texture("preview-thumb", color, egui::TextureOptions::LINEAR));
                    }
                } else {
                    text = String::from_utf8_lossy(&bytes[..bytes.len().min(64 * 1024)]).into_owned();
                }
            }
        }
        self.preview = Some(Preview {
            path: row.path.clone(),
            is_dir: row.is_dir,
            size: row.size,
            mtime: row.mtime,
            text,
            image,
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

fn load_system_fonts() -> (Option<Vec<u8>>, Option<Vec<u8>>) {
    let mut db = fontdb::Database::new();
    db.load_system_fonts();
    (
        first_available_face(&db, UI_FONT_PREFERENCE),
        first_available_face(&db, MONO_FONT_PREFERENCE),
    )
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

        if self.search_was_focused && !search_focused && !self.query.is_empty() && !self.pending {
            let q = self.query.clone();
            self.prefs.commit_query(&q);
        }
        self.search_was_focused = search_focused;

        if search_focused && self.query.is_empty() {
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

        ctx.request_repaint_after(Duration::from_millis(250));

        self.top_bar(ctx);
        egui::TopBottomPanel::bottom("status").show(ctx, |ui| self.status_bar(ui));
        self.sidebar(ctx);
        if self.prefs.show_preview {
            self.refresh_preview(ctx);
            egui::SidePanel::right("preview")
                .resizable(true)
                .default_width(380.0)
                .min_width(240.0)
                .show(ctx, |ui| self.preview_panel(ui));
        }
        egui::CentralPanel::default().show(ctx, |ui| self.results_table(ui));
    }
}

impl App {
    /// Floating search bar with in-bar toggles and the options menu.
    fn top_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("search").show(ctx, |ui| {
            ui.add_space(12.0);
            let dim = self.fg_dim();
            let hint = if self.content_mode {
                "Search file contents (regex)…"
            } else if self.regex_mode {
                "Search filenames (regex)…"
            } else {
                "Search files, folders, or contents…"
            };
            egui::Frame::new()
                .fill(if self.dark { WIDGET_DARK } else { WIDGET_LIGHT })
                .corner_radius(egui::CornerRadius::same(8))
                .inner_margin(egui::Margin::symmetric(12, 10))
                .stroke(egui::Stroke::new(1.0_f32, ACCENT.gamma_multiply(0.55)))
                .shadow(egui::Shadow {
                    offset: [0, 3],
                    blur: 10,
                    spread: 0,
                    color: ACCENT.gamma_multiply(0.25),
                })
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("🔍").size(19.0));
                        let edit = egui::TextEdit::singleline(&mut self.query)
                            .id(search_id())
                            .hint_text(egui::RichText::new(hint).color(dim))
                            .font(egui::TextStyle::Heading)
                            .desired_width(f32::INFINITY)
                            .margin(egui::vec2(6.0, 6.0));
                        let resp = ui.add(edit);
                        if resp.changed() {
                            self.last_edit = Instant::now();
                            self.history_idx = None;
                        }
                        if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                            let q = self.query.clone();
                            self.prefs.commit_query(&q);
                            self.history_idx = None;
                            if let Some(first) = self.results.first() {
                                App::open(&first.path);
                            }
                        }
                        // In-bar toggles: regex, case, history, options.
                        if ui
                            .selectable_label(self.regex_mode, ".*")
                            .on_hover_text("Regex mode")
                            .clicked()
                        {
                            self.regex_mode = !self.regex_mode;
                            self.last_edit = Instant::now();
                        }
                        if ui
                            .selectable_label(self.case_sensitive, "Aa")
                            .on_hover_text("Case-sensitive")
                            .clicked()
                        {
                            self.case_sensitive = !self.case_sensitive;
                            self.last_edit = Instant::now();
                        }
                        let history = self.prefs.history.clone();
                        ui.menu_button("🕘", |ui| {
                            ui.set_min_width(240.0);
                            ui.label(egui::RichText::new("Recent searches").small().color(dim));
                            ui.separator();
                            if history.is_empty() {
                                ui.weak("No recent searches yet");
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
                        ui.menu_button("≡", |ui| {
                            ui.set_min_width(220.0);
                            if ui.checkbox(&mut self.content_mode, "Match contents")
                                .on_hover_text("Search inside files (regex)")
                                .changed()
                            {
                                self.last_edit = Instant::now();
                            }
                            if ui.checkbox(&mut self.hidden, "Hidden files").changed() {
                                self.last_edit = Instant::now();
                            }
                            if ui.checkbox(&mut self.full_path, "Full path match").changed() {
                                self.last_edit = Instant::now();
                            }
                            ui.separator();
                            if ui.checkbox(&mut self.prefs.show_preview, "Preview pane").changed() {
                                self.prefs.save();
                            }
                            if ui.button(if self.dark { "☀️ Light theme" } else { "🌙 Dark theme" }).clicked() {
                                self.dark = !self.dark;
                                self.prefs.dark = Some(self.dark);
                                self.prefs.save();
                                self.apply_style(ctx);
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
                });
            ui.add_space(10.0);
        });
    }

    fn sidebar(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("sidebar")
            .resizable(true)
            .default_width(190.0)
            .min_width(140.0)
            .show(ctx, |ui| {
                ui.visuals_mut().panel_fill = if self.dark { SIDEBAR_BG } else { WIDGET_LIGHT };
                ui.add_space(12.0);
                ui.label(egui::RichText::new("Categories").small().color(self.fg_dim()));
                ui.add_space(6.0);
                for (label, icon, cat) in CATEGORIES {
                    let selected = self.category == *cat;
                    let resp = ui.selectable_label(selected, format!("{icon}  {label}"));
                    if resp.clicked() {
                        if selected {
                            self.category = Category::All;
                        } else {
                            self.category = *cat;
                        }
                        self.send_query();
                    }
                }
                ui.add_space(12.0);
                ui.separator();
                ui.add_space(6.0);
                ui.label(egui::RichText::new("Tips").small().color(self.fg_dim()));
                ui.add_space(4.0);
                for tip in [
                    "*.pdf  — glob patterns",
                    "a b    — both terms",
                    "!draft — exclude",
                    ".*     — regex mode",
                    "↑ in empty search: history",
                ] {
                    ui.label(egui::RichText::new(tip).small().color(self.fg_dim()));
                }
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
            self.empty_state(ui);
            return;
        }

        let ctx = ui.ctx().clone();
        let mut table = TableBuilder::new(ui)
            .striped(true)
            .resizable(true)
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::remainder().clip(true))
            .column(Column::exact(92.0).at_least(70.0).clip(true))
            .column(Column::exact(150.0).at_least(110.0).clip(true))
            .column(Column::exact(100.0).at_least(80.0).clip(true));
        if let Some(target) = self.scroll_to.take() {
            table = table.scroll_to_row(target, Some(egui::Align::Center));
        }

        table
            .header(30.0, |mut header| {
                header.col(|ui| {
                    if sort_button(ui, "Name", self.sort, |s| matches!(s, Sort::Name(_))).clicked() {
                        self.sort = cycle_sort(self.sort, Sort::Name(true));
                    }
                });
                header.col(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if sort_button(ui, "Size", self.sort, |s| matches!(s, Sort::Size(_))).clicked() {
                            self.sort = cycle_sort(self.sort, Sort::Size(true));
                        }
                    });
                });
                header.col(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if sort_button(ui, "Modified", self.sort, |s| matches!(s, Sort::Mtime(_))).clicked() {
                            self.sort = cycle_sort(self.sort, Sort::Mtime(true));
                        }
                    });
                });
                header.col(|_ui| {});
            })
            .body(|body| {
                let rows = self.results.len();
                let selected = self.selected;
                body.rows(44.0, rows, |mut row| {
                    let i = row.index();
                    row.set_selected(i == selected);
                    let r = &self.results[i];
                    let mut actions: Option<(bool, bool, bool)> = None;
                    // Row geometry (x-start + y-range) captured from column 1,
                    // used for whole-row hover detection in the actions column.
                    let mut row_left: Option<f32> = None;
                    let mut row_y: Option<egui::Rangef> = None;

                    row.col(|ui| {
                        let rect = ui.max_rect();
                        row_left = Some(rect.min.x);
                        row_y = Some(rect.y_range());
                        if i == selected {
                            let (bar, _) =
                                ui.allocate_exact_size(egui::vec2(3.0, 44.0), egui::Sense::hover());
                            ui.painter().rect_filled(bar, 0.0, ACCENT);
                        }
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new(icon_for(&r.path, r.is_dir)).size(17.0));
                            let (dot, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
                            ui.painter().circle_filled(dot.center(), 3.5, badge_color(&r.path, r.is_dir));
                            ui.vertical(|ui| {
                                let name = r
                                    .path
                                    .file_name()
                                    .map(|n| n.to_string_lossy().into_owned())
                                    .unwrap_or_else(|| r.path.display().to_string());
                                ui.label(egui::RichText::new(name).strong().color(self.fg()));
                                breadcrumb_ui(ui, &r.path, self.fg_dim());
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
                    row.col(|ui| {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let hovered = match (row_left, row_y) {
                                (Some(lx), Some(yr)) => {
                                    let r2 = ui.max_rect();
                                    let full = egui::Rect::from_min_max(
                                        egui::pos2(lx, yr.min),
                                        egui::pos2(r2.max.x, yr.max),
                                    );
                                    ui.rect_contains_pointer(full)
                                }
                                _ => false,
                            };
                            if hovered {
                                if ui.small_button("📂").on_hover_text("Open containing folder").clicked() {
                                    actions = Some((true, actions.is_some_and(|a| a.1), actions.is_some_and(|a| a.2)));
                                }
                                if ui.small_button("🔗").on_hover_text("Copy path").clicked() {
                                    actions = Some((actions.is_some_and(|a| a.0), true, actions.is_some_and(|a| a.2)));
                                }
                                if ui.small_button("🖥").on_hover_text("Open in terminal").clicked() {
                                    actions = Some((actions.is_some_and(|a| a.0), actions.is_some_and(|a| a.1), true));
                                }
                            }
                        });
                    });

                    let row_resp = row.response().clone();
                    if row_resp.double_clicked() {
                        App::open(&r.path);
                    }
                    if row_resp.clicked() {
                        self.selected = i;
                    }
                    match actions {
                        Some((folder, copy, term)) => {
                            if folder {
                                App::open_folder(&r.path);
                            }
                            if copy {
                                ctx.copy_text(r.path.display().to_string());
                            }
                            if term {
                                App::open_terminal(&r.path);
                            }
                        }
                        None => {}
                    }
                    row_resp.context_menu(|ui| {
                        if ui.button("Open").clicked() {
                            App::open(&r.path);
                            ui.close_menu();
                        }
                        if ui.button("Open containing folder").clicked() {
                            App::open_folder(&r.path);
                            ui.close_menu();
                        }
                        if ui.button("Open in terminal").clicked() {
                            App::open_terminal(&r.path);
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

    fn empty_state(&mut self, ui: &mut egui::Ui) {
        ui.add_space(40.0);
        ui.vertical_centered(|ui| {
            if self.status.state != State::Live && self.query.is_empty() {
                ui.spinner();
                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new(format!("Indexing… {} files", self.engine.counts().0))
                        .color(self.fg_dim()),
                );
                return;
            }
            if !self.query.is_empty() && !self.pending {
                ui.label(egui::RichText::new("🔎").size(52.0));
                ui.add_space(8.0);
                ui.label(egui::RichText::new("No results").size(22.0).strong().color(self.fg()));
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new("Try fewer terms, a different pattern, or tick “Match contents”")
                        .color(self.fg_dim()),
                );
                return;
            }
            // Idle state: search tips + recent searches.
            ui.label(egui::RichText::new("🔍").size(48.0));
            ui.add_space(8.0);
            ui.label(egui::RichText::new("Search tips").size(20.0).strong().color(self.fg()));
            ui.add_space(6.0);
            for tip in [
                "Type to search filenames instantly — “*.pdf”, “invoice 2026”, “!draft”",
                "Tick “.*” for regex, or “Match contents” to search inside files",
                "Press ↑ with an empty search box to recall previous queries",
            ] {
                ui.label(egui::RichText::new(format!("· {tip}")).color(self.fg_dim()));
            }
            if !self.prefs.history.is_empty() {
                ui.add_space(16.0);
                ui.label(egui::RichText::new("Recent searches").small().color(self.fg_dim()));
                ui.add_space(6.0);
                let history = self.prefs.history.clone();
                ui.horizontal_wrapped(|ui| {
                    for q in history.iter().take(8) {
                        if ui.button(q.clone()).clicked() {
                            self.query = q.clone();
                            self.last_edit = Instant::now();
                            self.history_idx = None;
                        }
                    }
                });
            }
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
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(icon_for(&pv.path, pv.is_dir)).size(22.0));
            ui.label(egui::RichText::new(name).strong().size(18.0).color(self.fg()));
        });
        ui.horizontal(|ui| {
            let (dot, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
            ui.painter().circle_filled(dot.center(), 3.5, badge_color(&pv.path, pv.is_dir));
            ui.label(egui::RichText::new(type_label(&pv.path, pv.is_dir)).color(self.fg_dim()));
            ui.label(egui::RichText::new("·").color(self.fg_dim()));
            ui.label(egui::RichText::new(human_size(pv.size)).color(self.fg_dim()));
            ui.label(egui::RichText::new("·").color(self.fg_dim()));
            ui.label(egui::RichText::new(human_time(pv.mtime)).color(self.fg_dim()));
        });
        ui.add_space(2.0);
        ui.label(
            egui::RichText::new(pv.path.display().to_string())
                .small()
                .monospace()
                .color(self.fg_dim()),
        );
        ui.separator();

        ui.horizontal(|ui| {
            if ui.button("Open").clicked() {
                App::open(&pv.path);
            }
            if ui.button("Copy path").clicked() {
                ui.ctx().copy_text(pv.path.display().to_string());
            }
            if ui.button("Terminal").on_hover_text("Open a terminal in this folder").clicked() {
                App::open_terminal(&pv.path);
            }
        });
        ui.add_space(8.0);

        if pv.is_dir {
            ui.add_space(16.0);
            ui.vertical_centered(|ui| {
                ui.label(egui::RichText::new("📁 Directory").size(30.0));
                ui.add_space(8.0);
                if ui.button("Open").clicked() {
                    App::open(&pv.path);
                }
            });
            return;
        }
        if let Some(tex) = &pv.image {
            ui.vertical_centered(|ui| {
                ui.add(
                    egui::Image::new(tex)
                        .max_size(egui::vec2(ui.available_width().min(320.0), 320.0))
                        .corner_radius(8),
                );
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
                    egui::RichText::new(&pv.text).monospace().color(if self.dark {
                        FG_DARK
                    } else {
                        egui::Color32::from_rgb(30, 34, 42)
                    }),
                );
            });
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            // Live-index indicator (pulses while indexing).
            let t = ui.input(|i| i.time);
            match self.status.state {
                State::Starting | State::Indexing => {
                    let a = ((t * 5.0).sin() * 0.5 + 0.5) as f32;
                    let (dot, _) = ui.allocate_exact_size(egui::vec2(9.0, 9.0), egui::Sense::hover());
                    ui.painter().circle_filled(dot.center(), 4.5, WARN_ORANGE.gamma_multiply(0.5 + 0.5 * a));
                    ui.label(egui::RichText::new(format!("Indexing… {}", self.engine.counts().0)).color(self.fg_dim()).small());
                }
                State::Live => {
                    let (dot, _) = ui.allocate_exact_size(egui::vec2(9.0, 9.0), egui::Sense::hover());
                    ui.painter().circle_filled(dot.center(), 4.5, OK_GREEN);
                    ui.label(egui::RichText::new("⚡ Live").color(self.fg_dim()).small());
                    let (files, dirs) = self.engine.counts();
                    ui.label(egui::RichText::new(format!("· {files} files · {dirs} dirs")).color(self.fg_dim()).small());
                }
            }
            if self.status.degraded {
                ui.label(egui::RichText::new("⚠ degraded (periodic rebuild)").color(WARN_ORANGE).small());
            }
            if self.status.overlay_pending > 0 {
                ui.label(egui::RichText::new(format!("· {} pending", self.status.overlay_pending)).color(self.fg_dim()).small());
            }
            match &self.status.content_index {
                ContentIndexStatus::Enabled { entries, bytes, pending } => {
                    ui.label(
                        egui::RichText::new(format!(
                            "· cache {} files · {} MiB{}",
                            entries,
                            bytes / (1024 * 1024),
                            if *pending > 0 { format!(" · {pending} pending") } else { String::new() }
                        ))
                        .color(self.fg_dim())
                        .small(),
                    );
                }
                ContentIndexStatus::Disabled => {}
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
                ui.separator();
                ui.label(
                    egui::RichText::new("↑↓ navigate · Enter open · Esc clear · Ctrl+F search")
                        .color(self.fg_dim())
                        .small(),
                );
            });
        });
    }
}

/// Clickable breadcrumb trail: `Home › Pictures › Screenshots`.
fn breadcrumb_ui(ui: &mut egui::Ui, path: &Path, dim: egui::Color32) {
    let Some(parent) = path.parent() else { return };
    let mut crumbs: Vec<(String, PathBuf)> = Vec::new();
    let mut cur = PathBuf::from("/");
    for comp in parent.components() {
        if let std::path::Component::Normal(c) = comp {
            cur.push(c);
            crumbs.push((c.to_string_lossy().into_owned(), cur.clone()));
        }
    }
    // Collapse "/home/<user>" into a single "Home" crumb.
    if crumbs.first().is_some_and(|(n, _)| n == "home") && crumbs.len() >= 2 {
        let home = crumbs[1].1.clone();
        crumbs.drain(..2);
        crumbs.insert(0, ("Home".to_string(), home));
    }
    // Keep the trail short: "… › last 3".
    let start = crumbs.len().saturating_sub(3);
    ui.horizontal(|ui| {
        if start > 0 {
            ui.label(egui::RichText::new("…").color(dim).small());
        }
        for (i, (name, dir)) in crumbs.iter().enumerate().skip(start) {
            if i > start {
                ui.label(egui::RichText::new("›").color(dim).small());
            }
            if ui
                .add(egui::Label::new(egui::RichText::new(name).small().color(dim)).sense(egui::Sense::click()))
                .on_hover_text("Open this folder")
                .clicked()
            {
                let _ = Command::new("xdg-open").arg(dir).spawn();
            }
        }
    });
}

/// Per-category accent dot for row badges.
fn badge_color(path: &Path, is_dir: bool) -> egui::Color32 {
    if is_dir {
        return OK_GREEN;
    }
    let ext = path.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase());
    match ext.as_deref() {
        Some("png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "bmp" | "tiff" | "ico") => IMG_BLUE,
        Some("zip" | "tar" | "gz" | "xz" | "bz2" | "7z" | "rar" | "zst" | "deb" | "rpm") => ARCH_ORANGE,
        Some("mp3" | "wav" | "flac" | "ogg" | "m4a" | "aac" | "opus") => AV_PURPLE,
        Some("mp4" | "mkv" | "avi" | "mov" | "webm" | "flv" | "mpg" | "mpeg" | "wmv") => AV_PURPLE,
        Some("rs" | "py" | "js" | "ts" | "go" | "c" | "cpp" | "h" | "hpp" | "java" | "rb"
        | "sh" | "toml" | "json" | "yaml" | "yml" | "html" | "css" | "sql" | "php" | "lua"
        | "zig" | "ex" | "exs" | "kt" | "swift") => CODE_CYAN,
        _ => DIM_DARK,
    }
}

fn type_label(path: &Path, is_dir: bool) -> String {
    if is_dir {
        return "Directory".to_string();
    }
    let ext = path.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase());
    match ext.as_deref() {
        Some("png" | "jpg" | "jpeg") => "Image",
        Some("gif") => "GIF image",
        Some("webp") => "WebP image",
        Some("svg") => "SVG vector",
        Some("pdf") => "PDF document",
        Some("doc" | "docx") => "Word document",
        Some("odt") => "ODT document",
        Some("xls" | "xlsx" | "csv") => "Spreadsheet",
        Some("ppt" | "pptx") => "Presentation",
        Some("zip" | "tar" | "gz" | "xz" | "bz2" | "7z" | "rar") => "Archive",
        Some("mp3" | "wav" | "flac" | "ogg" | "m4a" | "aac" | "opus") => "Audio",
        Some("mp4" | "mkv" | "avi" | "mov" | "webm") => "Video",
        Some("rs") => "Rust source",
        Some("py") => "Python source",
        Some("js" | "ts") => "JavaScript / TypeScript",
        Some("go") => "Go source",
        Some("sh") => "Shell script",
        Some("md") => "Markdown",
        Some("txt" | "log") => "Text",
        Some("json") => "JSON",
        Some("toml") => "TOML",
        _ => "File",
    }
    .to_string()
}

fn is_image_file(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref(),
        Some("png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "tiff" | "ico")
    )
}

fn find_in_path(bin: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        let cand = dir.join(bin);
        if cand.is_file() {
            return Some(cand);
        }
    }
    None
}

fn shell_quote(path: &Path) -> String {
    format!("{:?}", path.display().to_string())
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
        Some("rs" | "py" | "js" | "ts" | "go" | "c" | "cpp" | "h" | "hpp" | "java" | "rb"
        | "sh" | "toml" | "json" | "yaml" | "yml" | "html" | "css" | "sql" | "php" | "lua"
        | "zig" | "ex" | "exs" | "kt" | "swift") => "💻",
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

fn sort_button(
    ui: &mut egui::Ui,
    label: &str,
    current: Option<Sort>,
    is_active: impl Fn(Sort) -> bool,
) -> egui::Response {
    let mark = match current {
        Some(s) if is_active(s) => match s {
            Sort::Name(asc) | Sort::Size(asc) | Sort::Mtime(asc) => {
                if asc { " ▲" } else { " ▼" }
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
                if asc { ka.cmp(&kb) } else { kb.cmp(&ka) }
            });
        }
        Sort::Size(asc) => {
            results.sort_by(|a, b| if asc { a.size.cmp(&b.size) } else { b.size.cmp(&a.size) });
        }
        Sort::Mtime(asc) => {
            results.sort_by(|a, b| if asc { a.mtime.cmp(&b.mtime) } else { b.mtime.cmp(&a.mtime) });
        }
    }
}
