//! `everything-gui` library — native egui frontend over the core engine.
//!
//! Exposed as a library so the combined single-binary app (`everything-linux`)
//! can drive the same UI; [`run`] takes an already-chosen [`Backend`].
//!
//! Layout follows the "FileSearch Pro" reference: a menu bar and a labelled
//! toolbar, a search row (query + scope + go), a filter bar, a results header
//! with sorting and density, then three panes — a sidebar (categories, saved
//! searches, locations, advanced options), a virtualized results table, and a
//! preview/details panel — over a view tab strip, recent searches and a live
//! status bar. Tokyo Night palette; Wayland-first windowing.

use eframe::egui;
use egui_extras::{Column, TableBuilder};
use everything_core::api::DEFAULT_ADDR;
use everything_core::update::{CurlFetcher, InstallReport, Stage, UpdateConfig, Updater};
use everything_core::{
    Backend, Category, ContentIndexStatus, Query, ResultRow, SearchResponse, State, Status,
};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub mod ipc;
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
/// Largest file we will hash for the Details tab (a hash of a huge file would
/// stall the UI thread).
const MAX_HASH_BYTES: u64 = 512 * 1024 * 1024;

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
    /// Size / modified-time / extension filters from the filter bar.
    size: SizeFilter,
    modified: ModifiedFilter,
    extensions: Vec<String>,
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
            size: SizeFilter::Any,
            modified: ModifiedFilter::Any,
            extensions: Vec::new(),
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
            size: size_at(p.size_index),
            modified: modified_at(p.modified_index),
            extensions: p.extensions.clone(),
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
            size_index: size_index(self.size),
            modified_index: modified_index(self.modified),
            extensions: self.extensions.clone(),
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
    /// Index into [`SizeFilter::ALL`] / [`ModifiedFilter::ALL`].
    size_index: usize,
    modified_index: usize,
    /// Extension filter (lowercase, no dot).
    extensions: Vec<String>,
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
    /// Compact result rows (the density buttons on the results header).
    compact_rows: bool,
    /// User-defined saved searches.
    saved: Vec<SavedSearch>,
    /// Check for updates automatically at launch (at most once a day).
    check_updates: bool,
    /// Unix seconds of the last automatic update check (0 = never).
    update_checked_at: u64,
    /// Fuzzy (fzf-style) filename matching. A global mode (not per-tab).
    fuzzy: bool,
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
            compact_rows: false,
            saved: Vec::new(),
            check_updates: true,
            update_checked_at: 0,
            fuzzy: false,
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
    Relevance(bool),
}

/// Size filter (the `Size` control in the filter bar).
#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum SizeFilter {
    #[default]
    Any,
    Small,
    Medium,
    Large,
    Huge,
}

impl SizeFilter {
    const ALL: &'static [SizeFilter] = &[
        SizeFilter::Any,
        SizeFilter::Small,
        SizeFilter::Medium,
        SizeFilter::Large,
        SizeFilter::Huge,
    ];

    fn label(self) -> &'static str {
        match self {
            SizeFilter::Any => "Any",
            SizeFilter::Small => "< 1 MB",
            SizeFilter::Medium => "1 – 100 MB",
            SizeFilter::Large => "100 MB – 1 GB",
            SizeFilter::Huge => "> 1 GB",
        }
    }

    /// Inclusive byte bounds; `None` means unbounded.
    fn bounds(self) -> (Option<u64>, Option<u64>) {
        const KB: u64 = 1024;
        const MB: u64 = 1024 * KB;
        const GB: u64 = 1024 * MB;
        match self {
            SizeFilter::Any => (None, None),
            SizeFilter::Small => (None, Some(MB - 1)),
            SizeFilter::Medium => (Some(MB), Some(100 * MB - 1)),
            SizeFilter::Large => (Some(100 * MB), Some(GB - 1)),
            SizeFilter::Huge => (Some(GB), None),
        }
    }
}

/// Modified-time filter (the `Modified` control in the filter bar).
#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum ModifiedFilter {
    #[default]
    Any,
    Today,
    Week,
    Month,
    Year,
}

impl ModifiedFilter {
    const ALL: &'static [ModifiedFilter] = &[
        ModifiedFilter::Any,
        ModifiedFilter::Today,
        ModifiedFilter::Week,
        ModifiedFilter::Month,
        ModifiedFilter::Year,
    ];

    fn label(self) -> &'static str {
        match self {
            ModifiedFilter::Any => "Any",
            ModifiedFilter::Today => "Today",
            ModifiedFilter::Week => "Past week",
            ModifiedFilter::Month => "Past month",
            ModifiedFilter::Year => "Past year",
        }
    }

    fn secs(self) -> Option<i64> {
        match self {
            ModifiedFilter::Any => None,
            ModifiedFilter::Today => Some(24 * 3600),
            ModifiedFilter::Week => Some(7 * 24 * 3600),
            ModifiedFilter::Month => Some(30 * 24 * 3600),
            ModifiedFilter::Year => Some(365 * 24 * 3600),
        }
    }
}

/// Extensions offered as one-click chips in the `Ext` filter.
const COMMON_EXTENSIONS: &[&str] = &[
    "pdf", "docx", "xlsx", "pptx", "odt", "txt", "md", "csv", "json", "yaml", "toml", "xml",
    "html", "css", "js", "ts", "rs", "py", "go", "sh", "c", "cpp", "java", "log", "png", "jpg",
    "svg", "mp3", "mp4", "zip",
];

/// Result row density (the view buttons on the results header).
#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum Density {
    #[default]
    Comfortable,
    Compact,
}

impl Density {
    fn row_height(self) -> f32 {
        match self {
            Density::Comfortable => 48.0,
            Density::Compact => 30.0,
        }
    }
}

/// Which view fills the central area (the tab strip above the status bar).
#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum ViewTab {
    #[default]
    Results,
    Preview,
    Details,
    History,
}

impl ViewTab {
    const ALL: &'static [ViewTab] = &[
        ViewTab::Results,
        ViewTab::Preview,
        ViewTab::Details,
        ViewTab::History,
    ];

    fn label(self) -> &'static str {
        match self {
            ViewTab::Results => "Results",
            ViewTab::Preview => "Preview",
            ViewTab::Details => "Details",
            ViewTab::History => "Search History",
        }
    }
}

/// A saved search: a name plus the whole query configuration.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct SavedSearch {
    name: String,
    query: String,
    content_mode: bool,
    regex_mode: bool,
    case_sensitive: bool,
    under: Option<String>,
}

fn size_index(s: SizeFilter) -> usize {
    SizeFilter::ALL.iter().position(|x| *x == s).unwrap_or(0)
}

fn size_at(i: usize) -> SizeFilter {
    SizeFilter::ALL.get(i).copied().unwrap_or_default()
}

fn modified_index(m: ModifiedFilter) -> usize {
    ModifiedFilter::ALL
        .iter()
        .position(|x| *x == m)
        .unwrap_or(0)
}

fn modified_at(i: usize) -> ModifiedFilter {
    ModifiedFilter::ALL.get(i).copied().unwrap_or_default()
}

struct Preview {
    path: PathBuf,
    is_dir: bool,
    size: u64,
    mtime: i64,
    text: String,
    /// The bytes were read but are not text (so "binary file", not "empty").
    binary: bool,
    image: Option<egui::TextureHandle>,
}

/// Which part of a file the query is matched against (the search-row scope).
#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum Scope {
    #[default]
    Filenames,
    FullPath,
    Contents,
}

/// Which tab the right-hand panel shows.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum PanelTab {
    #[default]
    Preview,
    Details,
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
    // --- filter bar -------------------------------------------------------
    size: SizeFilter,
    modified: ModifiedFilter,
    extensions: Vec<String>,
    /// Text in the sidebar's own filter box (filters the sidebar lists).
    sidebar_filter: String,
    // --- view state -------------------------------------------------------
    density: Density,
    /// Which view fills the central area (Results / Preview / Details / History).
    view: ViewTab,
    /// Which tab the right-hand panel shows (independent of `view`).
    panel_tab: PanelTab,
    /// Bulk selection, keyed by path so it survives re-sorting.
    checked: HashSet<PathBuf>,
    /// SHA-256 of the selected file, computed on demand for the Details tab.
    hash: Option<(PathBuf, String)>,
    /// "Save current search" popup.
    show_save: bool,
    save_name: String,
    /// The saved-searches window.
    show_saved: bool,
    /// Location history for the Back/Forward toolbar buttons.
    loc_history: Vec<Option<String>>,
    loc_idx: usize,
    // --- live system stats for the status bar -----------------------------
    sys_at: Instant,
    sys_cpu: f32,
    sys_ram_used_kb: u64,
    sys_ram_total_kb: u64,
    /// Previous `/proc/stat` (busy, total) jiffies, for the CPU delta.
    sys_prev: (u64, u64),
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
    /// Screen rect of the search field. A press outside it drops the field's
    /// keyboard focus (focus follows the pointer); typing re-grabs it.
    search_rect: Option<egui::Rect>,
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
    // --- duplicate scans --------------------------------------------------
    dup_tx: mpsc::Sender<DupRequest>,
    dup_rx: mpsc::Receiver<DupMsg>,
    /// Only the newest scan's messages are accepted.
    dup_gen: u64,
    dup_progress: Option<(usize, usize)>,
    dups: Option<DupReport>,
    show_dups: bool,
    // --- self-update ------------------------------------------------------
    /// The “Software update” window is open.
    show_update: bool,
    update_ui: UpdateUi,
    /// Receiver for the in-flight check/install worker (one at a time).
    update_rx: Option<mpsc::Receiver<UpdateMsg>>,
    /// Version of a newer release, once known (drives the status-bar badge).
    update_banner: Option<String>,
    // --- ignore files -----------------------------------------------------
    /// The “Ignore files” window is open.
    show_ignore: bool,
    /// Edit buffer for the global ignore file.
    ignore_text: String,
    /// Transient status line shown inside the ignore window.
    ignore_msg: Option<String>,
    /// Commands from the control socket (`--toggle`, `--search …`).
    ipc_rx: mpsc::Receiver<ipc::Command>,
    /// Row context-menu actions, applied after the panels are drawn.
    pending_cmds: Vec<RowCmd>,
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
        // Control socket: lets `everything-linux --toggle` drive this window
        // (the portable way to bind a global hotkey — see gui/src/ipc.rs).
        let (ipc_tx, ipc_rx) = mpsc::channel::<ipc::Command>();
        ipc::spawn_listener(ipc_tx);
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

        let density = if prefs.compact_rows {
            Density::Compact
        } else {
            Density::Comfortable
        };
        let start_under = start.under.clone();
        // Background worker: duplicate scans. Only the newest request matters,
        // and a scan may take seconds on big result sets, so it never runs on
        // the UI thread. Progress is reported so the window can say so.
        let (dup_tx, dup_rx) = mpsc::channel::<DupMsg>();
        let (dup_req_tx, dup_req_rx) = mpsc::channel::<DupRequest>();
        {
            std::thread::Builder::new()
                .name("duplicates".into())
                .spawn(move || {
                    while let Ok(mut req) = dup_req_rx.recv() {
                        while let Ok(newer) = dup_req_rx.try_recv() {
                            req = newer;
                        }
                        let generation = req.generation;
                        let tx = dup_tx.clone();
                        let mut progress = move |done: usize, total: usize| {
                            let _ = tx.send(DupMsg::Progress(generation, done, total));
                        };
                        let report = find_duplicates(&req.paths, &mut progress);
                        if dup_tx.send(DupMsg::Done(generation, report)).is_err() {
                            break;
                        }
                    }
                })
                .expect("failed to spawn duplicates thread");
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
            size: start.size,
            modified: start.modified,
            extensions: start.extensions,
            sidebar_filter: String::new(),
            density,
            view: ViewTab::Results,
            panel_tab: PanelTab::Preview,
            checked: HashSet::new(),
            hash: None,
            show_save: false,
            save_name: String::new(),
            show_saved: false,
            loc_history: vec![start_under.clone()],
            loc_idx: 0,
            sys_at: Instant::now(),
            sys_cpu: 0.0,
            sys_ram_used_kb: 0,
            sys_ram_total_kb: 0,
            sys_prev: (0, 0),
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
            search_rect: None,
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
            dup_tx: dup_req_tx,
            dup_rx,
            dup_gen: 0,
            dup_progress: None,
            dups: None,
            show_dups: false,
            show_update: false,
            update_ui: UpdateUi::Idle,
            update_rx: None,
            update_banner: None,
            show_ignore: false,
            ignore_text: String::new(),
            ignore_msg: None,
            ipc_rx,
            pending_cmds: Vec::new(),
            theme_fp: theme_fingerprint(),
            theme_at: Instant::now(),
            theme_detect_at: Instant::now(),
        };
        app.apply_style(&cc.egui_ctx);
        cc.egui_ctx.set_zoom_factor(app.prefs.zoom);
        app.send_query();
        app.maybe_auto_check_updates();
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
            size: self.size,
            modified: self.modified,
            extensions: self.extensions.clone(),
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
        self.size = t.size;
        self.modified = t.modified;
        self.extensions = t.extensions;
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
        self.prune_selection();
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
        let (min_size, max_size) = self.size.bounds();
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
            extensions: self.extensions.clone(),
            min_size,
            max_size,
            modified_within_secs: self.modified.secs(),
            fuzzy: self.prefs.fuzzy,
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
        self.push_location();
        self.send_query();
    }

    /// Drop selected paths that are no longer in the result list.
    ///
    /// The selection is a set of paths, but the rows it refers to change with
    /// every query and tab switch — without this the header would keep claiming
    /// "N selected" and the bulk actions would act on files that are not on
    /// screen any more.
    fn prune_selection(&mut self) {
        retain_visible(&mut self.checked, &self.results);
    }

    /// Cycle the sort for a column and re-sort the current results in place
    /// (a header click must work without waiting for the next query response).
    fn toggle_sort(&mut self, prefer: Sort) {
        self.sort = cycle_sort(self.sort, prefer);
        if let Some(s) = self.sort {
            let needle = self.last_sent.clone();
            sort_results(&mut self.results, s, &needle, self.prefs.fuzzy);
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
        let mut binary = false;
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
                    let slice = &bytes[..bytes.len().min(64 * 1024)];
                    binary = !is_probably_text(slice);
                    text = String::from_utf8_lossy(slice).into_owned();
                }
            }
        }
        self.preview = Some(Preview {
            path: row.path.clone(),
            is_dir: row.is_dir,
            size: row.size,
            mtime: row.mtime,
            text,
            binary,
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

/// Bring the window to the front (show it and give it focus).
fn show_window(ctx: &egui::Context) {
    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
}

/// Icons painted by hand (no icon font, no emoji) so they look identical in
/// every theme and on every desktop.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Icon {
    Back,
    Forward,
    Home,
    Index,
    Content,
    Clock,
    Bookmark,
    Terminal,
    Reveal,
    Open,
}

fn paint_icon(painter: &egui::Painter, rect: egui::Rect, kind: Icon, color: egui::Color32) {
    let c = rect.center();
    let s = rect.width().min(rect.height()) * 0.5;
    let stroke = egui::Stroke::new(1.6_f32, color);
    let p = |x: f32, y: f32| egui::pos2(c.x + x * s, c.y + y * s);
    match kind {
        Icon::Back | Icon::Forward => {
            let dir = if kind == Icon::Back { -1.0 } else { 1.0 };
            // Shaft.
            painter.line_segment([p(-0.9 * dir, 0.0), p(0.9 * dir, 0.0)], stroke);
            // Arrow head at the far end.
            painter.line_segment([p(0.25 * dir, -0.6), p(0.9 * dir, 0.0)], stroke);
            painter.line_segment([p(0.25 * dir, 0.6), p(0.9 * dir, 0.0)], stroke);
        }
        Icon::Home => {
            painter.line_segment([p(-0.95, -0.1), p(0.0, -0.95)], stroke);
            painter.line_segment([p(0.0, -0.95), p(0.95, -0.1)], stroke);
            painter.rect_stroke(
                egui::Rect::from_min_max(p(-0.65, -0.1), p(0.65, 0.95)),
                egui::CornerRadius::same(1),
                stroke,
                egui::StrokeKind::Inside,
            );
        }
        Icon::Index => {
            for i in 0..3 {
                let r = egui::Rect::from_min_max(
                    p(-0.9, -0.95 + i as f32 * 0.62),
                    p(0.9, -0.45 + i as f32 * 0.62),
                );
                painter.rect_filled(r, egui::CornerRadius::same(3), color);
            }
        }
        Icon::Content => {
            painter.circle_stroke(p(-0.2, -0.2), s * 0.6, egui::Stroke::new(1.8_f32, color));
            painter.line_segment(
                [p(0.28, 0.28), p(0.9, 0.9)],
                egui::Stroke::new(1.8_f32, color),
            );
        }
        Icon::Clock => {
            painter.circle_stroke(c, s * 0.85, stroke);
            painter.line_segment([c, p(0.0, -0.5)], stroke);
            painter.line_segment([c, p(0.45, 0.15)], stroke);
        }
        Icon::Bookmark => {
            painter.add(egui::Shape::line(
                vec![
                    p(-0.55, -0.9),
                    p(0.55, -0.9),
                    p(0.55, 0.9),
                    p(0.0, 0.3),
                    p(-0.55, 0.9),
                    p(-0.55, -0.9),
                ],
                stroke,
            ));
        }
        Icon::Terminal => {
            painter.rect_stroke(
                egui::Rect::from_min_max(p(-0.95, -0.8), p(0.95, 0.8)),
                egui::CornerRadius::same(2),
                stroke,
                egui::StrokeKind::Inside,
            );
            painter.line_segment([p(-0.5, -0.35), p(-0.1, 0.0)], stroke);
            painter.line_segment([p(-0.1, 0.0), p(-0.5, 0.35)], stroke);
            painter.line_segment([p(0.1, 0.35), p(0.6, 0.35)], stroke);
        }
        Icon::Reveal => {
            painter.add(egui::Shape::line(
                vec![p(-0.9, 0.2), p(-0.9, 0.8), p(0.9, 0.8), p(0.9, 0.2)],
                stroke,
            ));
            painter.line_segment([p(0.0, -0.9), p(0.0, 0.1)], stroke);
            painter.line_segment([p(-0.4, -0.5), p(0.0, -0.9)], stroke);
            painter.line_segment([p(0.4, -0.5), p(0.0, -0.9)], stroke);
        }
        Icon::Open => {
            painter.add(egui::Shape::line(
                vec![
                    p(-0.8, -0.7),
                    p(0.35, -0.7),
                    p(0.6, -0.4),
                    p(0.8, -0.4),
                    p(0.8, 0.7),
                    p(-0.8, 0.7),
                    p(-0.8, -0.7),
                ],
                stroke,
            ));
        }
    }
}

/// A toolbar button: painted icon above a small label, like the reference UI.
/// Returns `true` only when it is enabled *and* clicked.
#[allow(clippy::too_many_arguments)]
fn tool_button(
    ui: &mut egui::Ui,
    t: &Theme,
    icon: Option<Icon>,
    glyph: Option<&str>,
    label: &str,
    tip: &str,
    active: bool,
    enabled: bool,
) -> bool {
    let font = egui::FontId::new(10.5, egui::FontFamily::Proportional);
    let text_w = ui
        .painter()
        .layout_no_wrap(label.to_string(), font.clone(), t.text)
        .size()
        .x;
    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2((text_w + 20.0).max(50.0), 42.0),
        egui::Sense::click(),
    );
    let hovered = enabled && resp.hovered();
    let radius = egui::CornerRadius::same(7);
    let fill = if active {
        Some(t.accent_soft())
    } else if hovered {
        Some(t.hover)
    } else {
        None
    };
    if let Some(fill) = fill {
        ui.painter().rect_filled(rect, radius, fill);
    }
    if active {
        ui.painter().rect_stroke(
            rect,
            radius,
            egui::Stroke::new(1.0_f32, t.accent),
            egui::StrokeKind::Inside,
        );
    }
    let fg = if !enabled {
        t.faint
    } else if active {
        t.accent
    } else {
        t.text
    };
    let icon_rect = egui::Rect::from_center_size(
        egui::pos2(rect.center().x, rect.top() + 15.0),
        egui::vec2(18.0, 18.0),
    );
    if let Some(kind) = icon {
        paint_icon(ui.painter(), icon_rect, kind, fg);
    } else if let Some(g) = glyph {
        ui.painter().text(
            icon_rect.center(),
            egui::Align2::CENTER_CENTER,
            g,
            egui::FontId::new(13.0, egui::FontFamily::Monospace),
            fg,
        );
    }
    let galley = ui.painter().layout_no_wrap(label.to_string(), font, fg);
    ui.painter().galley(
        egui::pos2(
            rect.center().x - galley.size().x * 0.5,
            rect.bottom() - 15.0,
        ),
        galley,
        fg,
    );
    let tip = if enabled {
        tip.to_string()
    } else {
        format!("{tip} — unavailable while indexing")
    };
    resp.on_hover_text(tip).clicked() && enabled
}

