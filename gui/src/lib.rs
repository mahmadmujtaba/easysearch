//! `everything-gui` library — native egui frontend over the core engine.
//!
//! Exposed as a library so the combined single-binary app (`everything-linux`)
//! can drive the same UI; [`run`] takes an already-chosen [`Backend`].
//!
//! Pro-Search layout (three panes): a category sidebar, a floating search bar
//! with in-bar toggles, a virtualized results table (two-line rows, badges,
//! breadcrumbs, hover actions), and a live preview pane (thumbnail + quick
//! actions). Tokyo Night palette; Wayland-first windowing.

use eframe::egui;
use egui_extras::{Column, TableBuilder};
use everything_core::api::DEFAULT_ADDR;
use everything_core::{
    Backend, Category, ContentIndexStatus, Query, ResultRow, SearchResponse, State, Status,
};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

mod tray;

const DEBOUNCE_MS: u128 = 120;
const HISTORY_CAP: usize = 20;
/// Reverse-DNS application id. Kept in sync with the packaging assets in
/// `packaging/` (desktop entry, AppStream metainfo, Flatpak manifest) so the
/// window, the launcher entry and the icon all agree.
const APP_ID: &str = "io.github.everythinglinux.EverythingForLinux";
/// Selectable UI zoom levels (1.0 = 100%).
const ZOOM_LEVELS: &[f32] = &[1.0, 1.1, 1.25];

/// Complete colour scheme for one appearance mode.
///
/// Both modes define *every* surface explicitly, so light mode never inherits a
/// dark-tuned constant — the previous cause of the washed-out light theme.
#[derive(Clone, Copy)]
struct Theme {
    dark: bool,
    /// Window background — the deepest surface.
    bg: egui::Color32,
    /// Docked panels: menu bar, search bar, sidebar, status bar.
    panel: egui::Color32,
    /// Elevated surfaces: popups, rows, chips.
    card: egui::Color32,
    /// Hovered row/widget background.
    hover: egui::Color32,
    /// Pressed/active widget background.
    active: egui::Color32,
    /// Table zebra striping.
    stripe: egui::Color32,
    /// Borders and separators.
    stroke: egui::Color32,
    /// Primary text.
    text: egui::Color32,
    /// Secondary text: file paths, metadata.
    dim: egui::Color32,
    /// Tertiary text: hints, tips, counts.
    faint: egui::Color32,
    /// Brand / selection accent.
    accent: egui::Color32,
    good: egui::Color32,
    warn: egui::Color32,
    bad: egui::Color32,
    kind_dir: egui::Color32,
    kind_img: egui::Color32,
    kind_doc: egui::Color32,
    kind_code: egui::Color32,
    kind_arch: egui::Color32,
    kind_av: egui::Color32,
}

impl Theme {
    const DARK: Theme = Theme {
        dark: true,
        bg: egui::Color32::from_rgb(0x16, 0x16, 0x1e),
        panel: egui::Color32::from_rgb(0x1a, 0x1b, 0x26),
        card: egui::Color32::from_rgb(0x20, 0x24, 0x37),
        hover: egui::Color32::from_rgb(0x2a, 0x30, 0x47),
        active: egui::Color32::from_rgb(0x34, 0x3b, 0x59),
        stripe: egui::Color32::from_rgb(0x1d, 0x1f, 0x2c),
        stroke: egui::Color32::from_rgb(0x2c, 0x32, 0x4a),
        text: egui::Color32::from_rgb(0xc6, 0xce, 0xf0),
        dim: egui::Color32::from_rgb(0x8c, 0x95, 0xbb),
        faint: egui::Color32::from_rgb(0x5c, 0x66, 0x8e),
        accent: egui::Color32::from_rgb(0x7a, 0xa2, 0xf7),
        good: egui::Color32::from_rgb(0x9e, 0xce, 0x6a),
        warn: egui::Color32::from_rgb(0xe0, 0xaf, 0x68),
        bad: egui::Color32::from_rgb(0xf7, 0x76, 0x8e),
        kind_dir: egui::Color32::from_rgb(0x9e, 0xce, 0x6a),
        kind_img: egui::Color32::from_rgb(0x7d, 0xcf, 0xff),
        kind_doc: egui::Color32::from_rgb(0xe0, 0xaf, 0x68),
        kind_code: egui::Color32::from_rgb(0x7a, 0xa2, 0xf7),
        kind_arch: egui::Color32::from_rgb(0xff, 0x9e, 0x64),
        kind_av: egui::Color32::from_rgb(0xbb, 0x9a, 0xf7),
    };

    const LIGHT: Theme = Theme {
        dark: false,
        bg: egui::Color32::from_rgb(0xef, 0xf1, 0xf6),
        panel: egui::Color32::from_rgb(0xff, 0xff, 0xff),
        card: egui::Color32::from_rgb(0xf5, 0xf7, 0xfb),
        hover: egui::Color32::from_rgb(0xe7, 0xeb, 0xf4),
        active: egui::Color32::from_rgb(0xda, 0xe0, 0xef),
        stripe: egui::Color32::from_rgb(0xf7, 0xf9, 0xfd),
        stroke: egui::Color32::from_rgb(0xdd, 0xe2, 0xec),
        text: egui::Color32::from_rgb(0x1a, 0x1f, 0x2e),
        dim: egui::Color32::from_rgb(0x59, 0x63, 0x78),
        faint: egui::Color32::from_rgb(0x8b, 0x94, 0xa9),
        accent: egui::Color32::from_rgb(0x35, 0x68, 0xd4),
        good: egui::Color32::from_rgb(0x2f, 0x9e, 0x44),
        warn: egui::Color32::from_rgb(0xa9, 0x6a, 0x00),
        bad: egui::Color32::from_rgb(0xc7, 0x33, 0x4d),
        kind_dir: egui::Color32::from_rgb(0x2f, 0x9e, 0x44),
        kind_img: egui::Color32::from_rgb(0x0b, 0x72, 0x85),
        kind_doc: egui::Color32::from_rgb(0xa9, 0x6a, 0x00),
        kind_code: egui::Color32::from_rgb(0x35, 0x68, 0xd4),
        kind_arch: egui::Color32::from_rgb(0xbc, 0x4c, 0x1d),
        kind_av: egui::Color32::from_rgb(0x7a, 0x3f, 0xc9),
    };

    fn of(dark: bool) -> Theme {
        if dark { Self::DARK } else { Self::LIGHT }
    }

    /// Soft accent used behind selected rows and labels.
    fn accent_soft(&self) -> egui::Color32 {
        self.accent
            .gamma_multiply(if self.dark { 0.26 } else { 0.15 })
    }

    /// Tinted fill for a type chip, derived from the chip's accent colour.
    fn chip_fill(&self, color: egui::Color32) -> egui::Color32 {
        color.gamma_multiply(if self.dark { 0.20 } else { 0.13 })
    }

    fn shadow(&self, blur: u8, y: i8) -> egui::Shadow {
        egui::Shadow {
            offset: [0, y],
            blur,
            spread: 0,
            color: egui::Color32::from_black_alpha(if self.dark { 130 } else { 26 }),
        }
    }
}

/// Font families to try for the UI (first one found on the system wins).
const UI_FONT_PREFERENCE: &[&str] = &[
    "Inter",
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

const RECENT_AGE_SECS: i64 = 7 * 24 * 3600;
const LARGE_MIN_BYTES: u64 = 1024 * 1024 * 1024; // 1 GiB

const CATEGORIES: &[(&str, Category)] = &[
    ("All files", Category::All),
    (
        "Recent",
        Category::Recent {
            max_age_secs: RECENT_AGE_SECS,
        },
    ),
    ("Images", Category::Images),
    ("Documents", Category::Docs),
    ("Code", Category::Code),
    ("Archives", Category::Archives),
    ("Audio", Category::Audio),
    ("Video", Category::Video),
    (
        "Large files",
        Category::Large {
            min_bytes: LARGE_MIN_BYTES,
        },
    ),
];

/// Accent colour for a sidebar category.
fn category_color(t: &Theme, cat: &Category) -> egui::Color32 {
    match cat {
        Category::All => t.accent,
        Category::Recent { .. } => t.good,
        Category::Images => t.kind_img,
        Category::Docs => t.kind_doc,
        Category::Code => t.kind_code,
        Category::Archives => t.kind_arch,
        Category::Audio | Category::Video => t.kind_av,
        Category::Large { .. } => t.warn,
    }
}

enum UiMsg {
    /// A search for one tab. `ui_query` is the raw search-box text, echoed back
    /// so a stale response (the query changed since) can be dropped.
    Search {
        tab: usize,
        ui_query: String,
        query: Query,
    },
}

enum OutMsg {
    Done {
        tab: usize,
        ui_query: String,
        result: Result<SearchResponse, String>,
    },
}

/// Per-tab search state.
///
/// The **active** tab's fields live directly on [`App`] (so all the UI code can
/// keep reading `self.query`, `self.results`, …); the other tabs are snapshots
/// held in `App::tabs` and swapped in/out when switching.
#[derive(Clone)]
struct TabState {
    query: String,
    regex_mode: bool,
    content_mode: bool,
    case_sensitive: bool,
    hidden: bool,
    full_path: bool,
    category: Category,
    /// Location filter: only paths under this directory (`None` = everywhere).
    under: Option<String>,
    results: Vec<ResultRow>,
    truncated: bool,
    error: Option<String>,
    elapsed_ms: u64,
    selected: usize,
    sort: Option<Sort>,
    last_sent: String,
    pending: bool,
}

impl Default for TabState {
    fn default() -> Self {
        TabState {
            query: String::new(),
            regex_mode: false,
            content_mode: false,
            case_sensitive: false,
            hidden: false,
            full_path: false,
            category: Category::All,
            under: None,
            results: Vec::new(),
            truncated: false,
            error: None,
            elapsed_ms: 0,
            selected: 0,
            sort: None,
            last_sent: String::new(),
            pending: false,
        }
    }
}

impl TabState {
    fn from_prefs(p: &TabPrefs) -> TabState {
        TabState {
            query: p.query.clone(),
            regex_mode: p.regex_mode,
            content_mode: p.content_mode,
            case_sensitive: p.case_sensitive,
            hidden: p.hidden,
            full_path: p.full_path,
            category: category_at(p.category_index),
            under: p.under.clone(),
            ..TabState::default()
        }
    }

    fn to_prefs(&self) -> TabPrefs {
        TabPrefs {
            query: self.query.clone(),
            regex_mode: self.regex_mode,
            content_mode: self.content_mode,
            case_sensitive: self.case_sensitive,
            hidden: self.hidden,
            full_path: self.full_path,
            category_index: category_index(&self.category),
            under: self.under.clone(),
        }
    }
}

/// Persisted (query-only) form of a tab; results are not stored.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct TabPrefs {
    query: String,
    regex_mode: bool,
    content_mode: bool,
    case_sensitive: bool,
    hidden: bool,
    full_path: bool,
    /// Index into [`CATEGORIES`] — keeps `Category` out of the persisted format.
    category_index: usize,
    /// Location filter (a directory path), persisted per tab.
    under: Option<String>,
}

fn category_index(cat: &Category) -> usize {
    CATEGORIES.iter().position(|(_, c)| c == cat).unwrap_or(0)
}

fn category_at(i: usize) -> Category {
    CATEGORIES.get(i).map(|(_, c)| *c).unwrap_or(Category::All)
}

/// Tab label: the query, or a placeholder for an empty search.
fn tab_title(tab: &TabState) -> String {
    let q = tab.query.trim();
    if q.is_empty() {
        return "New search".to_string();
    }
    let mut s: String = q.chars().take(22).collect();
    if q.chars().count() > 22 {
        s.push('…');
    }
    s
}

/// A request to recompute the sidebar's per-category counts. The worker fills in
/// `category` per row and forces `limit`; everything else comes from the tab.
struct CountRequest {
    base: Query,
    /// Identity of the query these counts describe; stale replies are dropped.
    key: String,
}

/// Per-category counts for one query, in [`CATEGORIES`] order.
struct Counts {
    key: String,
    per_category: Vec<u64>,
}