/// An accent-filled button for the primary action (the search row's `Search`).
fn primary_button(ui: &mut egui::Ui, t: &Theme, label: &str, icon: Option<Icon>) -> bool {
    let font = egui::FontId::new(12.5, egui::FontFamily::Proportional);
    let text_w = ui
        .painter()
        .layout_no_wrap(label.to_string(), font.clone(), t.bg)
        .size()
        .x;
    let w = text_w + if icon.is_some() { 44.0 } else { 26.0 };
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, 30.0), egui::Sense::click());
    let fill = if resp.hovered() {
        t.accent.gamma_multiply(0.86)
    } else {
        t.accent
    };
    ui.painter()
        .rect_filled(rect, egui::CornerRadius::same(7), fill);
    let mut x = rect.min.x + 11.0;
    if let Some(kind) = icon {
        let ir = egui::Rect::from_center_size(
            egui::pos2(x + 8.0, rect.center().y),
            egui::vec2(16.0, 16.0),
        );
        paint_icon(ui.painter(), ir, kind, t.bg);
        x += 21.0;
    }
    ui.painter().text(
        egui::pos2(x, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        font,
        t.bg,
    );
    resp.clicked()
}

/// Small uppercase label used before a filter control.
fn bar_label(t: &Theme, text: &str) -> egui::RichText {
    egui::RichText::new(text).size(11.0).color(t.faint)
}

/// A cell in the Details grid: muted key on the left, value on the right.
fn detail_row(ui: &mut egui::Ui, t: &Theme, key: &str, value: &str) {
    ui.horizontal_top(|ui| {
        ui.add_sized(
            egui::vec2(84.0, 16.0),
            egui::Label::new(egui::RichText::new(key).size(11.5).color(t.faint)),
        );
        ui.label(egui::RichText::new(value).size(12.0).color(t.text));
    });
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

/// Format a KiB amount the way the status bar wants it (`6.2 GB`).
fn human_kb(kb: u64) -> String {
    const MB: u64 = 1024;
    const GB: u64 = 1024 * MB;
    if kb == 0 {
        return "—".to_string();
    }
    if kb >= GB {
        format!("{:.1} GB", kb as f64 / GB as f64)
    } else {
        format!("{:.0} MB", kb as f64 / MB as f64)
    }
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
/// A bordered action button with a painted icon, used by Quick Actions.
fn quick_action(ui: &mut egui::Ui, t: &Theme, label: &str, icon: Icon) -> bool {
    let font = egui::FontId::new(11.0, egui::FontFamily::Proportional);
    let tw = ui
        .painter()
        .layout_no_wrap(label.to_string(), font.clone(), t.text)
        .size()
        .x;
    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2((tw + 36.0).max(86.0), 26.0),
        egui::Sense::click(),
    );
    let hovered = resp.hovered();
    let radius = egui::CornerRadius::same(6);
    ui.painter()
        .rect_filled(rect, radius, if hovered { t.hover } else { t.card });
    ui.painter().rect_stroke(
        rect,
        radius,
        egui::Stroke::new(1.0_f32, if hovered { t.accent } else { t.stroke }),
        egui::StrokeKind::Inside,
    );
    paint_icon(
        ui.painter(),
        egui::Rect::from_center_size(
            egui::pos2(rect.min.x + 15.0, rect.center().y),
            egui::vec2(13.0, 13.0),
        ),
        icon,
        if hovered { t.accent } else { t.dim },
    );
    ui.painter().text(
        egui::pos2(rect.min.x + 27.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        font,
        t.text,
    );
    resp.clicked()
}

/// Keep only the selected paths that are still in the visible result set.
fn retain_visible(checked: &mut HashSet<PathBuf>, results: &[ResultRow]) {
    if checked.is_empty() {
        return;
    }
    let visible: HashSet<&PathBuf> = results.iter().map(|r| &r.path).collect();
    checked.retain(|p| visible.contains(p));
}

/// Lowercase hex, without allocating through `format!` per byte.
fn hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 0x0f) as usize] as char);
    }
    s
}

/// SHA-256 of a file, as hex. `limit` caps how many bytes are read, which is
/// what makes the duplicate finder's first pass cheap.
fn hash_file(path: &Path, limit: Option<u64>) -> Option<String> {
    use sha2::{Digest, Sha256};
    let mut file = std::fs::File::open(path).ok()?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    let mut left = limit.unwrap_or(u64::MAX);
    while left > 0 {
        let want = if left >= buf.len() as u64 {
            buf.len()
        } else {
            left as usize
        };
        let n = std::io::Read::read(&mut file, &mut buf[..want]).ok()?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        left = left.saturating_sub(n as u64);
    }
    Some(hex(&hasher.finalize()))
}

/// SHA-256 of a whole file, for the Details panel.
fn sha256_of(path: &Path) -> Option<String> {
    hash_file(path, None)
}

/// A set of files with byte-identical contents.
struct DupGroup {
    size: u64,
    paths: Vec<PathBuf>,
}

/// The result of a duplicate scan.
struct DupReport {
    groups: Vec<DupGroup>,
    /// Candidate files (same size as at least one other).
    candidates: usize,
    /// Files that needed a full-content hash. Empty files are ignored: every
    /// empty file is trivially "identical" to every other, which is noise.
    hashed: usize,
    extra_bytes: u64,
    /// Set when the candidate list was cut short (see `DUP_CANDIDATE_CAP`).
    capped: bool,
}

/// How many candidate files one scan will look at, so a huge result set can
/// never turn a right-click into an unbounded disk read.
const DUP_CANDIDATE_CAP: usize = 50_000;
/// Bytes compared in the cheap first pass (size + first 64 KiB).
const DUP_PARTIAL: u64 = 64 * 1024;

/// Find files with identical contents among `paths`.
///
/// Three passes, cheapest first: group by size, then by size + the first 64 KiB,
/// and only then hash the survivors in full — so the expensive pass runs over the
/// smallest possible set. `progress(checked, total)` is called as it goes.
fn find_duplicates(paths: &[PathBuf], progress: &mut dyn FnMut(usize, usize)) -> DupReport {
    let capped = paths.len() > DUP_CANDIDATE_CAP;
    let paths = if capped {
        &paths[..DUP_CANDIDATE_CAP]
    } else {
        paths
    };

    let mut by_size: std::collections::HashMap<u64, Vec<PathBuf>> =
        std::collections::HashMap::new();
    for p in paths {
        if let Ok(md) = std::fs::metadata(p)
            && md.is_file()
            && md.len() > 0
        {
            by_size.entry(md.len()).or_default().push(p.clone());
        }
    }
    let mut sizes: Vec<(u64, Vec<PathBuf>)> =
        by_size.into_iter().filter(|(_, v)| v.len() > 1).collect();
    // Largest first: those are the duplicates that waste the most space.
    sizes.sort_by_key(|(size, _)| std::cmp::Reverse(*size));
    let total: usize = sizes.iter().map(|(_, v)| v.len()).sum();
    let candidates = total;
    let mut done = 0usize;
    let mut hashed = 0usize;
    let mut groups: Vec<DupGroup> = Vec::new();

    for (size, files) in sizes {
        // Report sparsely: one message per file would flood the channel on a
        // 50 000-candidate scan.
        if done.is_multiple_of(64) {
            progress(done, total);
        }
        let mut by_partial: std::collections::HashMap<String, Vec<PathBuf>> =
            std::collections::HashMap::new();
        for f in files {
            if let Some(h) = hash_file(&f, Some(DUP_PARTIAL)) {
                by_partial.entry(h).or_default().push(f);
            }
            done += 1;
            if done.is_multiple_of(64) || done == total {
                progress(done, total);
            }
        }
        for (_, same) in by_partial {
            if same.len() < 2 {
                continue;
            }
            let mut by_full: std::collections::HashMap<String, Vec<PathBuf>> =
                std::collections::HashMap::new();
            for f in same {
                if let Some(h) = hash_file(&f, None) {
                    by_full.entry(h).or_default().push(f);
                    hashed += 1;
                }
            }
            for (_, mut dup) in by_full {
                if dup.len() < 2 {
                    continue;
                }
                dup.sort();
                groups.push(DupGroup { size, paths: dup });
            }
        }
    }

    groups.sort_by_key(|g| std::cmp::Reverse(g.size * (g.paths.len() as u64 - 1)));
    let extra_bytes = groups
        .iter()
        .map(|g| g.size * (g.paths.len() as u64 - 1))
        .sum();
    DupReport {
        groups,
        candidates,
        hashed,
        extra_bytes,
        capped,
    }
}

/// A duplicate scan request (the newest one wins).
struct DupRequest {
    generation: u64,
    paths: Vec<PathBuf>,
}

enum DupMsg {
    Progress(u64, usize, usize),
    Done(u64, DupReport),
}

/// State of the self-update window.
enum UpdateUi {
    Idle,
    Checking,
    UpToDate,
    Available(Box<everything_core::Available>),
    /// Installing; `Some((stage, done, total))` once progress has arrived.
    Installing(Option<(Stage, u64, u64)>),
    Ready(Box<InstallReport>),
    Error(String),
}

/// Messages from the background update worker.
enum UpdateMsg {
    UpToDate,
    Available(Box<everything_core::Available>),
    Progress(Stage, u64, u64),
    Installed(Box<InstallReport>),
    Error(String),
}

/// What the background update worker should do.
enum UpdateTask {
    Check,
    Install(everything_core::Available),
}

/// An action chosen from a result row's context menu. Collected while the table
/// is being drawn and applied afterwards, because applying needs `&mut self`
/// while the rows borrow the result list.
enum RowCmd {
    Open(PathBuf),
    Folder(PathBuf),
    Terminal(PathBuf),
    /// `selection: true` = act on the whole checked set, not just this row.
    CopyPaths {
        path: PathBuf,
        selection: bool,
    },
    CopyNames {
        path: PathBuf,
        selection: bool,
    },
    Details(PathBuf),
    FilterTo(PathBuf),
    SearchName(PathBuf),
    AddToSelection(PathBuf),
    RemoveFromSelection(PathBuf),
    SelectAll,
    Invert,
    ClearSelection,
    FindDuplicates {
        selection: bool,
    },
}

/// `rwxr-xr-x (0755)` for a path, or `—` when it cannot be read.
fn file_perms(path: &Path) -> String {
    use std::os::unix::fs::PermissionsExt;
    match std::fs::metadata(path) {
        Ok(md) => {
            let mode = md.permissions().mode();
            let mut s = String::new();
            for shift in [6u32, 3, 0] {
                let bits = (mode >> shift) & 0o7;
                s.push(if bits & 4 != 0 { 'r' } else { '-' });
                s.push(if bits & 2 != 0 { 'w' } else { '-' });
                s.push(if bits & 1 != 0 { 'x' } else { '-' });
            }
            format!("{s} ({:04o})", mode & 0o7777)
        }
        Err(_) => "—".to_string(),
    }
}

/// Creation (birth) time where the filesystem records one, else `—`. ext4,
/// btrfs and xfs do; many others do not.
fn file_created(path: &Path) -> String {
    std::fs::metadata(path)
        .and_then(|m| m.created())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| human_time(d.as_secs() as i64))
        .unwrap_or_else(|| "—".to_string())
}

/// A best-effort MIME type from the extension (no libmagic dependency).
fn mime_for(path: &Path, is_dir: bool) -> String {
    if is_dir {
        return "inode/directory".to_string();
    }
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase());
    match ext.as_deref() {
        Some("pdf") => "application/pdf",
        Some("doc") => "application/msword",
        Some("docx") => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        Some("xls") => "application/vnd.ms-excel",
        Some("xlsx") => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        Some("ppt") => "application/vnd.ms-powerpoint",
        Some("pptx") => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        Some("odt") => "application/vnd.oasis.opendocument.text",
        Some("ods") => "application/vnd.oasis.opendocument.spreadsheet",
        Some("txt") | Some("log") => "text/plain",
        Some("md") => "text/markdown",
        Some("csv") => "text/csv",
        Some("json") => "application/json",
        Some("xml") => "application/xml",
        Some("yaml") | Some("yml") => "application/yaml",
        Some("toml") => "application/toml",
        Some("html") | Some("htm") => "text/html",
        Some("css") => "text/css",
        Some("js") => "text/javascript",
        Some("ts") => "application/typescript",
        Some("sh") => "application/x-shellscript",
        Some("zip") => "application/zip",
        Some("gz") => "application/gzip",
        Some("tar") => "application/x-tar",
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("svg") => "image/svg+xml",
        Some("mp3") => "audio/mpeg",
        Some("wav") => "audio/wav",
        Some("flac") => "audio/flac",
        Some("mp4") => "video/mp4",
        Some("mkv") => "video/x-matroska",
        Some("webm") => "video/webm",
        Some("rs") => "text/x-rust",
        Some("py") => "text/x-python",
        Some("go") => "text/x-go",
        _ => "application/octet-stream",
    }
    .to_string()
}

/// Small rounded pill naming a file's type (PDF, DOCX, MD, DIR, …).
fn type_pill(ui: &mut egui::Ui, t: &Theme, path: &Path, is_dir: bool) -> egui::Response {
    let color = type_color(t, path, is_dir);
    let galley = ui.painter().layout_no_wrap(
        chip_label(path, is_dir),
        egui::FontId::new(10.0, egui::FontFamily::Proportional),
        color,
    );
    let pad = egui::vec2(7.0, 3.0);
    let (rect, resp) = ui.allocate_exact_size(galley.size() + pad * 2.0, egui::Sense::hover());
    let radius = egui::CornerRadius::same(4);
    ui.painter().rect_filled(rect, radius, t.chip_fill(color));
    ui.painter().rect_stroke(
        rect,
        radius,
        egui::Stroke::new(1.0_f32, color.gamma_multiply(0.45)),
        egui::StrokeKind::Inside,
    );
    ui.painter().galley(rect.min + pad, galley, color);
    resp
}