/// Quick "search only here" locations for the sidebar.
fn locations() -> Vec<(&'static str, PathBuf)> {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return Vec::new();
    };
    let mut out = vec![("Home", home.clone())];
    for (label, sub) in [
        ("Desktop", "Desktop"),
        ("Documents", "Documents"),
        ("Downloads", "Downloads"),
        ("Pictures", "Pictures"),
        ("Music", "Music"),
        ("Videos", "Videos"),
        ("Workspace", "Workspace"),
        ("Projects", "Projects"),
    ] {
        let path = home.join(sub);
        if path.is_dir() {
            out.push((label, path));
        }
    }
    out
}

/// Short label for a location path (its last component).
fn location_label(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string())
}

// --- system theme ---------------------------------------------------------

/// Files whose modification indicates the desktop theme changed. KDE, GTK and
/// XFCE rewrite these; GNOME keeps its settings in the `dconf` database.
fn theme_watch_paths() -> Vec<PathBuf> {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return Vec::new();
    };
    [
        ".config/kdeglobals",
        ".config/gtk-3.0/settings.ini",
        ".config/gtk-4.0/settings.ini",
        ".config/dconf/user",
        ".config/xfce4/xfconf/xfce-perchannel-xml/xsettings.xml",
    ]
    .iter()
    .map(|p| home.join(p))
    .collect()
}

/// Cheap hash of the watched theme files (a `stat` per file, no reads).
fn theme_fingerprint() -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for path in theme_watch_paths() {
        match std::fs::metadata(&path) {
            Ok(md) => {
                let mtime = md
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_nanos())
                    .unwrap_or(0);
                mtime.hash(&mut hasher);
                md.len().hash(&mut hasher);
            }
            Err(_) => 0u64.hash(&mut hasher),
        }
    }
    hasher.finish()
}

/// Is the desktop configured for a dark scheme?
///
/// KDE's `kdeglobals` is the authoritative source on Plasma, so it is consulted
/// first; then the XDG portal (via `dark-light`), which covers GNOME and others;
/// then GTK's settings; and finally the app defaults to dark.
fn detect_system_dark() -> bool {
    if let Some(dark) = kde_globals_is_dark() {
        return dark;
    }
    match dark_light::detect() {
        dark_light::Mode::Dark => true,
        dark_light::Mode::Light => false,
        dark_light::Mode::Default => gtk_settings_is_dark().unwrap_or(true),
    }
}

/// Decide from KDE's window background colour (`~/.config/kdeglobals`).
fn kde_globals_is_dark() -> Option<bool> {
    let home = std::env::var_os("HOME")?;
    let text = std::fs::read_to_string(PathBuf::from(home).join(".config/kdeglobals")).ok()?;
    kde_globals_dark_from(&text)
}

/// Parse the `[Colors:Window] BackgroundNormal` value out of a kdeglobals file.
fn kde_globals_dark_from(text: &str) -> Option<bool> {
    let mut in_window_section = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_window_section = line == "[Colors:Window]";
            continue;
        }
        if in_window_section && let Some(value) = line.strip_prefix("BackgroundNormal=") {
            let rgb: Vec<f32> = value
                .split(',')
                .filter_map(|c| c.trim().parse::<f32>().ok())
                .collect();
            if rgb.len() >= 3 {
                // Rec. 601 luma, normalised to 0..=1.
                let luma = (0.299 * rgb[0] + 0.587 * rgb[1] + 0.114 * rgb[2]) / 255.0;
                return Some(luma < 0.5);
            }
        }
    }
    None
}

/// Decide from GTK's settings (`gtk-application-prefer-dark-theme` / theme name).
fn gtk_settings_is_dark() -> Option<bool> {
    let home = std::env::var_os("HOME")?;
    for rel in [
        ".config/gtk-4.0/settings.ini",
        ".config/gtk-3.0/settings.ini",
    ] {
        let Ok(text) = std::fs::read_to_string(PathBuf::from(&home).join(rel)) else {
            continue;
        };
        if let Some(dark) = gtk_settings_dark_from(&text) {
            return Some(dark);
        }
    }
    None
}

/// Parse a GTK `settings.ini` for a dark preference or a “dark” theme name.
fn gtk_settings_dark_from(text: &str) -> Option<bool> {
    for line in text.lines() {
        let line = line.trim();
        if let Some(v) = line.strip_prefix("gtk-application-prefer-dark-theme=") {
            let v = v.trim();
            return Some(v == "1" || v.eq_ignore_ascii_case("true"));
        }
        if let Some(v) = line.strip_prefix("gtk-theme-name=") {
            return Some(v.to_ascii_lowercase().contains("dark"));
        }
    }
    None
}

/// Run the GUI against an already-chosen search backend (blocks until exit).
pub fn run(backend: Arc<Backend>) -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Everything for Linux")
            // Must match the installed desktop entry / icon name so Wayland
            // compositors associate the window with it (and show the icon).
            .with_app_id(APP_ID)
            .with_inner_size([1240.0, 760.0])
            .with_min_inner_size([640.0, 400.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Everything for Linux",
        options,
        Box::new(move |cc| Ok(Box::new(App::new(cc, backend)))),
    )
}

/// Choose the search backend for the standalone `everything-gui` binary.
///
/// `--daemon <addr>` (or `EVERYTHING_DAEMON=<addr>`) uses that daemon;
/// otherwise a daemon already listening on [`DEFAULT_ADDR`] is used when
/// reachable; otherwise the engine runs in-process, so the GUI always works.
pub fn select_backend() -> Arc<Backend> {
    if let Some(addr) = daemon_from_env_or_args() {
        eprintln!("everything-gui: using daemon at {addr}");
        return Arc::new(Backend::remote(addr));
    }
    if everything_core::remote::probe(DEFAULT_ADDR, Duration::from_millis(300)) {
        eprintln!("everything-gui: using daemon at {DEFAULT_ADDR}");
        return Arc::new(Backend::remote(DEFAULT_ADDR));
    }
    eprintln!("everything-gui: no daemon on {DEFAULT_ADDR} — using the in-process engine");
    Arc::new(Backend::local(everything_core::Config::load()))
}

fn daemon_from_env_or_args() -> Option<String> {
    if let Ok(addr) = std::env::var("EVERYTHING_DAEMON") {
        if !addr.trim().is_empty() {
            return Some(addr);
        }
    }
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if let Some(rest) = arg.strip_prefix("--daemon=") {
            return Some(rest.to_string());
        }
        if arg == "--daemon" {
            return args.next();
        }
    }
    None
}

/// Persistent GUI preferences (`~/.config/everything-linux/gui.json`).
#[derive(Serialize, Deserialize)]
#[serde(default)]
struct GuiPrefs {
    /// None = follow the system theme.
    dark: Option<bool>,
    /// Preview pane on by default (it can be turned off in Settings/View).
    show_preview: bool,
    /// X button hides to the tray instead of quitting (opt-in).
    close_to_tray: bool,
    /// Most recent first.
    history: Vec<String>,
    /// UI zoom factor (1.0 = 100%).
    zoom: f32,
    /// Include folders in search results.
    include_dirs: bool,
    /// Open search tabs (queries only) restored on startup.
    tabs: Vec<TabPrefs>,
    /// Index of the tab that was active when the app was last closed.
    active_tab: usize,
    /// Whether the sidebar's TIPS cheat-sheet is expanded.
    sidebar_tips: bool,
}