/// The parent directory of a path, shortened to its last two components so it
/// fits a narrow column (`…/SAB/SDD`).
fn short_dir(path: &Path) -> String {
    let Some(parent) = path.parent() else {
        return String::new();
    };
    let comps: Vec<String> = parent
        .components()
        .filter_map(|c| match c {
            std::path::Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
            std::path::Component::RootDir => Some("/".to_string()),
            _ => None,
        })
        .collect();
    match comps.len() {
        0 => String::new(),
        1 | 2 => comps.join("/"),
        n => format!("…/{}/{}", comps[n - 2], comps[n - 1]),
    }
}

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
                        self.prune_selection();
                        if let Some(sort) = self.sort {
                            let needle = self.last_sent.clone();
                            sort_results(&mut self.results, sort, &needle, self.prefs.fuzzy);
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
                            let needle = t.last_sent.clone();
                            sort_results(&mut t.results, sort, &needle, self.prefs.fuzzy);
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
        self.refresh_sys_stats();

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
                extensions: self.extensions.clone(),
                min_size: self.size.bounds().0,
                max_size: self.size.bounds().1,
                modified_within_secs: self.modified.secs(),
                fuzzy: self.prefs.fuzzy,
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

        self.poll_updates(ctx);

        while let Ok(msg) = self.dup_rx.try_recv() {
            match msg {
                DupMsg::Progress(gen_id, done, total) => {
                    if gen_id == self.dup_gen {
                        self.dup_progress = Some((done, total));
                    }
                }
                DupMsg::Done(gen_id, report) => {
                    if gen_id == self.dup_gen {
                        self.dups = Some(report);
                        self.dup_progress = None;
                        self.show_dups = true;
                    }
                }
            }
        }

        // Keep polling while a scan runs: if egui is otherwise idle nothing
        // would wake it, and the duplicates window would sit on "comparing…"
        // long after the scan had finished.
        if self.dup_progress.is_some() {
            ctx.request_repaint();
        }

        // Row context-menu commands, applied here so they can take `&mut self`
        // and the window context (for the clipboard).
        if !self.pending_cmds.is_empty() {
            let cmds = std::mem::take(&mut self.pending_cmds);
            for cmd in cmds {
                self.apply_row_cmd(cmd, ctx);
            }
        }

        // Focus follows the pointer: a press anywhere outside the search field
        // drops its keyboard focus, so ↑/↓/Enter then drive the results list.
        if ctx.memory(|m| m.has_focus(search_id()))
            && ctx.input(|i| i.pointer.any_pressed())
            && let Some(pos) = ctx.input(|i| i.pointer.interact_pos())
            && !self.search_rect.is_some_and(|r| r.contains(pos))
        {
            ctx.memory_mut(|m| m.surrender_focus(search_id()));
        }

        // …but typing always lands in the search box: the first printable key
        // re-grabs the field and starts a new query, wherever the pointer left
        // focus. The events are inserted here (and swallowed) because egui
        // would otherwise drop input aimed at a widget that is not focused.
        let modal_open = self.show_about
            || self.show_settings
            || self.show_shortcuts
            || self.show_save
            || self.show_saved
            || self.show_dups
            || self.show_update
            || self.show_ignore;
        if !modal_open
            && ctx.memory(|m| m.focused().is_none())
            && let Some(text) = ctx.input(|i| {
                i.events.iter().find_map(|e| match e {
                    egui::Event::Text(t) if t.chars().any(|c| !c.is_control()) => Some(t.clone()),
                    _ => None,
                })
            })
        {
            ctx.memory_mut(|m| m.request_focus(search_id()));
            self.query.push_str(&text);
            self.last_edit = Instant::now();
            self.history_idx = None;
            // Put the caret after the inserted text, then drop the text events
            // so the (now focused) TextEdit does not insert them a second time.
            if let Some(mut state) = egui::TextEdit::load_state(ctx, search_id()) {
                let end = egui::text::CCursor::new(self.query.chars().count());
                state
                    .cursor
                    .set_char_range(Some(egui::text::CCursorRange::one(end)));
                state.store(ctx, search_id());
            }
            ctx.input_mut(|i| i.events.retain(|e| !matches!(e, egui::Event::Text(_))));
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
        if ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::A)) {
            // Only while the search field does not have focus, so text selection
            // inside the box keeps working.
            if !ctx.memory(|m| m.has_focus(search_id())) {
                self.checked = self.results.iter().map(|r| r.path.clone()).collect();
            }
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

        // Control-socket commands (a global hotkey bound to `--toggle`, etc.).
        let mut ipc_msgs = Vec::new();
        while let Ok(cmd) = self.ipc_rx.try_recv() {
            ipc_msgs.push(cmd);
        }
        for cmd in ipc_msgs {
            match cmd {
                ipc::Command::Toggle => {
                    if self.window_visible {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
                        self.window_visible = false;
                    } else {
                        show_window(ctx);
                        self.window_visible = true;
                    }
                }
                ipc::Command::Show => {
                    show_window(ctx);
                    self.window_visible = true;
                }
                ipc::Command::Hide => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
                    self.window_visible = false;
                }
                ipc::Command::Search(query) => {
                    show_window(ctx);
                    self.window_visible = true;
                    self.run_query(&query);
                    self.send_query();
                    ctx.memory_mut(|m| m.request_focus(search_id()));
                }
                ipc::Command::Quit => {
                    self.tray_quit = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }

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
                    show_window(ctx);
                    self.window_visible = true;
                }
                tray::TrayMsg::Toggle => {
                    if self.window_visible {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
                        self.window_visible = false;
                    } else {
                        show_window(ctx);
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
        self.toolbar(ctx);
        self.search_row(ctx);
        self.filter_row(ctx);
        self.results_header(ctx);
        egui::TopBottomPanel::bottom("status").show(ctx, |ui| self.status_bar(ui));
        self.recent_row(ctx);
        self.view_tabs(ctx);
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
        if self.show_saved {
            self.saved_dialog(ctx);
        }
        self.save_search_dialog(ctx);
        self.duplicates_window(ctx);
        self.update_dialog(ctx);
        self.ignore_dialog(ctx);
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
                ui.label(egui::RichText::new("Updates").strong());
                if ui
                    .checkbox(
                        &mut self.prefs.check_updates,
                        "Check for updates at launch (at most once a day)",
                    )
                    .on_hover_text(
                        "Releases are verified with a signed HTTPS manifest and installed in place — no .deb/.rpm.",
                    )
                    .changed()
                {
                    self.prefs.save();
                }
                let dim = self.fg_dim();
                let banner = self.update_banner.clone();
                ui.horizontal(|ui| {
                    if ui.button("Check now…").clicked() {
                        self.show_update = true;
                        self.start_update_check();
                    }
                    if let Some(version) = &banner {
                        ui.label(egui::RichText::new(format!("v{version} available")).color(dim));
                    }
                });
                ui.add_space(8.0);
                ui.label(egui::RichText::new("Search").strong());
                if ui
                    .checkbox(&mut self.prefs.fuzzy, "Fuzzy matching (fzf-style)")
                    .on_hover_text(
                        "Match the query's characters in order anywhere in the name: \
                         mtn → meeting-notes.md",
                    )
                    .changed()
                {
                    self.prefs.save();
                    self.send_query();
                }
                if ui
                    .add(egui::Slider::new(&mut self.limit, 100..=2000).text("Max results"))
                    .changed()
                {
                    self.send_query();
                }
                ui.add_space(8.0);
                ui.label(egui::RichText::new("Indexing").strong());
                let mut respect = self.status.respect_ignore_files;
                if ui
                    .checkbox(&mut respect, "Honor `.gitignore` / `.ignore` files")
                    .on_hover_text("Off: index everything, ignoring ignore files entirely.")
                    .changed()
                {
                    self.engine.set_respect_ignore(respect);
                    self.status.respect_ignore_files = respect;
                }
                if ui.button("Edit ignore files…").clicked() {
                    self.open_ignore_dialog();
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
                    ("Ctrl+A", "Select all results"),
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
                    ui.menu_button("Search", |ui| {
                        let changed = ui
                            .checkbox(&mut self.content_mode, "Match contents")
                            .changed()
                            | ui.checkbox(&mut self.regex_mode, "Regex mode").changed()
                            | ui.checkbox(&mut self.prefs.fuzzy, "Fuzzy matching (fzf-style)")
                                .changed()
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
                    ui.menu_button("Filters", |ui| {
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
                    ui.menu_button("Tools", |ui| {
                        if ui.button("Rebuild index").clicked() {
                            self.engine.rebuild();
                            ui.close_menu();
                        }
                        if ui.button("Ignore files…").clicked() {
                            self.open_ignore_dialog();
                            ui.close_menu();
                        }
                        if ui.button("Save current search…").clicked() {
                            self.show_save = true;
                            self.save_name = self.query.clone();
                            ui.close_menu();
                        }
                        if ui.button("Saved searches…").clicked() {
                            self.show_saved = true;
                            ui.close_menu();
                        }
                    });
                    ui.menu_button("Settings", |ui| {
                        if ui.button("Settings…").clicked() {
                            self.show_settings = true;
                            ui.close_menu();
                        }
                        ui.separator();
                        ui.label(egui::RichText::new("Zoom").small());
                        for level in ZOOM_LEVELS {
                            let label = format!("{:.0}%", level * 100.0);
                            if ui
                                .radio((self.prefs.zoom - *level).abs() < 0.001, label)
                                .clicked()
                            {
                                self.prefs.zoom = *level;
                                ui.ctx().set_zoom_factor(*level);
                                self.prefs.save();
                            }
                        }
                    });
                    ui.menu_button("Help", |ui| {
                        let update_label = match &self.update_banner {
                            Some(version) => format!("Update available: v{version}…"),
                            None => "Check for updates…".to_string(),
                        };
                        if ui.button(update_label).clicked() {
                            self.show_update = true;
                            if matches!(self.update_ui, UpdateUi::Idle | UpdateUi::Error(_)) {
                                self.start_update_check();
                            }
                            ui.close_menu();
                        }
                        ui.separator();
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

    /// Toolbar row: labelled action buttons, as in the reference UI.
    fn toolbar(&mut self, ctx: &egui::Context) {
        let t = self.theme();
        let indexing = matches!(self.status.state, State::Starting | State::Indexing);
        let home = self.home_path();
        let at_home = self.under.as_deref() == home.to_str();
        let recent = matches!(self.category, Category::Recent { .. });
        let saved_count = self.prefs.saved.len();
        egui::TopBottomPanel::top("toolbar")
            .frame(
                egui::Frame::new()
                    .fill(t.panel)
                    .inner_margin(egui::Margin::symmetric(10, 6)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 3.0;

                    if tool_button(
                        ui,
                        &t,
                        Some(Icon::Back),
                        None,
                        "Back",
                        "Previous location",
                        false,
                        self.loc_idx > 0,
                    ) {
                        self.nav_location(-1);
                    }
                    if tool_button(
                        ui,
                        &t,
                        Some(Icon::Forward),
                        None,
                        "Forward",
                        "Next location",
                        false,
                        self.loc_idx + 1 < self.loc_history.len(),
                    ) {
                        self.nav_location(1);
                    }
                    if tool_button(
                        ui,
                        &t,
                        Some(Icon::Home),
                        None,
                        "Home",
                        "Search your home directory",
                        at_home,
                        true,
                    ) {
                        self.set_under(home.to_str().map(|s| s.to_string()));
                    }
                    if tool_button(
                        ui,
                        &t,
                        Some(Icon::Index),
                        None,
                        "Index",
                        "Rebuild the index from disk",
                        false,
                        !indexing,
                    ) {
                        self.engine.rebuild();
                    }

                    ui.add_space(4.0);
                    ui.separator();
                    ui.add_space(4.0);

                    if tool_button(
                        ui,
                        &t,
                        Some(Icon::Content),
                        None,
                        "Content Search",
                        "Search inside file contents (ripgrep, always fresh)",
                        self.content_mode,
                        true,
                    ) {
                        self.content_mode = !self.content_mode;
                        self.send_query();
                    }
                    if tool_button(
                        ui,
                        &t,
                        None,
                        Some(".*"),
                        "Regex",
                        "Treat the query as a regular expression",
                        self.regex_mode,
                        true,
                    ) {
                        self.regex_mode = !self.regex_mode;
                        self.send_query();
                    }
                    if tool_button(
                        ui,
                        &t,
                        None,
                        Some("fz"),
                        "Fuzzy",
                        "Fuzzy matching: the query's characters in order, anywhere \
                         (mtn → meeting-notes.md)",
                        self.prefs.fuzzy,
                        true,
                    ) {
                        self.prefs.fuzzy = !self.prefs.fuzzy;
                        self.prefs.save();
                        self.send_query();
                    }

                    ui.add_space(4.0);
                    ui.separator();
                    ui.add_space(4.0);

                    if tool_button(
                        ui,
                        &t,
                        Some(Icon::Clock),
                        None,
                        "Recent",
                        "Files changed in the last 7 days",
                        recent,
                        true,
                    ) {
                        self.category = if recent {
                            Category::All
                        } else {
                            Category::Recent {
                                max_age_secs: RECENT_AGE_SECS,
                            }
                        };
                        self.send_query();
                    }
                    let saved_label = if saved_count > 0 {
                        format!("Saved ({saved_count})")
                    } else {
                        "Saved".to_string()
                    };
                    if tool_button(
                        ui,
                        &t,
                        Some(Icon::Bookmark),
                        None,
                        &saved_label,
                        "Your saved searches",
                        self.show_saved,
                        true,
                    ) {
                        self.show_saved = !self.show_saved;
                    }
                });
            });
    }

    /// Search row: the query field, the scope, the location and the go button.
    fn search_row(&mut self, ctx: &egui::Context) {
        let t = self.theme();
        let focused = ctx.memory(|m| m.has_focus(search_id()));
        egui::TopBottomPanel::top("searchrow")
            .frame(
                egui::Frame::new()
                    .fill(t.panel)
                    .inner_margin(egui::Margin::symmetric(12, 8)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    ui.label(bar_label(&t, "Search:"));

                    // The field takes whatever is left after the controls.
                    let reserved = 460.0;
                    let field_w = (ui.available_width() - reserved).max(180.0);
                    ui.scope(|ui| {
                        ui.set_width(field_w);
                        egui::Frame::new()
                            .fill(t.card)
                            .corner_radius(egui::CornerRadius::same(8))
                            .inner_margin(egui::Margin::symmetric(10, 5))
                            .stroke(egui::Stroke::new(
                                1.0_f32,
                                if focused { t.accent } else { t.stroke },
                            ))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    magnifier(ui, t.faint);
                                    ui.add_space(4.0);
                                    let hint = if self.content_mode {
                                        "Search inside files…"
                                    } else {
                                        "Search files and folders…"
                                    };
                                    let edit = egui::TextEdit::singleline(&mut self.query)
                                        .id(search_id())
                                        .hint_text(egui::RichText::new(hint).color(t.faint))
                                        .font(egui::FontId::new(
                                            15.0,
                                            egui::FontFamily::Proportional,
                                        ))
                                        .desired_width(f32::INFINITY)
                                        .margin(egui::vec2(2.0, 3.0))
                                        .frame(false);
                                    let resp = ui.add(edit);
                                    self.search_rect = Some(resp.rect);
                                    if resp.changed() {
                                        self.last_edit = Instant::now();
                                        self.history_idx = None;
                                    }
                                    if resp.lost_focus()
                                        && ui.input(|i| i.key_pressed(egui::Key::Enter))
                                    {
                                        let q = self.query.clone();
                                        self.prefs.commit_query(&q);
                                        self.sync_history();
                                        self.history_idx = None;
                                        if let Some(first) = self.results.first() {
                                            App::open(&first.path);
                                        }
                                    }
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

                    // Scope: what part of a file the query is matched against.
                    let scope = self.scope_label();
                    ui.menu_button(scope.to_string(), |ui| {
                        ui.set_min_width(190.0);
                        if ui
                            .selectable_label(self.scope() == Scope::Filenames, "Filenames")
                            .clicked()
                        {
                            self.set_scope(Scope::Filenames);
                            ui.close_menu();
                        }
                        if ui
                            .selectable_label(self.scope() == Scope::FullPath, "Full path")
                            .clicked()
                        {
                            self.set_scope(Scope::FullPath);
                            ui.close_menu();
                        }
                        if ui
                            .selectable_label(self.scope() == Scope::Contents, "Contents (ripgrep)")
                            .clicked()
                        {
                            self.set_scope(Scope::Contents);
                            ui.close_menu();
                        }
                    })
                    .response
                    .on_hover_text("Where the query is matched");

                    // Location.
                    let loc_label = match &self.under {
                        None => "All Locations".to_string(),
                        Some(p) => location_label(p),
                    };
                    ui.menu_button(loc_label, |ui| {
                        ui.set_min_width(220.0);
                        if ui
                            .selectable_label(self.under.is_none(), "All Locations")
                            .clicked()
                        {
                            self.set_under(None);
                            ui.close_menu();
                        }
                        for (label, path) in locations() {
                            let s = path.to_string_lossy().into_owned();
                            if ui
                                .selectable_label(self.under.as_deref() == Some(s.as_str()), label)
                                .clicked()
                            {
                                self.set_under(Some(s));
                                ui.close_menu();
                            }
                        }
                    })
                    .response
                    .on_hover_text("Restrict the search to one directory");

                    if primary_button(ui, &t, "Search", Some(Icon::Content)) {
                        let q = self.query.clone();
                        self.prefs.commit_query(&q);
                        self.sync_history();
                        self.send_query();
                    }
                });
            });
    }

    /// Filter bar: the row of dropdowns and toggles under the search row.
    fn filter_row(&mut self, ctx: &egui::Context) {
        let t = self.theme();
        egui::TopBottomPanel::top("filterrow")
            .frame(
                egui::Frame::new()
                    .fill(t.panel)
                    .inner_margin(egui::Margin::symmetric(12, 5)),
            )
            .show(ctx, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
                    let mut changed = false;

                    // Type (category).
                    ui.label(bar_label(&t, "Type"));
                    ui.menu_button(self.category_label(), |ui| {
                        ui.set_min_width(170.0);
                        for (label, cat) in CATEGORIES {
                            if ui.selectable_label(self.category == *cat, *label).clicked() {
                                self.category = *cat;
                                changed = true;
                                ui.close_menu();
                            }
                        }
                    });

                    // Size.
                    ui.label(bar_label(&t, "Size"));
                    ui.menu_button(self.size.label(), |ui| {
                        ui.set_min_width(170.0);
                        for s in SizeFilter::ALL {
                            if ui.selectable_label(self.size == *s, s.label()).clicked() {
                                self.size = *s;
                                changed = true;
                                ui.close_menu();
                            }
                        }
                    });

                    // Modified.
                    ui.label(bar_label(&t, "Modified"));
                    ui.menu_button(self.modified.label(), |ui| {
                        ui.set_min_width(170.0);
                        for m in ModifiedFilter::ALL {
                            if ui
                                .selectable_label(self.modified == *m, m.label())
                                .clicked()
                            {
                                self.modified = *m;
                                changed = true;
                                ui.close_menu();
                            }
                        }
                    });

                    // Path.
                    ui.label(bar_label(&t, "Path"));
                    ui.menu_button(
                        match &self.under {
                            None => "/ (everywhere)".to_string(),
                            Some(p) => p.clone(),
                        },
                        |ui| {
                            ui.set_min_width(240.0);
                            if ui
                                .selectable_label(self.under.is_none(), "Everywhere")
                                .clicked()
                            {
                                self.set_under(None);
                                ui.close_menu();
                            }
                            for (label, path) in locations() {
                                let s = path.to_string_lossy().into_owned();
                                if ui
                                    .selectable_label(
                                        self.under.as_deref() == Some(s.as_str()),
                                        label,
                                    )
                                    .clicked()
                                {
                                    self.set_under(Some(s));
                                    ui.close_menu();
                                }
                            }
                        },
                    );

                    // Extensions.
                    ui.label(bar_label(&t, "Ext:"));
                    let ext_label = if self.extensions.is_empty() {
                        "any".to_string()
                    } else {
                        let mut s = self.extensions.join(", ");
                        if s.len() > 22 {
                            s.truncate(21);
                            s.push('…');
                        }
                        s
                    };
                    ui.menu_button(ext_label, |ui| {
                        ui.set_min_width(200.0);
                        egui::ScrollArea::vertical()
                            .max_height(320.0)
                            .show(ui, |ui| {
                                for ext in COMMON_EXTENSIONS {
                                    let mut on = self.extensions.iter().any(|e| e == ext);
                                    if ui.checkbox(&mut on, *ext).changed() {
                                        if on {
                                            self.extensions.push((*ext).to_string());
                                        } else {
                                            self.extensions.retain(|e| e != ext);
                                        }
                                        changed = true;
                                    }
                                }
                            });
                        if !self.extensions.is_empty() {
                            ui.separator();
                            if ui.button("Clear extensions").clicked() {
                                self.extensions.clear();
                                changed = true;
                                ui.close_menu();
                            }
                        }
                    });

                    // Case / hidden.
                    if ui.checkbox(&mut self.case_sensitive, "Case").changed() {
                        changed = true;
                    }
                    if ui.checkbox(&mut self.hidden, "Hidden").changed() {
                        changed = true;
                    }

                    ui.add_space(4.0);
                    let any_filter = self.size != SizeFilter::Any
                        || self.modified != ModifiedFilter::Any
                        || !self.extensions.is_empty()
                        || self.under.is_some()
                        || self.category != Category::All
                        || self.hidden
                        || self.case_sensitive
                        || self.full_path
                        || self.content_mode;
                    if ui
                        .add_enabled(
                            any_filter,
                            egui::Button::new(egui::RichText::new("✕  Clear Filters").size(12.0)),
                        )
                        .clicked()
                    {
                        self.clear_filters();
                    }

                    if changed {
                        self.send_query();
                    }
                });
            });
    }

    /// Results header: the summary line, sorting and row density.
    fn results_header(&mut self, ctx: &egui::Context) {
        let t = self.theme();
        let (files, _dirs) = self.engine.counts();
        let shown = self.results.len();
        let total = self.counts.first().copied().unwrap_or(0).max(shown as u64);
        let checked = self.checked.len();
        egui::TopBottomPanel::top("resultshead")
            .frame(
                egui::Frame::new()
                    .fill(t.panel)
                    .inner_margin(egui::Margin::symmetric(12, 4)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let summary = if self.pending {
                        "searching…".to_string()
                    } else if self.error.is_some() {
                        "query error".to_string()
                    } else {
                        format!(
                            "{} results • {} files indexed • {} ms",
                            human_count(total),
                            human_count(files),
                            self.elapsed_ms
                        )
                    };
                    ui.label(egui::RichText::new(summary).size(11.5).color(t.dim));
                    if checked > 0 {
                        ui.label(
                            egui::RichText::new(format!("• {checked} selected"))
                                .size(11.5)
                                .color(t.accent),
                        );
                    }
                    if self.truncated {
                        ui.label(
                            egui::RichText::new(format!("• showing the first {shown}"))
                                .size(11.5)
                                .color(t.warn),
                        );
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // Density.
                        if ui
                            .selectable_label(self.density == Density::Compact, "Compact")
                            .on_hover_text("Compact rows")
                            .clicked()
                        {
                            self.density = Density::Compact;
                            self.prefs.compact_rows = true;
                            self.prefs.save();
                        }
                        if ui
                            .selectable_label(self.density == Density::Comfortable, "Cozy")
                            .on_hover_text("Two-line rows")
                            .clicked()
                        {
                            self.density = Density::Comfortable;
                            self.prefs.compact_rows = false;
                            self.prefs.save();
                        }
                        ui.add_space(10.0);
                        let sort_label = match self.sort {
                            None | Some(Sort::Relevance(_)) => "Relevance",
                            Some(Sort::Name(_)) => "Name",
                            Some(Sort::Size(_)) => "Size",
                            Some(Sort::Mtime(_)) => "Modified",
                        };
                        ui.menu_button(sort_label, |ui| {
                            ui.set_min_width(150.0);
                            let opts: [(&str, Sort); 4] = [
                                ("Relevance", Sort::Relevance(false)),
                                ("Name", Sort::Name(true)),
                                ("Size", Sort::Size(false)),
                                ("Modified", Sort::Mtime(false)),
                            ];
                            for (label, s) in opts {
                                let active = match (self.sort, s) {
                                    (None, Sort::Relevance(_)) => true,
                                    (Some(a), b) => same_key(a, b),
                                    _ => false,
                                };
                                if ui.selectable_label(active, label).clicked() {
                                    self.set_sort(s);
                                    ui.close_menu();
                                }
                            }
                        });
                        ui.label(bar_label(&t, "Sort by:"));
                    });
                });
            });
    }

    /// Which part of a file the query is matched against.
    fn scope(&self) -> Scope {
        if self.content_mode {
            Scope::Contents
        } else if self.full_path {
            Scope::FullPath
        } else {
            Scope::Filenames
        }
    }

    fn scope_label(&self) -> &'static str {
        match self.scope() {
            Scope::Filenames => "Filenames",
            Scope::FullPath => "Full path",
            Scope::Contents => "Contents (ripgrep)",
        }
    }

    fn set_scope(&mut self, s: Scope) {
        if self.scope() == s {
            return;
        }
        self.content_mode = s == Scope::Contents;
        self.full_path = s == Scope::FullPath;
        self.send_query();
    }

    fn category_label(&self) -> String {
        CATEGORIES
            .iter()
            .find(|(_, c)| *c == self.category)
            .map(|(label, _)| (*label).to_string())
            .unwrap_or_else(|| "All files".to_string())
    }

    fn home_path(&self) -> PathBuf {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/"))
    }

    /// Select a sort order and apply it to the results already on screen.
    fn set_sort(&mut self, prefer: Sort) {
        self.sort = Some(prefer);
        let needle = self.last_sent.clone();
        sort_results(&mut self.results, prefer, &needle, self.prefs.fuzzy);
    }

    fn clear_filters(&mut self) {
        self.category = Category::All;
        self.size = SizeFilter::Any;
        self.modified = ModifiedFilter::Any;
        self.extensions.clear();
        self.hidden = false;
        self.case_sensitive = false;
        self.full_path = false;
        self.content_mode = false;
        if self.under.is_some() {
            self.set_under(None);
        } else {
            self.send_query();
        }
    }

    /// Move through the location history (the Back/Forward toolbar buttons).
    fn nav_location(&mut self, delta: i32) {
        let next = self.loc_idx as i32 + delta;
        if next < 0 || next as usize >= self.loc_history.len() {
            return;
        }
        self.loc_idx = next as usize;
        self.under = self.loc_history[self.loc_idx].clone();
        self.send_query();
    }

    /// Remember a location change so Back/Forward can walk it.
    fn push_location(&mut self) {
        let here = self.under.clone();
        self.loc_history.truncate(self.loc_idx + 1);
        if self.loc_history.last().is_some_and(|l| *l == here) {
            return;
        }
        self.loc_history.push(here);
        self.loc_idx = self.loc_history.len() - 1;
    }

    fn sidebar(&mut self, ctx: &egui::Context) {
        let t = self.theme();
        egui::SidePanel::left("sidebar")
            .resizable(true)
            .default_width(236.0)
            .min_width(190.0)
            .max_width(360.0)
            .frame(
                egui::Frame::new()
                    .fill(t.panel)
                    .inner_margin(egui::Margin::symmetric(10, 10)),
            )
            .show(ctx, |ui| {
                // The sidebar's own filter box (it filters the lists below).
                egui::Frame::new()
                    .fill(t.card)
                    .corner_radius(egui::CornerRadius::same(7))
                    .inner_margin(egui::Margin::symmetric(8, 3))
                    .stroke(egui::Stroke::new(1.0_f32, t.stroke))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            magnifier(ui, t.faint);
                            ui.add_space(2.0);
                            ui.add(
                                egui::TextEdit::singleline(&mut self.sidebar_filter)
                                    .hint_text(
                                        egui::RichText::new("Search Everywhere").color(t.faint),
                                    )
                                    .font(egui::FontId::new(12.0, egui::FontFamily::Proportional))
                                    .desired_width(ui.available_width() - 6.0)
                                    .margin(egui::vec2(2.0, 2.0))
                                    .frame(false),
                            );
                        });
                    });
                ui.add_space(9.0);

                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        self.sidebar_categories(ui, &t);
                        ui.add_space(12.0);
                        self.sidebar_saved(ui, &t);
                        ui.add_space(12.0);
                        self.sidebar_locations(ui, &t);
                        ui.add_space(12.0);
                        self.sidebar_advanced(ui, &t);
                        ui.add_space(12.0);
                        self.sidebar_tips(ui, &t);
                    });

                ui.add_space(2.0);
                ui.separator();
                ui.add_space(2.0);
                ui.horizontal(|ui| {
                    self.live_badge(ui);
                    ui.label(
                        egui::RichText::new(self.backend_label.clone())
                            .size(10.0)
                            .color(t.faint),
                    );
                });
            });
    }

    /// Does a sidebar label pass the sidebar's own filter box?
    fn sidebar_matches(&self, haystack: &str) -> bool {
        let f = self.sidebar_filter.trim().to_lowercase();
        f.is_empty() || haystack.to_lowercase().contains(&f)
    }

    /// Category rows with live per-category result counts.
    fn sidebar_categories(&mut self, ui: &mut egui::Ui, t: &Theme) {
        ui.label(section_title(t, "CATEGORIES"));
        ui.add_space(5.0);
        let mut chosen: Option<Category> = None;
        for (i, (label, cat)) in CATEGORIES.iter().enumerate() {
            if !self.sidebar_matches(label) {
                continue;
            }
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

    /// The user's saved searches.
    fn sidebar_saved(&mut self, ui: &mut egui::Ui, t: &Theme) {
        let mut open_save = false;
        ui.horizontal(|ui| {
            ui.label(section_title(t, "SAVED SEARCHES"));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add(
                        egui::Button::new(egui::RichText::new("+").size(13.0).color(t.accent))
                            .frame(false),
                    )
                    .on_hover_text("Save the current search")
                    .clicked()
                {
                    open_save = true;
                }
            });
        });
        if open_save {
            self.show_save = true;
            self.save_name = self.query.clone();
        }
        ui.add_space(5.0);
        if self.prefs.saved.is_empty() {
            ui.label(
                egui::RichText::new("Nothing saved yet.")
                    .size(10.5)
                    .color(t.faint),
            );
            return;
        }
        let mut apply: Option<usize> = None;
        for (i, s) in self.prefs.saved.iter().enumerate() {
            if !self.sidebar_matches(&s.name) {
                continue;
            }
            let active = self.query == s.query;
            if nav_item(ui, t, t.warn, &s.name, None, active).clicked() {
                apply = Some(i);
            }
        }
        if let Some(i) = apply {
            self.apply_saved(i);
        }
    }

    /// Quick “search only here” locations.
    fn sidebar_locations(&mut self, ui: &mut egui::Ui, t: &Theme) {
        ui.label(section_title(t, "INDEXED LOCATIONS"));
        ui.add_space(5.0);
        let locs = locations();
        let mut pick: Option<Option<String>> = None;
        for (label, path) in &locs {
            let s = path.to_string_lossy().into_owned();
            if !self.sidebar_matches(label) && !self.sidebar_matches(&s) {
                continue;
            }
            let active = self.under.as_deref() == Some(s.as_str());
            if nav_item(ui, t, t.kind_dir, label, None, active).clicked() {
                pick = Some(if active { None } else { Some(s) });
            }
        }
        // An active filter that is not one of the quick locations.
        if let Some(under) = self.under.clone()
            && !locs.iter().any(|(_, p)| p.to_string_lossy() == under)
        {
            ui.add_space(4.0);
            if nav_item(ui, t, t.accent, &location_label(&under), None, true).clicked() {
                pick = Some(None);
            }
        }
        if let Some(under) = &self.under {
            ui.add_space(4.0);
            ui.label(egui::RichText::new(under.clone()).size(10.0).color(t.faint));
        }
        if let Some(p) = pick {
            self.set_under(p);
        }
    }

    /// Options that change what the index contains (not just how it is queried).
    fn sidebar_advanced(&mut self, ui: &mut egui::Ui, t: &Theme) {
        ui.label(section_title(t, "ADVANCED SEARCH"));
        ui.add_space(5.0);
        if opt_check(ui, t, &mut self.prefs.include_dirs, "Include folders") {
            self.prefs.save();
            self.send_query();
        }
        let cache = match &self.status.content_index {
            ContentIndexStatus::Enabled { entries, bytes, .. } => {
                format!("on · {} files · {} MiB", entries, bytes / (1024 * 1024))
            }
            ContentIndexStatus::Disabled => "off".to_string(),
        };
        ui.add_space(3.0);
        ui.label(
            egui::RichText::new(format!("Content index: {cache}"))
                .size(10.5)
                .color(t.faint),
        )
        .on_hover_text(
            "The optional background content cache is configured in \
             ~/.config/everything-linux/config.json (content_index_enabled) and \
             takes effect on restart.",
        );
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
            ui.add_space(5.0);
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
                            .size(10.5)
                            .color(t.dim),
                    );
                    ui.label(egui::RichText::new(meaning).size(10.5).color(t.faint));
                });
            }
        }
    }

    /// “LIVE” / “INDEX” pill; the dot pulses while the first pass runs.
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

    /// The results table: `# / Name / Path / Type / Size / Modified / Match /
    /// Relevance`, with bulk-selection checkboxes.
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

        let t = self.theme();
        let needle = self.last_sent.clone();
        let cozy = self.density.row_height() > 36.0;

        // Column widths are recomputed from the available width every frame so
        // the layout stays stable when the UI zoom (or the window) changes. The
        // metadata columns keep a fixed size; `Name` takes what is left.
        //
        // This deliberately avoids `TableBuilder::resizable`, which caches each
        // column's width in points after the first frame (including the
        // `remainder` column) and then lays them out absolutely — so when zoom
        // changed the available width, the total overflowed and the Size /
        // Modified / Actions columns were pushed off-screen.
        let spacing = ui.spacing().item_spacing.x;
        let check_w = 24.0_f32;
        let num_w = 28.0_f32;
        let path_w = 142.0_f32;
        let type_w = 58.0_f32;
        let size_w = 74.0_f32;
        let mod_w = 92.0_f32;
        let match_w = 120.0_f32;
        let rel_w = 74.0_f32;
        let fixed =
            check_w + num_w + path_w + type_w + size_w + mod_w + match_w + rel_w + spacing * 8.0;
        let name_w = (ui.available_width() - fixed - 2.0).max(90.0);

        let mut table = TableBuilder::new(ui)
            .striped(true)
            .sense(egui::Sense::click()) // rows must sense clicks, not just hover
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::exact(check_w))
            .column(Column::exact(num_w))
            .column(Column::exact(name_w))
            .column(Column::exact(path_w))
            .column(Column::exact(type_w))
            .column(Column::exact(size_w))
            .column(Column::exact(mod_w))
            .column(Column::exact(match_w))
            .column(Column::exact(rel_w));
        if let Some(target) = self.scroll_to.take() {
            table = table.scroll_to_row(target, Some(egui::Align::Center));
        }

        table
            .header(28.0, |mut header| {
                header.col(|_| {});
                header.col(|ui| {
                    ui.label(egui::RichText::new("#").size(10.5).color(t.faint));
                });
                header.col(|ui| {
                    if sort_button(ui, "Name", self.sort, |s| matches!(s, Sort::Name(_))).clicked()
                    {
                        self.toggle_sort(Sort::Name(true));
                    }
                });
                header.col(|ui| {
                    ui.label(egui::RichText::new("Path").size(11.0).color(t.faint));
                });
                header.col(|ui| {
                    ui.label(egui::RichText::new("Type").size(11.0).color(t.faint));
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
                    ui.label(egui::RichText::new("Match").size(11.0).color(t.faint));
                });
                header.col(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if sort_button(ui, "Relevance", self.sort, |s| {
                            matches!(s, Sort::Relevance(_))
                        })
                        .clicked()
                        {
                            self.toggle_sort(Sort::Relevance(false));
                        }
                    });
                });
            })
            .body(|body| {
                let rows = self.results.len();
                let selected = self.selected;
                let row_h = self.density.row_height();
                body.rows(row_h, rows, |mut row| {
                    let i = row.index();
                    row.set_selected(i == selected);
                    let r = &self.results[i];
                    let rel = relevance_score(&r.path, &needle, self.prefs.fuzzy);
                    let terms = matched_terms(&r.path, &needle, self.prefs.fuzzy);
                    let name = r
                        .path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| r.path.display().to_string());
                    let mut toggle: Option<PathBuf> = None;

                    // Bulk selection.
                    row.col(|ui| {
                        let mut on = self.checked.contains(&r.path);
                        if ui.add(egui::Checkbox::without_text(&mut on)).changed() {
                            toggle = Some(r.path.clone());
                        }
                    });
                    // Row number.
                    row.col(|ui| {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(
                                egui::RichText::new((i + 1).to_string())
                                    .size(10.5)
                                    .color(t.faint),
                            );
                        });
                    });
                    // Name (two-line when cozy).
                    row.col(|ui| {
                        let rect = ui.max_rect();
                        if i == selected {
                            let bar = egui::Rect::from_min_size(
                                egui::pos2(rect.min.x, rect.top() + 5.0),
                                egui::vec2(3.0, (rect.height() - 10.0).max(6.0)),
                            );
                            ui.painter()
                                .rect_filled(bar, egui::CornerRadius::same(2), t.accent);
                        }
                        ui.horizontal(|ui| {
                            type_chip(ui, &t, &r.path, r.is_dir);
                            ui.add_space(3.0);
                            if cozy {
                                ui.vertical(|ui| {
                                    ui.spacing_mut().item_spacing.y = 1.0;
                                    ui.label(egui::RichText::new(name).strong().color(t.text));
                                    breadcrumb_ui(ui, &r.path, t.faint);
                                });
                            } else {
                                ui.label(egui::RichText::new(name).strong().color(t.text));
                            }
                        });
                    });
                    // Path.
                    row.col(|ui| {
                        ui.label(
                            egui::RichText::new(short_dir(&r.path))
                                .size(11.0)
                                .color(t.dim),
                        )
                        .on_hover_text(r.path.display().to_string());
                    });
                    // Type.
                    row.col(|ui| {
                        type_pill(ui, &t, &r.path, r.is_dir);
                    });
                    // Size.
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
                    // Modified.
                    row.col(|ui| {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(egui::RichText::new(human_time(r.mtime)).color(t.dim));
                        });
                    });
                    // Which query terms this hit matched.
                    row.col(|ui| {
                        if terms.is_empty() {
                            ui.label(egui::RichText::new("—").size(11.0).color(t.faint));
                        } else {
                            for term in terms.iter().take(2) {
                                ui.label(
                                    egui::RichText::new(*term)
                                        .size(10.5)
                                        .monospace()
                                        .color(t.accent),
                                );
                            }
                        }
                    });
                    // Relevance.
                    row.col(|ui| {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(egui::RichText::new(format!("{rel}%")).size(10.5).color(
                                if rel >= 70 {
                                    t.good
                                } else if rel >= 40 {
                                    t.dim
                                } else {
                                    t.faint
                                },
                            ));
                            let (bar, _) =
                                ui.allocate_exact_size(egui::vec2(32.0, 4.0), egui::Sense::hover());
                            ui.painter()
                                .rect_filled(bar, egui::CornerRadius::same(2), t.stroke);
                            let filled = egui::Rect::from_min_size(
                                bar.min,
                                egui::vec2(bar.width() * (rel as f32 / 100.0), bar.height()),
                            );
                            ui.painter()
                                .rect_filled(filled, egui::CornerRadius::same(2), t.accent);
                        });
                    });

                    let row_resp = row.response().clone();
                    if row_resp.double_clicked() {
                        App::open(&r.path);
                    }
                    if row_resp.clicked() {
                        self.selected = i;
                    }
                    if let Some(p) = toggle {
                        if !self.checked.remove(&p) {
                            self.checked.insert(p);
                        }
                    }
                    // The menu acts on the whole selection when this row is part
                    // of it, otherwise on the row alone.
                    let n_sel = self.checked.len();
                    let in_sel = self.checked.contains(&r.path);
                    let multi = in_sel && n_sel > 1;
                    row_resp.context_menu(|ui| {
                        ui.set_min_width(250.0);
                        if multi {
                            ui.label(
                                egui::RichText::new(format!("{n_sel} files selected"))
                                    .size(11.0)
                                    .color(t.faint),
                            );
                            ui.separator();
                        }
                        if ui.button("Open").clicked() {
                            self.pending_cmds.push(RowCmd::Open(r.path.clone()));
                            ui.close_menu();
                        }
                        if ui.button("Open containing folder").clicked() {
                            self.pending_cmds.push(RowCmd::Folder(r.path.clone()));
                            ui.close_menu();
                        }
                        if ui.button("Open in terminal").clicked() {
                            self.pending_cmds.push(RowCmd::Terminal(r.path.clone()));
                            ui.close_menu();
                        }
                        ui.separator();
                        let copy_paths = if multi {
                            format!("Copy {n_sel} paths")
                        } else {
                            "Copy path".to_string()
                        };
                        if ui.button(copy_paths).clicked() {
                            self.pending_cmds.push(RowCmd::CopyPaths {
                                path: r.path.clone(),
                                selection: multi,
                            });
                            ui.close_menu();
                        }
                        let copy_names = if multi {
                            format!("Copy {n_sel} names")
                        } else {
                            "Copy name".to_string()
                        };
                        if ui.button(copy_names).clicked() {
                            self.pending_cmds.push(RowCmd::CopyNames {
                                path: r.path.clone(),
                                selection: multi,
                            });
                            ui.close_menu();
                        }
                        ui.separator();
                        if ui.button("Show in Details panel").clicked() {
                            self.pending_cmds.push(RowCmd::Details(r.path.clone()));
                            ui.close_menu();
                        }
                        if ui.button("Filter to this folder").clicked() {
                            self.pending_cmds.push(RowCmd::FilterTo(r.path.clone()));
                            ui.close_menu();
                        }
                        if ui.button("Search for this name").clicked() {
                            self.pending_cmds.push(RowCmd::SearchName(r.path.clone()));
                            ui.close_menu();
                        }
                        ui.separator();
                        if in_sel {
                            if ui.button("Remove from selection").clicked() {
                                self.pending_cmds
                                    .push(RowCmd::RemoveFromSelection(r.path.clone()));
                                ui.close_menu();
                            }
                        } else if ui.button("Add to selection").clicked() {
                            self.pending_cmds
                                .push(RowCmd::AddToSelection(r.path.clone()));
                            ui.close_menu();
                        }
                        if ui.button("Select all results").clicked() {
                            self.pending_cmds.push(RowCmd::SelectAll);
                            ui.close_menu();
                        }
                        if ui.button("Invert selection").clicked() {
                            self.pending_cmds.push(RowCmd::Invert);
                            ui.close_menu();
                        }
                        if n_sel > 0 && ui.button("Clear selection").clicked() {
                            self.pending_cmds.push(RowCmd::ClearSelection);
                            ui.close_menu();
                        }
                        ui.separator();
                        if ui.button("Find duplicates in results…").clicked() {
                            self.pending_cmds
                                .push(RowCmd::FindDuplicates { selection: false });
                            ui.close_menu();
                        }
                        let dup_sel = if n_sel >= 2 {
                            format!("Find duplicates in selection ({n_sel})")
                        } else {
                            "Find duplicates in selection".to_string()
                        };
                        if ui
                            .add_enabled(n_sel >= 2, egui::Button::new(dup_sel))
                            .on_hover_text("Only scan the checked rows")
                            .clicked()
                        {
                            self.pending_cmds
                                .push(RowCmd::FindDuplicates { selection: true });
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

    /// Right-hand panel: a Preview / Details switch over quick actions.
    fn preview_panel(&mut self, ui: &mut egui::Ui) {
        let t = self.theme();

        let Some(path) = self.preview.as_ref().map(|p| p.path.clone()) else {
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
        let is_dir = self.preview.as_ref().is_some_and(|p| p.is_dir);
        let size = self.preview.as_ref().map(|p| p.size).unwrap_or(0);
        let mtime = self.preview.as_ref().map(|p| p.mtime).unwrap_or(0);

        // The SHA-256 is only computed when the Details tab is open, once per
        // selection, and only for files small enough to hash quickly.
        if self.panel_tab == PanelTab::Details
            && !is_dir
            && size <= MAX_HASH_BYTES
            && self.hash.as_ref().map(|(p, _)| p.as_path()) != Some(path.as_path())
        {
            self.hash = sha256_of(&path).map(|h| (path.clone(), h));
        }

        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());

        ui.add_space(8.0);
        ui.horizontal(|ui| {
            type_chip(ui, &t, &path, is_dir);
            ui.add_space(4.0);
            ui.label(egui::RichText::new(&name).strong().size(14.0).color(t.text));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add(egui::Button::new(egui::RichText::new("✕").size(12.0)).frame(false))
                    .on_hover_text("Hide the preview pane")
                    .clicked()
                {
                    self.prefs.show_preview = false;
                    self.prefs.save();
                }
                if ui
                    .add(egui::Button::new(egui::RichText::new("⧉").size(12.0)).frame(false))
                    .on_hover_text("Open in the default application")
                    .clicked()
                {
                    App::open(&path);
                }
            });
        });
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(type_label(&path, is_dir))
                    .size(11.5)
                    .color(t.dim),
            );
            ui.label(egui::RichText::new("·").size(11.5).color(t.faint));
            ui.label(
                egui::RichText::new(human_size(size))
                    .size(11.5)
                    .color(t.dim),
            );
            ui.label(egui::RichText::new("·").size(11.5).color(t.faint));
            ui.label(
                egui::RichText::new(human_time(mtime))
                    .size(11.5)
                    .color(t.dim),
            );
        });
        ui.add_space(7.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            for (tab, label) in [
                (PanelTab::Preview, "Preview"),
                (PanelTab::Details, "Details"),
            ] {
                if ui.selectable_label(self.panel_tab == tab, label).clicked() {
                    self.panel_tab = tab;
                }
            }
        });
        ui.add_space(6.0);
        ui.separator();
        ui.add_space(6.0);

        match self.panel_tab {
            PanelTab::Preview => {
                let image = self.preview.as_ref().and_then(|p| p.image.clone());
                if is_dir {
                    ui.vertical_centered(|ui| {
                        ui.add_space(12.0);
                        ui.label(egui::RichText::new("Folder").size(14.0).color(t.dim));
                    });
                } else if let Some(tex) = image {
                    ui.vertical_centered(|ui| {
                        ui.add(
                            egui::Image::new(&tex)
                                .max_size(egui::vec2(ui.available_width().min(420.0), 420.0))
                                .corner_radius(10),
                        );
                    });
                } else if let Some(pv) = self.preview.as_ref() {
                    if !pv.text.is_empty() {
                        egui::ScrollArea::vertical()
                            .auto_shrink([false, false])
                            .max_height(ui.available_height() - 120.0)
                            .show(ui, |ui| {
                                ui.label(
                                    egui::RichText::new(&pv.text)
                                        .monospace()
                                        .size(11.5)
                                        .color(t.dim),
                                );
                            });
                    } else if pv.binary {
                        ui.label(
                            egui::RichText::new("No text preview for this binary file.")
                                .size(12.0)
                                .color(t.faint),
                        );
                    } else if pv.size == 0 {
                        ui.label(
                            egui::RichText::new("This file is empty (0 bytes).")
                                .size(12.0)
                                .color(t.faint),
                        );
                    } else {
                        ui.label(
                            egui::RichText::new("No text preview for this file.")
                                .size(12.0)
                                .color(t.faint),
                        );
                    }
                } else {
                    ui.label(
                        egui::RichText::new("No text preview for this file.")
                            .size(12.0)
                            .color(t.faint),
                    );
                }
            }
            PanelTab::Details => {
                let perms = file_perms(&path);
                let created = file_created(&path);
                let mime = mime_for(&path, is_dir);
                let hash = self
                    .hash
                    .as_ref()
                    .filter(|(p, _)| p == &path)
                    .map(|(_, h)| h.clone())
                    .unwrap_or_else(|| {
                        if is_dir || size > MAX_HASH_BYTES {
                            "not computed".to_string()
                        } else {
                            "(available in `sha256sum`)".to_string()
                        }
                    });
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .max_height(ui.available_height() - 130.0)
                    .show(ui, |ui| {
                        detail_row(ui, &t, "Name", &name);
                        detail_row(ui, &t, "Path", &path.display().to_string());
                        detail_row(
                            ui,
                            &t,
                            "Size",
                            &if is_dir {
                                "—".to_string()
                            } else {
                                format!("{} ({size} bytes)", human_size(size))
                            },
                        );
                        detail_row(ui, &t, "Modified", &human_time(mtime));
                        detail_row(ui, &t, "Created", &created);
                        detail_row(ui, &t, "MIME type", &mime);
                        detail_row(ui, &t, "Permissions", &perms);
                        detail_row(ui, &t, "SHA-256", &hash);
                    });
            }
        }

        ui.add_space(8.0);
        ui.separator();
        ui.add_space(6.0);
        ui.label(section_title(&t, "QUICK ACTIONS"));
        ui.add_space(5.0);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(5.0, 5.0);
            if quick_action(ui, &t, "Open", Icon::Open) {
                App::open(&path);
            }
            if quick_action(ui, &t, "Reveal", Icon::Reveal) {
                App::open_folder(&path);
            }
            if quick_action(ui, &t, "Copy path", Icon::Bookmark) {
                ui.ctx().copy_text(path.display().to_string());
            }
            if quick_action(ui, &t, "Terminal", Icon::Terminal) {
                App::open_terminal(&path);
            }
        });
    }

    // --- self-update ------------------------------------------------------

    /// At launch, look for a newer release quietly — at most once a day, so
    /// opening the app repeatedly doesn't hammer the release host.
    fn maybe_auto_check_updates(&mut self) {
        if !self.prefs.check_updates || UpdateConfig::disabled() {
            return;
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        const DAY: u64 = 24 * 3600;
        if self.prefs.update_checked_at != 0
            && now.saturating_sub(self.prefs.update_checked_at) < DAY
        {
            return;
        }
        self.prefs.update_checked_at = now;
        self.prefs.save();
        self.spawn_update_worker(UpdateTask::Check);
    }

    /// Run one update task on a background thread (only one at a time).
    fn spawn_update_worker(&mut self, task: UpdateTask) {
        if self.update_rx.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel::<UpdateMsg>();
        let spawned = std::thread::Builder::new()
            .name("update".into())
            .spawn(move || {
                let updater = Updater::new(UpdateConfig::default().with_env());
                let fetcher = CurlFetcher::new();
                match task {
                    UpdateTask::Check => match updater.check(&fetcher) {
                        Ok(Some(available)) => {
                            let _ = tx.send(UpdateMsg::Available(Box::new(available)));
                        }
                        Ok(None) => {
                            let _ = tx.send(UpdateMsg::UpToDate);
                        }
                        Err(e) => {
                            let _ = tx.send(UpdateMsg::Error(e.to_string()));
                        }
                    },
                    UpdateTask::Install(available) => {
                        let result =
                            updater.install(&available, &fetcher, &mut |stage, done, total| {
                                let _ = tx.send(UpdateMsg::Progress(stage, done, total));
                            });
                        match result {
                            Ok(report) => {
                                let _ = tx.send(UpdateMsg::Installed(Box::new(report)));
                            }
                            Err(e) => {
                                let _ = tx.send(UpdateMsg::Error(e.to_string()));
                            }
                        }
                    }
                }
            });
        match spawned {
            Ok(_) => self.update_rx = Some(rx),
            Err(e) => {
                self.update_ui = UpdateUi::Error(format!("cannot start the update worker: {e}"));
                self.update_rx = None;
            }
        }
    }

    fn start_update_check(&mut self) {
        if self.update_rx.is_some() {
            return;
        }
        self.update_ui = UpdateUi::Checking;
        self.spawn_update_worker(UpdateTask::Check);
    }

    fn start_update_install(&mut self, available: everything_core::Available) {
        if self.update_rx.is_some() {
            return;
        }
        self.update_ui = UpdateUi::Installing(None);
        self.spawn_update_worker(UpdateTask::Install(available));
    }

    /// Drain the update worker's channel and keep the UI repainting while it
    /// runs.
    fn poll_updates(&mut self, ctx: &egui::Context) {
        let Some(rx) = self.update_rx.take() else {
            return;
        };
        let mut running = true;
        loop {
            match rx.try_recv() {
                Ok(msg) => self.apply_update_msg(msg),
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    running = false;
                    break;
                }
            }
        }
        if running {
            self.update_rx = Some(rx);
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }

    fn apply_update_msg(&mut self, msg: UpdateMsg) {
        match msg {
            UpdateMsg::UpToDate => {
                self.update_banner = None;
                self.update_ui = UpdateUi::UpToDate;
            }
            UpdateMsg::Available(available) => {
                self.update_banner = Some(available.version.clone());
                self.update_ui = UpdateUi::Available(available);
            }
            UpdateMsg::Progress(stage, done, total) => {
                self.update_ui = UpdateUi::Installing(Some((stage, done, total)));
            }
            UpdateMsg::Installed(report) => {
                self.update_banner = None;
                self.update_ui = UpdateUi::Ready(report);
            }
            UpdateMsg::Error(message) => {
                self.update_ui = UpdateUi::Error(message);
            }
        }
    }

    /// Relaunch onto the freshly installed binary: stop the daemon (so the new
    /// process starts the new one) and hand over to a detached copy of ourselves.
    fn restart_into_new_version(&mut self) {
        self.engine.shutdown();
        std::thread::sleep(Duration::from_millis(400));
        if let Ok(exe) = std::env::current_exe() {
            let args: Vec<String> = std::env::args().skip(1).collect();
            // `setsid` detaches the relaunch so it outlives this process; fall
            // back to a plain spawn when it is unavailable.
            let detached = Command::new("setsid")
                .arg(&exe)
                .args(&args)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .is_ok();
            if !detached {
                let _ = Command::new(&exe).args(&args).spawn();
            }
        }
        std::process::exit(0);
    }

    /// The “Software update” window (opened from Help ▸ Check for updates…).
    fn update_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_update {
            return;
        }
        let t = self.theme();
        let mut open = true;
        let mut close = false;
        let mut do_check = false;
        let mut do_install: Option<everything_core::Available> = None;
        let mut do_restart = false;
        egui::Window::new("Software update")
            .collapsible(false)
            .resizable(true)
            .default_width(520.0)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.label(
                    egui::RichText::new(format!("Installed version:  v{}", env!("CARGO_PKG_VERSION")))
                        .color(t.dim),
                );
                ui.add_space(8.0);
                match &self.update_ui {
                    UpdateUi::Idle => {
                        ui.label("Check for a newer release over HTTPS.");
                    }
                    UpdateUi::Checking => {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label(egui::RichText::new("Checking the release manifest…").color(t.dim));
                        });
                    }
                    UpdateUi::UpToDate => {
                        ui.colored_label(t.good, "✔  You are running the newest release.");
                    }
                    UpdateUi::Available(available) => {
                        ui.colored_label(
                            t.accent,
                            format!("Version {} is available.", available.version),
                        );
                        if !available.notes.trim().is_empty() {
                            ui.add_space(6.0);
                            egui::ScrollArea::vertical().max_height(170.0).show(ui, |ui| {
                                ui.label(
                                    egui::RichText::new(available.notes.trim())
                                        .monospace()
                                        .size(11.5)
                                        .color(t.dim),
                                );
                            });
                        }
                        ui.add_space(6.0);
                        ui.label(
                            egui::RichText::new(
                                "Signature-checked and installed in place — no .deb/.rpm, no reinstall.",
                            )
                            .size(11.0)
                            .color(t.faint),
                        );
                        ui.add_space(8.0);
                        if ui.button("Install update").clicked() {
                            do_install = Some((**available).clone());
                        }
                    }
                    UpdateUi::Installing(progress) => {
                        let (fraction, text) = match progress {
                            Some((Stage::Downloading, done, total)) if *total > 0 => (
                                *done as f32 / *total as f32,
                                format!(
                                    "Downloading… {} / {} KiB",
                                    done / 1024,
                                    total / 1024
                                ),
                            ),
                            Some((Stage::Downloading, done, _)) => (
                                0.0,
                                format!("Downloading… {} KiB", done / 1024),
                            ),
                            Some((Stage::Verifying, _, _)) => {
                                (1.0, "Verifying checksum and signature…".to_string())
                            }
                            Some((Stage::Installing, _, _)) => (1.0, "Installing…".to_string()),
                            None => (0.0, "Starting…".to_string()),
                        };
                        ui.add(
                            egui::ProgressBar::new(fraction.clamp(0.0, 1.0)).show_percentage(),
                        );
                        ui.label(egui::RichText::new(text).color(t.dim));
                    }
                    UpdateUi::Ready(report) => {
                        ui.colored_label(t.good, format!("✔  Installed version {}.", report.version));
                        ui.add_space(4.0);
                        for file in &report.files {
                            ui.label(
                                egui::RichText::new(format!("   {}", file.display()))
                                    .monospace()
                                    .size(11.0)
                                    .color(t.dim),
                            );
                        }
                        ui.add_space(6.0);
                        ui.label(
                            egui::RichText::new(
                                "Restart to run the new version — the search daemon restarts with it.",
                            )
                            .color(t.dim),
                        );
                        ui.add_space(8.0);
                        if ui.button("Restart now").clicked() {
                            do_restart = true;
                        }
                    }
                    UpdateUi::Error(message) => {
                        ui.colored_label(t.bad, format!("Update failed: {message}"));
                        ui.add_space(4.0);
                        ui.label(
                            egui::RichText::new(
                                "Nothing was changed on disk. You can retry once the cause is fixed.",
                            )
                            .size(11.0)
                            .color(t.faint),
                        );
                    }
                }
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    let busy = matches!(self.update_ui, UpdateUi::Checking | UpdateUi::Installing(_));
                    if ui
                        .add_enabled(!busy, egui::Button::new("Check for updates"))
                        .clicked()
                    {
                        do_check = true;
                    }
                    if ui.button("Close").clicked() {
                        close = true;
                    }
                });
            });

        self.show_update = open && !close;
        if do_check {
            self.start_update_check();
        }
        if let Some(available) = do_install {
            self.start_update_install(available);
        }
        if do_restart {
            self.restart_into_new_version();
        }
    }

    // --- ignore files -----------------------------------------------------

    fn open_ignore_dialog(&mut self) {
        self.load_ignore_text();
        self.ignore_msg = None;
        self.show_ignore = true;
    }

    fn load_ignore_text(&mut self) {
        let path = everything_core::walker::global_ignore_file();
        self.ignore_text = std::fs::read_to_string(&path).unwrap_or_default();
    }

    /// The “Ignore files” window: toggle honoring ignore files, and edit the
    /// global ignore list (Tools ▸ Ignore files…).
    fn ignore_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_ignore {
            return;
        }
        let t = self.theme();
        let path = everything_core::walker::global_ignore_file();
        let mut open = true;
        let mut close = false;
        let mut save = false;
        let mut reload = false;
        let mut rebuild = false;
        let mut toggle: Option<bool> = None;
        egui::Window::new("Ignore files")
            .collapsible(false)
            .resizable(true)
            .default_width(580.0)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.label(
                    egui::RichText::new(
                        "Folders matching these patterns are skipped while indexing — build \
                         trees, caches, virtualenvs. Patterns use .gitignore syntax, matched \
                         relative to the search root; use a `**/` prefix to match at any depth.",
                    )
                    .size(12.0)
                    .color(t.dim),
                );
                ui.add_space(8.0);
                let mut respect = self.status.respect_ignore_files;
                if ui
                    .checkbox(
                        &mut respect,
                        "Honor `.gitignore` / `.ignore` files while indexing",
                    )
                    .on_hover_text(
                        "Off: index everything — ignore files (and this list) are not consulted.",
                    )
                    .changed()
                {
                    toggle = Some(respect);
                }
                ui.add_space(10.0);
                ui.label(egui::RichText::new("Global ignore file").strong());
                ui.label(
                    egui::RichText::new(path.display().to_string())
                        .monospace()
                        .size(11.0)
                        .color(t.faint),
                );
                ui.add_space(6.0);
                egui::ScrollArea::vertical()
                    .max_height(230.0)
                    .show(ui, |ui| {
                        ui.add(
                        egui::TextEdit::multiline(&mut self.ignore_text)
                            .font(egui::TextStyle::Monospace)
                            .desired_width(f32::INFINITY)
                            .desired_rows(10)
                            .hint_text(
                                "# one pattern per line, e.g.\n**/node_modules/\n**/.cache/\n*.tmp",
                            ),
                    );
                    });
                if let Some(msg) = &self.ignore_msg {
                    ui.add_space(4.0);
                    ui.label(egui::RichText::new(msg).size(11.5).color(t.dim));
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui
                        .button("Save & rebuild")
                        .on_hover_text("Write the file (creating it if needed) and reindex.")
                        .clicked()
                    {
                        save = true;
                    }
                    if ui.button("Reload").clicked() {
                        reload = true;
                    }
                    if ui.button("Rebuild index").clicked() {
                        rebuild = true;
                    }
                    if ui.button("Close").clicked() {
                        close = true;
                    }
                });
            });

        self.show_ignore = open && !close;
        if reload {
            self.load_ignore_text();
            self.ignore_msg = Some("Reloaded from disk.".to_string());
        }
        if let Some(on) = toggle {
            self.engine.set_respect_ignore(on);
            // Mirror it at once so the checkbox does not flicker while a remote
            // daemon's status is still in flight.
            self.status.respect_ignore_files = on;
            self.ignore_msg = Some(if on {
                "Honoring ignore files — reindexing…".to_string()
            } else {
                "Not consulting ignore files — reindexing…".to_string()
            });
        }
        if rebuild {
            self.engine.rebuild();
            self.ignore_msg = Some("Rebuilding the index…".to_string());
        }
        if save {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            match std::fs::write(&path, self.ignore_text.as_bytes()) {
                Ok(()) => {
                    self.engine.rebuild();
                    self.ignore_msg = Some(format!("Saved {} — reindexing…", path.display()));
                }
                Err(e) => {
                    self.ignore_msg = Some(format!("Cannot write {}: {e}", path.display()));
                }
            }
        }
    }

    /// Status bar: index state, live CPU/RAM, query stats, zoom and shortcuts.
    fn status_bar(&mut self, ui: &mut egui::Ui) {
        let t = self.theme();
        let (files, dirs) = self.engine.counts();
        ui.horizontal(|ui| {
            match self.status.state {
                State::Starting | State::Indexing => {
                    let a = ((ui.input(|i| i.time) * 5.0).sin() * 0.5 + 0.5) as f32;
                    status_dot(ui, t.warn.gamma_multiply(0.5 + 0.5 * a));
                    ui.label(
                        egui::RichText::new(format!("Indexing… {} files", human_count(files)))
                            .size(11.5)
                            .color(t.warn),
                    );
                }
                State::Live => {
                    status_dot(ui, t.good);
                    ui.label(
                        egui::RichText::new(format!(
                            "Indexing: idle ({} files)",
                            human_count(files)
                        ))
                        .size(11.5)
                        .color(t.dim),
                    );
                }
            }
            if self.status.degraded {
                let txt = if self.status.watch_failures > 0 {
                    format!("· {} dir(s) not realtime", self.status.watch_failures)
                } else {
                    "· periodic rebuild".to_string()
                };
                ui.label(egui::RichText::new(txt).color(t.warn).size(11.5));
            }
            if self.status.overlay_pending > 0 {
                ui.label(
                    egui::RichText::new(format!("· {} pending", self.status.overlay_pending))
                        .size(11.5)
                        .color(t.faint),
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
                    .size(11.5)
                    .color(t.faint),
                );
            }

            ui.separator();
            ui.label(
                egui::RichText::new(format!("CPU: {:.0}%", self.sys_cpu))
                    .size(11.0)
                    .color(t.faint),
            );
            ui.separator();
            ui.label(
                egui::RichText::new(format!(
                    "RAM: {} / {}",
                    human_kb(self.sys_ram_used_kb),
                    human_kb(self.sys_ram_total_kb)
                ))
                .size(11.0)
                .color(t.faint),
            );

            // UI zoom, kept on the left so a long hints string on the right can
            // never push it out of the window.
            ui.separator();
            ui.label(bar_label(&t, "Zoom"));
            for level in ZOOM_LEVELS {
                if ui
                    .selectable_label(
                        (self.prefs.zoom - *level).abs() < 0.001,
                        egui::RichText::new(format!("{:.0}%", level * 100.0)).size(10.5),
                    )
                    .clicked()
                {
                    self.prefs.zoom = *level;
                    ui.ctx().set_zoom_factor(*level);
                    self.prefs.save();
                }
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // Rightmost item of the status bar: the running version. It is
                // added first in this right-to-left layout so the (potentially
                // long) hints string can never push it out of the window.
                let app_version = env!("CARGO_PKG_VERSION");
                ui.label(
                    egui::RichText::new(format!("v{app_version}"))
                        .size(10.5)
                        .color(t.faint),
                )
                .on_hover_text("Running version — updates install in place");
                if let Some(version) = &self.update_banner {
                    ui.label(
                        egui::RichText::new(format!("⬆ v{version} available"))
                            .size(10.5)
                            .color(t.accent),
                    )
                    .on_hover_text("Open Help ▸ Check for updates");
                }
                ui.separator();
                ui.label(
                    egui::RichText::new("↑↓ · Enter open · Ctrl+F search · Ctrl+A all · Esc clear")
                        .size(10.5)
                        .color(t.faint),
                );
                ui.add_space(8.0);
                ui.separator();
                ui.label(
                    egui::RichText::new(format!("Entries: {}", human_count(files + dirs)))
                        .size(11.0)
                        .color(t.faint),
                );
                ui.separator();
                ui.label(
                    egui::RichText::new(format!("Search time: {} ms", self.elapsed_ms))
                        .size(11.0)
                        .color(t.faint),
                );
                ui.separator();
                ui.label(
                    egui::RichText::new(format!(
                        "Query: {} results",
                        human_count(self.results.len() as u64)
                    ))
                    .size(11.0)
                    .color(t.faint),
                );
            });
        });
    }

    /// Sample system CPU and RAM for the status bar (throttled to every 2 s).
    fn refresh_sys_stats(&mut self) {
        if self.sys_at.elapsed() < Duration::from_secs(2) {
            return;
        }
        self.sys_at = Instant::now();
        if let Ok(text) = std::fs::read_to_string("/proc/stat")
            && let Some(line) = text.lines().next()
        {
            let vals: Vec<u64> = line
                .split_whitespace()
                .skip(1)
                .filter_map(|v| v.parse().ok())
                .collect();
            if vals.len() >= 5 {
                let idle = vals[3] + vals.get(4).copied().unwrap_or(0);
                let total: u64 = vals.iter().sum();
                let (prev_busy, prev_total) = self.sys_prev;
                if prev_total != 0 && total > prev_total {
                    let dt = (total - prev_total) as f32;
                    let db = (total - idle).saturating_sub(prev_busy) as f32;
                    self.sys_cpu = (db / dt * 100.0).clamp(0.0, 100.0);
                }
                self.sys_prev = (total - idle, total);
            }
        }
        if let Ok(text) = std::fs::read_to_string("/proc/meminfo") {
            let mut total = 0u64;
            let mut avail = 0u64;
            for line in text.lines() {
                let value = || {
                    line.split_whitespace()
                        .nth(1)
                        .and_then(|v| v.parse::<u64>().ok())
                        .unwrap_or(0)
                };
                if line.starts_with("MemTotal:") {
                    total = value();
                } else if line.starts_with("MemAvailable:") {
                    avail = value();
                }
            }
            self.sys_ram_total_kb = total;
            self.sys_ram_used_kb = total.saturating_sub(avail);
        }
    }

    /// Apply a result-row context-menu choice. Deferred out of the table so it
    /// can take `&mut self` (and the window context, for the clipboard).
    fn apply_row_cmd(&mut self, cmd: RowCmd, ctx: &egui::Context) {
        let file_name = |p: &Path| {
            p.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| p.display().to_string())
        };
        match cmd {
            RowCmd::Open(p) => App::open(&p),
            RowCmd::Folder(p) => App::open_folder(&p),
            RowCmd::Terminal(p) => App::open_terminal(&p),
            RowCmd::CopyPaths { path, selection } => {
                let mut v: Vec<String> = if selection {
                    self.checked
                        .iter()
                        .map(|p| p.display().to_string())
                        .collect()
                } else {
                    vec![path.display().to_string()]
                };
                v.sort();
                ctx.copy_text(v.join("\n"));
            }
            RowCmd::CopyNames { path, selection } => {
                let mut v: Vec<String> = if selection {
                    self.checked.iter().map(|p| file_name(p)).collect()
                } else {
                    vec![file_name(&path)]
                };
                v.sort();
                ctx.copy_text(v.join("\n"));
            }
            RowCmd::Details(p) => {
                if let Some(i) = self.results.iter().position(|r| r.path == p) {
                    self.selected = i;
                }
                self.panel_tab = PanelTab::Details;
                if !self.prefs.show_preview {
                    self.prefs.show_preview = true;
                    self.prefs.save();
                }
                self.refresh_preview(ctx);
            }
            RowCmd::FilterTo(p) => {
                if let Some(parent) = p.parent() {
                    self.set_under(Some(parent.to_string_lossy().into_owned()));
                }
            }
            RowCmd::SearchName(p) => {
                let name = file_name(&p);
                let stem = name
                    .rsplit_once('.')
                    .map(|(s, _)| s.to_string())
                    .unwrap_or(name);
                self.run_query(&format!("*{stem}*"));
            }
            RowCmd::AddToSelection(p) => {
                self.checked.insert(p);
            }
            RowCmd::RemoveFromSelection(p) => {
                self.checked.remove(&p);
            }
            RowCmd::SelectAll => {
                self.checked = self.results.iter().map(|r| r.path.clone()).collect();
            }
            RowCmd::Invert => {
                let all: Vec<PathBuf> = self.results.iter().map(|r| r.path.clone()).collect();
                for p in all {
                    if !self.checked.remove(&p) {
                        self.checked.insert(p);
                    }
                }
            }
            RowCmd::ClearSelection => self.checked.clear(),
            RowCmd::FindDuplicates { selection } => {
                let paths: Vec<PathBuf> = if selection {
                    self.checked.iter().cloned().collect()
                } else {
                    self.results.iter().map(|r| r.path.clone()).collect()
                };
                self.start_duplicate_scan(paths);
            }
        }
    }

    /// Kick off a duplicate scan over `paths` on the worker thread.
    fn start_duplicate_scan(&mut self, paths: Vec<PathBuf>) {
        if paths.len() < 2 {
            return;
        }
        self.dup_gen += 1;
        self.dup_progress = Some((0, 0));
        self.dups = None;
        self.show_dups = true;
        let _ = self.dup_tx.send(DupRequest {
            generation: self.dup_gen,
            paths,
        });
    }

    /// The duplicate-scan window: files with identical contents, grouped.
    fn duplicates_window(&mut self, ctx: &egui::Context) {
        if !self.show_dups {
            return;
        }
        let t = self.theme();
        let mut open = true;
        let mut reveal: Option<PathBuf> = None;
        let mut copy: Option<String> = None;
        let mut select: Option<Vec<PathBuf>> = None;
        egui::Window::new("Duplicates in results")
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_width(680.0)
            .default_height(430.0)
            .show(ctx, |ui| {
                if let Some((done, total)) = self.dup_progress {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(
                            egui::RichText::new(if total == 0 {
                                "collecting candidates…".to_string()
                            } else {
                                format!("comparing {done} / {total} candidate files…")
                            })
                            .size(12.0)
                            .color(t.dim),
                        );
                    });
                    ui.add_space(4.0);
                }

                let Some(report) = &self.dups else {
                    ui.label(
                        egui::RichText::new(
                            "Right-click a result and choose “Find duplicates in results…”.",
                        )
                        .size(12.0)
                        .color(t.faint),
                    );
                    return;
                };

                ui.label(
                    egui::RichText::new(format!(
                        "{} group(s) · {} reclaimable · {} candidate file(s), {} fully hashed{}",
                        report.groups.len(),
                        human_size(report.extra_bytes),
                        report.candidates,
                        report.hashed,
                        if report.capped { " (scan capped)" } else { "" }
                    ))
                    .size(12.0)
                    .color(t.dim),
                );
                if report.groups.is_empty() {
                    ui.add_space(6.0);
                    ui.label(
                        egui::RichText::new("No duplicate contents in these results.")
                            .size(12.5)
                            .color(t.good),
                    );
                    return;
                }
                ui.add_space(6.0);

                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for g in &report.groups {
                            let wasted = g.size * (g.paths.len() as u64 - 1);
                            ui.horizontal(|ui| {
                                ui.label(
                                    egui::RichText::new(format!("{} copies", g.paths.len()))
                                        .strong()
                                        .color(t.text),
                                );
                                ui.label(
                                    egui::RichText::new(format!(
                                        "· {} each · {} wasted",
                                        human_size(g.size),
                                        human_size(wasted)
                                    ))
                                    .size(11.5)
                                    .color(t.warn),
                                );
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        if ui.small_button("Select in results").clicked() {
                                            select = Some(g.paths.clone());
                                        }
                                        if ui.small_button("Copy paths").clicked() {
                                            copy = Some(
                                                g.paths
                                                    .iter()
                                                    .map(|p| p.display().to_string())
                                                    .collect::<Vec<_>>()
                                                    .join("\n"),
                                            );
                                        }
                                    },
                                );
                            });
                            for p in &g.paths {
                                ui.horizontal(|ui| {
                                    ui.add_space(14.0);
                                    let name = p
                                        .file_name()
                                        .map(|n| n.to_string_lossy().into_owned())
                                        .unwrap_or_default();
                                    ui.label(
                                        egui::RichText::new(name)
                                            .monospace()
                                            .size(11.5)
                                            .color(t.text),
                                    );
                                    ui.label(
                                        egui::RichText::new(short_dir(p)).size(10.5).color(t.faint),
                                    );
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            if ui.small_button("Reveal").clicked() {
                                                reveal = Some(p.clone());
                                            }
                                        },
                                    )
                                    .response
                                    .on_hover_text(p.display().to_string());
                                });
                            }
                            ui.add_space(6.0);
                            ui.separator();
                        }
                    });
            });

        if let Some(p) = reveal {
            App::open_folder(&p);
        }
        if let Some(text) = copy {
            ctx.copy_text(text);
        }
        if let Some(paths) = select {
            self.checked = paths.into_iter().collect();
        }
        self.show_dups = open;
    }

    /// The view tab strip above the status bar, plus the bulk-action controls.
    fn view_tabs(&mut self, ctx: &egui::Context) {
        let t = self.theme();
        let n = self.results.len();
        egui::TopBottomPanel::bottom("viewtabs")
            .frame(
                egui::Frame::new()
                    .fill(t.panel)
                    .inner_margin(egui::Margin::symmetric(10, 4)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    for v in ViewTab::ALL {
                        if ui.selectable_label(self.view == *v, v.label()).clicked() {
                            self.view = *v;
                        }
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add_enabled(
                                !self.checked.is_empty(),
                                egui::Button::new(egui::RichText::new("Copy paths").size(11.5)),
                            )
                            .on_hover_text("Copy the selected paths to the clipboard")
                            .clicked()
                        {
                            let mut paths: Vec<String> = self
                                .results
                                .iter()
                                .filter(|r| self.checked.contains(&r.path))
                                .map(|r| r.path.display().to_string())
                                .collect();
                            paths.sort();
                            ui.ctx().copy_text(paths.join("\n"));
                        }
                        if ui
                            .add_enabled(
                                n > 0,
                                egui::Button::new(egui::RichText::new("Invert").size(11.5)),
                            )
                            .clicked()
                        {
                            let all: Vec<PathBuf> =
                                self.results.iter().map(|r| r.path.clone()).collect();
                            for p in all {
                                if !self.checked.remove(&p) {
                                    self.checked.insert(p);
                                }
                            }
                        }
                        if ui
                            .add_enabled(
                                n > 0,
                                egui::Button::new(egui::RichText::new("Select All").size(11.5)),
                            )
                            .clicked()
                        {
                            self.checked = self.results.iter().map(|r| r.path.clone()).collect();
                        }
                        ui.label(bar_label(&t, "Bulk Actions:"));
                    });
                });
            });
    }

    /// The recent-searches chip row.
    fn recent_row(&mut self, ctx: &egui::Context) {
        let t = self.theme();
        let history = self.prefs.history.clone();
        egui::TopBottomPanel::bottom("recent")
            .frame(
                egui::Frame::new()
                    .fill(t.panel)
                    .inner_margin(egui::Margin::symmetric(10, 3)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(bar_label(&t, "Recent searches:"));
                    if history.is_empty() {
                        ui.label(
                            egui::RichText::new(
                                "nothing yet — press Enter in the search box to keep one",
                            )
                            .size(11.0)
                            .color(t.faint),
                        );
                    }
                    for q in history.iter().take(8) {
                        if chip(ui, &t, q, false).clicked() {
                            self.run_query(q);
                        }
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if !history.is_empty()
                            && ui
                                .add(
                                    egui::Button::new(egui::RichText::new("Clear").size(11.0))
                                        .frame(false),
                                )
                                .clicked()
                        {
                            self.prefs.clear_history();
                            self.sync_history();
                        }
                    });
                });
            });
    }

    /// Manager for saved searches.
    fn saved_dialog(&mut self, ctx: &egui::Context) {
        let t = self.theme();
        let mut open = self.show_saved;
        let mut apply: Option<usize> = None;
        let mut delete: Option<usize> = None;
        egui::Window::new("Saved searches")
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_width(400.0)
            .show(ctx, |ui| {
                if self.prefs.saved.is_empty() {
                    ui.label(
                        egui::RichText::new("No saved searches yet.")
                            .size(12.0)
                            .color(t.dim),
                    );
                    ui.label(
                        egui::RichText::new(
                            "Set up a query and filters, then use \"Save current search…\".",
                        )
                        .size(11.0)
                        .color(t.faint),
                    );
                }
                for (i, s) in self.prefs.saved.iter().enumerate() {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(&s.name).strong().color(t.text));
                        ui.label(
                            egui::RichText::new(if s.query.is_empty() {
                                "(all files)"
                            } else {
                                s.query.as_str()
                            })
                            .size(11.0)
                            .color(t.dim),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.small_button("Delete").clicked() {
                                delete = Some(i);
                            }
                            if ui.small_button("Apply").clicked() {
                                apply = Some(i);
                            }
                        });
                    });
                }
                ui.separator();
                if ui.button("Save current search…").clicked() {
                    self.show_save = true;
                    self.save_name = self.query.clone();
                }
            });
        if let Some(i) = delete {
            self.prefs.saved.remove(i);
            self.prefs.save();
        }
        if let Some(i) = apply {
            self.apply_saved(i);
        }
        self.show_saved = open;
    }

    /// Name-and-save popup for the current query.
    fn save_search_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_save {
            return;
        }
        let t = self.theme();
        let mut save = false;
        let mut cancel = false;
        egui::Window::new("Save current search")
            .collapsible(false)
            .resizable(false)
            .default_width(340.0)
            .show(ctx, |ui| {
                ui.label(bar_label(&t, "Name"));
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut self.save_name)
                        .desired_width(f32::INFINITY)
                        .hint_text("e.g. Work invoices"),
                );
                if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    save = true;
                }
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(format!(
                        "Query: {}   |   Type: {}   |   Location: {}",
                        if self.query.is_empty() {
                            "(all files)"
                        } else {
                            self.query.as_str()
                        },
                        self.category_label(),
                        match &self.under {
                            None => "everywhere".to_string(),
                            Some(p) => location_label(p),
                        }
                    ))
                    .size(11.0)
                    .color(t.faint),
                );
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.button("Save").clicked() {
                        save = true;
                    }
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                });
            });
        if save {
            let name = {
                let n = self.save_name.trim();
                if n.is_empty() {
                    if self.query.is_empty() {
                        "All files".to_string()
                    } else {
                        self.query.clone()
                    }
                } else {
                    n.to_string()
                }
            };
            self.prefs.saved.retain(|s| s.name != name);
            self.prefs.saved.push(SavedSearch {
                name,
                query: self.query.clone(),
                content_mode: self.content_mode,
                regex_mode: self.regex_mode,
                case_sensitive: self.case_sensitive,
                under: self.under.clone(),
            });
            if self.prefs.saved.len() > 50 {
                self.prefs.saved.remove(0);
            }
            self.prefs.save();
            self.save_name.clear();
            self.show_save = false;
            self.show_saved = true;
        } else if cancel {
            self.show_save = false;
        }
    }

    /// Apply a saved search to the live query.
    fn apply_saved(&mut self, i: usize) {
        let Some(s) = self.prefs.saved.get(i).cloned() else {
            return;
        };
        self.query = s.query;
        self.content_mode = s.content_mode;
        self.regex_mode = s.regex_mode;
        self.case_sensitive = s.case_sensitive;
        if self.under != s.under {
            self.under = s.under;
            self.push_location();
        }
        self.history_idx = None;
        self.prefs.commit_query(&self.query.clone());
        self.sync_history();
        self.send_query();
    }
}

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