impl Default for GuiPrefs {
    fn default() -> Self {
        GuiPrefs {
            dark: None,
            show_preview: true,
            close_to_tray: false,
            history: Vec::new(),
            zoom: 1.0,
            include_dirs: true,
            tabs: Vec::new(),
            active_tab: 0,
            sidebar_tips: true,
        }
    }
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
        let mut p: GuiPrefs = std::fs::read_to_string(Self::path())
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        // Snap a missing or invalid persisted zoom to the nearest level.
        p.zoom = if p.zoom.is_finite() {
            *ZOOM_LEVELS
                .iter()
                .min_by(|a, b| (p.zoom - **a).abs().total_cmp(&(p.zoom - **b).abs()))
                .unwrap_or(&1.0)
        } else {
            1.0
        };
        p
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
    engine: Arc<Backend>,
    prefs: GuiPrefs,
    query: String,
    regex_mode: bool,
    content_mode: bool,
    case_sensitive: bool,
    hidden: bool,
    full_path: bool,
    category: Category,
    /// Location filter: only paths under this directory (`None` = everywhere).
    under: Option<String>,
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
    tray_rx: Option<mpsc::Receiver<tray::TrayMsg>>,
    // Held for its lifetime: dropping the handle unregisters the tray item.
    #[allow(dead_code)]
    tray_handle: Option<ksni::blocking::Handle<tray::AppTray>>,
    tray_quit: bool,
    /// Our own view of window visibility (egui 0.31 exposes no readback).
    window_visible: bool,
    /// Recent searches shared with the tray menu.
    history_shared: Arc<Mutex<Vec<String>>>,
    show_about: bool,
    show_settings: bool,
    show_shortcuts: bool,
    /// Snapshot state of every tab; the active one is mirrored in the fields
    /// above and refreshed via [`App::snapshot`] before a switch or a save.
    tabs: Vec<TabState>,
    active_tab: usize,
    /// Set when a tab's state changed; persisted by a throttled background save.
    dirty: bool,
    last_save: Instant,
    /// Per-category result counts for the sidebar facets (see `CountRequest`).
    counts: Vec<u64>,
    /// Query key the current `counts` were computed for.
    counts_key: String,
    counts_tx: mpsc::Sender<CountRequest>,
    counts_rx: mpsc::Receiver<Counts>,
    /// Throttle so typing does not trigger a facet recount per keystroke.
    counts_at: Instant,
    /// Fingerprint of the desktop theme files, to follow system theme changes.
    theme_fp: u64,
    theme_at: Instant,
    /// When the theme was last re-detected (slow safety net).
    theme_detect_at: Instant,
    /// Human label for the search backend ("in-process" or "daemon HOST:PORT").
    backend_label: String,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>, backend: Arc<Backend>) -> App {
        let (query_tx, query_rx) = mpsc::channel::<UiMsg>();
        let (result_tx, result_rx) = mpsc::channel::<OutMsg>();
        let backend_worker = Arc::clone(&backend);

        std::thread::Builder::new()
            .name("search".into())
            .spawn(move || {
                while let Ok(UiMsg::Search {
                    tab,
                    ui_query,
                    query,
                }) = query_rx.recv()
                {
                    let result = backend_worker.search(&query);
                    let _ = result_tx.send(OutMsg::Done {
                        tab,
                        ui_query,
                        result,
                    });
                }
            })
            .expect("failed to spawn search thread");

        let prefs = GuiPrefs::load();
        let dark = match prefs.dark {
            Some(d) => d,
            None => detect_system_dark(),
        };
        let (ui_font, mono_font) = load_system_fonts();
        let status_snapshot = backend.status_snapshot();
        let backend_label = backend.label();
        let history_shared = Arc::new(Mutex::new(prefs.history.clone()));
        let (tray_rx, tray_handle) =
            match tray::spawn_tray("Everything for Linux", Arc::clone(&history_shared)) {
                Ok((rx, handle)) => (Some(rx), Some(handle)),
                Err(e) => {
                    eprintln!("system tray unavailable: {e}");
                    (None, None)
                }
            };

        // Restore the tabs that were open when the app last closed (at least one).
        let tab_prefs = if prefs.tabs.is_empty() {
            vec![TabPrefs::default()]
        } else {
            prefs.tabs.clone()
        };
        let active_tab = prefs.active_tab.min(tab_prefs.len() - 1);
        let tabs: Vec<TabState> = tab_prefs.iter().map(TabState::from_prefs).collect();
        let start = tabs[active_tab].clone();

        // Background worker: recompute the sidebar's per-category counts when
        // asked (kept off the UI thread; coalesces bursts).
        let (counts_tx, counts_rx) = mpsc::channel::<Counts>();
        let (count_req_tx, count_req_rx) = mpsc::channel::<CountRequest>();
        {
            let backend = Arc::clone(&backend);
            std::thread::Builder::new()
                .name("facets".into())
                .spawn(move || {
                    while let Ok(mut req) = count_req_rx.recv() {
                        // Coalesce: only the newest request matters.
                        while let Ok(newer) = count_req_rx.try_recv() {
                            req = newer;
                        }
                        let mut per_category = Vec::with_capacity(CATEGORIES.len());
                        for (_, cat) in CATEGORIES {
                            let mut q = req.base.clone();
                            q.category = *cat;
                            q.limit = 1;
                            per_category.push(backend.count(&q).unwrap_or(0));
                        }
                        if counts_tx
                            .send(Counts {
                                key: req.key,
                                per_category,
                            })
                            .is_err()
                        {
                            break;
                        }
                    }
                })
                .expect("failed to spawn facet thread");
        }

        let mut app = App {
            engine: backend,
            prefs,
            query: start.query,
            regex_mode: start.regex_mode,
            content_mode: start.content_mode,
            case_sensitive: start.case_sensitive,
            hidden: start.hidden,
            full_path: start.full_path,
            category: start.category,
            under: start.under,
            limit: 500,
            results: start.results,
            truncated: start.truncated,
            error: start.error,
            elapsed_ms: start.elapsed_ms,
            status: status_snapshot,
            last_sent: String::new(), // force an initial search for the active tab
            pending: false,
            last_edit: Instant::now(),
            query_tx,
            result_rx,
            selected: start.selected,
            scroll_to: None,
            sort: start.sort,
            preview: None,
            dark,
            history_idx: None,
            search_was_focused: false,
            ui_font,
            mono_font,
            tray_rx,
            tray_handle,
            tray_quit: false,
            window_visible: true,
            history_shared,
            show_about: false,
            show_settings: false,
            show_shortcuts: false,
            tabs,
            active_tab,
            dirty: false,
            last_save: Instant::now(),
            backend_label,
            counts: Vec::new(),
            counts_key: "\u{0}counts-pending".to_string(),
            counts_tx: count_req_tx,
            counts_rx,
            counts_at: Instant::now(),
            theme_fp: theme_fingerprint(),
            theme_at: Instant::now(),
            theme_detect_at: Instant::now(),
        };
        app.apply_style(&cc.egui_ctx);
        cc.egui_ctx.set_zoom_factor(app.prefs.zoom);
        app.send_query();
        app
    }

    fn apply_style(&self, ctx: &egui::Context) {
        let t = self.theme();
        let mut v = if t.dark {
            egui::Visuals::dark()
        } else {
            egui::Visuals::light()
        };
        v.override_text_color = Some(t.text);
        v.panel_fill = t.panel;
        v.window_fill = t.panel;
        v.extreme_bg_color = t.bg;
        v.faint_bg_color = t.stripe;
        v.window_stroke = egui::Stroke::new(1.0_f32, t.stroke);
        v.window_corner_radius = egui::CornerRadius::same(12);
        v.menu_corner_radius = egui::CornerRadius::same(10);
        v.window_shadow = t.shadow(28, 10);
        v.popup_shadow = t.shadow(18, 6);
        v.selection.bg_fill = t.accent_soft();
        v.selection.stroke = egui::Stroke::new(1.0_f32, t.accent);
        v.hyperlink_color = t.accent;
        v.warn_fg_color = t.warn;
        v.error_fg_color = t.bad;

        let radius = egui::CornerRadius::same(7);
        let set = |w: &mut egui::style::WidgetVisuals,
                   bg: egui::Color32,
                   weak: egui::Color32,
                   fg: egui::Color32,
                   border: egui::Color32| {
            w.bg_fill = bg;
            w.weak_bg_fill = weak;
            w.bg_stroke = egui::Stroke::new(1.0_f32, border);
            w.fg_stroke = egui::Stroke::new(1.0_f32, fg);
            w.corner_radius = radius;
            w.expansion = 0.0;
        };
        set(
            &mut v.widgets.noninteractive,
            t.panel,
            t.card,
            t.text,
            t.stroke,
        );
        set(&mut v.widgets.inactive, t.card, t.card, t.text, t.stroke);
        set(&mut v.widgets.hovered, t.hover, t.hover, t.text, t.accent);
        set(&mut v.widgets.active, t.active, t.active, t.text, t.accent);
        set(&mut v.widgets.open, t.card, t.card, t.text, t.stroke);
        ctx.set_visuals(v);

        // System font first; the bundled egui fonts remain as glyph fallbacks
        // (emoji, CJK, rare symbols the system font may lack).
        let mut fonts = egui::FontDefinitions::default();
        if let Some(bytes) = &self.ui_font {
            fonts.font_data.insert(
                "ui".to_owned(),
                Arc::new(egui::FontData::from_owned(bytes.clone())),
            );
            fonts
                .families
                .entry(egui::FontFamily::Proportional)
                .or_default()
                .insert(0, "ui".to_owned());
        }
        if let Some(bytes) = &self.mono_font {
            fonts.font_data.insert(
                "mono".to_owned(),
                Arc::new(egui::FontData::from_owned(bytes.clone())),
            );
            fonts
                .families
                .entry(egui::FontFamily::Monospace)
                .or_default()
                .insert(0, "mono".to_owned());
        }
        ctx.set_fonts(fonts);

        let mut style = (*ctx.style()).clone();
        style.spacing.item_spacing = egui::vec2(8.0, 8.0);
        style.spacing.button_padding = egui::vec2(10.0, 6.0);
        style.spacing.interact_size = egui::vec2(28.0, 28.0);
        style.spacing.menu_margin = egui::Margin::same(6);
        style.spacing.window_margin = egui::Margin::same(10);
        style.spacing.scroll = egui::style::ScrollStyle::solid();
        style.text_styles.insert(
            egui::TextStyle::Heading,
            egui::FontId::new(19.0, egui::FontFamily::Proportional),
        );
        style.text_styles.insert(
            egui::TextStyle::Body,
            egui::FontId::new(14.5, egui::FontFamily::Proportional),
        );
        style.text_styles.insert(
            egui::TextStyle::Button,
            egui::FontId::new(13.5, egui::FontFamily::Proportional),
        );
        style.text_styles.insert(
            egui::TextStyle::Small,
            egui::FontId::new(12.0, egui::FontFamily::Proportional),
        );
        style.text_styles.insert(
            egui::TextStyle::Monospace,
            egui::FontId::new(13.0, egui::FontFamily::Monospace),
        );
        ctx.set_style(style);
    }

    /// The active colour scheme.
    fn theme(&self) -> Theme {
        Theme::of(self.dark)
    }

    fn fg_dim(&self) -> egui::Color32 {
        self.theme().dim
    }

    /// Copy the live search fields into the active tab's snapshot.
    fn snapshot(&mut self) {
        let i = self.active_tab;
        if i >= self.tabs.len() {
            return;
        }
        self.tabs[i] = TabState {
            query: self.query.clone(),
            regex_mode: self.regex_mode,
            content_mode: self.content_mode,
            case_sensitive: self.case_sensitive,
            hidden: self.hidden,
            full_path: self.full_path,
            category: self.category,
            under: self.under.clone(),
            results: self.results.clone(),
            truncated: self.truncated,
            error: self.error.clone(),
            elapsed_ms: self.elapsed_ms,
            selected: self.selected,
            sort: self.sort,
            last_sent: self.last_sent.clone(),
            pending: self.pending,
        };
    }

    /// Load a tab snapshot into the live search fields.
    fn restore(&mut self, i: usize) {
        let t = self.tabs[i].clone();
        self.query = t.query;
        self.regex_mode = t.regex_mode;
        self.content_mode = t.content_mode;
        self.case_sensitive = t.case_sensitive;
        self.hidden = t.hidden;
        self.full_path = t.full_path;
        self.category = t.category;
        self.under = t.under;
        self.results = t.results;
        self.truncated = t.truncated;
        self.error = t.error;
        self.elapsed_ms = t.elapsed_ms;
        self.selected = t.selected;
        self.sort = t.sort;
        self.last_sent = t.last_sent;
        self.pending = t.pending;
        // Transient view state is rebuilt for the newly shown tab.
        self.preview = None;
        self.scroll_to = None;
        self.history_idx = None;
        self.last_edit = Instant::now();
    }

    fn switch_tab(&mut self, i: usize) {
        if i >= self.tabs.len() || i == self.active_tab {
            return;
        }
        self.snapshot();
        self.active_tab = i;
        self.restore(i);
        self.save_prefs();
    }

    fn new_tab(&mut self) {
        self.snapshot();
        self.tabs.push(TabState::default());
        self.active_tab = self.tabs.len() - 1;
        self.restore(self.active_tab);
        self.save_prefs();
        self.send_query();
    }

    fn close_tab(&mut self, i: usize) {
        if self.tabs.len() <= 1 || i >= self.tabs.len() {
            return;
        }
        // Keep what the closing tab was searching for in the history.
        let q = self.tabs[i].query.trim().to_string();
        if !q.is_empty() {
            self.prefs.commit_query(&q);
        }
        if i == self.active_tab {
            self.tabs.remove(i);
            self.active_tab = i.min(self.tabs.len() - 1);
            self.restore(self.active_tab);
        } else {
            self.tabs.remove(i);
            if i < self.active_tab {
                self.active_tab -= 1;
            }
        }
        self.sync_history();
        self.save_prefs();
    }

    /// Persist tabs + shared preferences (captures the live tab first).
    fn save_prefs(&mut self) {
        self.snapshot();
        self.prefs.tabs = self.tabs.iter().map(TabState::to_prefs).collect();
        self.prefs.active_tab = self.active_tab;
        self.prefs.save();
        self.dirty = false;
        self.last_save = Instant::now();
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
            include_dirs: self.prefs.include_dirs,
            under: self.under.clone(),
            limit: self.limit,
        };
        let _ = self.query_tx.send(UiMsg::Search {
            tab: self.active_tab,
            ui_query: self.query.clone(),
            query: q,
        });
        self.last_sent = self.query.clone();
        self.pending = true;
        self.selected = 0;
        self.scroll_to = None;
        self.dirty = true;
    }

    /// Restrict (or un-restrict) the search to a directory and re-run it.
    fn set_under(&mut self, under: Option<String>) {
        if self.under == under {
            return;
        }
        self.under = under;
        self.send_query();
    }

    /// Cycle the sort for a column and re-sort the current results in place
    /// (a header click must work without waiting for the next query response).
    fn toggle_sort(&mut self, prefer: Sort) {
        self.sort = cycle_sort(self.sort, prefer);
        if let Some(s) = self.sort {
            sort_results(&mut self.results, s);
        }
    }

    /// Mirror the persisted history into the shared tray snapshot.
    fn sync_history(&mut self) {
        if let Ok(mut h) = self.history_shared.lock() {
            h.clone_from(&self.prefs.history);
        }
    }

    /// Run a query (from the tray's recent-searches menu).
    fn run_query(&mut self, q: &str) {
        let q = q.trim().to_string();
        if q.is_empty() {
            return;
        }
        self.query = q;
        self.last_edit = Instant::now();
        self.history_idx = None;
        self.prefs.commit_query(&self.query);
        self.sync_history();
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
        let dir = if path.is_dir() {
            path
        } else {
            path.parent().unwrap_or(path)
        };
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
            let Some(full) = find_in_path(bin) else {
                continue;
            };
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
                        image = Some(ctx.load_texture(
                            "preview-thumb",
                            color,
                            egui::TextureOptions::LINEAR,
                        ));
                    }
                } else {
                    text =
                        String::from_utf8_lossy(&bytes[..bytes.len().min(64 * 1024)]).into_owned();
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

/// Full-width sidebar row: colour dot, label, optional right-aligned value, and
/// an accent bar when selected. Painted manually so the text is centred
/// regardless of which system font is in use.
fn nav_item(
    ui: &mut egui::Ui,
    t: &Theme,
    dot: egui::Color32,
    label: &str,
    right: Option<&str>,
    selected: bool,
) -> egui::Response {
    let (rect, resp) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 28.0), egui::Sense::click());
    let radius = egui::CornerRadius::same(7);
    if selected {
        ui.painter().rect_filled(rect, radius, t.accent_soft());
        let bar = egui::Rect::from_min_size(
            egui::pos2(rect.min.x + 1.0, rect.center().y - 7.0),
            egui::vec2(3.0, 14.0),
        );
        ui.painter()
            .rect_filled(bar, egui::CornerRadius::same(2), t.accent);
    } else if resp.hovered() {
        ui.painter().rect_filled(rect, radius, t.hover);
    }

    let dot_x = rect.min.x + 15.0;
    ui.painter()
        .circle_filled(egui::pos2(dot_x, rect.center().y), 3.5, dot);

    // Right-aligned value (e.g. a facet count) reserves its own space.
    let value_font = egui::FontId::new(11.5, egui::FontFamily::Proportional);
    let value_w = right
        .map(|v| {
            ui.painter()
                .layout_no_wrap(v.to_string(), value_font.clone(), t.faint)
                .size()
                .x
        })
        .unwrap_or(0.0);
    let label_left = dot_x + 12.0;
    let label_right = rect.max.x - 10.0 - if right.is_some() { value_w + 8.0 } else { 0.0 };

    let font = egui::FontId::new(13.0, egui::FontFamily::Proportional);
    let available = (label_right - label_left).max(0.0);
    let mut text = label.to_string();
    let mut galley = ui
        .painter()
        .layout_no_wrap(text.clone(), font.clone(), t.text);
    while galley.size().x > available && !text.is_empty() {
        text.pop();
        galley = ui
            .painter()
            .layout_no_wrap(format!("{text}…"), font.clone(), t.text);
    }
    ui.painter().galley(
        egui::pos2(label_left, rect.center().y - galley.size().y * 0.5),
        galley,
        t.text,
    );

    if let Some(value) = right {
        ui.painter().text(
            egui::pos2(rect.max.x - 10.0, rect.center().y),
            egui::Align2::RIGHT_CENTER,
            value,
            value_font,
            if selected { t.accent } else { t.faint },
        );
    }
    resp
}

/// Small-caps style section heading for the sidebar.
fn section_title(t: &Theme, title: &str) -> egui::RichText {
    egui::RichText::new(title)
        .size(10.5)
        .strong()
        .color(t.faint)
}

/// Group thousands: `85613` → `85,613`.
fn human_count(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// Small rounded stat tile: a big value over a muted caption.
fn mini_stat(ui: &mut egui::Ui, t: &Theme, caption: &str, value: &str, width: f32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 48.0), egui::Sense::hover());
    let radius = egui::CornerRadius::same(9);
    ui.painter().rect_filled(rect, radius, t.card);
    ui.painter().rect_stroke(
        rect,
        radius,
        egui::Stroke::new(1.0_f32, t.stroke),
        egui::StrokeKind::Inside,
    );
    ui.painter().text(
        egui::pos2(rect.min.x + 10.0, rect.min.y + 9.0),
        egui::Align2::LEFT_TOP,
        value,
        egui::FontId::new(17.0, egui::FontFamily::Proportional),
        t.text,
    );
    ui.painter().text(
        egui::pos2(rect.min.x + 10.0, rect.max.y - 9.0),
        egui::Align2::LEFT_BOTTOM,
        caption,
        egui::FontId::new(10.0, egui::FontFamily::Proportional),
        t.faint,
    );
}

/// A small pill chip, used for the sidebar's quick locations.
fn chip(ui: &mut egui::Ui, t: &Theme, label: &str, active: bool) -> egui::Response {
    let font = egui::FontId::new(12.0, egui::FontFamily::Proportional);
    let galley = ui.painter().layout_no_wrap(
        label.to_string(),
        font,
        if active { t.accent } else { t.text },
    );
    let pad = egui::vec2(9.0, 4.0);
    let (rect, resp) = ui.allocate_exact_size(galley.size() + pad * 2.0, egui::Sense::click());
    let radius = egui::CornerRadius::same(7);
    let (fill, stroke) = if active {
        (t.accent_soft(), egui::Stroke::new(1.0_f32, t.accent))
    } else if resp.hovered() {
        (t.hover, egui::Stroke::new(1.0_f32, t.accent))
    } else {
        (t.card, egui::Stroke::new(1.0_f32, t.stroke))
    };
    ui.painter().rect_filled(rect, radius, fill);
    ui.painter()
        .rect_stroke(rect, radius, stroke, egui::StrokeKind::Inside);
    ui.painter().galley(
        rect.min + pad,
        galley,
        if active { t.accent } else { t.text },
    );
    resp
}

/// A sidebar checkbox row (square box + label) for the search options.
fn opt_check(ui: &mut egui::Ui, t: &Theme, value: &mut bool, label: &str) -> bool {
    let (rect, resp) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 24.0), egui::Sense::click());
    if resp.hovered() {
        ui.painter()
            .rect_filled(rect, egui::CornerRadius::same(6), t.hover);
    }
    let side = 15.0;
    let bx = egui::Rect::from_min_size(
        egui::pos2(rect.min.x + 4.0, rect.center().y - side / 2.0),
        egui::vec2(side, side),
    );
    let radius = egui::CornerRadius::same(4);
    let stroke = if *value {
        egui::Stroke::new(1.0_f32, t.accent)
    } else {
        egui::Stroke::new(1.0_f32, t.stroke)
    };
    if *value {
        ui.painter().rect_filled(bx, radius, t.accent);
        let c = bx.center();
        let mark = egui::Stroke::new(1.8_f32, t.panel);
        ui.painter().line_segment(
            [egui::pos2(c.x - 3.5, c.y), egui::pos2(c.x - 1.0, c.y + 2.5)],
            mark,
        );
        ui.painter().line_segment(
            [
                egui::pos2(c.x - 1.0, c.y + 2.5),
                egui::pos2(c.x + 3.5, c.y - 2.5),
            ],
            mark,
        );
    } else {
        ui.painter()
            .rect_filled(bx, radius, egui::Color32::TRANSPARENT);
    }
    ui.painter()
        .rect_stroke(bx, radius, stroke, egui::StrokeKind::Inside);
    ui.painter().text(
        egui::pos2(bx.max.x + 9.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::new(12.5, egui::FontFamily::Proportional),
        t.text,
    );
    if resp.clicked() {
        *value = !*value;
        return true;
    }
    false
}

/// One tab pill: title, and a “×” to close it. Returns `(switched, closed)`.
fn tab_button(
    ui: &mut egui::Ui,
    t: &Theme,
    title: &str,
    active: bool,
    closable: bool,
) -> (bool, bool) {
    let galley = ui.painter().layout_no_wrap(
        title.to_string(),
        egui::FontId::new(12.5, egui::FontFamily::Proportional),
        t.text,
    );
    let text_size = galley.size();
    let close_w = if closable { 18.0 } else { 0.0 };
    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2(22.0 + text_size.x + close_w, 26.0),
        egui::Sense::click(),
    );
    let radius = egui::CornerRadius::same(7);
    if active {
        ui.painter().rect_filled(rect, radius, t.accent_soft());
        let bar = egui::Rect::from_min_size(
            egui::pos2(rect.min.x + 1.0, rect.center().y - 7.0),
            egui::vec2(3.0, 14.0),
        );
        ui.painter()
            .rect_filled(bar, egui::CornerRadius::same(2), t.accent);
    } else if resp.hovered() {
        ui.painter().rect_filled(rect, radius, t.hover);
    }
    ui.painter().galley(
        egui::pos2(rect.min.x + 11.0, rect.center().y - text_size.y * 0.5),
        galley,
        if active { t.text } else { t.dim },
    );

    let mut closed = false;
    if closable {
        let crect = egui::Rect::from_center_size(
            egui::pos2(rect.max.x - 11.0, rect.center().y),
            egui::vec2(14.0, 14.0),
        );
        let close_hover = ui.rect_contains_pointer(crect);
        ui.painter().text(
            crect.center(),
            egui::Align2::CENTER_CENTER,
            "×",
            egui::FontId::new(12.0, egui::FontFamily::Proportional),
            if close_hover { t.text } else { t.faint },
        );
        closed = resp.clicked() && close_hover;
    }
    let switched = resp.clicked() && !closed;
    (switched, closed)
}

/// Small vector magnifier used as the search bar's leading icon.
fn magnifier(ui: &mut egui::Ui, color: egui::Color32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::hover());
    let c = rect.center();
    let stroke = egui::Stroke::new(1.6_f32, color);
    ui.painter()
        .circle_stroke(egui::pos2(c.x - 1.5, c.y - 1.5), 5.0, stroke);
    ui.painter().line_segment(
        [
            egui::pos2(c.x + 2.0, c.y + 2.0),
            egui::pos2(c.x + 6.0, c.y + 6.0),
        ],
        stroke,
    );
}

/// Compact toggle pill for the in-bar `.*` / `Aa` switches.
fn toggle(ui: &mut egui::Ui, t: &Theme, value: &mut bool, text: &str, tip: &str) -> bool {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(30.0, 26.0), egui::Sense::click());
    let radius = egui::CornerRadius::same(6);
    let selected = *value;
    let fill = if selected {
        t.accent_soft()
    } else if resp.hovered() {
        t.hover
    } else {
        egui::Color32::TRANSPARENT
    };
    ui.painter().rect_filled(rect, radius, fill);
    if selected {
        ui.painter().rect_stroke(
            rect,
            radius,
            egui::Stroke::new(1.0_f32, t.accent.gamma_multiply(0.55)),
            egui::StrokeKind::Inside,
        );
    }
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        text,
        egui::FontId::new(12.0, egui::FontFamily::Monospace),
        if selected { t.accent } else { t.dim },
    );
    let clicked = resp.clicked();
    resp.on_hover_text(tip);
    if clicked {
        *value = !*value;
    }
    clicked
}

/// Rounded file-type chip shown at the start of every result row.
fn type_chip(ui: &mut egui::Ui, t: &Theme, path: &Path, is_dir: bool) -> egui::Response {
    let color = type_color(t, path, is_dir);
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(30.0, 30.0), egui::Sense::hover());
    let radius = egui::CornerRadius::same(8);
    ui.painter().rect_filled(rect, radius, t.chip_fill(color));
    ui.painter().rect_stroke(
        rect,
        radius,
        egui::Stroke::new(1.0_f32, color.gamma_multiply(0.45)),
        egui::StrokeKind::Inside,
    );
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        chip_label(path, is_dir),
        egui::FontId::new(10.5, egui::FontFamily::Proportional),
        color,
    );
    resp
}

/// Short uppercase token inside a type chip (extension, or a category word).
fn chip_label(path: &Path, is_dir: bool) -> String {
    if is_dir {
        return "DIR".to_string();
    }
    match path.extension().and_then(|e| e.to_str()) {
        Some(e) if !e.is_empty() => e.chars().take(4).collect::<String>().to_ascii_uppercase(),
        _ => "FILE".to_string(),
    }
}

/// Accent colour for a file's type.
fn type_color(t: &Theme, path: &Path, is_dir: bool) -> egui::Color32 {
    if is_dir {
        return t.kind_dir;
    }
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase());
    match ext.as_deref() {
        Some("png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "bmp" | "tiff" | "ico" | "avif") => {
            t.kind_img
        }
        Some(
            "zip" | "tar" | "gz" | "xz" | "bz2" | "7z" | "rar" | "zst" | "deb" | "rpm" | "jar",
        ) => t.kind_arch,
        Some(
            "mp3" | "wav" | "flac" | "ogg" | "m4a" | "aac" | "opus" | "mp4" | "mkv" | "avi" | "mov"
            | "webm" | "flv" | "mpg" | "mpeg" | "wmv",
        ) => t.kind_av,
        Some(
            "pdf" | "doc" | "docx" | "odt" | "rtf" | "xls" | "xlsx" | "csv" | "ods" | "ppt"
            | "pptx" | "odp" | "txt" | "md" | "log",
        ) => t.kind_doc,
        Some(
            "rs" | "py" | "js" | "ts" | "tsx" | "jsx" | "go" | "c" | "cpp" | "h" | "hpp" | "java"
            | "rb" | "sh" | "toml" | "json" | "yaml" | "yml" | "html" | "css" | "sql" | "php"
            | "lua" | "zig" | "ex" | "exs" | "kt" | "swift" | "xml" | "ini" | "conf",
        ) => t.kind_code,
        _ => t.dim,
    }
}

/// Small filled status dot for the status bar.
fn status_dot(ui: &mut egui::Ui, color: egui::Color32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), 4.0, color);
}

#[derive(Clone, Copy)]
enum RowAction {
    Folder,
    Copy,
    Terminal,
}