/// Best-effort "is this text?" test for the preview pane: no NUL bytes and only
/// a trace of other control characters. A byte near the 64 KiB cut that splits a
/// multi-byte character is fine — UTF-8 continuation bytes are >= 0x80, not
/// control codes — so truncation never turns text into "binary".
fn is_probably_text(bytes: &[u8]) -> bool {
    if bytes.is_empty() {
        return true;
    }
    let control = bytes
        .iter()
        .filter(|&&b| b == 0 || (b < 0x09) || (0x0e..0x20).contains(&b))
        .count();
    control * 100 / bytes.len() < 2
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
            Sort::Name(asc) | Sort::Size(asc) | Sort::Mtime(asc) | Sort::Relevance(asc) => {
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
            Sort::Relevance(true) => Some(Sort::Relevance(false)),
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
            | (Sort::Relevance(_), Sort::Relevance(_))
    )
}

/// A transparent 0–100 relevance score.
///
/// Terms that match the file *name* score highest (exact > prefix > substring),
/// then matches anywhere in the path; short names get a small bonus because they
/// are usually the more specific hit. A query with no positive terms (including
/// an empty one) scores everything equally.
fn relevance_score(path: &Path, needle: &str, fuzzy: bool) -> u8 {
    let terms: Vec<String> = needle
        .split_whitespace()
        .filter(|t| !t.starts_with('!'))
        .map(|t| t.trim_matches('*').to_lowercase())
        .filter(|t| !t.is_empty())
        .collect();
    if terms.is_empty() {
        return 100;
    }
    if fuzzy {
        return fuzzy_relevance(path, &terms);
    }
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let full = path.to_string_lossy().to_lowercase();
    let mut total: u32 = 0;
    for t in &terms {
        total += if name == *t {
            100
        } else if name.starts_with(t.as_str()) {
            70
        } else if name.contains(t.as_str()) {
            55
        } else if full.contains(t.as_str()) {
            25
        } else {
            0
        };
    }
    let mut score = (total / terms.len() as u32) as i32;
    if name.chars().count() <= 12 {
        score += 5;
    }
    score.clamp(0, 100) as u8
}