/// Compact drawn action button for a result row (no emoji, so it aligns and
/// renders identically on every system).
fn action_button(ui: &mut egui::Ui, t: &Theme, kind: RowAction, tip: &str) -> bool {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(28.0, 26.0), egui::Sense::click());
    let hovered = resp.hovered();
    if hovered {
        ui.painter()
            .rect_filled(rect, egui::CornerRadius::same(6), t.hover);
    }
    let stroke = egui::Stroke::new(1.3_f32, if hovered { t.accent } else { t.dim });
    let c = rect.center();
    match kind {
        RowAction::Folder => {
            let body =
                egui::Rect::from_min_size(egui::pos2(c.x - 7.0, c.y - 3.5), egui::vec2(14.0, 10.0));
            ui.painter().rect_stroke(
                body,
                egui::CornerRadius::same(2),
                stroke,
                egui::StrokeKind::Inside,
            );
            let tab =
                egui::Rect::from_min_size(egui::pos2(c.x - 7.0, c.y - 6.0), egui::vec2(6.0, 3.0));
            ui.painter().rect_stroke(
                tab,
                egui::CornerRadius::same(1),
                stroke,
                egui::StrokeKind::Inside,
            );
        }
        RowAction::Copy => {
            let back =
                egui::Rect::from_min_size(egui::pos2(c.x - 6.0, c.y - 6.0), egui::vec2(9.0, 11.0));
            let front =
                egui::Rect::from_min_size(egui::pos2(c.x - 2.0, c.y - 3.0), egui::vec2(9.0, 11.0));
            ui.painter().rect_stroke(
                back,
                egui::CornerRadius::same(2),
                stroke,
                egui::StrokeKind::Inside,
            );
            ui.painter()
                .rect_filled(front, egui::CornerRadius::same(2), t.panel);
            ui.painter().rect_stroke(
                front,
                egui::CornerRadius::same(2),
                stroke,
                egui::StrokeKind::Inside,
            );
        }
        RowAction::Terminal => {
            ui.painter().text(
                c,
                egui::Align2::CENTER_CENTER,
                ">_",
                egui::FontId::new(11.0, egui::FontFamily::Monospace),
                if hovered { t.accent } else { t.dim },
            );
        }
    }
    let clicked = resp.clicked();
    resp.on_hover_text(tip);
    clicked
}

/// Load the UI and monospace system fonts.
///
/// Uses fontconfig (`fc-list`) to resolve each preferred family to a concrete
/// file, then picks the smallest upright, normal-weight, single-face candidate.
/// This deliberately avoids scanning every installed font (previously ~3,000
/// files, which cost ~14 MiB) and avoids huge `.ttc` collections — loading
/// `Inter.ttc` copied 12 MiB and, because a collection's face 0 is not
/// necessarily Regular, could also pick the wrong weight.
fn load_system_fonts() -> (Option<Vec<u8>>, Option<Vec<u8>>) {
    (
        resolve_font(UI_FONT_PREFERENCE, false),
        resolve_font(MONO_FONT_PREFERENCE, true),
    )
}

fn resolve_font(families: &[&str], mono: bool) -> Option<Vec<u8>> {
    for family in families {
        if let Some(bytes) = resolve_family(family, mono) {
            return Some(bytes);
        }
    }
    None
}

fn resolve_family(family: &str, mono: bool) -> Option<Vec<u8>> {
    let out = Command::new("fc-list")
        .arg("-f")
        .arg("%{file}|W%{weight}|S%{slant}|P%{spacing}\n")
        .arg(format!(":family={family}"))
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let want_spacing = if mono { "P100" } else { "P" };
    let mut best: Option<(usize, PathBuf)> = None;
    for line in text.lines() {
        let mut f = line.split('|');
        let (Some(file), Some(weight), Some(slant), Some(spacing)) =
            (f.next(), f.next(), f.next(), f.next())
        else {
            continue;
        };
        if weight != "W80" || slant != "S0" {
            continue;
        }
        // Proportional fonts report an empty spacing (`P`); monospace fonts 100.
        let spacing_ok = spacing == want_spacing || (!mono && spacing == "P0");
        if !spacing_ok {
            continue;
        }
        let path = PathBuf::from(file);
        if !matches!(
            path.extension()
                .and_then(|e| e.to_str())
                .map(|e| e.to_ascii_lowercase())
                .as_deref(),
            Some("ttf" | "otf")
        ) {
            continue;
        }
        let name_len = path
            .file_name()
            .map(|n| n.to_string_lossy().len())
            .unwrap_or(usize::MAX);
        if best.as_ref().is_none_or(|(len, _)| name_len < *len) {
            best = Some((name_len, path));
        }
    }
    std::fs::read(best?.1).ok()
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        while let Ok(OutMsg::Done {
            tab,
            ui_query,
            result,
        }) = self.result_rx.try_recv()
        {
            if tab == self.active_tab {
                if ui_query != self.last_sent {
                    continue; // stale: the query changed since this search was sent
                }
                match result {
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
            } else if let Some(t) = self.tabs.get_mut(tab) {
                // Result for a background tab: update its snapshot only.
                if ui_query != t.last_sent {
                    continue;
                }
                match result {
                    Ok(r) => {
                        t.results = r.results;
                        t.truncated = r.truncated;
                        t.elapsed_ms = r.elapsed_ms;
                        t.error = None;
                        t.pending = false;
                        if let Some(sort) = t.sort {
                            sort_results(&mut t.results, sort);
                        }
                    }
                    Err(e) => {
                        t.error = Some(e);
                        t.pending = false;
                    }
                }
            }
        }
        self.status = self.engine.status_snapshot();

        if self.query != self.last_sent {
            self.dirty = true;
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

        // Persist tab state occasionally while editing (throttled), so closing
        // the app keeps the open searches.
        if self.dirty && self.last_save.elapsed() >= Duration::from_secs(2) {
            self.save_prefs();
        }

        // Sidebar facets: apply the newest counts, and ask for a recount once the
        // active query has settled (throttled, and coalesced in the worker).
        while let Ok(counts) = self.counts_rx.try_recv() {
            if counts.key == self.counts_key {
                self.counts = counts.per_category;
            }
        }
        if !self.pending
            && self.last_sent != self.counts_key
            && self.counts_at.elapsed() >= Duration::from_millis(500)
        {
            let base = Query {
                name: if self.content_mode {
                    String::new()
                } else {
                    self.last_sent.clone()
                },
                regex_mode: self.regex_mode,
                case_sensitive: self.case_sensitive,
                include_hidden: self.hidden,
                full_path: self.full_path,
                content: None,
                category: Category::All,
                include_dirs: self.prefs.include_dirs,
                under: self.under.clone(),
                limit: 1,
            };
            self.counts_key = self.last_sent.clone();
            let _ = self.counts_tx.send(CountRequest {
                base,
                key: self.last_sent.clone(),
            });
            self.counts_at = Instant::now();
        }

        // "Follow system" tracks live theme changes: KDE/GTK/XFCE rewrite their
        // config files when the user switches scheme, so poll a cheap fingerprint
        // every second and re-detect when it moves. A slower unconditional check
        // covers desktops that only report through the XDG portal.
        if self.prefs.dark.is_none() && self.theme_at.elapsed() >= Duration::from_secs(1) {
            self.theme_at = Instant::now();
            let fingerprint = theme_fingerprint();
            let due = self.theme_detect_at.elapsed() >= Duration::from_secs(15);
            if fingerprint != self.theme_fp || due {
                self.theme_fp = fingerprint;
                self.theme_detect_at = Instant::now();
                let dark = detect_system_dark();
                if dark != self.dark {
                    self.dark = dark;
                    self.apply_style(ctx);
                }
            }
        }

        let search_focused = ctx.memory(|m| m.has_focus(search_id()));

        if self.search_was_focused && !search_focused && !self.query.is_empty() && !self.pending {
            let q = self.query.clone();
            self.prefs.commit_query(&q);
            self.sync_history();
            self.save_prefs();
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
        if ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::T)) {
            self.new_tab();
            ctx.memory_mut(|m| m.request_focus(search_id()));
        }
        if ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::W)) {
            let active = self.active_tab;
            self.close_tab(active);
        }
        if ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::Tab)) {
            let next = (self.active_tab + 1) % self.tabs.len().max(1);
            self.switch_tab(next);
        }
        for (n, key) in [
            (0usize, egui::Key::Num1),
            (1, egui::Key::Num2),
            (2, egui::Key::Num3),
            (3, egui::Key::Num4),
            (4, egui::Key::Num5),
            (5, egui::Key::Num6),
            (6, egui::Key::Num7),
            (7, egui::Key::Num8),
            (8, egui::Key::Num9),
        ] {
            if n < self.tabs.len() && ctx.input(|i| i.modifiers.command && i.key_pressed(key)) {
                self.switch_tab(n);
            }
        }

        ctx.request_repaint_after(Duration::from_millis(250));

        // Tray messages: toggle/open the window, run a search, or quit.
        let mut tray_msgs = Vec::new();
        if let Some(rx) = &self.tray_rx {
            while let Ok(msg) = rx.try_recv() {
                tray_msgs.push(msg);
            }
        }
        for msg in tray_msgs {
            match msg {
                tray::TrayMsg::Open => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                    self.window_visible = true;
                }
                tray::TrayMsg::Toggle => {
                    if self.window_visible {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
                        self.window_visible = false;
                    } else {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                        self.window_visible = true;
                    }
                }
                tray::TrayMsg::Search(q) => {
                    self.run_query(&q);
                    self.send_query();
                }
                tray::TrayMsg::Quit => {
                    self.tray_quit = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }

        // Close button: default = quit the app. With "close to tray" enabled
        // (opt-in setting) the window hides instead; only Quit then exits.
        if ctx.input(|i| i.viewport().close_requested()) {
            if self.prefs.close_to_tray && !self.tray_quit {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
                self.window_visible = false;
            }
            // else: let the close proceed and the app exit.
        }

        self.menu_bar(ctx);
        self.tab_bar(ctx);
        self.top_bar(ctx);
        egui::TopBottomPanel::bottom("status").show(ctx, |ui| self.status_bar(ui));
        self.bottom_controls(ctx);
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

        if self.show_about {
            self.about_dialog(ctx);
        }
        if self.show_settings {
            self.settings_dialog(ctx);
        }
        if self.show_shortcuts {
            self.shortcuts_dialog(ctx);
        }
    }

    /// Persist open tabs and history on shutdown (eframe calls this on exit and
    /// on every `auto_save_interval` tick).
    fn save(&mut self, _storage: &mut dyn eframe::Storage) {
        self.save_prefs();
    }
}