/// A 0–100 relevance score for fuzzy mode, from the core's [`fuzzy_score`].
///
/// Each term contributes its best score in the name (or half of it in the path),
/// normalised against the best a term of that length could score, so terms of
/// different lengths stay comparable.
fn fuzzy_relevance(path: &Path, terms: &[String]) -> u8 {
    use everything_core::matcher::fuzzy_score;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let full = path.to_string_lossy().into_owned();
    let mut total = 0.0_f32;
    for t in terms {
        let best = fuzzy_score(t, &name, false)
            .map(|s| s as f32)
            .or_else(|| fuzzy_score(t, &full, false).map(|s| s as f32 * 0.5));
        let ceiling = (24 * t.chars().count().max(1)) as f32;
        total += best.map(|v| (v / ceiling).clamp(0.0, 1.0)).unwrap_or(0.0);
    }
    ((total / terms.len() as f32) * 100.0)
        .round()
        .clamp(0.0, 100.0) as u8
}

/// Which query terms appear in this result, for the `Match` column.
fn matched_terms<'a>(path: &Path, needle: &'a str, fuzzy: bool) -> Vec<&'a str> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let full = path.to_string_lossy().into_owned();
    let name_lc = name.to_lowercase();
    let full_lc = full.to_lowercase();
    needle
        .split_whitespace()
        .filter(|t| !t.starts_with('!'))
        .filter(|t| {
            let bare = t.trim_matches('*');
            if bare.is_empty() {
                return false;
            }
            if fuzzy {
                everything_core::matcher::fuzzy_score(bare, &name, false).is_some()
                    || everything_core::matcher::fuzzy_score(bare, &full, false).is_some()
            } else {
                let bare = bare.to_lowercase();
                name_lc.contains(&bare) || full_lc.contains(&bare)
            }
        })
        .collect()
}