impl App {
    fn about_dialog(&mut self, ctx: &egui::Context) {
        let (files, dirs) = self.engine.counts();
        egui::Window::new("About")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.add_space(4.0);
                ui.vertical_centered(|ui| {
                    ui.label(
                        egui::RichText::new("Everything for Linux")
                            .size(20.0)
                            .strong(),
                    );
                    ui.label(
                        egui::RichText::new(format!("Version {}", env!("CARGO_PKG_VERSION")))
                            .color(self.fg_dim()),
                    );
                });
                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new(
                        "Realtime filename & content search for Linux — Everything-style, \
                         low-memory (mmap index), Wayland-first.",
                    )
                    .small(),
                );
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new(format!(
                        "Index: {files} files · {dirs} dirs · {}",
                        if self.status.base_entries > 0 {
                            "mmap-backed"
                        } else {
                            "in-memory"
                        }
                    ))
                    .small()
                    .color(self.fg_dim()),
                );
                ui.label(
                    egui::RichText::new("License: MIT · Rust + egui + ripgrep engine")
                        .small()
                        .color(self.fg_dim()),
                );
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if ui.button("Shortcuts").clicked() {
                        self.show_shortcuts = true;
                        self.show_about = false;
                    }
                    if ui.button("Close").clicked() {
                        self.show_about = false;
                    }
                });
            });
    }

    fn settings_dialog(&mut self, ctx: &egui::Context) {
        egui::Window::new("Settings")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.add_space(4.0);
                ui.label(egui::RichText::new("Appearance").strong());
                ui.radio(self.prefs.dark.is_none(), "Follow system theme")
                    .clicked()
                    .then(|| {
                        self.prefs.dark = None;
                        self.dark = !matches!(dark_light::detect(), dark_light::Mode::Light);
                        self.apply_style(ctx);
                        self.prefs.save();
                    });
                ui.radio(self.prefs.dark == Some(true), "Dark")
                    .clicked()
                    .then(|| {
                        self.prefs.dark = Some(true);
                        self.dark = true;
                        self.apply_style(ctx);
                        self.prefs.save();
                    });
                ui.radio(self.prefs.dark == Some(false), "Light")
                    .clicked()
                    .then(|| {
                        self.prefs.dark = Some(false);
                        self.dark = false;
                        self.apply_style(ctx);
                        self.prefs.save();
                    });
                if ui
                    .checkbox(&mut self.prefs.show_preview, "Preview pane")
                    .changed()
                {
                    self.prefs.save();
                }
                if ui
                    .checkbox(
                        &mut self.prefs.close_to_tray,
                        "Keep running in tray when the window is closed",
                    )
                    .on_hover_text("Off (default): the X button quits the app.")
                    .changed()
                {
                    self.prefs.save();
                }
                ui.add_space(8.0);
                ui.label(egui::RichText::new("Search").strong());
                if ui
                    .add(egui::Slider::new(&mut self.limit, 100..=2000).text("Max results"))
                    .changed()
                {
                    self.send_query();
                }
                ui.add_space(8.0);
                ui.label(egui::RichText::new("Configuration").strong());
                let config_path = everything_core::Config::default_path();
                ui.label(
                    egui::RichText::new(config_path.display().to_string())
                        .small()
                        .monospace()
                        .color(self.fg_dim()),
                );
                ui.horizontal(|ui| {
                    if ui.button("Open config file").clicked() {
                        let _ = Command::new("xdg-open").arg(&config_path).spawn();
                    }
                    if ui.button("Reset GUI settings").clicked() {
                        self.prefs = GuiPrefs::default();
                        self.prefs.save();
                        self.dark = !matches!(dark_light::detect(), dark_light::Mode::Light);
                        self.apply_style(ctx);
                        self.sync_history();
                    }
                });
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if ui.button("Close").clicked() {
                        self.show_settings = false;
                    }
                });
            });
    }

    fn shortcuts_dialog(&mut self, ctx: &egui::Context) {
        egui::Window::new("Keyboard shortcuts")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                let rows = [
                    ("↑ / ↓ / PgUp / PgDn", "Navigate results"),
                    ("Enter", "Open the selected file"),
                    ("Double-click", "Open a result"),
                    ("Esc", "Clear the search"),
                    ("Ctrl+F", "Focus the search box"),
                    ("Ctrl+T", "New tab"),
                    ("Ctrl+W", "Close tab"),
                    ("Ctrl+Tab", "Next tab"),
                    ("Ctrl+1..9", "Select tab"),
                    ("↑ / ↓ (empty search)", "Cycle search history"),
                    ("Click column headers", "Sort results"),
                ];
                ui.add_space(4.0);
                for (key, what) in rows {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(key).monospace().strong());
                        ui.label(egui::RichText::new(what).color(self.fg_dim()));
                    });
                }
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if ui.button("Close").clicked() {
                        self.show_shortcuts = false;
                    }
                });
            });
    }
}

impl App {
    /// Application menu bar (File / Edit / View / Settings / Help).
    fn menu_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("menubar")
            .exact_height(30.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.menu_button("File", |ui| {
                        if ui.button("New tab").clicked() {
                            self.new_tab();
                            ctx.memory_mut(|m| m.request_focus(search_id()));
                            ui.close_menu();
                        }
                        if ui.button("Close tab").clicked() {
                            let active = self.active_tab;
                            self.close_tab(active);
                            ui.close_menu();
                        }
                        ui.separator();
                        if ui.button("Focus search").clicked() {
                            ctx.memory_mut(|m| m.request_focus(search_id()));
                            ui.close_menu();
                        }
                        if ui
                            .button("Reload index")
                            .on_hover_text("Rebuild the index from disk")
                            .clicked()
                        {
                            self.engine.rebuild();
                            ui.close_menu();
                        }
                        ui.separator();
                        if ui.button("Quit").clicked() {
                            self.tray_quit = true;
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                            ui.close_menu();
                        }
                    });
                    ui.menu_button("Edit", |ui| {
                        let changed = ui
                            .checkbox(&mut self.content_mode, "Match contents")
                            .changed()
                            | ui.checkbox(&mut self.regex_mode, "Regex mode").changed()
                            | ui.checkbox(&mut self.case_sensitive, "Case-sensitive")
                                .changed()
                            | ui.checkbox(&mut self.hidden, "Hidden files").changed()
                            | ui.checkbox(&mut self.full_path, "Full path match")
                                .changed();
                        if changed {
                            self.send_query();
                        }
                        ui.separator();
                        if ui.button("Clear search history").clicked() {
                            self.prefs.clear_history();
                            self.sync_history();
                            ui.close_menu();
                        }
                    });
                    ui.menu_button("View", |ui| {
                        if ui
                            .checkbox(&mut self.prefs.show_preview, "Preview pane")
                            .changed()
                        {
                            self.prefs.save();
                        }
                        ui.separator();
                        ui.label(egui::RichText::new("Theme").small());
                        if ui
                            .radio(self.prefs.dark.is_none(), "Follow system")
                            .clicked()
                        {
                            self.prefs.dark = None;
                            self.dark = !matches!(dark_light::detect(), dark_light::Mode::Light);
                            self.apply_style(ctx);
                            self.prefs.save();
                        }
                        if ui.radio(self.prefs.dark == Some(true), "Dark").clicked() {
                            self.prefs.dark = Some(true);
                            self.dark = true;
                            self.apply_style(ctx);
                            self.prefs.save();
                        }
                        if ui.radio(self.prefs.dark == Some(false), "Light").clicked() {
                            self.prefs.dark = Some(false);
                            self.dark = false;
                            self.apply_style(ctx);
                            self.prefs.save();
                        }
                    });
                    ui.menu_button("Settings", |ui| {
                        if ui.button("Settings…").clicked() {
                            self.show_settings = true;
                            ui.close_menu();
                        }
                    });
                    ui.menu_button("Help", |ui| {
                        if ui.button("About").clicked() {
                            self.show_about = true;
                            ui.close_menu();
                        }
                        if ui.button("Keyboard shortcuts").clicked() {
                            self.show_shortcuts = true;
                            ui.close_menu();
                        }
                    });
                });
            });
    }

    /// Tab strip: one pill per open search, plus a “+” action.
    fn tab_bar(&mut self, ctx: &egui::Context) {
        let t = self.theme();
        egui::TopBottomPanel::top("tabs")
            .frame(
                egui::Frame::new()
                    .fill(t.panel)
                    .inner_margin(egui::Margin::symmetric(14, 4)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let mut switch: Option<usize> = None;
                    let mut close: Option<usize> = None;
                    let mut new_tab = false;
                    let closable = self.tabs.len() > 1;
                    for i in 0..self.tabs.len() {
                        let active = i == self.active_tab;
                        let title = tab_title(&self.tabs[i]);
                        let (clicked, closed) = tab_button(ui, &t, &title, active, closable);
                        if closed {
                            close = Some(i);
                        } else if clicked {
                            switch = Some(i);
                        }
                    }
                    ui.add_space(6.0);
                    if ui.button("+").on_hover_text("New tab (Ctrl+T)").clicked() {
                        new_tab = true;
                    }
                    if let Some(i) = switch {
                        self.switch_tab(i);
                    }
                    if let Some(i) = close {
                        self.close_tab(i);
                    }
                    if new_tab {
                        self.new_tab();
                        ctx.memory_mut(|m| m.request_focus(search_id()));
                    }
                });
            });
    }

    /// Floating search bar with in-bar toggles and the options menu.
    fn top_bar(&mut self, ctx: &egui::Context) {
        let t = self.theme();
        let focused = ctx.memory(|m| m.has_focus(search_id()));
        egui::TopBottomPanel::top("search")
            .frame(
                egui::Frame::new()
                    .fill(t.panel)
                    .inner_margin(egui::Margin::symmetric(14, 12)),
            )
            .show(ctx, |ui| {
                let hint = if self.content_mode {
                    "Search file contents (regex)…"
                } else if self.regex_mode {
                    "Search filenames (regex)…"
                } else {
                    "Search files and folders…"
                };
                egui::Frame::new()
                    .fill(t.card)
                    .corner_radius(egui::CornerRadius::same(10))
                    .inner_margin(egui::Margin::symmetric(12, 8))
                    .stroke(egui::Stroke::new(
                        1.0_f32,
                        if focused { t.accent } else { t.stroke },
                    ))
                    .shadow(t.shadow(14, 4))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            magnifier(ui, t.faint);
                            ui.add_space(4.0);
                            let edit = egui::TextEdit::singleline(&mut self.query)
                                .id(search_id())
                                .hint_text(egui::RichText::new(hint).color(t.faint))
                                .font(egui::FontId::new(16.0, egui::FontFamily::Proportional))
                                .desired_width(f32::INFINITY)
                                .margin(egui::vec2(2.0, 4.0))
                                .frame(false);
                            let resp = ui.add(edit);
                            if resp.changed() {
                                self.last_edit = Instant::now();
                                self.history_idx = None;
                            }
                            if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                                let q = self.query.clone();
                                self.prefs.commit_query(&q);
                                self.sync_history();
                                self.history_idx = None;
                                if let Some(first) = self.results.first() {
                                    App::open(&first.path);
                                }
                            }

                            // In-bar toggles: regex, case-sensitive.
                            if toggle(ui, &t, &mut self.regex_mode, ".*", "Regex mode") {
                                self.send_query();
                            }
                            if toggle(ui, &t, &mut self.case_sensitive, "Aa", "Case-sensitive") {
                                self.send_query();
                            }

                            let history = self.prefs.history.clone();
                            ui.menu_button(egui::RichText::new("History").size(13.0), |ui| {
                                ui.set_min_width(240.0);
                                if history.is_empty() {
                                    ui.weak("No recent searches yet");
                                }
                                for q in &history {
                                    if ui.button(q).clicked() {
                                        self.run_query(q);
                                        ui.close_menu();
                                    }
                                }
                                if !history.is_empty() {
                                    ui.separator();
                                    if ui.button("Clear history").clicked() {
                                        self.prefs.clear_history();
                                        self.sync_history();
                                        ui.close_menu();
                                    }
                                }
                            });
                            ui.menu_button(egui::RichText::new("Filters").size(13.0), |ui| {
                                ui.set_min_width(220.0);
                                if ui
                                    .checkbox(&mut self.content_mode, "Match contents")
                                    .on_hover_text("Search inside files (regex)")
                                    .changed()
                                {
                                    self.send_query();
                                }
                                if ui.checkbox(&mut self.hidden, "Hidden files").changed() {
                                    self.send_query();
                                }
                                if ui
                                    .checkbox(&mut self.full_path, "Full path match")
                                    .changed()
                                {
                                    self.send_query();
                                }
                                ui.separator();
                                if ui
                                    .checkbox(&mut self.prefs.show_preview, "Preview pane")
                                    .changed()
                                {
                                    self.prefs.save();
                                }
                                if ui
                                    .button(if self.dark {
                                        "Switch to light theme"
                                    } else {
                                        "Switch to dark theme"
                                    })
                                    .clicked()
                                {
                                    self.dark = !self.dark;
                                    self.prefs.dark = Some(self.dark);
                                    self.prefs.save();
                                    self.apply_style(ui.ctx());
                                }
                            });
                            if !self.query.is_empty()
                                && ui.button("×").on_hover_text("Clear").clicked()
                            {
                                self.query.clear();
                                self.history_idx = None;
                                self.send_query();
                            }
                            if self.pending {
                                ui.spinner();
                            }
                        });
                    });
            });
    }

    fn sidebar(&mut self, ctx: &egui::Context) {
        let t = self.theme();
        egui::SidePanel::left("sidebar")
            .resizable(true)
            .default_width(228.0)
            .min_width(180.0)
            .max_width(340.0)
            .frame(
                egui::Frame::new()
                    .fill(t.panel)
                    .inner_margin(egui::Margin::symmetric(12, 12)),
            )
            .show(ctx, |ui| {
                // Header and the two match tiles stay pinned; only the lists
                // below scroll, so the brand and the numbers never scroll away.
                self.sidebar_header(ui);
                ui.add_space(10.0);
                self.sidebar_stats(ui);
                ui.add_space(10.0);
                ui.separator();
                ui.add_space(4.0);

                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        self.sidebar_categories(ui, &t);

                        ui.add_space(14.0);
                        self.sidebar_locations(ui, &t);

                        ui.add_space(14.0);
                        self.sidebar_options(ui, &t);

                        ui.add_space(14.0);
                        self.sidebar_tips(ui, &t);
                    });

                ui.add_space(4.0);
                ui.separator();
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new(self.backend_label.clone())
                        .size(10.5)
                        .color(t.faint),
                );
            });
    }

    /// Brand + a live-index pill at the top of the sidebar.
    fn sidebar_header(&self, ui: &mut egui::Ui) {
        let t = self.theme();
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("⚡").size(16.0).color(t.accent));
            ui.add_space(2.0);
            ui.label(
                egui::RichText::new("Everything")
                    .size(14.5)
                    .strong()
                    .color(t.text),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                self.live_badge(ui);
            });
        });
    }

    /// “LIVE” / “INDEXING” pill; the dot pulses while the first pass runs.
    fn live_badge(&self, ui: &mut egui::Ui) {
        let t = self.theme();
        let live = self.status.state == State::Live;
        let color = if live { t.good } else { t.warn };
        let pulse = if live {
            1.0
        } else {
            ((ui.input(|i| i.time) * 5.0).sin() * 0.5 + 0.5) as f32
        };
        let text = if live { "LIVE" } else { "INDEX" };
        let galley = ui.painter().layout_no_wrap(
            text.to_string(),
            egui::FontId::new(10.0, egui::FontFamily::Proportional),
            color,
        );
        let (rect, _) = ui.allocate_exact_size(
            egui::vec2(galley.size().x + 24.0, 18.0),
            egui::Sense::hover(),
        );
        ui.painter()
            .rect_filled(rect, egui::CornerRadius::same(9), t.chip_fill(color));
        ui.painter().circle_filled(
            egui::pos2(rect.min.x + 9.0, rect.center().y),
            3.5,
            color.gamma_multiply(0.4 + 0.6 * pulse),
        );
        ui.painter().galley(
            egui::pos2(rect.min.x + 17.0, rect.center().y - galley.size().y * 0.5),
            galley,
            color,
        );
        if !live {
            ui.ctx().request_repaint();
        }
    }

    /// Two tiles: matching files for the current query, and what is shown.
    fn sidebar_stats(&self, ui: &mut egui::Ui) {
        let t = self.theme();
        let matches = self.counts.first().copied().unwrap_or(0);
        let shown = self.results.len() as u64;
        let indexed = self.engine.counts().0;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            let w = ((ui.available_width() - 8.0) / 2.0).max(60.0);
            mini_stat(ui, &t, "matches", &human_count(matches), w);
            mini_stat(ui, &t, "shown", &human_count(shown), w);
        });
        ui.add_space(7.0);
        ui.label(
            egui::RichText::new(format!("{} files indexed", human_count(indexed)))
                .size(10.5)
                .color(t.faint),
        );
    }

    /// Category rows with live per-category result counts.
    fn sidebar_categories(&mut self, ui: &mut egui::Ui, t: &Theme) {
        ui.label(section_title(t, "CATEGORIES"));
        ui.add_space(6.0);
        let mut chosen: Option<Category> = None;
        for (i, (label, cat)) in CATEGORIES.iter().enumerate() {
            let selected = self.category == *cat;
            let count = self
                .counts
                .get(i)
                .map(|c| human_count(*c))
                .unwrap_or_else(|| "—".to_string());
            if nav_item(ui, t, category_color(t, cat), label, Some(&count), selected).clicked() {
                chosen = Some(if selected { Category::All } else { *cat });
            }
        }
        if let Some(c) = chosen {
            self.category = c;
            self.send_query();
        }
    }

    /// Quick “search only here” chips, plus a clear control when a filter is on.
    fn sidebar_locations(&mut self, ui: &mut egui::Ui, t: &Theme) {
        let locs = locations();
        let mut clear = false;
        ui.horizontal(|ui| {
            ui.label(section_title(t, "LOCATIONS"));
            if self.under.is_some() {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add(
                            egui::Button::new(
                                egui::RichText::new("Clear").size(10.5).color(t.accent),
                            )
                            .frame(false),
                        )
                        .clicked()
                    {
                        clear = true;
                    }
                });
            }
        });
        ui.add_space(6.0);
        let mut pick: Option<Option<String>> = None;
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(5.0, 5.0);
            for (label, path) in &locs {
                let s = path.to_string_lossy().into_owned();
                let active = self.under.as_deref() == Some(s.as_str());
                if chip(ui, t, label, active).clicked() {
                    pick = Some(if active { None } else { Some(s) });
                }
            }
        });
        if let Some(sel) = pick {
            self.set_under(sel);
        }
        // A filter that is not one of the quick locations (restored or custom).
        if let Some(under) = self.under.clone()
            && !locs.iter().any(|(_, p)| p.to_string_lossy() == under)
        {
            ui.add_space(7.0);
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("▸").size(12.0).color(t.accent));
                ui.label(
                    egui::RichText::new(location_label(&under))
                        .size(12.0)
                        .color(t.text),
                );
            });
        }
        if clear {
            self.set_under(None);
        }
    }

    /// Search toggles, mirroring the in-bar options.
    fn sidebar_options(&mut self, ui: &mut egui::Ui, t: &Theme) {
        ui.label(section_title(t, "SEARCH OPTIONS"));
        ui.add_space(6.0);
        let mut changed = false;
        changed |= opt_check(ui, t, &mut self.content_mode, "Match contents");
        changed |= opt_check(ui, t, &mut self.regex_mode, "Regex");
        changed |= opt_check(ui, t, &mut self.case_sensitive, "Case-sensitive");
        changed |= opt_check(ui, t, &mut self.hidden, "Hidden files");
        changed |= opt_check(ui, t, &mut self.full_path, "Full-path match");
        if changed {
            self.send_query();
        }
    }

    /// Collapsible cheat-sheet; its open state is persisted.
    fn sidebar_tips(&mut self, ui: &mut egui::Ui, t: &Theme) {
        let open = self.prefs.sidebar_tips;
        let arrow = if open { "▾" } else { "▸" };
        let resp = ui.add(
            egui::Button::new(
                egui::RichText::new(format!("{arrow}  TIPS"))
                    .size(10.5)
                    .strong()
                    .color(t.faint),
            )
            .frame(false),
        );
        if resp.clicked() {
            self.prefs.sidebar_tips = !open;
            self.prefs.save();
        }
        if open {
            ui.add_space(6.0);
            for (token, meaning) in [
                ("*.pdf", "glob pattern"),
                ("a b", "all terms"),
                ("!draft", "exclude"),
                ("^src/", "path prefix"),
                (".*", "regex mode"),
            ] {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(token)
                            .monospace()
                            .size(11.0)
                            .color(t.dim),
                    );
                    ui.label(egui::RichText::new(meaning).size(11.0).color(t.faint));
                });
            }
        }
    }

    fn results_table(&mut self, ui: &mut egui::Ui) {
        if let Some(err) = &self.error {
            ui.add_space(24.0);
            ui.vertical_centered(|ui| {
                ui.colored_label(ui.visuals().error_fg_color, format!("Query error: {err}"));
            });
            return;
        }

        if self.results.is_empty() {
            self.empty_state(ui);
            return;
        }

        let ctx = ui.ctx().clone();
        let t = self.theme();

        // Column widths are recomputed from the available width every frame so
        // the layout stays stable when the UI zoom (or the window) changes. The
        // metadata columns keep a fixed size; the Name column takes the rest.
        //
        // This deliberately avoids `TableBuilder::resizable`, which caches each
        // column's width in points after the first frame (including the
        // `remainder` column) and then lays them out absolutely — so when zoom
        // changed the available width, the total overflowed and the Size /
        // Modified / Actions columns were pushed off-screen.
        let spacing = ui.spacing().item_spacing.x;
        let size_w = 92.0_f32;
        let mod_w = 148.0_f32;
        let act_w = 92.0_f32;
        let fixed = size_w + mod_w + act_w + spacing * 3.0;
        let name_w = (ui.available_width() - fixed - 2.0).max(80.0);

        let mut table = TableBuilder::new(ui)
            .striped(true)
            .sense(egui::Sense::click()) // rows must sense clicks, not just hover
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::exact(name_w))
            .column(Column::exact(size_w))
            .column(Column::exact(mod_w))
            .column(Column::exact(act_w));
        if let Some(target) = self.scroll_to.take() {
            table = table.scroll_to_row(target, Some(egui::Align::Center));
        }

        table
            .header(34.0, |mut header| {
                header.col(|ui| {
                    if sort_button(ui, "Name", self.sort, |s| matches!(s, Sort::Name(_))).clicked()
                    {
                        self.toggle_sort(Sort::Name(true));
                    }
                });
                header.col(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if sort_button(ui, "Size", self.sort, |s| matches!(s, Sort::Size(_)))
                            .clicked()
                        {
                            self.toggle_sort(Sort::Size(true));
                        }
                    });
                });
                header.col(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if sort_button(ui, "Modified", self.sort, |s| matches!(s, Sort::Mtime(_)))
                            .clicked()
                        {
                            self.toggle_sort(Sort::Mtime(true));
                        }
                    });
                });
                header.col(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(egui::RichText::new("Actions").size(11.5).color(t.faint));
                    });
                });
            })
            .body(|body| {
                let rows = self.results.len();
                let selected = self.selected;
                body.rows(48.0, rows, |mut row| {
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
                            let bar = egui::Rect::from_min_size(
                                egui::pos2(rect.min.x, rect.top() + 7.0),
                                egui::vec2(3.0, rect.height() - 14.0),
                            );
                            ui.painter()
                                .rect_filled(bar, egui::CornerRadius::same(2), t.accent);
                        }
                        ui.horizontal(|ui| {
                            type_chip(ui, &t, &r.path, r.is_dir);
                            ui.add_space(2.0);
                            ui.vertical(|ui| {
                                ui.spacing_mut().item_spacing.y = 1.0;
                                let name = r
                                    .path
                                    .file_name()
                                    .map(|n| n.to_string_lossy().into_owned())
                                    .unwrap_or_else(|| r.path.display().to_string());
                                ui.label(egui::RichText::new(name).strong().color(t.text));
                                breadcrumb_ui(ui, &r.path, t.faint);
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
                                .color(t.dim),
                            );
                        });
                    });
                    row.col(|ui| {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(egui::RichText::new(human_time(r.mtime)).color(t.dim));
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
                                if action_button(
                                    ui,
                                    &t,
                                    RowAction::Folder,
                                    "Open containing folder",
                                ) {
                                    actions = Some((
                                        true,
                                        actions.is_some_and(|a| a.1),
                                        actions.is_some_and(|a| a.2),
                                    ));
                                }
                                if action_button(ui, &t, RowAction::Copy, "Copy path") {
                                    actions = Some((
                                        actions.is_some_and(|a| a.0),
                                        true,
                                        actions.is_some_and(|a| a.2),
                                    ));
                                }
                                if action_button(ui, &t, RowAction::Terminal, "Open in terminal") {
                                    actions = Some((
                                        actions.is_some_and(|a| a.0),
                                        actions.is_some_and(|a| a.1),
                                        true,
                                    ));
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
        let t = self.theme();
        ui.add_space(64.0);
        ui.vertical_centered(|ui| {
            if self.status.state != State::Live && self.query.is_empty() {
                ui.spinner();
                ui.add_space(10.0);
                ui.label(
                    egui::RichText::new(format!("Indexing… {} files", self.engine.counts().0))
                        .color(t.dim),
                );
                return;
            }
            if !self.query.is_empty() && !self.pending {
                ui.label(
                    egui::RichText::new("No matches")
                        .size(19.0)
                        .strong()
                        .color(t.text),
                );
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new(format!("Nothing found for “{}”", self.query)).color(t.dim),
                );
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new(
                        "Try fewer terms, a different pattern, or enable “Match contents”.",
                    )
                    .size(12.5)
                    .color(t.faint),
                );
                return;
            }
            // Idle state: search tips + recent searches.
            ui.label(
                egui::RichText::new("Start typing to search")
                    .size(19.0)
                    .strong()
                    .color(t.text),
            );
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new(
                    "Search every file and folder in your home directory, instantly.",
                )
                .color(t.dim),
            );
            ui.add_space(18.0);
            for tip in [
                "Glob patterns — *.pdf, report-*.txt",
                "Multiple terms are ANDed — invoice 2026",
                "Exclude with ! — logo !draft",
                "Regex — toggle .* in the search bar",
                "Contents — tick “Match contents” to search inside files",
            ] {
                ui.label(egui::RichText::new(tip).size(12.5).color(t.faint));
            }
            if !self.prefs.history.is_empty() {
                ui.add_space(22.0);
                ui.label(
                    egui::RichText::new("RECENT SEARCHES")
                        .size(11.0)
                        .strong()
                        .color(t.faint),
                );
                ui.add_space(8.0);
                let history = self.prefs.history.clone();
                ui.horizontal_wrapped(|ui| {
                    for q in history.iter().take(8) {
                        if ui.button(q.clone()).clicked() {
                            self.run_query(q);
                        }
                    }
                });
            }
        });
    }

    fn preview_panel(&mut self, ui: &mut egui::Ui) {
        let t = self.theme();
        let Some(pv) = &self.preview else {
            ui.add_space(64.0);
            ui.vertical_centered(|ui| {
                ui.label(
                    egui::RichText::new("No file selected")
                        .size(15.0)
                        .color(t.dim),
                );
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new("Pick a result to preview it here.")
                        .size(12.5)
                        .color(t.faint),
                );
            });
            return;
        };

        let name = pv
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            type_chip(ui, &t, &pv.path, pv.is_dir);
            ui.add_space(4.0);
            ui.label(egui::RichText::new(name).strong().size(15.0).color(t.text));
        });
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(type_label(&pv.path, pv.is_dir))
                    .size(12.5)
                    .color(t.dim),
            );
            ui.label(egui::RichText::new("·").size(12.5).color(t.faint));
            ui.label(
                egui::RichText::new(human_size(pv.size))
                    .size(12.5)
                    .color(t.dim),
            );
            ui.label(egui::RichText::new("·").size(12.5).color(t.faint));
            ui.label(
                egui::RichText::new(human_time(pv.mtime))
                    .size(12.5)
                    .color(t.dim),
            );
        });
        ui.add_space(8.0);
        ui.label(
            egui::RichText::new(pv.path.display().to_string())
                .size(12.0)
                .monospace()
                .color(t.faint),
        );
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            if ui.button("Open").clicked() {
                App::open(&pv.path);
            }
            if ui.button("Copy path").clicked() {
                ui.ctx().copy_text(pv.path.display().to_string());
            }
            if ui
                .button("Terminal")
                .on_hover_text("Open a terminal in this folder")
                .clicked()
            {
                App::open_terminal(&pv.path);
            }
        });
        ui.add_space(10.0);
        ui.separator();
        ui.add_space(10.0);

        if pv.is_dir {
            ui.vertical_centered(|ui| {
                ui.label(egui::RichText::new("Folder").size(14.0).color(t.dim));
                ui.add_space(10.0);
                if ui.button("Open in file manager").clicked() {
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
                        .corner_radius(10),
                );
            });
            return;
        }
        if pv.text.is_empty() {
            ui.label(egui::RichText::new("No preview (binary or too large)").color(t.faint));
            return;
        }
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.label(
                    egui::RichText::new(&pv.text)
                        .monospace()
                        .size(12.0)
                        .color(t.dim),
                );
            });
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        let t = self.theme();
        ui.horizontal(|ui| {
            // Live-index indicator (pulses while indexing).
            match self.status.state {
                State::Starting | State::Indexing => {
                    let a = ((ui.input(|i| i.time) * 5.0).sin() * 0.5 + 0.5) as f32;
                    status_dot(ui, t.warn.gamma_multiply(0.5 + 0.5 * a));
                    ui.label(
                        egui::RichText::new(format!("Indexing… {} files", self.engine.counts().0))
                            .color(t.faint)
                            .size(12.0),
                    );
                }
                State::Live => {
                    status_dot(ui, t.good);
                    ui.label(
                        egui::RichText::new("Live")
                            .color(t.good)
                            .size(12.0)
                            .strong(),
                    );
                    let (files, dirs) = self.engine.counts();
                    ui.label(
                        egui::RichText::new(format!("{files} files · {dirs} folders"))
                            .color(t.faint)
                            .size(12.0),
                    );
                }
            }
            if self.status.degraded {
                let txt = if self.status.watch_failures > 0 {
                    format!("· {} dir(s) not realtime", self.status.watch_failures)
                } else {
                    "· periodic rebuild".to_string()
                };
                ui.label(egui::RichText::new(txt).color(t.warn).size(12.0));
            }
            if self.status.overlay_pending > 0 {
                ui.label(
                    egui::RichText::new(format!("· {} pending", self.status.overlay_pending))
                        .color(t.faint)
                        .size(12.0),
                );
            }
            if let ContentIndexStatus::Enabled {
                entries,
                bytes,
                pending,
            } = &self.status.content_index
            {
                ui.label(
                    egui::RichText::new(format!(
                        "· cache {entries} files · {} MiB{}",
                        bytes / (1024 * 1024),
                        if *pending > 0 {
                            format!(" · {pending} pending")
                        } else {
                            String::new()
                        }
                    ))
                    .color(t.faint)
                    .size(12.0),
                );
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
                    label.push_str(&format!("  ·  {} ms", self.elapsed_ms));
                }
                ui.label(egui::RichText::new(label).color(t.dim).size(12.0));
                ui.separator();
                let (backend_text, backend_color) = if self.engine.connected() {
                    (self.backend_label.clone(), t.faint)
                } else {
                    (format!("{} · unreachable", self.backend_label), t.bad)
                };
                ui.label(
                    egui::RichText::new(backend_text)
                        .color(backend_color)
                        .size(12.0),
                );
                ui.separator();
                ui.label(
                    egui::RichText::new("↑↓ navigate · Enter open · Ctrl+T tab")
                        .color(t.faint)
                        .size(12.0),
                );
            });
        });
    }

    /// Bottom control strip: UI zoom, file-type filter, and folder toggle.
    fn bottom_controls(&mut self, ctx: &egui::Context) {
        let t = self.theme();
        egui::TopBottomPanel::bottom("controls")
            .frame(
                egui::Frame::new()
                    .fill(t.panel)
                    .inner_margin(egui::Margin::symmetric(14, 6)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    // --- zoom levels ------------------------------------------
                    ui.label(egui::RichText::new("Zoom").size(12.0).color(t.faint));
                    let current = ctx.zoom_factor();
                    for level in ZOOM_LEVELS {
                        let active = (current - level).abs() < 0.001;
                        let label =
                            egui::RichText::new(format!("{}%", (level * 100.0).round() as i32))
                                .size(12.0);
                        if ui.selectable_label(active, label).clicked() {
                            ctx.set_zoom_factor(*level);
                            self.prefs.zoom = *level;
                            self.prefs.save();
                        }
                    }

                    ui.separator();

                    // --- file-type filter ------------------------------------
                    ui.label(egui::RichText::new("Type").size(12.0).color(t.faint));
                    let current = CATEGORIES
                        .iter()
                        .find(|(_, c)| *c == self.category)
                        .map(|(l, _)| *l)
                        .unwrap_or("All files");
                    let mut chosen: Option<Category> = None;
                    egui::ComboBox::from_id_salt("type_filter")
                        .selected_text(egui::RichText::new(current).size(12.5))
                        .width(150.0)
                        .show_ui(ui, |ui| {
                            for (label, cat) in CATEGORIES {
                                if ui.selectable_label(self.category == *cat, *label).clicked() {
                                    chosen = Some(*cat);
                                }
                            }
                        });
                    if let Some(c) = chosen {
                        self.category = c;
                        self.send_query();
                    }

                    ui.separator();

                    // --- include folders --------------------------------------
                    if ui
                        .checkbox(&mut self.prefs.include_dirs, "Folders")
                        .on_hover_text("Include folders in results")
                        .changed()
                    {
                        self.prefs.save();
                        self.send_query();
                    }
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
                .add(
                    egui::Label::new(egui::RichText::new(name).small().color(dim))
                        .sense(egui::Sense::click()),
                )
                .on_hover_text("Open this folder")
                .clicked()
            {
                let _ = Command::new("xdg-open").arg(dir).spawn();
            }
        }
    });
}