fn sort_results(results: &mut [ResultRow], sort: Sort, needle: &str, fuzzy: bool) {
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
        Sort::Relevance(asc) => {
            results.sort_by_cached_key(|r| {
                let s = relevance_score(&r.path, needle, fuzzy);
                if asc { s } else { 255 - s }
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_detection_distinguishes_empty_binary_and_text() {
        assert!(is_probably_text(b"")); // empty file is still "text"
        assert!(is_probably_text(b"hello\nworld\t!"));
        assert!(is_probably_text("caf\u{e9} \u{1f600}".as_bytes())); // UTF-8 multibyte
        // A 64 KiB cut through a multi-byte character must not look binary.
        let mut truncated = vec![b'a'; 65_534];
        truncated.extend_from_slice("\u{1f600}".as_bytes());
        assert!(is_probably_text(&truncated[..65_535]));
        assert!(!is_probably_text(&[0x00, 0x01, 0x02, 0x03])); // NUL → binary
        // Dense high bytes are not control codes, so this stays "text".
        assert!(is_probably_text(&[0xff; 64]));
        let mut mostly_binary = vec![0u8; 100];
        mostly_binary[0] = b'A';
        assert!(!is_probably_text(&mostly_binary));
    }

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

    #[test]
    fn selection_is_pruned_to_the_visible_results() {
        let row = |p: &str| ResultRow {
            path: PathBuf::from(p),
            size: 1,
            mtime: 0,
            is_dir: false,
        };
        let mut checked: HashSet<PathBuf> =
            ["/a", "/b", "/gone"].iter().map(PathBuf::from).collect();

        retain_visible(&mut checked, &[row("/a"), row("/b")]);
        assert_eq!(checked.len(), 2, "the stale path is dropped");
        assert!(!checked.contains(&PathBuf::from("/gone")));
        assert!(checked.contains(&PathBuf::from("/a")));

        // A new query with no matches must clear the selection outright.
        retain_visible(&mut checked, &[]);
        assert!(checked.is_empty());
    }

    #[test]
    fn duplicates_need_identical_contents() {
        let dir = std::env::temp_dir().join(format!("efl-dups-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let w = |name: &str, bytes: &[u8]| {
            let p = dir.join(name);
            std::fs::write(&p, bytes).unwrap();
            p
        };
        // Two real duplicates, a same-size near-miss, an empty file, a unique file.
        let a = w("a.txt", b"hello world");
        let b = w("b.txt", b"hello world");
        let near = w("near.txt", b"hello worlD"); // same size, one byte different
        let empty = w("empty.txt", b"");
        let unique = w("unique.txt", b"unique");

        let paths = vec![a.clone(), b.clone(), near, empty.clone(), unique];
        let mut calls = 0;
        let report = find_duplicates(&paths, &mut |_, _| calls += 1);

        assert_eq!(report.groups.len(), 1, "exactly one duplicate pair");
        let g = &report.groups[0];
        assert_eq!(g.paths, vec![a.clone(), b.clone()]);
        assert_eq!(g.size, 11);
        assert_eq!(report.extra_bytes, 11, "one redundant copy");
        assert!(
            !report.groups.iter().any(|g| g.paths.contains(&empty)),
            "empty files must not be reported: every empty file is trivially identical"
        );
        // Only the same-size files (a, b, near) are candidates.
        assert_eq!(report.candidates, 3);
        assert!(calls > 0, "progress is reported");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn duplicates_survive_a_prefix_longer_than_the_cheap_pass() {
        let dir = std::env::temp_dir().join(format!("efl-dups2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // Both share the first 100 KiB; only the tails differ, so the cheap
        // (size + 64 KiB) pass cannot tell them apart and the full hash must.
        let mut x = vec![b'x'; 100 * 1024];
        let mut y = x.clone();
        x.extend_from_slice(b"tail-A");
        y.extend_from_slice(b"tail-B");
        let px = dir.join("x.bin");
        let py = dir.join("y.bin");
        std::fs::write(&px, &x).unwrap();
        std::fs::write(&py, &y).unwrap();

        let report = find_duplicates(&[px, py], &mut |_, _| {});
        assert!(
            report.groups.is_empty(),
            "files differing past the 64 KiB prefix are not duplicates"
        );
        assert_eq!(report.candidates, 2, "but they were compared");
        assert_eq!(report.hashed, 2, "and fully hashed");
        let _ = std::fs::remove_dir_all(dir);
    }
}