fn type_label(path: &Path, is_dir: bool) -> String {
    if is_dir {
        return "Directory".to_string();
    }
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase());
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
        path.extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .as_deref(),
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
    let active = current.is_some_and(&is_active);
    let mark = match current {
        Some(s) if is_active(s) => match s {
            Sort::Name(asc) | Sort::Size(asc) | Sort::Mtime(asc) => {
                if asc {
                    "  ↑"
                } else {
                    "  ↓"
                }
            }
        },
        _ => "",
    };
    ui.selectable_label(
        active,
        egui::RichText::new(format!("{label}{mark}"))
            .size(12.0)
            .strong(),
    )
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn category_index_roundtrips() {
        for (i, (_, cat)) in CATEGORIES.iter().enumerate() {
            assert_eq!(category_index(cat), i);
            assert_eq!(category_at(i), *cat);
        }
        // Out-of-range indices (e.g. from an older/newer build) fall back to All.
        assert_eq!(category_at(usize::MAX), Category::All);
    }

    #[test]
    fn tab_prefs_roundtrip_keeps_query_and_filters() {
        let tab = TabState {
            query: "invoice 2026".into(),
            regex_mode: true,
            case_sensitive: true,
            hidden: true,
            category: Category::Images,
            ..TabState::default()
        };
        let back = TabState::from_prefs(&tab.to_prefs());
        assert_eq!(back.query, "invoice 2026");
        assert!(back.regex_mode);
        assert!(back.case_sensitive);
        assert!(back.hidden);
        assert!(!back.content_mode);
        assert_eq!(back.category, Category::Images);
        // A restored tab must re-run its search rather than show stale results.
        assert_eq!(back.last_sent, "");
        assert!(back.results.is_empty());
    }

    #[test]
    fn old_gui_json_without_tabs_still_loads() {
        let p: GuiPrefs = serde_json::from_str(r#"{"dark":true,"history":["a b"]}"#).unwrap();
        assert!(p.tabs.is_empty());
        assert_eq!(p.active_tab, 0);
        assert_eq!(p.zoom, 1.0);
        assert!(p.include_dirs);
        assert_eq!(p.history, vec!["a b".to_string()]);
    }

    #[test]
    fn tab_title_falls_back_and_truncates() {
        let mut tab = TabState::default();
        assert_eq!(tab_title(&tab), "New search");
        tab.query = "   ".into();
        assert_eq!(tab_title(&tab), "New search");
        tab.query = "a".repeat(40);
        let title = tab_title(&tab);
        assert_eq!(title.chars().count(), 23); // 22 + ellipsis
        assert!(title.ends_with('…'));
    }

    #[test]
    fn human_count_groups_thousands() {
        assert_eq!(human_count(0), "0");
        assert_eq!(human_count(7), "7");
        assert_eq!(human_count(999), "999");
        assert_eq!(human_count(1_000), "1,000");
        assert_eq!(human_count(85_613), "85,613");
        assert_eq!(human_count(1_234_567), "1,234,567");
    }

    #[test]
    fn location_label_is_last_component() {
        assert_eq!(location_label("/home/a/Documents"), "Documents");
        assert_eq!(location_label("Documents"), "Documents");
        assert_eq!(location_label("/"), "/");
    }

    #[test]
    fn saved_tab_keeps_location_filter() {
        let tab = TabState {
            under: Some("/home/a/Downloads".into()),
            ..TabState::default()
        };
        assert_eq!(
            TabState::from_prefs(&tab.to_prefs()).under.as_deref(),
            Some("/home/a/Downloads")
        );
    }

    #[test]
    fn kde_background_luma_decides_dark() {
        let dark = "[General]\nDarkMode=true\n[Colors:Window]\nBackgroundNormal=32,35,38\n";
        assert_eq!(kde_globals_dark_from(dark), Some(true));
        let light = "[Colors:Window]\nBackgroundNormal=239,240,241\n";
        assert_eq!(kde_globals_dark_from(light), Some(false));
        // The colour from another section must not be mistaken for the window one.
        let other = "[Colors:Button]\nBackgroundNormal=32,35,38\n";
        assert_eq!(kde_globals_dark_from(other), None);
        assert_eq!(kde_globals_dark_from(""), None);
    }

    #[test]
    fn gtk_settings_decide_dark() {
        assert_eq!(
            gtk_settings_dark_from("[Settings]\ngtk-theme-name=Adwaita-dark\n"),
            Some(true)
        );
        assert_eq!(
            gtk_settings_dark_from("[Settings]\ngtk-theme-name=Breeze\n"),
            Some(false)
        );
        assert_eq!(
            gtk_settings_dark_from("gtk-application-prefer-dark-theme=1\n"),
            Some(true)
        );
        assert_eq!(gtk_settings_dark_from("# nothing\n"), None);
    }
}
