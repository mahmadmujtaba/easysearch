//! `easysearch-gui` library — native egui frontend over the core engine.
//!
//! Exposed as a library so the combined single-binary app (`easysearch`)
//! can drive the same UI; [`run`] takes an already-chosen [`Backend`].
//!
//! Layout follows the "FileSearch Pro" reference: a menu bar and a labelled
//! toolbar, a search row (query + scope + go), a filter bar, a results header
//! with sorting and density, then three panes — a sidebar (categories, saved
//! searches, locations, advanced options), a virtualized results table, and a
//! preview/details panel — over a view tab strip, recent searches and a live
//! status bar. Tokyo Night palette; Wayland-first windowing.

use easysearch_core::logo;
use easysearch_core::proto::Event;
use easysearch_core::{
    Backend, Category, ContentIndexStatus, Query, ResultRow, SearchResponse, State, Status,
    TagStore,
};
use eframe::egui;
use egui_extras::{Column, TableBuilder};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// System tray (StatusNotifierItem). Lives in the **background host** process,
/// not the window, so the icon outlives any single window.
pub mod tray;

const DEBOUNCE_MS: u128 = 120;

/// The query cheat-sheet, shown on hover over the search bar (it used to be a
/// sidebar section).
const SEARCH_TIPS: &str = "Queries:  *.pdf glob  ·  a b all terms  ·  !draft exclude  ·  \
^src/ path prefix  ·  .* regex mode";

/// Content search (ripgrep over whole files) is held back until the pattern has
/// at least this many characters: a one- or two-letter pattern would scan the
/// whole disk for almost no signal.
const CONTENT_MIN_CHARS: usize = 3;
const HISTORY_CAP: usize = 20;
/// Reverse-DNS application id. Kept in sync with the packaging assets in
/// `packaging/` (desktop entry, AppStream metainfo, Flatpak manifest) so the
/// window, the launcher entry and the icon all agree.
const APP_ID: &str = "io.github.easysearch.EasySearch";
/// The product name without a version. Used where the build stamp would be noise
/// (the tray), so the version only ever appears in the app window.
pub const APP_NAME: &str = "EasySearch";
/// Selectable UI zoom levels (1.0 = 100%).
const ZOOM_LEVELS: &[f32] = &[0.95, 1.0, 1.1, 1.25];

/// The colour scheme the user picked, stored by name in `gui.json`.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
enum ThemeChoice {
    /// The original Tokyo-Night-style dark palette.
    #[default]
    Dark,
    Light,
    /// Drawn from the logo: teal ground, teal accent, pink and lime highlights.
    Brand,
}

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

/// Colours for the preview's code highlighter. Each is a role the theme already
/// defines, so a highlighted preview always matches the active appearance
/// (comment and string use the muted and “good” tones, keywords and calls the
/// archive/avenue accents, and so on).
struct SyntaxColors {
    text: egui::Color32,
    comment: egui::Color32,
    string: egui::Color32,
    keyword: egui::Color32,
    number: egui::Color32,
    type_: egui::Color32,
    func: egui::Color32,
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
        faint: egui::Color32::from_rgb(0x6b, 0x74, 0x88),
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

    /// The brand palette, taken from the logo: a deep teal ground, the mark's
    /// teal as the accent, its pink for errors and archives, and its lime for
    /// “good”. Dark, so every surface is tuned for a dark ground (see
    /// [`accent_soft`](Theme::accent_soft) and [`shadow`](Theme::shadow)).
    const BRAND: Theme = Theme {
        dark: true,
        bg: egui::Color32::from_rgb(0x0e, 0x15, 0x17),
        panel: egui::Color32::from_rgb(0x12, 0x20, 0x1f),
        card: egui::Color32::from_rgb(0x18, 0x30, 0x2e),
        hover: egui::Color32::from_rgb(0x1f, 0x3d, 0x3a),
        active: egui::Color32::from_rgb(0x27, 0x50, 0x49),
        stripe: egui::Color32::from_rgb(0x14, 0x26, 0x25),
        stroke: egui::Color32::from_rgb(0x24, 0x44, 0x40),
        text: egui::Color32::from_rgb(0xea, 0xf3, 0xf0),
        dim: egui::Color32::from_rgb(0xa2, 0xba, 0xb5),
        faint: egui::Color32::from_rgb(0x6e, 0x88, 0x83),
        accent: egui::Color32::from_rgb(0x4e, 0xc5, 0xbe),
        good: egui::Color32::from_rgb(0xb9, 0xd6, 0x4e),
        warn: egui::Color32::from_rgb(0xe4, 0xb3, 0x63),
        bad: egui::Color32::from_rgb(0xf4, 0x68, 0x91),
        kind_dir: egui::Color32::from_rgb(0x4e, 0xc5, 0xbe),
        kind_img: egui::Color32::from_rgb(0x74, 0xd3, 0xd3),
        kind_doc: egui::Color32::from_rgb(0xe4, 0xb3, 0x63),
        kind_code: egui::Color32::from_rgb(0xb5, 0x8a, 0xc9),
        kind_arch: egui::Color32::from_rgb(0xf4, 0x68, 0x91),
        kind_av: egui::Color32::from_rgb(0xd9, 0x8c, 0xb3),
    };

    fn of(choice: ThemeChoice) -> Theme {
        match choice {
            ThemeChoice::Dark => Self::DARK,
            ThemeChoice::Light => Self::LIGHT,
            ThemeChoice::Brand => Self::BRAND,
        }
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

    fn syntax(&self) -> SyntaxColors {
        SyntaxColors {
            text: self.text,
            comment: self.faint,
            string: self.good,
            keyword: self.kind_av,
            number: self.kind_arch,
            type_: self.kind_img,
            func: self.accent,
        }
    }
}

const RECENT_AGE_SECS: i64 = 7 * 24 * 3600;
const LARGE_MIN_BYTES: u64 = 1024 * 1024 * 1024; // 1 GiB
/// Largest file we will hash for the Details tab (a hash of a huge file would
/// stall the UI thread).
const MAX_HASH_BYTES: u64 = 512 * 1024 * 1024;

/// How many visible rows “Find files by hash” will hash before stopping.
const HASH_SCAN_LIMIT: usize = 2000;

/// How much of a text file the preview reads. Larger files show this head, with
/// a note, rather than being read whole.
const TEXT_PREVIEW_BYTES: usize = 256 * 1024;

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

/// The glyph that stands for a sidebar category.
fn category_icon(cat: &Category) -> Icon {
    match cat {
        Category::All => Icon::CatAll,
        Category::Recent { .. } => Icon::Clock,
        Category::Images => Icon::CatImage,
        Category::Docs => Icon::CatDoc,
        Category::Code => Icon::CatCode,
        Category::Archives => Icon::CatArchive,
        Category::Audio => Icon::CatAudio,
        Category::Video => Icon::CatVideo,
        Category::Large { .. } => Icon::CatLarge,
    }
}

/// The glyph that stands for a file, by its extension (a folder for folders).
fn file_icon(path: &Path, is_dir: bool) -> Icon {
    if is_dir {
        return Icon::Folder;
    }
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase());
    match ext.as_deref() {
        Some("png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "bmp" | "tiff" | "ico" | "avif") => {
            Icon::CatImage
        }
        Some("mp3" | "wav" | "flac" | "ogg" | "m4a" | "aac" | "opus") => Icon::CatAudio,
        Some("mp4" | "mkv" | "avi" | "mov" | "webm" | "flv" | "mpg" | "mpeg" | "wmv") => {
            Icon::CatVideo
        }
        Some(
            "zip" | "tar" | "gz" | "xz" | "bz2" | "7z" | "rar" | "zst" | "deb" | "rpm" | "jar",
        ) => Icon::CatArchive,
        Some(
            "pdf" | "doc" | "docx" | "odt" | "rtf" | "xls" | "xlsx" | "csv" | "ods" | "ppt"
            | "pptx" | "odp" | "txt" | "md" | "log",
        ) => Icon::CatDoc,
        Some(
            "rs" | "py" | "js" | "ts" | "tsx" | "jsx" | "go" | "c" | "cpp" | "h" | "hpp" | "java"
            | "rb" | "sh" | "toml" | "json" | "yaml" | "yml" | "html" | "css" | "sql" | "php"
            | "lua" | "zig" | "ex" | "exs" | "kt" | "swift" | "xml" | "ini" | "conf",
        ) => Icon::CatCode,
        _ => Icon::File,
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
    /// With `content_mode`: match the name *or* the content (the Full text scope).
    full_text: bool,
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
            full_text: false,
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
            // The scope is **not** restored: filename search is the default, and
            // content search (ripgrep over whole files) is an explicit choice made
            // in the session. The rest of the tab is restored as it was.
            content_mode: false,
            full_text: false,
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
            full_text: self.full_text,
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
    /// `#[serde(default)]` keeps older `gui.json` files loadable.
    #[serde(default)]
    full_text: bool,
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
    /// The sidebar's quick locations, counted with `under` set to each.
    locations: Vec<PathBuf>,
    /// Identity of the query these counts describe; stale replies are dropped.
    key: String,
}

/// Per-category counts for one query, in [`CATEGORIES`] order, plus one count per
/// requested location (same order).
struct Counts {
    key: String,
    per_category: Vec<u64>,
    per_location: Vec<u64>,
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

/// Run one window against `backend`, returning when it closes.
///
/// `events` carries commands pushed by the host (show / hide / quit / search);
/// `quit` is set when the host asked for a full quit rather than a hide. The
/// window owns no engine of its own: closing it just ends this process, and the
/// host (with the tray and the index) keeps running.
///
/// `initial_query` runs as soon as the window opens (a relaunch driven by
/// `--search`, or the tray's recent-searches menu).
pub fn run_with(
    backend: Arc<Backend>,
    initial_query: Option<String>,
    events: mpsc::Receiver<Event>,
    quit: Arc<AtomicBool>,
) -> eframe::Result {
    let mut viewport = egui::ViewportBuilder::default()
        .with_title(app_title())
        // Must match the installed desktop entry / icon name so Wayland
        // compositors associate the window with it (and show the icon).
        .with_app_id(APP_ID)
        .with_inner_size([1240.0, 760.0])
        .with_min_inner_size([640.0, 400.0]);
    // X11 — and Wayland sessions whose compositor does not resolve the app id —
    // take the window icon from here, so the mark is compiled in rather than
    // read from the icon theme.
    viewport = viewport.with_icon(window_icon());
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    eframe::run_native(
        &app_title(),
        options,
        Box::new(move |cc| Ok(Box::new(App::new(cc, backend, initial_query, events, quit)))),
    )
}

/// The version and build stamp, e.g. `v0.46.0-20260927`.
///
/// `BUILD_STAMP` is set by `build.rs` (`YYYYMMDD`, the build machine's UTC
/// date); the version comes from the crate manifest. Both are shown in the
/// window title, the status bar and the About dialog, so a screenshot or a bug
/// report identifies exactly which build it is.
pub fn version_stamp() -> String {
    format!(
        "v{}-{}",
        env!("CARGO_PKG_VERSION"),
        option_env!("BUILD_STAMP").unwrap_or("00000000")
    )
}

/// The window (and app) title: the name and the version/build stamp.
pub fn app_title() -> String {
    format!("{APP_NAME} {}", version_stamp())
}

/// The standalone `easysearch-gui` development binary: one window, no host.
/// Closing it exits (there is no tray or background process behind it).
pub fn run(backend: Arc<Backend>) -> eframe::Result {
    let (_tx, events) = mpsc::channel();
    run_with(backend, None, events, Arc::new(AtomicBool::new(false)))
}

/// The persisted recent searches (`gui.json`), for the host's tray menu.
pub fn recent_searches() -> Vec<String> {
    GuiPrefs::load().history
}

/// Empty the recent-search history (the tray's *Clear history*).
pub fn clear_recent_searches() {
    let mut prefs = GuiPrefs::load();
    prefs.clear_history();
}

/// The logo rendered for the window icon.
fn window_icon() -> egui::IconData {
    const SIZE: u32 = 256;
    egui::IconData {
        rgba: logo::rgba(SIZE),
        width: SIZE,
        height: SIZE,
    }
}

/// Choose the search backend for the standalone `easysearch-gui` binary.
///
/// The engine runs in this process. The standalone GUI is a development
/// convenience — prefer the combined `easysearch` app, which spawns the engine as
/// its child, so a closed window leaves the index running and the tray available.
pub fn select_backend() -> Arc<Backend> {
    Arc::new(Backend::local(easysearch_core::Config::load()))
}

/// Default for [`GuiPrefs::theme`].
fn default_theme() -> ThemeChoice {
    ThemeChoice::Dark
}

/// Read the theme leniently: a name (`"dark"`, `"light"`, `"brand"`), or the
/// older boolean the field used to be (`"dark": true|false`, and `null` — the
/// pre-0.33 "follow system", which now means Dark), so an existing `gui.json`
/// keeps loading instead of being reset.
fn theme_from_json<'de, D>(de: D) -> Result<ThemeChoice, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::Error as _;
    match Option::<serde_json::Value>::deserialize(de)? {
        None | Some(serde_json::Value::Null) => Ok(ThemeChoice::Dark),
        Some(serde_json::Value::Bool(true)) => Ok(ThemeChoice::Dark),
        Some(serde_json::Value::Bool(false)) => Ok(ThemeChoice::Light),
        Some(serde_json::Value::String(name)) => match name.to_ascii_lowercase().as_str() {
            "dark" => Ok(ThemeChoice::Dark),
            "light" => Ok(ThemeChoice::Light),
            "brand" => Ok(ThemeChoice::Brand),
            other => Err(D::Error::custom(format!("unknown theme {other:?}"))),
        },
        // Anything else (a number, an object) falls back to the default rather
        // than discarding every other preference in the file.
        Some(_) => Ok(ThemeChoice::Dark),
    }
}

/// Persistent GUI preferences (`~/.config/easysearch/gui.json`).
#[derive(Serialize, Deserialize)]
#[serde(default)]
struct GuiPrefs {
    /// Colour scheme: **always an explicit choice**, Dark by default.
    ///
    /// Stored by name. The field used to be a boolean called `dark`, and before
    /// that an optional one where `null` meant "follow the system"; both are read
    /// back ([`theme_from_json`]), so an old `gui.json` keeps working.
    #[serde(
        default = "default_theme",
        alias = "dark",
        deserialize_with = "theme_from_json"
    )]
    theme: ThemeChoice,
    /// Preview pane on by default (it can be turned off in Settings/View).
    show_preview: bool,
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
    /// Fuzzy (fzf-style) filename matching. A global mode (not per-tab).
    fuzzy: bool,
    /// Let the content pattern span lines (content search only). Slower.
    multiline: bool,
    /// Whether the “content search uses more memory” warning has been shown.
    content_warning_seen: bool,
}

impl Default for GuiPrefs {
    fn default() -> Self {
        GuiPrefs {
            theme: ThemeChoice::Dark,
            show_preview: true,
            history: Vec::new(),
            zoom: 1.0,
            include_dirs: true,
            tabs: Vec::new(),
            active_tab: 0,
            sidebar_tips: true,
            compact_rows: false,
            saved: Vec::new(),
            fuzzy: false,
            multiline: false,
            content_warning_seen: false,
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
        base.join("easysearch").join("gui.json")
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

#[derive(Clone, Copy, PartialEq, Debug)]
enum Sort {
    Name(bool),
    Size(bool),
    Mtime(bool),
    Created(bool),
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

/// How the query text is matched — the toolbar's Simple / Regex / Fuzzy radio.
/// The three are mutually exclusive; `Simple` is plain text and globs.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum MatchMode {
    #[default]
    Simple,
    Regex,
    Fuzzy,
}

/// Output format for “Export results…”.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum ExportFormat {
    #[default]
    Csv,
    Tsv,
    Json,
}

impl ExportFormat {
    fn label(self) -> &'static str {
        match self {
            ExportFormat::Csv => "CSV",
            ExportFormat::Tsv => "TSV",
            ExportFormat::Json => "JSON",
        }
    }

    fn ext(self) -> &'static str {
        match self {
            ExportFormat::Csv => "csv",
            ExportFormat::Tsv => "tsv",
            ExportFormat::Json => "json",
        }
    }
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
    /// Render `text` with light Markdown styling.
    markdown: bool,
    /// The language to highlight `text` with, when it is source code.
    lang: Option<Lang>,
    /// The text is the head of a larger file.
    truncated: bool,
    /// Metadata rows for audio/video (key, value).
    media: Vec<(String, String)>,
    /// A short note shown instead of a body (why there is no preview).
    note: Option<String>,
}

/// Which part of a file the query is matched against (the search-row scope).
#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum Scope {
    #[default]
    Filenames,
    FullPath,
    Contents,
    /// The query is matched against the file name **or** its contents.
    FullText,
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
    /// With `content_mode`: the query matches the name *or* the contents.
    full_text: bool,
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
    /// Text in the results header's “Filter results…” box: narrows the visible
    /// rows without re-querying (`all_results` keeps the full set).
    result_filter: String,
    /// Client-side quick filters over the visible rows.
    filter_empty: bool,
    filter_broken: bool,
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
    theme: ThemeChoice,
    /// The logo, rasterised on first use (see [`App::logo`]).
    logo_tex: Option<egui::TextureHandle>,
    history_idx: Option<usize>,
    search_was_focused: bool,
    /// Screen rect of the search field. A press outside it — or moving the
    /// pointer into the results — drops the field's keyboard focus (focus
    /// follows the pointer); typing re-grabs it.
    search_rect: Option<egui::Rect>,
    /// Screen rect of the central results area, so a pointer moving into it can
    /// take the keyboard focus off the search field.
    results_rect: Option<egui::Rect>,
    ui_font: Option<Vec<u8>>,
    mono_font: Option<Vec<u8>>,
    /// Events pushed by the host process (tray, `--toggle`, `--quit`, `--search`).
    events: mpsc::Receiver<Event>,
    /// Set when a real Quit arrives: the window closes the whole app, not just
    /// itself. A plain hide leaves it false, and the host keeps running.
    quit: Arc<AtomicBool>,
    show_about: bool,
    show_settings: bool,
    show_shortcuts: bool,
    /// The “Export results…” popup.
    show_export: bool,
    export_path: String,
    export_format: ExportFormat,
    export_error: Option<String>,
    /// The “Find files by hash” popup.
    show_hash: bool,
    hash_query: String,
    hash_matches: Vec<PathBuf>,
    hash_scanned: usize,
    hash_ran: bool,
    /// The “Rename” dialog: which row is being renamed and the text being typed.
    rename_target: Option<PathBuf>,
    rename_value: String,
    rename_error: Option<String>,
    /// A content search is held back because the pattern is shorter than
    /// [`CONTENT_MIN_CHARS`].
    content_blocked: bool,
    /// The one-time “content search uses more memory” notice.
    show_content_warning: bool,
    /// Snapshot state of every tab; the active one is mirrored in the fields
    /// above and refreshed via [`App::snapshot`] before a switch or a save.
    tabs: Vec<TabState>,
    active_tab: usize,
    /// Set when a tab's state changed; persisted by a throttled background save.
    dirty: bool,
    last_save: Instant,
    /// Per-category result counts for the sidebar facets (see `CountRequest`).
    counts: Vec<u64>,
    /// One count per sidebar location (see [`locations`]), same order.
    location_counts: Vec<u64>,
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
    // --- trash -----------------------------------------------------------
    /// Files checked in the duplicates window, awaiting a move to the trash.
    dup_checked: HashSet<PathBuf>,
    /// Files awaiting confirmation before a move to the trash.
    trash_confirm: Vec<PathBuf>,
    /// Outcome of the last move-to-trash, shown briefly.
    trash_msg: Option<String>,
    // --- ignore files -----------------------------------------------------
    /// The “Ignore files” window is open.
    show_ignore: bool,
    /// Edit buffer for the global ignore file.
    ignore_text: String,
    /// Transient status line shown inside the ignore window.
    ignore_msg: Option<String>,
    // --- excluded folders -------------------------------------------------
    /// The “Excluded folders” window is open.
    show_excludes: bool,
    /// Edit buffer: one directory per line (mirrors `config.exclude_dirs`).
    exclude_text: String,
    /// Transient status line shown inside the excluded-folders window.
    exclude_msg: Option<String>,
    /// Row context-menu actions, applied after the panels are drawn.
    pending_cmds: Vec<RowCmd>,
    /// Human label for the search backend ("in-process" or "engine pid N").
    backend_label: String,
    // --- tags -------------------------------------------------------------
    /// Per-path tags, loaded from `~/.config/easysearch/tags.json`.
    tags: TagStore,
    /// The raw engine results; `results` is this after the active tag filter.
    all_results: Vec<ResultRow>,
    /// When set, only results carrying this tag are shown.
    tag_filter: Option<String>,
    /// The tag editor window is open.
    show_tags: bool,
    /// New-tag text in the editor.
    tag_input: String,
    /// Paths the tag editor acts on (the selection, or a single row).
    tag_targets: Vec<PathBuf>,
}

impl App {
    fn new(
        cc: &eframe::CreationContext<'_>,
        backend: Arc<Backend>,
        initial_query: Option<String>,
        events: mpsc::Receiver<Event>,
        quit: Arc<AtomicBool>,
    ) -> App {
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
        let theme = prefs.theme;
        let (ui_font, mono_font) = load_system_fonts();
        let status_snapshot = backend.status_snapshot();
        let backend_label = backend.label();

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
                        // One count per sidebar location: the same query
                        // restricted to that directory.
                        let mut per_location = Vec::with_capacity(req.locations.len());
                        for loc in &req.locations {
                            let mut q = req.base.clone();
                            q.under = Some(loc.to_string_lossy().into_owned());
                            q.limit = 1;
                            per_location.push(backend.count(&q).unwrap_or(0));
                        }
                        if counts_tx
                            .send(Counts {
                                key: req.key,
                                per_category,
                                per_location,
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

        let tags = TagStore::load(TagStore::default_path());

        let mut app = App {
            engine: backend,
            prefs,
            query: start.query,
            regex_mode: start.regex_mode,
            content_mode: start.content_mode,
            full_text: start.full_text,
            case_sensitive: start.case_sensitive,
            hidden: start.hidden,
            full_path: start.full_path,
            category: start.category,
            under: start.under,
            size: start.size,
            modified: start.modified,
            extensions: start.extensions,
            sidebar_filter: String::new(),
            result_filter: String::new(),
            filter_empty: false,
            filter_broken: false,
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
            theme,
            logo_tex: None,
            history_idx: None,
            search_was_focused: false,
            search_rect: None,
            results_rect: None,
            ui_font,
            mono_font,
            events,
            quit,
            show_about: false,
            show_settings: false,
            show_shortcuts: false,
            show_export: false,
            export_path: String::new(),
            export_format: ExportFormat::default(),
            export_error: None,
            show_hash: false,
            hash_query: String::new(),
            hash_matches: Vec::new(),
            hash_scanned: 0,
            hash_ran: false,
            rename_target: None,
            rename_value: String::new(),
            rename_error: None,
            content_blocked: false,
            show_content_warning: false,
            tabs,
            active_tab,
            dirty: false,
            last_save: Instant::now(),
            backend_label,
            counts: Vec::new(),
            location_counts: Vec::new(),
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
            dup_checked: HashSet::new(),
            trash_confirm: Vec::new(),
            trash_msg: None,
            show_ignore: false,
            ignore_text: String::new(),
            ignore_msg: None,
            show_excludes: false,
            exclude_text: String::new(),
            exclude_msg: None,
            pending_cmds: Vec::new(),
            tags,
            all_results: Vec::new(),
            tag_filter: None,
            show_tags: false,
            tag_input: String::new(),
            tag_targets: Vec::new(),
        };
        app.apply_style(&cc.egui_ctx);
        cc.egui_ctx.set_zoom_factor(app.prefs.zoom);
        if let Some(q) = initial_query {
            let q = q.trim().to_string();
            if !q.is_empty() {
                app.run_query(&q);
                app.prefs.commit_query(&q);
            }
        }
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
        // A visible border on every state and a colour shift on hover/press, so
        // buttons read as buttons and the accent carries the interaction.
        set(&mut v.widgets.inactive, t.card, t.card, t.text, t.stroke);
        set(
            &mut v.widgets.hovered,
            t.accent_soft(),
            t.accent_soft(),
            t.accent,
            t.accent,
        );
        set(&mut v.widgets.active, t.accent, t.accent, t.bg, t.accent);
        set(&mut v.widgets.open, t.active, t.active, t.accent, t.accent);
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
        style.spacing.item_spacing = egui::vec2(6.0, 4.0);
        style.spacing.button_padding = egui::vec2(9.0, 4.0);
        style.spacing.interact_size = egui::vec2(24.0, 22.0);
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
        Theme::of(self.theme)
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
            full_text: self.full_text,
            case_sensitive: self.case_sensitive,
            hidden: self.hidden,
            full_path: self.full_path,
            category: self.category,
            under: self.under.clone(),
            size: self.size,
            modified: self.modified,
            extensions: self.extensions.clone(),
            results: self.all_results.clone(),
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
        self.full_text = t.full_text;
        self.case_sensitive = t.case_sensitive;
        self.hidden = t.hidden;
        self.full_path = t.full_path;
        self.category = t.category;
        self.under = t.under;
        self.size = t.size;
        self.modified = t.modified;
        self.extensions = t.extensions;
        self.all_results = t.results;
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
        self.apply_tag_filter();
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
        // In the *Contents* scope the query text is the content pattern and the
        // name is empty; in *Full text* it is both, matched as alternatives.
        //
        // A content scan reads whole files, so it is held back until the pattern
        // is long enough to be worth it (see [`CONTENT_MIN_CHARS`]).
        let pattern = self.query.trim();
        let content_ready = pattern.chars().count() >= CONTENT_MIN_CHARS;
        self.content_blocked = self.content_mode && !pattern.is_empty() && !content_ready;
        let content = if self.content_mode && content_ready && !pattern.is_empty() {
            Some(self.query.clone())
        } else {
            None
        };
        // *Contents* alone has no name query, so with the content part held back
        // there is nothing to run: empty the table and keep the hint up.
        if self.content_mode && !self.full_text && content.is_none() {
            self.results.clear();
            self.all_results.clear();
            self.checked.clear();
            self.truncated = false;
            self.error = None;
            self.elapsed_ms = 0;
            self.selected = 0;
            self.pending = false;
            self.last_sent = self.query.clone();
            self.dirty = true;
            return;
        }
        let name = if self.content_mode && !self.full_text {
            String::new()
        } else {
            self.query.clone()
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
            multiline: self.prefs.multiline,
            content_or_name: self.full_text,
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
            sort_results(&mut self.all_results, s, &needle, self.prefs.fuzzy);
            self.apply_tag_filter();
        }
    }

    /// Run a query (from the command line, a saved search or the tray).
    fn run_query(&mut self, q: &str) {
        let q = q.trim().to_string();
        if q.is_empty() {
            return;
        }
        self.query = q;
        self.last_edit = Instant::now();
        self.history_idx = None;
        self.prefs.commit_query(&self.query);
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
        let path = row.path.clone();
        let mut preview = Preview {
            path: path.clone(),
            is_dir: row.is_dir,
            size: row.size,
            mtime: row.mtime,
            text: String::new(),
            binary: false,
            image: None,
            markdown: false,
            lang: None,
            truncated: false,
            media: Vec::new(),
            note: None,
        };

        if !row.is_dir {
            if is_image_file(&path) {
                preview.image = load_image_texture(ctx, &path);
                if preview.image.is_none() {
                    preview.note = Some("This image could not be decoded.".to_string());
                }
            } else if is_av_file(&path) {
                // Audio and video: what the file is, best effort (ffprobe may not
                // be installed), plus a frame of video if ffmpeg can grab one.
                preview.media = media_metadata(&path);
                if is_video_file(&path) {
                    preview.image = video_thumbnail(ctx, &path);
                }
                if preview.media.is_empty() {
                    preview.note = Some(
                        "No metadata — install ffmpeg (ffprobe) for media details.".to_string(),
                    );
                }
            } else if is_document_file(&path) {
                // PDF / docx / odt: the text layer, extracted in-process (the same
                // reader content search uses — no external tool).
                match easysearch_core::content_index::extract_text(&path, 8 * 1024 * 1024) {
                    Some(text) if !text.trim().is_empty() => {
                        let (head, truncated) = head_of(&text, TEXT_PREVIEW_BYTES);
                        preview.text = head.to_string();
                        preview.truncated = truncated;
                    }
                    _ => {
                        preview.note = Some(
                            "No extractable text (a scanned page or an empty document)."
                                .to_string(),
                        );
                    }
                }
            } else if is_spreadsheet_or_slides(&path) {
                preview.note = Some(
                    "Spreadsheets and slides have no in-app preview yet — open it with ⧉."
                        .to_string(),
                );
            } else {
                match read_text_head(&path, TEXT_PREVIEW_BYTES) {
                    TextHead::Text { text, truncated } => {
                        preview.text = text;
                        preview.truncated = truncated;
                        preview.markdown = is_markdown_file(&path);
                        preview.lang = if preview.markdown {
                            None
                        } else {
                            lang_for_path(&path)
                        };
                    }
                    TextHead::Binary => preview.binary = true,
                    TextHead::Empty => {}
                    TextHead::Unreadable => {
                        preview.note = Some("This file could not be read.".to_string());
                    }
                }
            }
        }
        self.preview = Some(preview);
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

/// Bring the window to the front. The window only exists while it is shown (the
/// host opens one and closes it on hide), so this just asks for focus.
fn show_window(ctx: &egui::Context) {
    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
}

/// Pop a desktop notification that the session's notification service dismisses
/// after five seconds — the OS-level replacement for the old in-app dialogs.
///
/// Best effort, like the other external helpers: it shells out to `notify-send`
/// (libnotify), which is present on essentially every Linux desktop. If it is
/// missing nothing appears, but the action it reports has already been done, so
/// a silent failure is fine. Output is discarded: this process talks to its host
/// over stdout, which must not be polluted.
fn notify_desktop(summary: &str, body: &str) {
    let hint = format!("string:desktop-entry:{APP_ID}");
    let _ = Command::new("notify-send")
        .args([
            "-a",
            APP_ID,
            "-h",
            hint.as_str(),
            "-t",
            "5000",
            summary,
            body,
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}

/// Icons painted by hand (no icon font, no emoji) so they look identical in
/// every theme and on every desktop.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Icon {
    Back,
    Forward,
    Home,
    /// A database cylinder (the “rebuild the index” button).
    Database,
    Content,
    Clock,
    Bookmark,
    Terminal,
    Reveal,
    Open,
    // Sidebar categories and result types — drawn as coloured glyphs rather
    // than coloured dots, so the column reads as icons (see `paint_icon`).
    Folder,
    File,
    CatAll,
    CatImage,
    CatDoc,
    CatCode,
    CatArchive,
    CatAudio,
    CatVideo,
    CatLarge,
    /// A result tag (the sidebar's TAGS section and tag chips).
    Tag,
    /// The menu-bar quick actions.
    Broom,
    Reset,
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
        Icon::Database => {
            // A database cylinder: a top rim, two sides and the bottom rim.
            painter.add(egui::Shape::ellipse_stroke(
                p(0.0, -0.5),
                egui::vec2(0.75 * s, 0.28 * s),
                stroke,
            ));
            painter.add(egui::Shape::ellipse_stroke(
                p(0.0, 0.55),
                egui::vec2(0.75 * s, 0.28 * s),
                stroke,
            ));
            painter.line_segment([p(-0.75, -0.5), p(-0.75, 0.55)], stroke);
            painter.line_segment([p(0.75, -0.5), p(0.75, 0.55)], stroke);
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
        // --- sidebar / result glyphs -------------------------------------
        Icon::Folder => {
            // Tab + body, filled (so it reads as an icon at 14 px).
            painter.rect_filled(
                egui::Rect::from_min_max(p(-0.9, -0.6), p(-0.15, -0.1)),
                egui::CornerRadius::same(1),
                color,
            );
            painter.rect_filled(
                egui::Rect::from_min_max(p(-0.9, -0.25), p(0.9, 0.8)),
                egui::CornerRadius::same(2),
                color,
            );
        }
        Icon::File => {
            painter.rect_stroke(
                egui::Rect::from_min_max(p(-0.65, -0.9), p(0.65, 0.9)),
                egui::CornerRadius::same(2),
                stroke,
                egui::StrokeKind::Inside,
            );
            painter.line_segment([p(0.2, -0.9), p(0.65, -0.45)], stroke);
        }
        Icon::CatAll => {
            for (dx, dy) in [(-0.8, -0.8), (0.05, -0.8), (-0.8, 0.05), (0.05, 0.05)] {
                painter.rect_filled(
                    egui::Rect::from_min_max(p(dx, dy), p(dx + 0.75, dy + 0.75)),
                    egui::CornerRadius::same(2),
                    color,
                );
            }
        }
        Icon::CatImage => {
            painter.rect_stroke(
                egui::Rect::from_min_max(p(-0.85, -0.7), p(0.85, 0.7)),
                egui::CornerRadius::same(2),
                stroke,
                egui::StrokeKind::Inside,
            );
            painter.circle_filled(p(-0.38, -0.32), s * 0.15, color);
            painter.add(egui::Shape::convex_polygon(
                vec![p(-0.65, 0.55), p(-0.05, -0.02), p(0.35, 0.55)],
                color,
                egui::Stroke::NONE,
            ));
            painter.add(egui::Shape::convex_polygon(
                vec![p(0.05, 0.55), p(0.45, 0.18), p(0.75, 0.55)],
                color,
                egui::Stroke::NONE,
            ));
        }
        Icon::CatDoc => {
            painter.rect_stroke(
                egui::Rect::from_min_max(p(-0.65, -0.9), p(0.65, 0.9)),
                egui::CornerRadius::same(2),
                stroke,
                egui::StrokeKind::Inside,
            );
            for i in 0..3 {
                let y = -0.45 + i as f32 * 0.42;
                painter.line_segment([p(-0.4, y), p(0.4, y)], stroke);
            }
        }
        Icon::CatCode => {
            painter.line_segment([p(-0.15, -0.5), p(-0.7, 0.0)], stroke);
            painter.line_segment([p(-0.7, 0.0), p(-0.15, 0.5)], stroke);
            painter.line_segment([p(0.15, -0.5), p(0.7, 0.0)], stroke);
            painter.line_segment([p(0.7, 0.0), p(0.15, 0.5)], stroke);
            painter.line_segment([p(0.35, -0.75), p(-0.35, 0.75)], stroke);
        }
        Icon::Tag => {
            // A luggage-tag outline with its punch hole.
            painter.rect_stroke(
                egui::Rect::from_min_max(p(-0.85, -0.55), p(0.85, 0.55)),
                egui::CornerRadius::same(3),
                stroke,
                egui::StrokeKind::Inside,
            );
            painter.circle_filled(p(-0.45, 0.0), s * 0.18, color);
        }
        Icon::CatArchive => {
            painter.rect_stroke(
                egui::Rect::from_min_max(p(-0.85, -0.8), p(0.85, 0.8)),
                egui::CornerRadius::same(2),
                stroke,
                egui::StrokeKind::Inside,
            );
            painter.line_segment([p(-0.85, -0.3), p(0.85, -0.3)], stroke);
            painter.line_segment([p(-0.25, 0.05), p(0.25, 0.05)], stroke);
        }
        Icon::CatAudio => {
            // A quaver: filled head, stem, flag.
            painter.circle_filled(p(-0.35, 0.5), s * 0.3, color);
            painter.line_segment(
                [p(-0.05, 0.5), p(-0.05, -0.75)],
                egui::Stroke::new(2.0_f32, color),
            );
            painter.line_segment(
                [p(-0.05, -0.75), p(0.5, -0.45)],
                egui::Stroke::new(2.0_f32, color),
            );
        }
        Icon::CatVideo => {
            painter.rect_filled(
                egui::Rect::from_min_max(p(-0.9, -0.6), p(0.9, 0.6)),
                egui::CornerRadius::same(2),
                color,
            );
            painter.add(egui::Shape::convex_polygon(
                vec![p(-0.2, -0.32), p(-0.2, 0.32), p(0.4, 0.0)],
                egui::Color32::from_rgb(0x10, 0x14, 0x18),
                egui::Stroke::NONE,
            ));
        }
        Icon::CatLarge => {
            for i in 0..3 {
                let h = 0.4 + i as f32 * 0.35;
                let x = -0.75 + i as f32 * 0.55;
                painter.rect_filled(
                    egui::Rect::from_min_max(p(x, 0.8 - h), p(x + 0.35, 0.8)),
                    egui::CornerRadius::same(1),
                    color,
                );
            }
        }
        // --- menu-bar quick actions --------------------------------------
        Icon::Broom => {
            // A broom: a straight handle with a fanned head.
            painter.line_segment([p(-0.8, -0.85), p(0.1, -0.05)], stroke);
            painter.line_segment([p(-0.3, -0.3), p(0.5, 0.35)], stroke);
            painter.line_segment([p(0.1, -0.05), p(-0.25, 0.85)], stroke);
            painter.line_segment([p(0.28, 0.1), p(0.15, 0.9)], stroke);
            painter.line_segment([p(0.5, 0.35), p(0.6, 0.9)], stroke);
        }
        Icon::Reset => {
            // A circular arrow: an open ring with an arrow head at its end.
            let r = 0.78_f32;
            let start = -0.6_f32;
            let sweep = 0.78_f32;
            let steps = 26;
            let mut pts = Vec::with_capacity(steps + 1);
            for i in 0..=steps {
                let a = start + sweep * std::f32::consts::TAU * (i as f32 / steps as f32);
                pts.push(p(r * a.cos(), r * a.sin()));
            }
            painter.add(egui::Shape::line(pts, stroke));
            let a = start + sweep * std::f32::consts::TAU;
            let tip = (r * a.cos(), r * a.sin());
            let tan = (-a.sin(), a.cos());
            let perp = (-tan.1, tan.0);
            let (back, side) = (0.4_f32, 0.32_f32);
            painter.line_segment(
                [
                    p(tip.0, tip.1),
                    p(
                        tip.0 - tan.0 * back + perp.0 * side,
                        tip.1 - tan.1 * back + perp.1 * side,
                    ),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    p(tip.0, tip.1),
                    p(
                        tip.0 - tan.0 * back - perp.0 * side,
                        tip.1 - tan.1 * back - perp.1 * side,
                    ),
                ],
                stroke,
            );
        }
    }
}

/// A small, tinted menu-bar button: a hand-painted icon and a label on a soft
/// coloured background. Returns the response so the caller can act on a click.
fn quick_button(
    ui: &mut egui::Ui,
    icon: Icon,
    label: &str,
    tint: egui::Color32,
    tip: &str,
) -> egui::Response {
    let font = egui::FontId::new(12.0, egui::FontFamily::Proportional);
    let text_w = ui
        .painter()
        .layout_no_wrap(label.to_string(), font.clone(), tint)
        .size()
        .x;
    let (pad, icon_w, gap) = (8.0_f32, 15.0_f32, 6.0_f32);
    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2(pad + icon_w + gap + text_w + pad, 21.0),
        egui::Sense::click(),
    );
    let radius = egui::CornerRadius::same(6);
    let bg = tint.linear_multiply(if resp.hovered() { 0.30 } else { 0.18 });
    ui.painter().rect_filled(rect, radius, bg);
    let icon_rect = egui::Rect::from_center_size(
        egui::pos2(rect.min.x + pad + icon_w / 2.0, rect.center().y),
        egui::vec2(icon_w, icon_w),
    );
    paint_icon(ui.painter(), icon_rect, icon, tint);
    ui.painter().text(
        egui::pos2(rect.min.x + pad + icon_w + gap, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        font,
        tint,
    );
    resp.on_hover_text(tip)
}

/// A toolbar button drawn as a coloured outline: the fill stays the panel
/// background and only the border and the ink carry the tint, so the bar reads
/// as a line of rings. Hovering brightens it and shows `tip` as the description.
#[allow(clippy::too_many_arguments)]
fn outline_button(
    ui: &mut egui::Ui,
    t: &Theme,
    icon: Option<Icon>,
    label: Option<&str>,
    tip: &str,
    tint: egui::Color32,
    active: bool,
    enabled: bool,
) -> egui::Response {
    let font = egui::FontId::new(12.5, egui::FontFamily::Proportional);
    let (pad, icon_w, gap) = (12.0_f32, 20.0_f32, 7.0_f32);
    let text_w = label.map_or(0.0, |l| {
        ui.painter()
            .layout_no_wrap(l.to_string(), font.clone(), t.text)
            .size()
            .x
    });
    let mut width = pad * 2.0 + text_w;
    if icon.is_some() {
        width += icon_w;
        if label.is_some() {
            width += gap;
        }
    }
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, 36.0), egui::Sense::click());
    let hovered = enabled && resp.hovered();
    let (fill, border, ink) = if !enabled {
        (egui::Color32::TRANSPARENT, t.stroke, t.faint)
    } else if active {
        (tint.gamma_multiply(0.30), tint, tint)
    } else if hovered {
        (tint.gamma_multiply(0.16), tint, tint)
    } else {
        (
            egui::Color32::TRANSPARENT,
            tint.gamma_multiply(0.72),
            t.text,
        )
    };
    let radius = egui::CornerRadius::same(9);
    ui.painter().rect_filled(rect, radius, fill);
    ui.painter().rect_stroke(
        rect,
        radius,
        egui::Stroke::new(1.2_f32, border),
        egui::StrokeKind::Inside,
    );
    let mut x = rect.min.x + pad;
    if let Some(kind) = icon {
        let ir = egui::Rect::from_center_size(
            egui::pos2(x + icon_w * 0.5, rect.center().y),
            egui::vec2(icon_w, icon_w),
        );
        paint_icon(ui.painter(), ir, kind, ink);
        x += icon_w;
        if label.is_some() {
            x += gap;
        }
    }
    if let Some(l) = label {
        ui.painter().text(
            egui::pos2(x, rect.center().y),
            egui::Align2::LEFT_CENTER,
            l,
            font,
            ink,
        );
    }
    resp.on_hover_text(tip)
}

/// One segment of a [`capsule`].
struct Segment<'a> {
    icon: Option<Icon>,
    label: Option<&'a str>,
    tip: &'a str,
    enabled: bool,
    selected: bool,
}

/// A single coloured outline holding several segments side by side — the joined
/// Back/Forward control and the Simple/Regex/Fuzzy match-mode radio group. Only
/// the border, the separators and the active segment carry the colour; the fill
/// stays the panel background. Returns the index of a clicked, enabled segment.
fn capsule(
    ui: &mut egui::Ui,
    t: &Theme,
    salt: &str,
    tint: egui::Color32,
    segments: &[Segment<'_>],
) -> Option<usize> {
    let font = egui::FontId::new(12.5, egui::FontFamily::Proportional);
    let (pad, icon_w, gap, height) = (12.0_f32, 20.0_f32, 7.0_f32, 36.0_f32);
    // Measure the labels first, before the capsule is allocated, so the painter is
    // not borrowed across the (mutable) allocation.
    let label_widths: Vec<f32> = segments
        .iter()
        .map(|s| {
            s.label.map_or(0.0, |l| {
                ui.painter()
                    .layout_no_wrap(l.to_string(), font.clone(), t.text)
                    .size()
                    .x
            })
        })
        .collect();
    let widths: Vec<f32> = segments
        .iter()
        .zip(&label_widths)
        .map(|(s, &label_w)| {
            let mut w = pad * 2.0 + label_w;
            if s.icon.is_some() {
                w += icon_w;
                if s.label.is_some() {
                    w += gap;
                }
            }
            w
        })
        .collect();
    let total: f32 = widths.iter().sum();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(total, height), egui::Sense::hover());
    let radius = egui::CornerRadius::same(9);
    ui.painter().rect_stroke(
        rect,
        radius,
        egui::Stroke::new(1.2_f32, tint.gamma_multiply(0.72)),
        egui::StrokeKind::Inside,
    );
    let mut clicked = None;
    let mut x = rect.min.x;
    for (i, s) in segments.iter().enumerate() {
        let seg =
            egui::Rect::from_min_size(egui::pos2(x, rect.min.y), egui::vec2(widths[i], height));
        if i > 0 {
            ui.painter().line_segment(
                [
                    egui::pos2(x, seg.top() + 8.0),
                    egui::pos2(x, seg.bottom() - 8.0),
                ],
                egui::Stroke::new(1.0_f32, tint.gamma_multiply(0.35)),
            );
        }
        let resp = ui
            .interact(seg, ui.id().with((salt, i)), egui::Sense::click())
            .on_hover_text(s.tip);
        let hovered = s.enabled && resp.hovered();
        let (fill, ink) = if !s.enabled {
            (egui::Color32::TRANSPARENT, t.faint)
        } else if s.selected {
            (tint.gamma_multiply(0.30), tint)
        } else if hovered {
            (tint.gamma_multiply(0.16), tint)
        } else {
            (egui::Color32::TRANSPARENT, t.text)
        };
        if fill != egui::Color32::TRANSPARENT {
            ui.painter().rect_filled(seg.shrink(1.0), radius, fill);
        }
        let icon_w_eff = if s.icon.is_some() { icon_w } else { 0.0 };
        let text_w = label_widths[i];
        let gap_eff = if s.icon.is_some() && s.label.is_some() {
            gap
        } else {
            0.0
        };
        let content_w = icon_w_eff + gap_eff + text_w;
        let start = seg.center().x - content_w * 0.5;
        if let Some(kind) = s.icon {
            let ir = egui::Rect::from_center_size(
                egui::pos2(start + icon_w_eff * 0.5, seg.center().y),
                egui::vec2(icon_w, icon_w),
            );
            paint_icon(ui.painter(), ir, kind, ink);
        }
        if let Some(l) = s.label {
            ui.painter().text(
                egui::pos2(start + icon_w_eff + gap_eff, seg.center().y),
                egui::Align2::LEFT_CENTER,
                l,
                font.clone(),
                ink,
            );
        }
        if s.enabled && resp.clicked() {
            clicked = Some(i);
        }
        x += widths[i];
    }
    clicked
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

/// Full-width sidebar row: colour icon, label, optional right-aligned value, and
/// an accent bar when selected. Painted manually so the text is centred
/// regardless of which system font is in use.
fn nav_item(
    ui: &mut egui::Ui,
    t: &Theme,
    icon: Icon,
    color: egui::Color32,
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
    let icon_rect =
        egui::Rect::from_center_size(egui::pos2(dot_x, rect.center().y), egui::vec2(15.0, 15.0));
    paint_icon(
        ui.painter(),
        icon_rect,
        icon,
        if selected { t.accent } else { color },
    );

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

/// A small pill for a tag. Clicking it is left to the caller (to filter by the
/// tag); `active` draws the filled, selected style. A leading `#` is added when
/// the label does not already carry one.
fn tag_chip(ui: &mut egui::Ui, t: &Theme, tag: &str, active: bool) -> egui::Response {
    let color = t.accent;
    let label = if tag.starts_with('#') {
        tag.to_string()
    } else {
        format!("#{tag}")
    };
    let fill = if active { color } else { t.chip_fill(color) };
    let fg = if active { t.bg } else { color };
    ui.add(
        egui::Button::new(egui::RichText::new(label).size(10.0).color(fg))
            .fill(fill)
            .stroke(egui::Stroke::new(1.0_f32, color.gamma_multiply(0.6)))
            .corner_radius(egui::CornerRadius::same(6))
            .min_size(egui::vec2(0.0, 15.0)),
    )
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
    // A coloured glyph rather than the extension text: the name already ends in
    // the extension, and an icon reads at a glance.
    paint_icon(
        ui.painter(),
        egui::Rect::from_center_size(rect.center(), egui::vec2(17.0, 17.0)),
        file_icon(path, is_dir),
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
    // Hardlinks to one file are the same file, not duplicates: keep one path per
    // (device, inode) so they are never reported against each other.
    let mut seen: std::collections::HashSet<(u64, u64)> = std::collections::HashSet::new();
    for p in paths {
        if let Ok(md) = std::fs::metadata(p)
            && md.is_file()
            && md.len() > 0
        {
            if !seen.insert(inode_key(&md)) {
                continue;
            }
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

/// `(device, inode)` for an open handle to a file, so hardlinks collapse to one.
fn inode_key(md: &std::fs::Metadata) -> (u64, u64) {
    use std::os::unix::fs::MetadataExt;
    (md.dev(), md.ino())
}

/// The most recently modified path in `paths` (ties broken by name). Used as
/// the copy to *keep* when auto-selecting the rest for the trash.
fn newest_path(paths: &[PathBuf]) -> Option<PathBuf> {
    paths
        .iter()
        .max_by_key(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok())
        .cloned()
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
    /// Copy the paths as `file://` URIs, one per line.
    CopyUris {
        path: PathBuf,
        selection: bool,
    },
    /// Copy the paths quoted for pasting into a POSIX shell, one per line.
    CopyShell {
        path: PathBuf,
        selection: bool,
    },
    Details(PathBuf),
    FilterTo(PathBuf),
    SearchName(PathBuf),
    /// Open the tag editor for a row (or the whole checked selection).
    EditTags {
        path: PathBuf,
        selection: bool,
    },
    /// Show only results carrying this tag.
    FilterTag(String),
    ClearTagFilter,
    /// Add a directory to `config.exclude_dirs` and reindex without it.
    ExcludeFromIndex(PathBuf),
    /// Ask for confirmation, then move a row (or the selection) to the trash.
    TrashToConfirm {
        path: PathBuf,
        selection: bool,
    },
    AddToSelection(PathBuf),
    RemoveFromSelection(PathBuf),
    SelectAll,
    Invert,
    ClearSelection,
    FindDuplicates {
        selection: bool,
    },
}

/// A `file://` URI for a path, percent-encoding every byte outside the RFC 3986
/// unreserved set (leaving `/` alone), so it is a valid, paste-anywhere link even
/// when the path has spaces or non-UTF-8 bytes.
fn file_uri(path: &Path) -> String {
    use std::os::unix::ffi::OsStrExt;
    let mut out = String::from("file://");
    for &b in path.as_os_str().as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Quote a path for pasting into a POSIX shell: bare when every character is
/// safe, otherwise single-quoted with embedded quotes escaped (`'` → `'\''`).
fn shell_escape(s: &str) -> String {
    let safe = !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_.:/=+,@%^".contains(c));
    if safe {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

/// Serialise the rows to CSV, TSV or JSON for “Export results…”.
fn export_text(results: &[ResultRow], format: ExportFormat) -> String {
    let name_of = |r: &ResultRow| {
        r.path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    };
    match format {
        ExportFormat::Csv | ExportFormat::Tsv => {
            let sep = if format == ExportFormat::Csv {
                ','
            } else {
                '\t'
            };
            let esc = |s: &str| {
                if s.contains(sep) || s.contains('"') || s.contains('\n') || s.contains('\r') {
                    format!("\"{}\"", s.replace('"', "\"\""))
                } else {
                    s.to_string()
                }
            };
            let mut out = String::from("name");
            for col in ["path", "size", "modified"] {
                out.push(sep);
                out.push_str(col);
            }
            out.push('\n');
            for r in results {
                out.push_str(&esc(&name_of(r)));
                out.push(sep);
                out.push_str(&esc(&r.path.to_string_lossy()));
                out.push(sep);
                out.push_str(&r.size.to_string());
                out.push(sep);
                out.push_str(&r.mtime.to_string());
                out.push('\n');
            }
            out
        }
        ExportFormat::Json => {
            let rows: Vec<serde_json::Value> = results
                .iter()
                .map(|r| {
                    serde_json::json!({
                        "name": name_of(r),
                        "path": r.path.to_string_lossy(),
                        "size": r.size,
                        "modified": r.mtime,
                        "is_dir": r.is_dir,
                    })
                })
                .collect();
            serde_json::to_string_pretty(&rows).unwrap_or_else(|_| "[]".to_string())
        }
    }
}

/// A file of zero bytes, or a directory with no entries.
fn is_empty_entry(path: &Path, is_dir: bool) -> bool {
    if is_dir {
        std::fs::read_dir(path)
            .map(|mut d| d.next().is_none())
            .unwrap_or(false)
    } else {
        std::fs::metadata(path)
            .map(|m| m.len() == 0)
            .unwrap_or(false)
    }
}

/// A symlink whose target does not resolve.
fn is_broken_symlink(path: &Path) -> bool {
    matches!(std::fs::symlink_metadata(path), Ok(m) if m.file_type().is_symlink())
        && std::fs::metadata(path).is_err()
}

/// Case-insensitive comparison of a stored digest with the user's query.
fn hash_eq(stored: &str, wanted: &str) -> bool {
    stored.eq_ignore_ascii_case(wanted.trim())
}

/// Check a user-typed name for the rename dialog, returning the trimmed name or
/// the reason it cannot be used.
fn validate_rename(name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("Enter a name.".to_string());
    }
    if name == "." || name == ".." {
        return Err("That name is reserved.".to_string());
    }
    if name.contains('/') {
        return Err("A name cannot contain a slash.".to_string());
    }
    Ok(name.to_string())
}

/// Does a result path contain `needle` (already lower-cased)? Powers the
/// results-header “Filter results…” box; an empty needle matches everything.
fn path_contains(path: &Path, needle: &str) -> bool {
    needle.is_empty() || path.to_string_lossy().to_lowercase().contains(needle)
}

/// The XDG autostart entry that starts the background host at login.
fn autostart_entry() -> String {
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=EasySearch\n\
         Comment=Realtime filename and content search\n\
         Exec=easysearch --hidden\n\
         Icon={APP_ID}\n\
         Terminal=false\n\
         X-GNOME-Autostart-enabled=true\n"
    )
}

/// `~/.config/autostart/easysearch.desktop`, honouring `XDG_CONFIG_HOME`.
fn autostart_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("autostart").join("easysearch.desktop"))
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
///
/// Read live rather than stored in the index: a `stat` per *visible* row is
/// cheap (the table only renders a screenful), and it means the column is right
/// even for files created since the last index build. The same reason applies to
/// sorting by it, which is a one-off per click.
fn file_created(path: &Path) -> String {
    let secs = birth_secs(path);
    if secs <= 0 {
        return "—".to_string();
    }
    let text = human_time(secs);
    if text.is_empty() {
        "—".to_string()
    } else {
        text
    }
}

/// Birth time in unix seconds, or `0` when the filesystem has no `btime`.
fn birth_secs(path: &Path) -> i64 {
    std::fs::metadata(path)
        .and_then(|m| m.created())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
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

/// Small rounded pill naming a file's type (an icon: folder, image, document…).
fn type_pill(ui: &mut egui::Ui, t: &Theme, path: &Path, is_dir: bool) -> egui::Response {
    let color = type_color(t, path, is_dir);
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(24.0, 20.0), egui::Sense::hover());
    let radius = egui::CornerRadius::same(5);
    ui.painter().rect_filled(rect, radius, t.chip_fill(color));
    ui.painter().rect_stroke(
        rect,
        radius,
        egui::Stroke::new(1.0_f32, color.gamma_multiply(0.45)),
        egui::StrokeKind::Inside,
    );
    paint_icon(
        ui.painter(),
        egui::Rect::from_center_size(rect.center(), egui::vec2(13.0, 13.0)),
        file_icon(path, is_dir),
        color,
    );
    resp
}

/// The parent directory of a path, shortened to its last two components so it
/// fits a narrow column (`…/SAB/SDD`). An absolute path keeps its leading slash
/// without doubling it.
fn short_dir(path: &Path) -> String {
    let Some(parent) = path.parent() else {
        return String::new();
    };
    let absolute = parent.is_absolute();
    let comps: Vec<String> = parent
        .components()
        .filter_map(|c| match c {
            std::path::Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
            std::path::Component::ParentDir => Some("..".to_string()),
            _ => None,
        })
        .collect();
    // The root component is not collected (it is re-added as a single leading
    // slash below), so a parent directly under `/` no longer renders as `//home`.
    let slash = if absolute { "/" } else { "" };
    match comps.len() {
        0 if absolute => "/".to_string(),
        0 => String::new(),
        1 => format!("{slash}{}", comps[0]),
        2 => format!("{slash}{}/{}", comps[0], comps[1]),
        n => format!("…/{}/{}", comps[n - 2], comps[n - 1]),
    }
}

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

/// Load the desktop's own fonts, so the app looks native.
///
/// egui draws from font *files* it is handed, so "use the system font" means
/// asking fontconfig which file the configured family resolves to and passing
/// that over — the bundled egui faces stay in the fallback chain for glyphs the
/// system font lacks (emoji, CJK, rare symbols). The desktop's own choice comes
/// first (KDE's `kdeglobals`, then GTK's `settings.ini`), falling back to
/// fontconfig's `sans-serif` / `monospace`.
///
/// Resolving to a *file* also avoids copying a whole `.ttc` collection (an
/// `Inter.ttc` is ~12 MiB) and the risk that a collection's face 0 is not the
/// regular weight.
fn load_system_fonts() -> (Option<Vec<u8>>, Option<Vec<u8>>) {
    (
        resolve_system_font(&ui_font_candidates(), false),
        resolve_system_font(&mono_font_candidates(), true),
    )
}

/// Family names to try for the UI font, most specific first.
fn ui_font_candidates() -> Vec<String> {
    let mut families: Vec<String> = configured_family(false).into_iter().collect();
    families.push("sans-serif".to_string());
    families
}

/// Family names to try for the monospace font, most specific first.
fn mono_font_candidates() -> Vec<String> {
    let mut families: Vec<String> = configured_family(true).into_iter().collect();
    families.push("monospace".to_string());
    families
}

/// The desktop's configured font family: KDE first, then GTK.
fn configured_family(mono: bool) -> Option<String> {
    kde_family(mono).or_else(|| gtk_family(mono))
}

/// KDE's `[General] font=` / `fixed=` (`Family,size,weight,…`).
fn kde_family(mono: bool) -> Option<String> {
    let home = std::env::var_os("HOME")?;
    let text = std::fs::read_to_string(PathBuf::from(home).join(".config/kdeglobals")).ok()?;
    kde_family_from(&text, if mono { "fixed" } else { "font" })
}

/// The family out of a KDE `key=Family,size,…` line.
fn kde_family_from(text: &str, key: &str) -> Option<String> {
    let rest = text
        .lines()
        .find_map(|line| line.trim().strip_prefix(key)?.strip_prefix('='))?;
    // The value is `Family,size,weight,…`; the family may itself contain spaces.
    let family = rest.split(',').next().unwrap_or("").trim();
    (!family.is_empty()).then(|| family.to_string())
}

/// GTK's `gtk-font-name` / `gtk-monospace-font-name` (`Family 10`).
fn gtk_family(mono: bool) -> Option<String> {
    let home = std::env::var_os("HOME")?;
    let key = if mono {
        "gtk-monospace-font-name"
    } else {
        "gtk-font-name"
    };
    for rel in [
        ".config/gtk-4.0/settings.ini",
        ".config/gtk-3.0/settings.ini",
    ] {
        let Ok(text) = std::fs::read_to_string(PathBuf::from(&home).join(rel)) else {
            continue;
        };
        if let Some(family) = gtk_family_from(&text, key) {
            return Some(family);
        }
    }
    None
}

/// The family out of a GTK `key=Family Size` line (the size is dropped).
fn gtk_family_from(text: &str, key: &str) -> Option<String> {
    let value = text
        .lines()
        .find_map(|line| line.trim().strip_prefix(key)?.strip_prefix('='))?
        .trim();
    let family = match value.rsplit_once(' ') {
        Some((family, size)) if size.parse::<f32>().is_ok() => family.trim(),
        _ => value,
    };
    (!family.is_empty()).then(|| family.to_string())
}

fn resolve_system_font(candidates: &[String], mono: bool) -> Option<Vec<u8>> {
    for family in candidates {
        if let Some(bytes) = fontconfig_file(family, mono) {
            return Some(bytes);
        }
    }
    None
}

/// Ask fontconfig which font *file* it would use for `family` at regular weight.
///
/// `fc-match`, unlike `fc-list`, resolves aliases and generic names, so
/// `sans-serif` finds whatever the system has actually chosen.
fn fontconfig_file(family: &str, mono: bool) -> Option<Vec<u8>> {
    let pattern = if mono {
        format!("{family}:spacing=100:weight=regular:slant=roman")
    } else {
        format!("{family}:weight=regular:slant=roman")
    };
    let out = Command::new("fc-match")
        .arg("-f")
        .arg("%{file}")
        .arg(&pattern)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let path = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
    // Skip `.ttc` collections: face 0 is not necessarily the regular weight, and
    // the whole collection would be copied into memory.
    let extension = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase());
    if !matches!(extension.as_deref(), Some("ttf" | "otf")) {
        return None;
    }
    std::fs::read(path).ok()
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
                        self.all_results = r.results;
                        // Trim the allocation to the rows we actually hold: a
                        // big content search can over-allocate its result Vec.
                        self.all_results.shrink_to_fit();
                        self.truncated = r.truncated;
                        self.elapsed_ms = r.elapsed_ms;
                        self.error = None;
                        self.pending = false;
                        self.trash_msg = None;
                        if let Some(sort) = self.sort {
                            let needle = self.last_sent.clone();
                            sort_results(&mut self.all_results, sort, &needle, self.prefs.fuzzy);
                        }
                        self.apply_tag_filter();
                        self.prune_selection();
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
                self.location_counts = counts.per_location;
            }
        }
        if !self.pending
            && self.last_sent != self.counts_key
            && self.counts_at.elapsed() >= Duration::from_millis(500)
        {
            let base = Query {
                name: if self.content_mode && !self.full_text {
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
                multiline: self.prefs.multiline,
                // Facet counts are name-based; the content half of a Full text
                // query is not counted here (it is approximated as before).
                content_or_name: false,
                limit: 1,
            };
            self.counts_key = self.last_sent.clone();
            let _ = self.counts_tx.send(CountRequest {
                base,
                locations: locations().into_iter().map(|(_, p)| p).collect(),
                key: self.last_sent.clone(),
            });
            self.counts_at = Instant::now();
        }

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

        // …and simply moving the pointer into the results does the same, so the
        // keyboard goes with the mouse: hovering the list highlights the row
        // under the cursor and hands ↑/↓/Enter and Ctrl+A to the results, while
        // the field keeps focus as long as the pointer stays over it.
        if self.view == ViewTab::Results
            && ctx.memory(|m| m.has_focus(search_id()))
            && ctx.input(|i| i.pointer.delta() != egui::Vec2::ZERO)
            && let Some(pos) = ctx.input(|i| i.pointer.interact_pos())
            && self.results_rect.is_some_and(|r| r.contains(pos))
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
            || self.show_tags
            || self.show_excludes
            || !self.trash_confirm.is_empty()
            || self.show_ignore
            || self.show_export
            || self.show_hash
            || self.rename_target.is_some();
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
            // Alt+↑ is a separate shortcut (parent location); keep the plain
            // arrows from also firing on it.
            let (up, down, pgup, pgdn) = ctx.input(|i| {
                (
                    i.key_pressed(egui::Key::ArrowUp) && !i.modifiers.alt,
                    i.key_pressed(egui::Key::ArrowDown) && !i.modifiers.alt,
                    i.key_pressed(egui::Key::PageUp),
                    i.key_pressed(egui::Key::PageDown),
                )
            });
            if down {
                self.select(self.selected + 1, ctx);
            }
            if up {
                self.select(self.selected.saturating_sub(1), ctx);
            }
            if pgdn {
                self.select(self.selected + 20, ctx);
            }
            if pgup {
                self.select(self.selected.saturating_sub(20), ctx);
            }
            // Enter opens the row; Ctrl+Enter opens its containing folder.
            if let Some(path) = self.results.get(self.selected).map(|r| r.path.clone()) {
                let (enter, ctrl_enter) = ctx.input(|i| {
                    (
                        i.key_pressed(egui::Key::Enter),
                        i.modifiers.command && i.key_pressed(egui::Key::Enter),
                    )
                });
                if ctrl_enter {
                    App::open_folder(&path);
                } else if enter {
                    App::open(&path);
                }
            }
        }
        // Escape first drops the bulk selection (unmarking every row); with
        // nothing selected it clears the search, as before.
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            if !self.checked.is_empty() {
                self.checked.clear();
            } else if !self.query.is_empty() {
                self.query.clear();
                self.history_idx = None;
                self.send_query();
            }
        }
        // F5 re-runs the current search; Alt+↑ narrows the location to its parent.
        if ctx.input(|i| i.key_pressed(egui::Key::F5)) {
            self.send_query();
        }
        // F2 renames the selected row, unless the search box has the keyboard.
        if ctx.input(|i| i.key_pressed(egui::Key::F2))
            && !ctx.memory(|m| m.has_focus(search_id()))
            && let Some(path) = self.results.get(self.selected).map(|r| r.path.clone())
        {
            self.rename_value = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            self.rename_error = None;
            self.rename_target = Some(path);
        }
        if ctx.input(|i| i.modifiers.alt && i.key_pressed(egui::Key::ArrowUp)) {
            let parent = self
                .under
                .as_deref()
                .and_then(|u| Path::new(u).parent())
                .map(|p| p.to_string_lossy().into_owned());
            if let Some(parent) = parent {
                self.set_under(Some(parent));
            }
        }
        if ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::F)) {
            ctx.memory_mut(|m| m.request_focus(search_id()));
        }
        // Ctrl+A selects every result row, the Everything behaviour, even
        // while the search field still holds focus after typing. The keystroke
        // is consumed so the field does not also select its text; when there is
        // nothing to select it falls through, leaving normal editing alone.
        if !self.results.is_empty()
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::A))
        {
            self.checked = self.results.iter().map(|r| r.path.clone()).collect();
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

        // Events pushed by the host process (the tray, `--toggle`/`--quit`, or a
        // shortcut's `--search`). The host owns the window: it opens one, and a
        // hide simply closes it (the host and its engine keep running).
        while let Ok(event) = self.events.try_recv() {
            match event {
                Event::Show => show_window(ctx),
                Event::Hide => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
                Event::Quit => {
                    self.quit.store(true, Ordering::SeqCst);
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
                Event::Search { query } => {
                    show_window(ctx);
                    self.run_query(&query);
                    self.send_query();
                    ctx.memory_mut(|m| m.request_focus(search_id()));
                }
                Event::FocusSearch => {
                    show_window(ctx);
                    ctx.memory_mut(|m| m.request_focus(search_id()));
                }
                Event::NewSearch => {
                    show_window(ctx);
                    self.clear_results();
                    ctx.memory_mut(|m| m.request_focus(search_id()));
                }
                Event::ClearResults => self.clear_results(),
                Event::Settings => {
                    show_window(ctx);
                    self.show_settings = true;
                }
                Event::About => {
                    show_window(ctx);
                    self.show_about = true;
                }
                Event::OpenIndexFolder => {
                    show_window(ctx);
                    Self::open(&easysearch_core::Config::default_disk_index_dir());
                }
                Event::ClearHistory => {
                    self.prefs.clear_history();
                    self.history_idx = None;
                }
            }
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
        // The bottom view tabs drive the central area: Preview and Details share
        // the preview renderer, and History lists the recent searches.
        if matches!(self.view, ViewTab::Preview | ViewTab::Details) {
            self.refresh_preview(ctx);
        }
        egui::CentralPanel::default().show(ctx, |ui| match self.view {
            ViewTab::Results => self.results_table(ui),
            ViewTab::Preview => {
                self.panel_tab = PanelTab::Preview;
                self.preview_panel(ui);
            }
            ViewTab::Details => {
                self.panel_tab = PanelTab::Details;
                self.preview_panel(ui);
            }
            ViewTab::History => self.history_view(ui),
        });

        if self.show_about {
            self.about_dialog(ctx);
        }
        if self.show_settings {
            self.settings_dialog(ctx);
        }
        if self.show_shortcuts {
            self.shortcuts_dialog(ctx);
        }
        if self.show_content_warning {
            self.content_warning_dialog(ctx);
        }
        if self.show_saved {
            self.saved_dialog(ctx);
        }
        self.save_search_dialog(ctx);
        self.duplicates_window(ctx);
        self.tags_dialog(ctx);
        self.excludes_dialog(ctx);
        self.trash_confirm_dialog(ctx);
        self.ignore_dialog(ctx);
        if self.show_export {
            self.export_dialog(ctx);
        }
        if self.show_hash {
            self.hash_dialog(ctx);
        }
        if self.rename_target.is_some() {
            self.rename_dialog(ctx);
        }
    }

    /// Persist open tabs and history on shutdown (eframe calls this on exit and
    /// on every `auto_save_interval` tick).
    fn save(&mut self, _storage: &mut dyn eframe::Storage) {
        self.save_prefs();
        let _ = self.tags.save();
    }
}

impl App {
    /// The logo as a texture, uploaded once and reused.
    fn logo(&mut self, ctx: &egui::Context) -> egui::TextureHandle {
        const SIZE: u32 = 512;
        self.logo_tex
            .get_or_insert_with(|| {
                let image = egui::ColorImage::from_rgba_unmultiplied(
                    [SIZE as usize, SIZE as usize],
                    &logo::rgba(SIZE),
                );
                ctx.load_texture("easysearch-logo", image, egui::TextureOptions::LINEAR)
            })
            .clone()
    }

    /// The one-time notice shown when content search is first switched on.
    fn content_warning_dialog(&mut self, ctx: &egui::Context) {
        egui::Window::new("Content search")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.add_space(4.0);
                ui.label(egui::RichText::new("Content search uses more memory and CPU.").strong());
                ui.add_space(4.0);
                ui.label(
                    "EasySearch is built for filename search, which reads only the index. \
                     Searching inside files runs the embedded ripgrep pass over your files: \
                     it reads them live (so results are always current), and it holds working \
                     buffers for the files it scans — expect noticeably more memory and CPU \
                     while it runs, and slower first results on a large tree.",
                );
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new(
                        "The optional background content cache is spooled to disk, not RAM, \
                         and is off by default (Settings ▸ Indexing).",
                    )
                    .small()
                    .color(self.fg_dim()),
                );
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if ui.button("Got it").clicked() {
                        self.prefs.content_warning_seen = true;
                        self.prefs.save();
                        self.show_content_warning = false;
                    }
                });
            });
    }

    fn about_dialog(&mut self, ctx: &egui::Context) {
        let (files, dirs) = self.engine.counts();
        egui::Window::new("About")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.add_space(4.0);
                ui.vertical_centered(|ui| {
                    let mark = self.logo(ui.ctx());
                    ui.add(egui::Image::new((mark.id(), egui::vec2(168.0, 168.0))));
                    ui.add_space(6.0);
                    ui.label(egui::RichText::new("EasySearch").size(20.0).strong());
                    ui.label(
                        egui::RichText::new(format!("Version {}", crate::version_stamp()))
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
                ui.horizontal(|ui| {
                    ui.label("Theme");
                    for (choice, label, hover) in [
                        (ThemeChoice::Dark, "Dark", "The original dark palette."),
                        (ThemeChoice::Light, "Light", "The light palette."),
                        (
                            ThemeChoice::Brand,
                            "Brand",
                            "The logo's colours: a deep teal ground, teal accent, pink and lime.",
                        ),
                    ] {
                        if ui
                            .radio(self.prefs.theme == choice, label)
                            .on_hover_text(hover)
                            .clicked()
                        {
                            self.prefs.theme = choice;
                            self.theme = choice;
                            self.apply_style(ctx);
                            self.prefs.save();
                        }
                    }
                });
                if ui
                    .checkbox(&mut self.prefs.show_preview, "Preview pane")
                    .changed()
                {
                    self.prefs.save();
                }
                ui.label(
                    egui::RichText::new(
                        "Closing the window leaves EasySearch running in the \
                                         tray, so the index and the search stay warm.",
                    )
                    .small()
                    .color(self.fg_dim()),
                );
                ui.add_space(8.0);
                ui.label(egui::RichText::new("Search").strong());
                let mut fuzzy = self.prefs.fuzzy;
                if ui
                    .checkbox(&mut fuzzy, "Fuzzy matching (fzf-style)")
                    .on_hover_text(
                        "Match the query's characters in order anywhere in the name: \
                         mtn → meeting-notes.md",
                    )
                    .changed()
                {
                    self.prefs.fuzzy = fuzzy;
                    if fuzzy {
                        // Fuzzy is one of the three match modes (the toolbar's
                        // Simple / Regex / Fuzzy radio), so it clears Regex.
                        self.regex_mode = false;
                    }
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
                let mut follow = self.status.follow_symlinks;
                if ui
                    .checkbox(&mut follow, "Follow symbolic links")
                    .on_hover_text("Index the targets of symlinked folders too.")
                    .changed()
                {
                    self.engine.set_follow_symlinks(follow);
                    self.status.follow_symlinks = follow;
                }
                let mut content_index = matches!(
                    self.status.content_index,
                    ContentIndexStatus::Enabled { .. }
                );
                if ui
                    .checkbox(&mut content_index, "Background content index")
                    .on_hover_text(
                        "Cache extracted document text to speed up repeated content searches. \
                         Off: content queries read files live, always fresh. The cache is \
                         spooled to disk, not held in RAM.",
                    )
                    .changed()
                {
                    self.engine.set_content_index(content_index);
                    // Mirror it so the checkbox holds still until the fresh
                    // status (the engine's is polled) arrives.
                    if !content_index {
                        self.status.content_index = ContentIndexStatus::Disabled;
                    } else if !matches!(
                        self.status.content_index,
                        ContentIndexStatus::Enabled { .. }
                    ) {
                        self.status.content_index = ContentIndexStatus::Enabled {
                            entries: 0,
                            bytes: 0,
                            pending: 0,
                        };
                    }
                }
                if ui.button("Edit ignore files…").clicked() {
                    self.open_ignore_dialog();
                }
                ui.add_space(8.0);
                ui.label(egui::RichText::new("Startup").strong());
                let mut autostart = Self::autostart_enabled();
                if ui
                    .checkbox(&mut autostart, "Start EasySearch at login (background)")
                    .on_hover_text(
                        "Writes an XDG autostart entry that runs `easysearch --hidden`, so the \
                         tray and the index are ready without opening a window.",
                    )
                    .changed()
                    && let Err(e) = Self::set_autostart(autostart)
                {
                    notify_desktop("Autostart", &format!("Could not update autostart: {e}"));
                }
                ui.add_space(8.0);
                ui.label(egui::RichText::new("Configuration").strong());
                let config_path = easysearch_core::Config::default_path();
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
                        self.theme = self.prefs.theme;
                        self.apply_style(ctx);
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
                    ("Ctrl+Enter", "Open the containing folder"),
                    ("F5", "Re-run the search"),
                    ("Alt+↑", "Go to the parent location"),
                    ("F2", "Rename the selected file"),
                    ("Double-click", "Open a result"),
                    ("Esc", "Clear the row selection, then the search"),
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
                    let t = self.theme();
                    ui.menu_button(egui::RichText::new("File").color(t.accent).strong(), |ui| {
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
                        if ui
                            .button("Export results…")
                            .on_hover_text("Write the visible rows to a CSV, TSV or JSON file")
                            .clicked()
                        {
                            self.open_export();
                            ui.close_menu();
                        }
                        if ui
                            .button("Find files by hash…")
                            .on_hover_text(
                                "Hash the visible files and keep the ones matching a SHA-256",
                            )
                            .clicked()
                        {
                            self.open_hash();
                            ui.close_menu();
                        }
                        ui.separator();
                        if ui
                            .button("Hide window")
                            .on_hover_text(
                                "Hides the window but keeps EasySearch running in the tray, \
                                 so the engine keeps indexing (and a search stays warm). \
                                 Open it again from the tray or `easysearch --toggle`.",
                            )
                            .clicked()
                        {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                            ui.close_menu();
                        }
                        if ui
                            .button("Quit EasySearch")
                            .on_hover_text(
                                "Stops the app and its engine process. The index on disk \
                                 is kept, so the next start is instant.",
                            )
                            .clicked()
                        {
                            self.quit.store(true, Ordering::SeqCst);
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                            ui.close_menu();
                        }
                    });
                    ui.menu_button(egui::RichText::new("Search").color(t.good).strong(), |ui| {
                        let mut content_on = self.content_mode;
                        let content_changed = ui
                            .checkbox(&mut content_on, "Match contents")
                            .on_hover_text(
                                "Search inside files (ripgrep). Uses more memory and CPU.",
                            )
                            .changed();
                        // Regex and fuzzy are one three-way choice (the toolbar's
                        // Simple / Regex / Fuzzy radio), so turning one on clears
                        // the other.
                        let mut regex = self.regex_mode;
                        let mut fuzzy = self.prefs.fuzzy;
                        let mut mode_changed = ui.checkbox(&mut regex, "Regex mode").changed();
                        mode_changed |= ui
                            .checkbox(&mut fuzzy, "Fuzzy matching (fzf-style)")
                            .changed();
                        if mode_changed {
                            if regex {
                                fuzzy = false;
                            } else if fuzzy {
                                regex = false;
                            }
                            self.regex_mode = regex;
                            self.prefs.fuzzy = fuzzy;
                        }
                        let changed = mode_changed
                            | ui.checkbox(
                                &mut self.prefs.multiline,
                                "Multiline content (regex spans lines)",
                            )
                            .changed()
                            | ui.checkbox(&mut self.case_sensitive, "Case-sensitive")
                                .changed()
                            | ui.checkbox(&mut self.hidden, "Hidden files").changed()
                            | ui.checkbox(&mut self.full_path, "Full path match")
                                .changed();
                        if content_changed {
                            let was = self.content_mode;
                            self.content_mode = content_on;
                            if !content_on {
                                self.full_text = false;
                            }
                            self.after_scope_change(was, content_on);
                        } else if changed {
                            self.send_query();
                        }
                        ui.separator();
                        if ui.button("Clear search history").clicked() {
                            self.prefs.clear_history();
                            ui.close_menu();
                        }
                    });
                    ui.menu_button(
                        egui::RichText::new("Filters").color(t.warn).strong(),
                        |ui| {
                            if ui
                                .checkbox(&mut self.prefs.show_preview, "Preview pane")
                                .changed()
                            {
                                self.prefs.save();
                            }
                            ui.separator();
                            ui.label(egui::RichText::new("Theme").small());
                            for (choice, label) in [
                                (ThemeChoice::Dark, "Dark"),
                                (ThemeChoice::Light, "Light"),
                                (ThemeChoice::Brand, "Brand"),
                            ] {
                                if ui.radio(self.prefs.theme == choice, label).clicked() {
                                    self.prefs.theme = choice;
                                    self.theme = choice;
                                    self.apply_style(ctx);
                                    self.prefs.save();
                                }
                            }
                        },
                    );
                    ui.menu_button(
                        egui::RichText::new("Tools").color(t.kind_img).strong(),
                        |ui| {
                            if ui.button("Rebuild index").clicked() {
                                self.engine.rebuild();
                                ui.close_menu();
                            }
                            if ui.button("Ignore files…").clicked() {
                                self.open_ignore_dialog();
                                ui.close_menu();
                            }
                            if ui.button("Excluded folders…").clicked() {
                                self.open_excludes_dialog();
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
                        },
                    );
                    ui.menu_button(
                        egui::RichText::new("Settings").color(t.kind_code).strong(),
                        |ui| {
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
                        },
                    );
                    ui.menu_button(
                        egui::RichText::new("Help").color(t.kind_av).strong(),
                        |ui| {
                            if ui.button("About").clicked() {
                                self.show_about = true;
                                ui.close_menu();
                            }
                            if ui.button("Keyboard shortcuts").clicked() {
                                self.show_shortcuts = true;
                                ui.close_menu();
                            }
                        },
                    );
                    // At the far right: two tinted quick actions to the *left* of
                    // the mark — free up memory, and reset every setting.
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let t = self.theme();
                        let mark = self.logo(ui.ctx());
                        ui.add(egui::Image::new((mark.id(), egui::vec2(22.0, 22.0))))
                            .on_hover_text(app_title());
                        ui.add_space(8.0);
                        if quick_button(
                            ui,
                            Icon::Reset,
                            "Reset defaults",
                            t.warn,
                            "Reset every search setting, filter, tab and saved search to its \
                             default. Your theme and zoom are kept.",
                        )
                        .clicked()
                        {
                            self.reset_defaults(ctx);
                        }
                        if quick_button(
                            ui,
                            Icon::Broom,
                            "Free memory",
                            t.accent,
                            "Stop a content search, clear the results and return freed pages to \
                             the operating system.",
                        )
                        .clicked()
                        {
                            self.free_memory();
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

    /// Toolbar row: a left-aligned strip of coloured-outline controls. Back and
    /// Forward share one capsule, Home and Index are compact glyphs, the match
    /// mode is a Simple/Regex/Fuzzy radio group, and Recent and Saved close it.
    fn toolbar(&mut self, ctx: &egui::Context) {
        let t = self.theme();
        let indexing = matches!(self.status.state, State::Starting | State::Indexing);
        let home = self.home_path();
        let at_home = self.under.as_deref() == home.to_str();
        let recent = matches!(self.category, Category::Recent { .. });
        let saved_count = self.prefs.saved.len();
        let mode = self.match_mode();
        egui::TopBottomPanel::top("toolbar")
            .frame(
                egui::Frame::new()
                    .fill(t.panel)
                    .inner_margin(egui::Margin::symmetric(10, 6)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;

                    // Back and Forward share one slot.
                    match capsule(
                        ui,
                        &t,
                        "nav",
                        t.accent,
                        &[
                            Segment {
                                icon: Some(Icon::Back),
                                label: None,
                                tip: "Back — the previous location",
                                enabled: self.loc_idx > 0,
                                selected: false,
                            },
                            Segment {
                                icon: Some(Icon::Forward),
                                label: None,
                                tip: "Forward — the next location",
                                enabled: self.loc_idx + 1 < self.loc_history.len(),
                                selected: false,
                            },
                        ],
                    ) {
                        Some(0) => self.nav_location(-1),
                        Some(1) => self.nav_location(1),
                        _ => {}
                    }

                    // Home and Index: compact glyphs.
                    if outline_button(
                        ui,
                        &t,
                        Some(Icon::Home),
                        None,
                        "Search your home directory",
                        t.kind_dir,
                        at_home,
                        true,
                    )
                    .clicked()
                    {
                        self.set_under(home.to_str().map(|s| s.to_string()));
                    }
                    if outline_button(
                        ui,
                        &t,
                        Some(Icon::Database),
                        None,
                        if indexing {
                            "Rebuilding the index…"
                        } else {
                            "Rebuild the index from disk"
                        },
                        t.warn,
                        false,
                        !indexing,
                    )
                    .clicked()
                    {
                        self.engine.rebuild();
                    }

                    // Match mode: Simple / Regex / Fuzzy, one of three.
                    match capsule(
                        ui,
                        &t,
                        "mode",
                        t.accent,
                        &[
                            Segment {
                                icon: None,
                                label: Some("Simple"),
                                tip: "Plain text and globs (*, ?, [abc])",
                                enabled: true,
                                selected: mode == MatchMode::Simple,
                            },
                            Segment {
                                icon: None,
                                label: Some("Regex"),
                                tip: "Treat the query as a regular expression",
                                enabled: true,
                                selected: mode == MatchMode::Regex,
                            },
                            Segment {
                                icon: None,
                                label: Some("Fuzzy"),
                                tip: "Fuzzy matching: the query's characters in order, \
                                      anywhere (mtn → meeting-notes.md)",
                                enabled: true,
                                selected: mode == MatchMode::Fuzzy,
                            },
                        ],
                    ) {
                        Some(0) => self.set_match_mode(MatchMode::Simple),
                        Some(1) => self.set_match_mode(MatchMode::Regex),
                        Some(2) => self.set_match_mode(MatchMode::Fuzzy),
                        _ => {}
                    }

                    // Recent and Saved searches close the row.
                    if outline_button(
                        ui,
                        &t,
                        Some(Icon::Clock),
                        Some("Recent"),
                        "Files changed in the last 7 days",
                        t.kind_av,
                        recent,
                        true,
                    )
                    .clicked()
                    {
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
                    if outline_button(
                        ui,
                        &t,
                        Some(Icon::Bookmark),
                        Some(saved_label.as_str()),
                        "Your saved searches",
                        t.kind_img,
                        self.show_saved,
                        true,
                    )
                    .clicked()
                    {
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
                            })
                            .response
                            .on_hover_text(SEARCH_TIPS);
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
                        if ui
                            .selectable_label(
                                self.scope() == Scope::FullText,
                                "Full text (name or contents)",
                            )
                            .on_hover_text(
                                "The query matches the file name or its contents, not both",
                            )
                            .clicked()
                        {
                            self.set_scope(Scope::FullText);
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
                    // Content-only: let the pattern span lines.
                    if self.content_mode
                        && ui
                            .checkbox(&mut self.prefs.multiline, "Multiline")
                            .on_hover_text(
                                "Let the content pattern span lines (e.g. foo\\nbar). Much slower.",
                            )
                            .changed()
                    {
                        self.prefs.save();
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
                    if self.content_blocked {
                        ui.label(
                            egui::RichText::new(format!(
                                "• content search starts at {CONTENT_MIN_CHARS} characters"
                            ))
                            .size(11.5)
                            .color(t.warn),
                        );
                    }
                    if let Some(tag) = self.tag_filter.clone() {
                        ui.label(egui::RichText::new("•").size(11.5).color(t.faint));
                        if tag_chip(ui, &t, &format!("#{tag}"), true)
                            .on_hover_text("Clear the tag filter")
                            .clicked()
                        {
                            self.set_tag_filter(None);
                        }
                    }
                    // Narrow the visible rows without re-querying the index.
                    ui.add_space(8.0);
                    let filter_resp = ui.add(
                        egui::TextEdit::singleline(&mut self.result_filter)
                            .hint_text(
                                egui::RichText::new("Filter results…")
                                    .size(11.0)
                                    .color(t.faint),
                            )
                            .font(egui::FontId::new(11.5, egui::FontFamily::Proportional))
                            .desired_width(150.0)
                            .margin(egui::vec2(3.0, 2.0)),
                    );
                    if filter_resp.changed() {
                        self.apply_tag_filter();
                    }
                    if !self.result_filter.is_empty()
                        && ui
                            .small_button("✕")
                            .on_hover_text("Clear the result filter")
                            .clicked()
                    {
                        self.result_filter.clear();
                        self.apply_tag_filter();
                    }
                    if ui
                        .checkbox(&mut self.filter_empty, "Empty")
                        .on_hover_text("Only empty files and folders")
                        .changed()
                        | ui.checkbox(&mut self.filter_broken, "Broken links")
                            .on_hover_text("Only broken symbolic links")
                            .changed()
                    {
                        self.apply_tag_filter();
                    }
                    if let Some(msg) = &self.trash_msg {
                        ui.label(
                            egui::RichText::new(format!("• {msg}"))
                                .size(11.5)
                                .color(t.dim),
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
                            Some(Sort::Created(_)) => "Created",
                        };
                        ui.menu_button(sort_label, |ui| {
                            ui.set_min_width(150.0);
                            let opts: [(&str, Sort); 5] = [
                                ("Relevance", Sort::Relevance(false)),
                                ("Name", Sort::Name(true)),
                                ("Size", Sort::Size(false)),
                                ("Modified", Sort::Mtime(false)),
                                ("Created", Sort::Created(false)),
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
            if self.full_text {
                Scope::FullText
            } else {
                Scope::Contents
            }
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
            Scope::FullText => "Full text (name or contents)",
        }
    }

    /// The current match mode, derived from the regex/fuzzy switches.
    fn match_mode(&self) -> MatchMode {
        if self.regex_mode {
            MatchMode::Regex
        } else if self.prefs.fuzzy {
            MatchMode::Fuzzy
        } else {
            MatchMode::Simple
        }
    }

    /// Set the match mode; regex and fuzzy are mutually exclusive.
    fn set_match_mode(&mut self, mode: MatchMode) {
        let (regex, fuzzy) = match mode {
            MatchMode::Simple => (false, false),
            MatchMode::Regex => (true, false),
            MatchMode::Fuzzy => (false, true),
        };
        if self.regex_mode == regex && self.prefs.fuzzy == fuzzy {
            return;
        }
        self.regex_mode = regex;
        self.prefs.fuzzy = fuzzy;
        self.prefs.save();
        self.send_query();
    }

    fn set_scope(&mut self, s: Scope) {
        if self.scope() == s {
            return;
        }
        let was = self.content_mode;
        let on = matches!(s, Scope::Contents | Scope::FullText);
        self.content_mode = on;
        self.full_text = s == Scope::FullText;
        self.full_path = s == Scope::FullPath;
        self.after_scope_change(was, on);
    }

    /// Finish a scope change: warn when content search is switched **on**, and
    /// release what a content search was holding when it is switched **off**,
    /// then re-run the query.
    fn after_scope_change(&mut self, was_content: bool, is_content: bool) {
        if is_content && !was_content && !self.prefs.content_warning_seen {
            self.show_content_warning = true;
        }
        if was_content && !is_content {
            self.release_content_memory();
        }
        self.send_query();
    }

    /// Drop everything a content search left behind and return the pages.
    ///
    /// Content search scans whole files through the engine's ripgrep pass — the
    /// heaviest thing this app does. Leaving the scope drops the rows it
    /// produced and asks the engine to `malloc_trim`, rather than keeping both
    /// resident until the next search happens to replace them.
    fn release_content_memory(&mut self) {
        self.results.clear();
        self.results.shrink_to_fit();
        self.truncated = false;
        self.error = None;
        self.preview = None;
        self.hash = None;
        self.dups = None;
        self.checked.clear();
        self.pending = false;
        self.engine.trim_memory();
        self.dirty = true;
    }

    /// Release as much memory as we can right now (the *Free memory* button).
    ///
    /// The current rows — and any content-search matches in them — are the biggest
    /// thing the window holds, so they are dropped and shrunk; the engine is then
    /// asked to return freed pages to the OS (`malloc_trim` in the engine
    /// process). The query text is left as it is, so pressing Enter re-runs it.
    fn free_memory(&mut self) {
        // Leave a content search first: it is the heaviest thing running, and
        // dropping out of the scope stops new scans and releases the buffers the
        // last one left behind (the engine is asked to `malloc_trim` as well).
        let was_content = self.content_mode || self.full_text;
        self.content_mode = false;
        self.full_text = false;
        self.content_blocked = false;

        self.results.clear();
        self.results.shrink_to_fit();
        self.all_results.clear();
        self.all_results.shrink_to_fit();
        self.checked.clear();
        self.result_filter.clear();
        self.filter_empty = false;
        self.filter_broken = false;
        self.truncated = false;
        self.error = None;
        self.elapsed_ms = 0;
        self.selected = 0;
        self.scroll_to = None;
        self.pending = false;
        self.preview = None;
        self.hash = None;
        self.dups = None;
        self.dup_checked.clear();
        self.dup_progress = None;
        self.engine.trim_memory();
        self.dirty = true;
        let extra = if was_content {
            "Stopped the content search and released its buffers. "
        } else {
            ""
        };
        notify_desktop(
            "Memory freed",
            &format!(
                "{extra}Cleared the results and asked the engine to return freed pages to the \
                 operating system. The index is untouched — press Enter to run the search again."
            ),
        );
    }

    /// Put every search setting, filter and tab back to its default (the *Reset
    /// defaults* button). The user's theme and zoom are kept; the index, tags and
    /// files on disk are left alone.
    fn reset_defaults(&mut self, ctx: &egui::Context) {
        // The appearance choices are the user's, not a default: keep the current
        // theme and zoom across a reset, and put only the search-related settings
        // back to their defaults.
        let theme = self.theme;
        let zoom = self.prefs.zoom;
        self.prefs = GuiPrefs::default();
        self.prefs.theme = theme;
        self.prefs.zoom = zoom;

        // Live tab state, back to the defaults the fresh-start window would have.
        self.query.clear();
        self.last_sent.clear();
        self.regex_mode = false;
        self.content_mode = false;
        self.full_text = false;
        self.case_sensitive = false;
        self.hidden = false;
        self.full_path = false;
        self.category = Category::All;
        self.size = SizeFilter::Any;
        self.modified = ModifiedFilter::Any;
        self.extensions.clear();
        self.under = None;
        self.sort = None;
        self.density = Density::default();
        self.view = ViewTab::default();
        self.panel_tab = PanelTab::default();
        self.checked.clear();
        self.results.clear();
        self.all_results.clear();
        self.preview = None;
        self.hash = None;
        self.dups = None;
        self.show_dups = false;
        self.dup_checked.clear();
        self.dup_progress = None;
        self.sidebar_filter.clear();
        self.result_filter.clear();
        self.filter_empty = false;
        self.filter_broken = false;
        self.selected = 0;
        self.truncated = false;
        self.error = None;
        self.elapsed_ms = 0;
        self.pending = false;
        self.history_idx = None;
        self.loc_history = vec![None];
        self.loc_idx = 0;
        self.tabs = vec![TabState::default()];
        self.active_tab = 0;

        // Persist the result exactly as a fresh window would, with the user's
        // theme and zoom kept.
        self.theme = self.prefs.theme;
        ctx.set_zoom_factor(self.prefs.zoom);
        self.apply_style(ctx);
        self.save_prefs();
        self.send_query();
        notify_desktop(
            "Settings reset",
            "Search settings, filters, tabs and saved searches are back to their defaults. \
             Your theme and zoom were kept.",
        );
    }

    // --- find by hash -----------------------------------------------------

    fn open_hash(&mut self) {
        self.hash_query.clear();
        self.hash_matches.clear();
        self.hash_scanned = 0;
        self.hash_ran = false;
        self.show_hash = true;
    }

    /// Hash the visible files and keep those matching `hash_query`.
    fn run_hash_search(&mut self) {
        let want = self.hash_query.trim().to_string();
        self.hash_matches.clear();
        self.hash_scanned = 0;
        self.hash_ran = true;
        if want.is_empty() {
            return;
        }
        for r in self.results.iter().take(HASH_SCAN_LIMIT) {
            if r.is_dir || r.size > MAX_HASH_BYTES {
                continue;
            }
            self.hash_scanned += 1;
            if let Some(h) = sha256_of(&r.path)
                && hash_eq(&h, &want)
            {
                self.hash_matches.push(r.path.clone());
            }
        }
    }

    fn hash_dialog(&mut self, ctx: &egui::Context) {
        let mut run = false;
        egui::Window::new("Find files by hash")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.add_space(4.0);
                ui.label(egui::RichText::new("SHA-256 digest").strong());
                ui.add(
                    egui::TextEdit::singleline(&mut self.hash_query)
                        .desired_width(380.0)
                        .hint_text("e.g. 9f86d0818…  (case-insensitive)"),
                );
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(format!(
                        "Hashes the visible files (up to {HASH_SCAN_LIMIT}), skipping folders \
                         and files over 512 MB."
                    ))
                    .small()
                    .color(self.fg_dim()),
                );
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Find").clicked() {
                        run = true;
                    }
                    if ui.button("Close").clicked() {
                        self.show_hash = false;
                    }
                });
                if self.hash_ran {
                    ui.add_space(6.0);
                    if self.hash_matches.is_empty() {
                        ui.label(
                            egui::RichText::new(format!(
                                "No match among {} hashed files.",
                                self.hash_scanned
                            ))
                            .color(self.fg_dim()),
                        );
                    } else {
                        ui.label(
                            egui::RichText::new(format!(
                                "{} match(es) in {} hashed files:",
                                self.hash_matches.len(),
                                self.hash_scanned
                            ))
                            .strong(),
                        );
                        for p in self.hash_matches.iter().take(50) {
                            ui.label(
                                egui::RichText::new(p.display().to_string())
                                    .small()
                                    .monospace()
                                    .color(self.fg_dim()),
                            );
                        }
                    }
                }
            });
        if run {
            self.run_hash_search();
        }
    }

    // --- rename -----------------------------------------------------------

    fn rename_dialog(&mut self, ctx: &egui::Context) {
        let Some(old) = self.rename_target.clone() else {
            return;
        };
        let mut submit = false;
        egui::Window::new("Rename")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(short_dir(&old))
                        .small()
                        .color(self.fg_dim()),
                );
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut self.rename_value)
                        .desired_width(360.0)
                        .hint_text("New name"),
                );
                if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    submit = true;
                }
                if let Some(e) = &self.rename_error {
                    ui.add_space(4.0);
                    ui.colored_label(ui.visuals().error_fg_color, e);
                }
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if ui.button("Rename").clicked() {
                        submit = true;
                    }
                    if ui.button("Cancel").clicked() {
                        self.rename_target = None;
                    }
                });
            });
        if submit {
            self.commit_rename();
        }
    }

    /// Rename the target row, refusing an empty/reserved name or a clash.
    fn commit_rename(&mut self) {
        let Some(old) = self.rename_target.clone() else {
            return;
        };
        let new_name = match validate_rename(&self.rename_value) {
            Ok(n) => n,
            Err(e) => {
                self.rename_error = Some(e);
                return;
            }
        };
        let Some(parent) = old.parent() else {
            self.rename_error = Some("The path has no parent directory.".to_string());
            return;
        };
        let new = parent.join(&new_name);
        if new == old {
            self.rename_target = None;
            return;
        }
        if new.exists() {
            self.rename_error = Some(format!("{} already exists.", new.display()));
            return;
        }
        match std::fs::rename(&old, &new) {
            Ok(()) => {
                for row in self.results.iter_mut() {
                    if row.path == old {
                        row.path = new.clone();
                    }
                }
                for row in self.all_results.iter_mut() {
                    if row.path == old {
                        row.path = new.clone();
                    }
                }
                if self.checked.remove(&old) {
                    self.checked.insert(new.clone());
                }
                self.rename_target = None;
                self.rename_error = None;
                notify_desktop("Renamed", &format!("{} → {}", old.display(), new.display()));
            }
            Err(e) => self.rename_error = Some(format!("Could not rename: {e}")),
        }
    }

    // --- export -----------------------------------------------------------

    /// Open the “Export results…” dialog with a sensible default path.
    fn open_export(&mut self) {
        self.export_path = self
            .home_path()
            .join(format!("easysearch-results.{}", self.export_format.ext()))
            .to_string_lossy()
            .into_owned();
        self.export_error = None;
        self.show_export = true;
    }

    fn export_dialog(&mut self, ctx: &egui::Context) {
        let mut export = false;
        egui::Window::new("Export results")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label("Format");
                    for fmt in [ExportFormat::Csv, ExportFormat::Tsv, ExportFormat::Json] {
                        if ui.radio(self.export_format == fmt, fmt.label()).clicked() {
                            // Keep the file's extension in step with the format.
                            let new_ext = fmt.ext();
                            for old in ["csv", "tsv", "json"] {
                                if let Some(stem) =
                                    self.export_path.strip_suffix(&format!(".{old}"))
                                {
                                    self.export_path = format!("{stem}.{new_ext}");
                                    break;
                                }
                            }
                            self.export_format = fmt;
                            self.export_error = None;
                        }
                    }
                });
                ui.add_space(4.0);
                ui.label("File");
                ui.add(
                    egui::TextEdit::singleline(&mut self.export_path)
                        .desired_width(380.0)
                        .hint_text("Where to write the file"),
                );
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(format!("{} rows will be written.", self.results.len()))
                        .small()
                        .color(self.fg_dim()),
                );
                if let Some(err) = &self.export_error {
                    ui.add_space(4.0);
                    ui.colored_label(ui.visuals().error_fg_color, err);
                }
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if ui.button("Export").clicked() {
                        export = true;
                    }
                    if ui.button("Cancel").clicked() {
                        self.show_export = false;
                    }
                });
            });
        if export {
            self.run_export();
        }
    }

    /// Write the visible rows to `export_path`, or report the failure in the dialog.
    fn run_export(&mut self) {
        let path = PathBuf::from(self.export_path.trim());
        if path.as_os_str().is_empty() {
            self.export_error = Some("Enter a file path.".to_string());
            return;
        }
        match std::fs::write(&path, export_text(&self.results, self.export_format)) {
            Ok(()) => {
                self.show_export = false;
                self.export_error = None;
                notify_desktop(
                    "Results exported",
                    &format!("{} rows written to {}", self.results.len(), path.display()),
                );
            }
            Err(e) => self.export_error = Some(format!("Could not write {}: {e}", path.display())),
        }
    }

    fn category_label(&self) -> String {
        CATEGORIES
            .iter()
            .find(|(_, c)| *c == self.category)
            .map(|(label, _)| (*label).to_string())
            .unwrap_or_else(|| "All files".to_string())
    }

    /// Is the login-autostart entry present?
    fn autostart_enabled() -> bool {
        autostart_path().is_some_and(|p| p.exists())
    }

    /// Create or remove the login-autostart entry (`~/.config/autostart`).
    fn set_autostart(enabled: bool) -> std::io::Result<()> {
        let Some(path) = autostart_path() else {
            return Err(std::io::Error::other("no config directory"));
        };
        if enabled {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            std::fs::write(&path, autostart_entry())
        } else {
            match std::fs::remove_file(&path) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(e),
            }
        }
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
        sort_results(&mut self.all_results, prefer, &needle, self.prefs.fuzzy);
        self.apply_tag_filter();
    }

    /// Empty the current query and result list, leaving tabs and filters alone.
    ///
    /// Wired to the tray's *Clear results* and *New search*: the rows (and any
    /// content-search matches) are the largest transient allocation the UI holds,
    /// so this frees them and leaves an empty table behind.
    fn clear_results(&mut self) {
        self.query.clear();
        self.last_sent.clear();
        self.results.clear();
        self.all_results.clear();
        self.checked.clear();
        self.result_filter.clear();
        self.filter_empty = false;
        self.filter_broken = false;
        self.truncated = false;
        self.error = None;
        self.elapsed_ms = 0;
        self.selected = 0;
        self.scroll_to = None;
        self.pending = false;
        self.preview = None;
        self.dirty = true;
    }

    fn clear_filters(&mut self) {
        self.category = Category::All;
        self.size = SizeFilter::Any;
        self.modified = ModifiedFilter::Any;
        self.extensions.clear();
        self.hidden = false;
        self.case_sensitive = false;
        self.full_path = false;
        let was_content = self.content_mode;
        self.content_mode = false;
        self.full_text = false;
        if self.under.is_some() {
            self.set_under(None);
        } else if was_content {
            // Clearing the filters left content search; release its memory
            // before the re-run (set_under would send the query itself).
            self.release_content_memory();
            self.send_query();
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
                        self.sidebar_tags(ui, &t);
                        ui.add_space(12.0);
                        self.sidebar_locations(ui, &t);
                        ui.add_space(12.0);
                        self.sidebar_advanced(ui, &t);
                        ui.add_space(12.0);
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
            if nav_item(
                ui,
                t,
                category_icon(cat),
                category_color(t, cat),
                label,
                Some(&count),
                selected,
            )
            .clicked()
            {
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
            if nav_item(ui, t, Icon::Bookmark, t.warn, &s.name, None, active).clicked() {
                apply = Some(i);
            }
        }
        if let Some(i) = apply {
            self.apply_saved(i);
        }
    }

    /// User tags, each with how many tagged paths carry it; clicking one filters
    /// the results to that tag.
    fn sidebar_tags(&mut self, ui: &mut egui::Ui, t: &Theme) {
        ui.label(section_title(t, "TAGS"));
        ui.add_space(5.0);
        let all = self.tags.all();
        if all.is_empty() {
            ui.label(
                egui::RichText::new("Right-click a result ▸ Tags…")
                    .size(10.5)
                    .color(t.faint),
            );
            return;
        }
        let mut pick: Option<Option<String>> = None;
        for (tag, count) in &all {
            if !self.sidebar_matches(tag) {
                continue;
            }
            let active = self.tag_filter.as_deref() == Some(tag.as_str());
            let n = count.to_string();
            if nav_item(
                ui,
                t,
                Icon::Tag,
                t.kind_arch,
                &format!("#{tag}"),
                Some(&n),
                active,
            )
            .clicked()
            {
                pick = Some(if active { None } else { Some(tag.clone()) });
            }
        }
        if let Some(p) = pick {
            self.set_tag_filter(p);
        }
    }

    /// Quick “search only here” locations.
    fn sidebar_locations(&mut self, ui: &mut egui::Ui, t: &Theme) {
        ui.label(section_title(t, "INDEXED LOCATIONS"));
        ui.add_space(5.0);
        let locs = locations();
        let mut pick: Option<Option<String>> = None;
        for (i, (label, path)) in locs.iter().enumerate() {
            let s = path.to_string_lossy().into_owned();
            if !self.sidebar_matches(label) && !self.sidebar_matches(&s) {
                continue;
            }
            let active = self.under.as_deref() == Some(s.as_str());
            let count = self
                .location_counts
                .get(i)
                .map(|n| human_count(*n))
                .unwrap_or_default();
            if nav_item(ui, t, Icon::Folder, t.kind_dir, label, Some(&count), active).clicked() {
                pick = Some(if active { None } else { Some(s) });
            }
        }
        // An active filter that is not one of the quick locations.
        if let Some(under) = self.under.clone()
            && !locs.iter().any(|(_, p)| p.to_string_lossy() == under)
        {
            ui.add_space(4.0);
            if nav_item(
                ui,
                t,
                Icon::Folder,
                t.accent,
                &location_label(&under),
                None,
                true,
            )
            .clicked()
            {
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
        ui.add_space(4.0);
        let n_excluded = self.status.exclude_dirs.len();
        if ui
            .add(
                egui::Button::new(
                    egui::RichText::new(format!("Excluded folders ({n_excluded})…"))
                        .size(11.0)
                        .color(t.dim),
                )
                .frame(false),
            )
            .on_hover_text("Folders left out of the index (Tools ▸ Excluded folders…)")
            .clicked()
        {
            self.open_excludes_dialog();
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
             ~/.config/easysearch/config.json (content_index_enabled) and \
             takes effect on restart.",
        );
    }

    /// Collapsible cheat-sheet; its open state is persisted.
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
    /// The central area when the *Search History* tab is selected.
    fn history_view(&mut self, ui: &mut egui::Ui) {
        let t = self.theme();
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("Recent searches")
                    .strong()
                    .size(15.0)
                    .color(t.accent),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add_enabled(
                        !self.prefs.history.is_empty(),
                        egui::Button::new("Clear history"),
                    )
                    .clicked()
                {
                    self.prefs.clear_history();
                }
            });
        });
        ui.separator();
        if self.prefs.history.is_empty() {
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new("Nothing searched yet — your recent queries appear here.")
                    .size(12.5)
                    .color(t.dim),
            );
            return;
        }
        let history = self.prefs.history.clone();
        let mut run: Option<String> = None;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for q in &history {
                    if ui
                        .selectable_label(false, egui::RichText::new(q).size(13.0))
                        .on_hover_text("Run this search again")
                        .clicked()
                    {
                        run = Some(q.clone());
                    }
                }
            });
        if let Some(q) = run {
            self.run_query(&q);
            self.send_query();
        }
    }

    fn results_table(&mut self, ui: &mut egui::Ui) {
        // The central area is the results region: a pointer moving into it takes
        // the keyboard focus off the search field (see `update`).
        self.results_rect = Some(ui.max_rect());

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
        // Only a moving pointer moves the highlight onto the hovered row, so the
        // keyboard can still walk the list while the cursor sits still.
        let pointer_moved = ui.input(|i| i.pointer.delta() != egui::Vec2::ZERO);

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
        let created_w = 92.0_f32;
        let match_w = 120.0_f32;
        let rel_w = 74.0_f32;
        let fixed = check_w
            + num_w
            + path_w
            + type_w
            + size_w
            + mod_w
            + created_w
            + match_w
            + rel_w
            + spacing * 9.0;
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
            .column(Column::exact(created_w))
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
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if sort_button(ui, "Created", self.sort, |s| matches!(s, Sort::Created(_)))
                            .on_hover_text("Birth time, read live (— when the filesystem has none)")
                            .clicked()
                        {
                            self.toggle_sort(Sort::Created(true));
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
                    let r = &self.results[i];
                    // A row reads as selected when it is the cursor row *or* part of
                    // the bulk selection, so "Select all" visibly marks every row.
                    row.set_selected(i == selected || self.checked.contains(&r.path));
                    let rel = relevance_score(&r.path, &needle, self.prefs.fuzzy);
                    let terms = matched_terms(&r.path, &needle, self.prefs.fuzzy);
                    let name = r
                        .path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| r.path.display().to_string());
                    let mut toggle: Option<PathBuf> = None;
                    let row_tags: Vec<String> = self
                        .tags
                        .tags_of(&r.path)
                        .map(|s| s.iter().cloned().collect())
                        .unwrap_or_default();

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
                            for tag in row_tags.iter().take(2) {
                                ui.add_space(2.0);
                                if tag_chip(ui, &t, tag, false)
                                    .on_hover_text(format!("#{tag} — click to filter"))
                                    .clicked()
                                {
                                    self.pending_cmds.push(RowCmd::FilterTag(tag.clone()));
                                }
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
                    // Created (birth time, read live).
                    row.col(|ui| {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(egui::RichText::new(file_created(&r.path)).color(t.dim));
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
                    // Focus follows the pointer: the highlighted row tracks the
                    // cursor, so the row under the mouse — not the first result —
                    // is what the preview, Enter and the context menu act on.
                    if pointer_moved && row_resp.hovered() {
                        self.selected = i;
                    }
                    if row_resp.double_clicked() {
                        App::open(&r.path);
                    }
                    if row_resp.clicked() {
                        self.selected = i;
                    }
                    if let Some(p) = toggle
                        && !self.checked.remove(&p)
                    {
                        self.checked.insert(p);
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
                        let copy_uris = if multi {
                            format!("Copy {n_sel} as URIs")
                        } else {
                            "Copy as URI".to_string()
                        };
                        if ui.button(copy_uris).clicked() {
                            self.pending_cmds.push(RowCmd::CopyUris {
                                path: r.path.clone(),
                                selection: multi,
                            });
                            ui.close_menu();
                        }
                        let copy_shell = if multi {
                            format!("Copy {n_sel} shell-escaped")
                        } else {
                            "Copy shell-escaped".to_string()
                        };
                        if ui
                            .button(copy_shell)
                            .on_hover_text("Quoted for pasting into a shell")
                            .clicked()
                        {
                            self.pending_cmds.push(RowCmd::CopyShell {
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
                        if ui.button("Tags…").clicked() {
                            self.pending_cmds.push(RowCmd::EditTags {
                                path: r.path.clone(),
                                selection: multi,
                            });
                            ui.close_menu();
                        }
                        for tag in &row_tags {
                            if ui.button(format!("Filter by #{tag}")).clicked() {
                                self.pending_cmds.push(RowCmd::FilterTag(tag.clone()));
                                ui.close_menu();
                            }
                        }
                        if self.tag_filter.is_some() && ui.button("Clear tag filter").clicked() {
                            self.pending_cmds.push(RowCmd::ClearTagFilter);
                            ui.close_menu();
                        }
                        if r.is_dir {
                            ui.separator();
                            if ui
                                .button("Exclude folder from the index")
                                .on_hover_text(
                                    "Leave this folder (and everything under it) out of the index.",
                                )
                                .clicked()
                            {
                                self.pending_cmds.push(RowCmd::ExcludeFromIndex(r.path.clone()));
                                ui.close_menu();
                            }
                        }
                        ui.separator();
                        if ui
                            .button("Move to Trash…")
                            .on_hover_text(
                                "Recoverable: the file goes to the desktop Trash, not gone forever.",
                            )
                            .clicked()
                        {
                            self.pending_cmds.push(RowCmd::TrashToConfirm {
                                path: r.path.clone(),
                                selection: multi,
                            });
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
            // Idle state: the mark, then search tips + recent searches.
            let mark = self.logo(ui.ctx());
            ui.add(egui::Image::new((mark.id(), egui::vec2(200.0, 200.0))));
            ui.add_space(12.0);
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
        let tags: Vec<String> = self
            .tags
            .tags_of(&path)
            .map(|s| s.iter().cloned().collect())
            .unwrap_or_default();

        ui.add_space(8.0);
        ui.horizontal(|ui| {
            type_chip(ui, &t, &path, is_dir);
            ui.add_space(4.0);
            ui.label(egui::RichText::new(&name).strong().size(14.0).color(t.text));
            for tag in tags.iter().take(3) {
                ui.add_space(2.0);
                tag_chip(ui, &t, tag, false);
            }
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
                let has_media = self.preview.as_ref().is_some_and(|p| !p.media.is_empty());
                if is_dir {
                    ui.vertical_centered(|ui| {
                        ui.add_space(12.0);
                        ui.label(egui::RichText::new("Folder").size(14.0).color(t.dim));
                    });
                } else if has_media {
                    // Audio/video: a frame (video) plus what ffprobe knows.
                    if let Some(tex) = image {
                        ui.vertical_centered(|ui| {
                            ui.add(
                                egui::Image::new(&tex)
                                    .max_size(egui::vec2(ui.available_width().min(420.0), 300.0))
                                    .corner_radius(10),
                            );
                        });
                        ui.add_space(6.0);
                    }
                    if let Some(pv) = self.preview.as_ref() {
                        for (key, value) in &pv.media {
                            detail_row(ui, &t, key, value);
                        }
                    }
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
                                if pv.markdown {
                                    markdown_preview(ui, &t, &pv.text);
                                } else if let Some(lang) = pv.lang {
                                    code_preview(ui, &t, lang, &pv.text);
                                } else {
                                    ui.label(
                                        egui::RichText::new(&pv.text)
                                            .monospace()
                                            .size(11.5)
                                            .color(t.dim),
                                    );
                                }
                                if pv.truncated {
                                    ui.add_space(6.0);
                                    ui.label(
                                        egui::RichText::new("… preview truncated")
                                            .size(11.0)
                                            .color(t.faint),
                                    );
                                }
                            });
                    } else if let Some(note) = &pv.note {
                        ui.label(egui::RichText::new(note).size(12.0).color(t.faint));
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
                        detail_row(
                            ui,
                            &t,
                            "Tags",
                            &if tags.is_empty() {
                                "—".to_string()
                            } else {
                                tags.iter()
                                    .map(|t| format!("#{t}"))
                                    .collect::<Vec<_>>()
                                    .join("  ")
                            },
                        );
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
            if quick_action(ui, &t, "Tag", Icon::Tag) {
                self.open_tag_editor(vec![path.clone()]);
            }
        });
    }

    // --- ignore files -----------------------------------------------------

    fn open_ignore_dialog(&mut self) {
        self.load_ignore_text();
        self.ignore_msg = None;
        self.show_ignore = true;
    }

    fn load_ignore_text(&mut self) {
        let path = easysearch_core::walker::global_ignore_file();
        self.ignore_text = std::fs::read_to_string(&path).unwrap_or_default();
    }

    /// The “Ignore files” window: toggle honoring ignore files, and edit the
    /// global ignore list (Tools ▸ Ignore files…).
    fn ignore_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_ignore {
            return;
        }
        let t = self.theme();
        let path = easysearch_core::walker::global_ignore_file();
        let mut open = true;
        let mut close = false;
        let mut save = false;
        let mut reload = false;
        let mut rebuild = false;
        let mut toggle: Option<bool> = None;
        let mut follow_toggle: Option<bool> = None;
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
                ui.add_space(4.0);
                let mut follow = self.status.follow_symlinks;
                if ui
                    .checkbox(&mut follow, "Follow symbolic links")
                    .on_hover_text(
                        "Index the targets of symlinked folders too (cycles are skipped).",
                    )
                    .changed()
                {
                    follow_toggle = Some(follow);
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
            // engine's status is still in flight.
            self.status.respect_ignore_files = on;
            self.ignore_msg = Some(if on {
                "Honoring ignore files — reindexing…".to_string()
            } else {
                "Not consulting ignore files — reindexing…".to_string()
            });
        }
        if let Some(on) = follow_toggle {
            self.engine.set_follow_symlinks(on);
            self.status.follow_symlinks = on;
            self.ignore_msg = Some(if on {
                "Following symbolic links — reindexing…".to_string()
            } else {
                "Not following symbolic links — reindexing…".to_string()
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

    // --- excluded folders -------------------------------------------------

    fn open_excludes_dialog(&mut self) {
        let cfg = easysearch_core::Config::load();
        self.exclude_text = cfg.exclude_dirs.join("\n");
        self.exclude_msg = None;
        self.show_excludes = true;
    }

    /// Add `path` to `config.exclude_dirs`, save it and reindex without it.
    /// Used by the result-row context menu.
    fn exclude_dir_now(&mut self, path: &Path) {
        let mut cfg = easysearch_core::Config::load();
        let entry = path.to_string_lossy().into_owned();
        if !cfg.exclude_dirs.iter().any(|d| d.trim() == entry.trim()) {
            cfg.exclude_dirs.push(entry);
        }
        self.exclude_text = cfg.exclude_dirs.join("\n");
        self.show_excludes = true;
        match cfg.save() {
            Ok(()) => {
                self.engine.set_exclude_dirs(cfg.exclude_dirs.clone());
                self.exclude_msg = Some(format!("Excluded {} — reindexing…", path.display()));
            }
            Err(e) => self.exclude_msg = Some(format!("Cannot save config: {e}")),
        }
    }

    /// The “Excluded folders” window: edit `config.exclude_dirs` (Tools ▸
    /// Excluded folders…). Unlike ignore patterns these are exact folders — the
    /// folder and everything under it is left out of the index.
    fn excludes_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_excludes {
            return;
        }
        let t = self.theme();
        let config_path = easysearch_core::Config::default_path();
        let mut open = true;
        let mut close = false;
        let mut save = false;
        let mut reload = false;
        egui::Window::new("Excluded folders")
            .collapsible(false)
            .resizable(true)
            .default_width(560.0)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.label(
                    egui::RichText::new(
                        "Whole directory trees to leave out of the index, one absolute path per \
                         line (`~` means your home folder). Unlike the ignore patterns, these \
                         are exact folders: the folder and everything under it is skipped.",
                    )
                    .size(12.0)
                    .color(t.dim),
                );
                ui.add_space(8.0);
                ui.label(egui::RichText::new("config.json ▸ exclude_dirs").strong());
                ui.label(
                    egui::RichText::new(config_path.display().to_string())
                        .monospace()
                        .size(11.0)
                        .color(t.faint),
                );
                ui.add_space(6.0);
                egui::ScrollArea::vertical()
                    .max_height(230.0)
                    .show(ui, |ui| {
                        ui.add(
                            egui::TextEdit::multiline(&mut self.exclude_text)
                                .font(egui::TextStyle::Monospace)
                                .desired_width(f32::INFINITY)
                                .desired_rows(10)
                                .hint_text("~/VirtualBox VMs\n/srv/scratch"),
                        );
                    });
                if let Some(msg) = &self.exclude_msg {
                    ui.add_space(4.0);
                    ui.label(egui::RichText::new(msg).size(11.5).color(t.dim));
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui
                        .button("Save & rebuild")
                        .on_hover_text("Write config.json and reindex without these folders.")
                        .clicked()
                    {
                        save = true;
                    }
                    if ui.button("Reload").clicked() {
                        reload = true;
                    }
                    if ui.button("Close").clicked() {
                        close = true;
                    }
                });
            });
        self.show_excludes = open && !close;
        if reload {
            let cfg = easysearch_core::Config::load();
            self.exclude_text = cfg.exclude_dirs.join("\n");
            self.exclude_msg = Some("Reloaded from config.json.".to_string());
        }
        if save {
            let mut cfg = easysearch_core::Config::load();
            cfg.exclude_dirs = self
                .exclude_text
                .lines()
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty())
                .collect();
            self.exclude_text = cfg.exclude_dirs.join("\n");
            match cfg.save() {
                Ok(()) => {
                    self.engine.set_exclude_dirs(cfg.exclude_dirs.clone());
                    self.exclude_msg = Some(format!(
                        "Saved {} folder(s) — reindexing…",
                        cfg.exclude_dirs.len()
                    ));
                }
                Err(e) => self.exclude_msg = Some(format!("Cannot save config: {e}")),
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
            if self.content_mode {
                ui.label(
                    egui::RichText::new("· content search: more memory & CPU")
                        .size(11.5)
                        .color(t.warn),
                )
                .on_hover_text(
                    "Content search scans your files live with ripgrep, so it needs more \
                     memory and CPU than filename search. Switching the scope back to \
                     Filenames releases it.",
                );
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

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // Rightmost item of the status bar: the running version. It is
                // added first in this right-to-left layout so the (potentially
                // long) hints string can never push it out of the window.
                let app_version = crate::version_stamp();
                ui.label(egui::RichText::new(app_version).size(10.5).color(t.faint))
                    .on_hover_text("Running version — fully offline, no network access");
                ui.separator();
                // UI zoom, as a compact dropdown on the right.
                egui::ComboBox::from_id_salt("status-zoom")
                    .selected_text(
                        egui::RichText::new(format!("{:.0}%", self.prefs.zoom * 100.0)).size(10.5),
                    )
                    .width(72.0)
                    .show_ui(ui, |ui| {
                        for level in ZOOM_LEVELS {
                            let label = format!("{:.0}%", level * 100.0);
                            if ui
                                .selectable_label((self.prefs.zoom - *level).abs() < 0.001, label)
                                .clicked()
                            {
                                self.prefs.zoom = *level;
                                ui.ctx().set_zoom_factor(*level);
                                self.prefs.save();
                            }
                        }
                    })
                    .response
                    .on_hover_text("UI zoom");
                ui.label(bar_label(&t, "Zoom"));
                ui.separator();
                ui.label(
                    egui::RichText::new("↑↓ · Enter open · Ctrl+F search · Ctrl+A all · Esc clear")
                        .size(10.5)
                        .color(t.dim),
                );
                ui.add_space(8.0);
                ui.separator();
                ui.label(
                    egui::RichText::new(format!("Entries: {}", human_count(files + dirs)))
                        .size(11.0)
                        .color(t.dim),
                );
                ui.separator();
                ui.label(
                    egui::RichText::new(format!("Search time: {} ms", self.elapsed_ms))
                        .size(11.0)
                        .color(t.dim),
                );
                ui.separator();
                ui.label(
                    egui::RichText::new(format!(
                        "Query: {} results",
                        human_count(self.results.len() as u64)
                    ))
                    .size(11.0)
                    .color(t.dim),
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

    // --- tags -------------------------------------------------------------

    /// Rebuild the displayed results from the raw set, applying the active tag
    /// filter and the results-header “Filter results…” text box.
    fn apply_tag_filter(&mut self) {
        let needle = self.result_filter.trim().to_lowercase();
        self.results = self
            .all_results
            .iter()
            .filter(|r| match &self.tag_filter {
                Some(tag) => self.tags.has(&r.path, tag),
                None => true,
            })
            .filter(|r| path_contains(&r.path, &needle))
            .filter(|r| !self.filter_empty || is_empty_entry(&r.path, r.is_dir))
            .filter(|r| !self.filter_broken || is_broken_symlink(&r.path))
            .cloned()
            .collect();
        if self.selected >= self.results.len() {
            self.selected = self.results.len().saturating_sub(1);
        }
    }

    /// Set (or, with `None`, clear) the tag filter and refresh the visible rows.
    fn set_tag_filter(&mut self, tag: Option<String>) {
        self.tag_filter = tag
            .map(|t| t.trim().trim_start_matches('#').trim().to_string())
            .filter(|t| !t.is_empty());
        self.apply_tag_filter();
        self.prune_selection();
        self.preview = None;
    }

    /// Open the tag editor for `paths` (a single row or the checked selection).
    fn open_tag_editor(&mut self, paths: Vec<PathBuf>) {
        if paths.is_empty() {
            return;
        }
        self.tag_targets = paths;
        self.tag_input.clear();
        self.show_tags = true;
    }

    /// The tag editor: add a tag to, or remove one from, every target at once.
    fn tags_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_tags {
            return;
        }
        let t = self.theme();
        let targets = self.tag_targets.clone();
        let mut open = true;
        egui::Window::new("Tags")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(400.0)
            .show(ctx, |ui| {
                let who = match targets.as_slice() {
                    [one] => one
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| one.display().to_string()),
                    many => format!("{} files", many.len()),
                };
                ui.label(egui::RichText::new(who).strong().size(13.0).color(t.text));
                if let Some(first) = targets.first() {
                    ui.label(
                        egui::RichText::new(short_dir(first))
                            .size(10.5)
                            .color(t.faint),
                    );
                }
                ui.add_space(8.0);

                ui.horizontal(|ui| {
                    let resp = ui.add(
                        egui::TextEdit::singleline(&mut self.tag_input)
                            .hint_text("Add a tag…")
                            .desired_width(ui.available_width() - 66.0),
                    );
                    let submit = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if ui.button("Add").clicked() || submit {
                        let tag = self.tag_input.clone();
                        if !tag.trim().is_empty() {
                            let mut changed = false;
                            for p in &targets {
                                changed |= self.tags.add(p, &tag);
                            }
                            if changed {
                                let _ = self.tags.save();
                                self.apply_tag_filter();
                            }
                        }
                        self.tag_input.clear();
                    }
                });
                ui.add_space(10.0);

                // The union of the targets' tags, with how many carry each.
                let mut counts: BTreeMap<String, usize> = BTreeMap::new();
                for p in &targets {
                    if let Some(tags) = self.tags.tags_of(p) {
                        for tag in tags {
                            *counts.entry(tag.clone()).or_insert(0) += 1;
                        }
                    }
                }
                if counts.is_empty() {
                    ui.label(
                        egui::RichText::new("No tags yet — type one above.")
                            .size(11.5)
                            .color(t.faint),
                    );
                } else {
                    ui.label(section_title(&t, "CURRENT TAGS"));
                    ui.add_space(5.0);
                    let mut remove: Option<String> = None;
                    for (tag, n) in &counts {
                        ui.horizontal(|ui| {
                            let label = if *n == targets.len() {
                                tag.clone()
                            } else {
                                format!("{tag}  ({n}/{})", targets.len())
                            };
                            tag_chip(ui, &t, &label, false);
                            if ui
                                .add(
                                    egui::Button::new(
                                        egui::RichText::new("✕").size(11.0).color(t.faint),
                                    )
                                    .frame(false),
                                )
                                .on_hover_text("Remove from all targets")
                                .clicked()
                            {
                                remove = Some(tag.clone());
                            }
                        });
                    }
                    if let Some(tag) = remove {
                        for p in &targets {
                            self.tags.remove(p, &tag);
                        }
                        let _ = self.tags.save();
                        self.apply_tag_filter();
                    }
                }

                ui.add_space(10.0);
                ui.separator();
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Close").clicked() {
                        self.show_tags = false;
                    }
                });
            });
        if !open {
            self.show_tags = false;
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
            RowCmd::CopyUris { path, selection } => {
                let mut v: Vec<String> = if selection {
                    self.checked.iter().map(|p| file_uri(p)).collect()
                } else {
                    vec![file_uri(&path)]
                };
                v.sort();
                ctx.copy_text(v.join("\n"));
            }
            RowCmd::CopyShell { path, selection } => {
                let mut v: Vec<String> = if selection {
                    self.checked
                        .iter()
                        .map(|p| shell_escape(&p.to_string_lossy()))
                        .collect()
                } else {
                    vec![shell_escape(&path.to_string_lossy())]
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
            RowCmd::EditTags { path, selection } => {
                let targets: Vec<PathBuf> = if selection {
                    self.checked.iter().cloned().collect()
                } else {
                    vec![path]
                };
                self.open_tag_editor(targets);
            }
            RowCmd::FilterTag(tag) => self.set_tag_filter(Some(tag)),
            RowCmd::ClearTagFilter => self.set_tag_filter(None),
            RowCmd::ExcludeFromIndex(p) => {
                self.exclude_dir_now(&p);
            }
            RowCmd::TrashToConfirm { path, selection } => {
                let mut v: Vec<PathBuf> = if selection {
                    self.checked.iter().cloned().collect()
                } else {
                    vec![path]
                };
                v.sort();
                v.dedup();
                self.trash_confirm = v;
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
        self.dup_checked.clear();
        self.trash_msg = None;
        self.show_dups = true;
        let _ = self.dup_tx.send(DupRequest {
            generation: self.dup_gen,
            paths,
        });
    }

    /// Move `paths` to the freedesktop trash, then drop them from every list.
    ///
    /// Nothing is deleted permanently: each file is recorded in
    /// `~/.local/share/Trash` with its original path, so it can be restored.
    /// Only paths that were actually moved are removed from the results.
    fn trash_paths(&mut self, paths: &[PathBuf]) {
        let mut moved: Vec<PathBuf> = Vec::new();
        let mut failed = 0usize;
        for p in paths {
            match easysearch_core::trash::move_to_trash(p) {
                Ok(_) => moved.push(p.clone()),
                Err(e) => {
                    failed += 1;
                    eprintln!("trash: {e}");
                }
            }
        }
        if !moved.is_empty() {
            let gone: HashSet<PathBuf> = moved.iter().cloned().collect();
            self.results.retain(|r| !gone.contains(&r.path));
            self.all_results.retain(|r| !gone.contains(&r.path));
            self.checked.retain(|p| !gone.contains(p));
            self.dup_checked.retain(|p| !gone.contains(p));
            if let Some(report) = &mut self.dups {
                for g in &mut report.groups {
                    g.paths.retain(|p| !gone.contains(p));
                }
                report.groups.retain(|g| g.paths.len() > 1);
            }
            if self.selected >= self.results.len() {
                self.selected = self.results.len().saturating_sub(1);
            }
            self.preview = None;
        }
        self.trash_msg = Some(match (moved.len(), failed) {
            (0, f) => format!("Could not move {f} file(s) to the Trash."),
            (m, 0) => format!("Moved {m} file(s) to the Trash."),
            (m, f) => format!("Moved {m} file(s) to the Trash; {f} failed."),
        });
    }

    /// Confirm a move to the trash before doing it — recoverable, but a change
    /// on disk all the same.
    fn trash_confirm_dialog(&mut self, ctx: &egui::Context) {
        if self.trash_confirm.is_empty() {
            return;
        }
        let t = self.theme();
        let n = self.trash_confirm.len();
        let mut open = true;
        let mut cancel = false;
        let mut confirm = false;
        egui::Window::new("Move to Trash?")
            .collapsible(false)
            .resizable(false)
            .default_width(470.0)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.label(
                    egui::RichText::new(format!(
                        "Move {n} file{} to the Trash?",
                        if n == 1 { "" } else { "s" }
                    ))
                    .strong()
                    .size(13.0)
                    .color(t.text),
                );
                ui.label(
                    egui::RichText::new(
                        "Nothing is deleted permanently — they can be restored from the desktop \
                         Trash.",
                    )
                    .size(12.0)
                    .color(t.dim),
                );
                ui.add_space(8.0);
                egui::ScrollArea::vertical()
                    .max_height(180.0)
                    .show(ui, |ui| {
                        for p in self.trash_confirm.iter().take(200) {
                            ui.label(
                                egui::RichText::new(p.display().to_string())
                                    .monospace()
                                    .size(11.0)
                                    .color(t.faint),
                            );
                        }
                        if n > 200 {
                            ui.label(
                                egui::RichText::new(format!("…and {} more", n - 200))
                                    .size(11.0)
                                    .color(t.faint),
                            );
                        }
                    });
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                    if ui
                        .button("Move to Trash")
                        .on_hover_text("Recoverable from the desktop Trash.")
                        .clicked()
                    {
                        confirm = true;
                    }
                });
            });
        if cancel || !open {
            self.trash_confirm.clear();
        } else if confirm {
            let paths = std::mem::take(&mut self.trash_confirm);
            self.trash_paths(&paths);
        }
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

                // Bulk helpers: the safe default is to keep one copy per group.
                ui.horizontal(|ui| {
                    if ui
                        .button("Select all but one per group")
                        .on_hover_text("Keep the most recently modified copy of each group.")
                        .clicked()
                    {
                        for g in &report.groups {
                            let keep = newest_path(&g.paths);
                            for p in &g.paths {
                                if Some(p) != keep.as_ref() {
                                    self.dup_checked.insert(p.clone());
                                }
                            }
                        }
                    }
                    if ui.button("Clear selection").clicked() {
                        self.dup_checked.clear();
                    }
                    ui.label(
                        egui::RichText::new(format!("{} selected", self.dup_checked.len()))
                            .size(11.5)
                            .color(t.dim),
                    );
                });
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
                                        if ui.small_button("Select all").clicked() {
                                            for p in &g.paths {
                                                self.dup_checked.insert(p.clone());
                                            }
                                        }
                                        if ui
                                            .small_button("Keep newest")
                                            .on_hover_text(
                                                "Check every copy except the newest one.",
                                            )
                                            .clicked()
                                        {
                                            let keep = newest_path(&g.paths);
                                            for p in &g.paths {
                                                if Some(p) != keep.as_ref() {
                                                    self.dup_checked.insert(p.clone());
                                                } else {
                                                    self.dup_checked.remove(p);
                                                }
                                            }
                                        }
                                    },
                                );
                            });
                            for p in &g.paths {
                                ui.horizontal(|ui| {
                                    ui.add_space(14.0);
                                    let mut on = self.dup_checked.contains(p);
                                    if ui.add(egui::Checkbox::without_text(&mut on)).changed() {
                                        if on {
                                            self.dup_checked.insert(p.clone());
                                        } else {
                                            self.dup_checked.remove(p);
                                        }
                                    }
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

                // Footer: the trash action, plus a warning when a whole group
                // is checked (that would remove every remaining copy).
                let n_dup = self.dup_checked.len();
                let whole_group = report.groups.iter().any(|g| {
                    !g.paths.is_empty() && g.paths.iter().all(|p| self.dup_checked.contains(p))
                });
                ui.add_space(6.0);
                ui.separator();
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if let Some(msg) = &self.trash_msg {
                        ui.label(egui::RichText::new(msg).size(11.5).color(t.dim));
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add_enabled(
                                n_dup > 0,
                                egui::Button::new(egui::RichText::new(format!(
                                    "Move {n_dup} to Trash…"
                                ))),
                            )
                            .on_hover_text(
                                "Recoverable: the files go to the desktop Trash, not gone forever.",
                            )
                            .clicked()
                        {
                            self.trash_confirm = self.dup_checked.iter().cloned().collect();
                        }
                    });
                });
                if whole_group {
                    ui.label(
                        egui::RichText::new(
                            "⚠ A whole group is checked — every remaining copy would be trashed.",
                        )
                        .size(11.0)
                        .color(t.warn),
                    );
                }
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

// --- previews -------------------------------------------------------------

/// The head of a file, decoded as text when it looks like text.
#[derive(Debug)]
enum TextHead {
    Text {
        text: String,
        truncated: bool,
    },
    /// Read as bytes and looks binary.
    Binary,
    Empty,
    Unreadable,
}

/// Read up to `cap` bytes of a file and decide whether it is text.
fn read_text_head(path: &Path, cap: usize) -> TextHead {
    use std::io::Read;
    let Ok(mut file) = std::fs::File::open(path) else {
        return TextHead::Unreadable;
    };
    let Ok(meta) = file.metadata() else {
        return TextHead::Unreadable;
    };
    if meta.len() == 0 {
        return TextHead::Empty;
    }
    let mut buf = vec![0u8; cap];
    let mut filled = 0;
    while filled < cap {
        match file.read(&mut buf[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(_) => return TextHead::Unreadable,
        }
    }
    buf.truncate(filled);
    if !is_probably_text(&buf) {
        return TextHead::Binary;
    }
    let truncated = meta.len() > filled as u64;
    // A multi-byte character may straddle the cut; `lossy` replaces the tail.
    TextHead::Text {
        text: String::from_utf8_lossy(&buf).into_owned(),
        truncated,
    }
}

/// The first `cap` bytes of `text`, cut on a character boundary.
fn head_of(text: &str, cap: usize) -> (&str, bool) {
    if text.len() <= cap {
        return (text, false);
    }
    let mut end = cap;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    (&text[..end], true)
}

fn ext_lower(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
}

fn is_markdown_file(path: &Path) -> bool {
    matches!(
        ext_lower(path).as_deref(),
        Some("md" | "markdown" | "mdown")
    )
}

/// Documents whose text layer is extracted in-process (see `core::content_index`).
///
/// The OpenDocument (LibreOffice) formats are first-class here: `.odt`, `.ods`
/// and `.odp` are all read through the same in-process ODF reader.
fn is_document_file(path: &Path) -> bool {
    matches!(
        ext_lower(path).as_deref(),
        Some("pdf" | "docx" | "odt" | "ods" | "odp" | "odg")
    )
}

/// Formats with no in-app preview yet, so the panel says so instead of guessing.
fn is_spreadsheet_or_slides(path: &Path) -> bool {
    matches!(
        ext_lower(path).as_deref(),
        Some("xls" | "xlsx" | "ppt" | "pptx")
    )
}

fn is_video_file(path: &Path) -> bool {
    matches!(
        ext_lower(path).as_deref(),
        Some("mp4" | "mkv" | "avi" | "mov" | "webm" | "flv" | "mpg" | "mpeg" | "wmv" | "m4v")
    )
}

fn is_av_file(path: &Path) -> bool {
    is_video_file(path)
        || matches!(
            ext_lower(path).as_deref(),
            Some("mp3" | "wav" | "flac" | "ogg" | "oga" | "m4a" | "aac" | "opus" | "wma")
        )
}

/// Decode an image file to a texture, scaled down for the panel.
fn load_image_texture(ctx: &egui::Context, path: &Path) -> Option<egui::TextureHandle> {
    let bytes = std::fs::read(path).ok()?;
    let decoded = image::load_from_memory(&bytes).ok()?;
    texture_from_image(ctx, decoded, &format!("preview:{}", path.display()))
}

fn texture_from_image(
    ctx: &egui::Context,
    decoded: image::DynamicImage,
    name: &str,
) -> Option<egui::TextureHandle> {
    let thumb = decoded.thumbnail(1024, 1024);
    let rgba = thumb.to_rgba8();
    let (w, h) = (rgba.width() as usize, rgba.height() as usize);
    Some(ctx.load_texture(
        name,
        egui::ColorImage::from_rgba_unmultiplied([w, h], rgba.as_raw()),
        egui::TextureOptions::LINEAR,
    ))
}

/// A frame from a video, via `ffmpeg` when it is installed. Best effort.
fn video_thumbnail(ctx: &egui::Context, path: &Path) -> Option<egui::TextureHandle> {
    // Seek a little in, so the frame is not a black title card.
    let out = Command::new("ffmpeg")
        .args(["-v", "quiet", "-ss", "1", "-i"])
        .arg(path)
        .args(["-frames:v", "1", "-f", "image2", "-vcodec", "png", "pipe:1"])
        .output()
        .ok()?;
    if !out.status.success() || out.stdout.is_empty() {
        return None;
    }
    let decoded = image::load_from_memory(&out.stdout).ok()?;
    texture_from_image(ctx, decoded, &format!("vthumb:{}", path.display()))
}

/// Audio/video metadata via `ffprobe` (JSON), when it is installed. Best effort.
fn media_metadata(path: &Path) -> Vec<(String, String)> {
    let Ok(out) = Command::new("ffprobe")
        .args([
            "-v",
            "quiet",
            "-print_format",
            "json",
            "-show_format",
            "-show_streams",
        ])
        .arg(path)
        .output()
    else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&out.stdout) else {
        return Vec::new();
    };

    let mut rows: Vec<(String, String)> = Vec::new();
    if let Some(format) = value.get("format") {
        if let Some(secs) = format
            .get("duration")
            .and_then(|d| d.as_str())
            .and_then(|d| d.parse::<f64>().ok())
        {
            rows.push(("Duration".into(), human_duration(secs)));
        }
        if let Some(long) = format.get("format_long_name").and_then(|n| n.as_str()) {
            rows.push(("Format".into(), long.to_string()));
        }
        if let Some(bits) = format
            .get("bit_rate")
            .and_then(|b| b.as_str())
            .and_then(|b| b.parse::<u64>().ok())
        {
            rows.push((
                "Bitrate".into(),
                format!("{:.0} kbps", bits as f64 / 1000.0),
            ));
        }
        for (key, label) in [("artist", "Artist"), ("title", "Title"), ("album", "Album")] {
            if let Some(tag) = format
                .get("tags")
                .and_then(|tags| tags.get(key))
                .and_then(|v| v.as_str())
            {
                rows.push((label.into(), tag.to_string()));
            }
        }
    }
    if let Some(streams) = value.get("streams").and_then(|s| s.as_array()) {
        if let Some(video) = streams
            .iter()
            .find(|s| s.get("codec_type").and_then(|c| c.as_str()) == Some("video"))
        {
            if let Some(codec) = video.get("codec_name").and_then(|c| c.as_str()) {
                rows.push(("Video codec".into(), codec.to_string()));
            }
            if let (Some(w), Some(h)) = (
                video.get("width").and_then(|x| x.as_u64()),
                video.get("height").and_then(|x| x.as_u64()),
            ) {
                rows.push(("Resolution".into(), format!("{w}×{h}")));
            }
        }
        if let Some(audio) = streams
            .iter()
            .find(|s| s.get("codec_type").and_then(|c| c.as_str()) == Some("audio"))
        {
            if let Some(codec) = audio.get("codec_name").and_then(|c| c.as_str()) {
                rows.push(("Audio codec".into(), codec.to_string()));
            }
            if let Some(rate) = audio.get("sample_rate").and_then(|x| x.as_str()) {
                rows.push(("Sample rate".into(), format!("{rate} Hz")));
            }
            if let Some(channels) = audio.get("channels").and_then(|x| x.as_u64()) {
                rows.push(("Channels".into(), channels.to_string()));
            }
        }
    }
    rows
}

fn human_duration(secs: f64) -> String {
    let total = secs.round().max(0.0) as u64;
    let (h, m, s) = (total / 3600, (total % 3600) / 60, total % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// Render Markdown with light styling: headings, bullets, quotes, fenced code
/// (syntax-highlighted when the fence names a language) and the inline emphasis
/// markers stripped. Deliberately small — this is a preview, not a Markdown
/// engine.
fn markdown_preview(ui: &mut egui::Ui, t: &Theme, text: &str) {
    let mut lines = text.lines();
    while let Some(raw) = lines.next() {
        let trimmed = raw.trim_start();
        if let Some(tag) = trimmed.strip_prefix("```") {
            let tag = tag.trim();
            let mut block = String::new();
            for line in lines.by_ref() {
                if line.trim_start().starts_with("```") {
                    break;
                }
                block.push_str(line);
                block.push('\n');
            }
            let body = block.trim_end_matches('\n');
            if let Some(lang) = Lang::from_tag(tag) {
                code_preview(ui, t, lang, body);
            } else {
                for line in body.lines() {
                    ui.label(
                        egui::RichText::new(line)
                            .monospace()
                            .size(11.0)
                            .color(t.dim),
                    );
                }
            }
            continue;
        }
        if trimmed.is_empty() {
            ui.add_space(5.0);
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("# ") {
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new(strip_emphasis(rest))
                    .strong()
                    .size(16.5)
                    .color(t.text),
            );
        } else if let Some(rest) = heading(trimmed) {
            ui.add_space(3.0);
            ui.label(
                egui::RichText::new(strip_emphasis(rest))
                    .strong()
                    .size(14.0)
                    .color(t.text),
            );
        } else if let Some(rest) = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
            .or_else(|| trimmed.strip_prefix("+ "))
        {
            ui.label(
                egui::RichText::new(format!("• {}", strip_emphasis(rest)))
                    .size(12.5)
                    .color(t.text),
            );
        } else if let Some(rest) = trimmed.strip_prefix("> ") {
            ui.label(
                egui::RichText::new(strip_emphasis(rest))
                    .italics()
                    .size(12.5)
                    .color(t.dim),
            );
        } else {
            ui.label(
                egui::RichText::new(strip_emphasis(raw))
                    .size(12.5)
                    .color(t.text),
            );
        }
    }
}

/// How big a file still gets syntax highlighting. Larger previews fall back to
/// plain monospace, so laying out tens of thousands of coloured runs never
/// stalls the UI.
const MAX_HIGHLIGHT_BYTES: usize = 128 * 1024;

/// A language the preview highlighter has a token table for. This is a keyword
/// table plus comment/string syntax, **not** a grammar: the scanner has no
/// parser, no error recovery and no state between lines, which is all a preview
/// needs.
#[derive(Clone, Copy)]
enum Lang {
    Rust,
    Python,
    Js,
    C,
    Java,
    Go,
    Shell,
    Json,
    Yaml,
    Toml,
    Markup,
    Css,
    Sql,
    Ruby,
    Php,
    Lua,
    Kotlin,
    Swift,
    Csharp,
    R,
    Haskell,
    Scala,
    Dart,
    Perl,
    Ini,
    Make,
    Docker,
    Nix,
    Elixir,
    Erlang,
    Clojure,
}

/// The token classes and punctuation a [`Lang`] recognises.
struct LangSpec {
    /// Markers that begin a comment that runs to the end of the line.
    line: &'static [&'static str],
    /// A `(open, close)` pair for a comment that can span lines.
    block: Option<(&'static str, &'static str)>,
    /// Quote characters that open a string.
    quotes: &'static [char],
    /// Whether a string may span lines (template literals, triple quotes, …).
    multiline_strings: bool,
    keywords: &'static [&'static str],
    types: &'static [&'static str],
    constants: &'static [&'static str],
}

impl Lang {
    /// The language for a Markdown fence tag (`rust`, `python`, `sh`, …).
    fn from_tag(tag: &str) -> Option<Lang> {
        let t = tag.trim().to_ascii_lowercase();
        Some(match t.as_str() {
            "rust" | "rs" => Lang::Rust,
            "python" | "py" | "python3" => Lang::Python,
            "js" | "javascript" | "jsx" | "ts" | "typescript" | "tsx" | "node" => Lang::Js,
            "c" | "h" | "cpp" | "c++" | "cxx" | "hpp" | "objc" => Lang::C,
            "java" => Lang::Java,
            "go" | "golang" => Lang::Go,
            "sh" | "bash" | "shell" | "zsh" | "console" => Lang::Shell,
            "json" | "jsonc" => Lang::Json,
            "yaml" | "yml" => Lang::Yaml,
            "toml" => Lang::Toml,
            "html" | "xml" | "svg" | "vue" | "svelte" | "markup" => Lang::Markup,
            "css" | "scss" | "sass" | "less" => Lang::Css,
            "sql" => Lang::Sql,
            "ruby" | "rb" => Lang::Ruby,
            "php" => Lang::Php,
            "lua" => Lang::Lua,
            "kotlin" | "kt" => Lang::Kotlin,
            "swift" => Lang::Swift,
            "csharp" | "c#" | "cs" => Lang::Csharp,
            "r" => Lang::R,
            "haskell" | "hs" => Lang::Haskell,
            "scala" => Lang::Scala,
            "dart" => Lang::Dart,
            "perl" | "pl" => Lang::Perl,
            "ini" => Lang::Ini,
            "make" | "makefile" => Lang::Make,
            "docker" | "dockerfile" => Lang::Docker,
            "nix" => Lang::Nix,
            "elixir" | "ex" | "exs" => Lang::Elixir,
            "erlang" | "erl" => Lang::Erlang,
            "clojure" | "clj" => Lang::Clojure,
            _ => return None,
        })
    }

    fn spec(self) -> LangSpec {
        match self {
            Lang::Rust => LangSpec {
                line: &["//"],
                block: Some(("/*", "*/")),
                quotes: &['"'],
                multiline_strings: false,
                keywords: &[
                    "as",
                    "async",
                    "await",
                    "break",
                    "const",
                    "continue",
                    "crate",
                    "dyn",
                    "else",
                    "enum",
                    "extern",
                    "fn",
                    "for",
                    "if",
                    "impl",
                    "in",
                    "let",
                    "loop",
                    "match",
                    "mod",
                    "move",
                    "mut",
                    "pub",
                    "ref",
                    "return",
                    "self",
                    "static",
                    "struct",
                    "super",
                    "trait",
                    "type",
                    "unsafe",
                    "use",
                    "where",
                    "while",
                    "union",
                    "macro_rules",
                    "trait",
                ],
                types: &[
                    "bool", "char", "str", "String", "Vec", "Option", "Result", "Box", "Rc", "Arc",
                    "HashMap", "HashSet", "BTreeMap", "Cow", "i8", "i16", "i32", "i64", "i128",
                    "isize", "u8", "u16", "u32", "u64", "u128", "usize", "f32", "f64", "Self",
                ],
                constants: &["true", "false", "None", "Some", "Ok", "Err"],
            },
            Lang::Python => LangSpec {
                line: &["#"],
                block: None,
                quotes: &['"', '\''],
                multiline_strings: true,
                keywords: &[
                    "and", "as", "assert", "async", "await", "break", "class", "continue", "def",
                    "del", "elif", "else", "except", "finally", "for", "from", "global", "if",
                    "import", "in", "is", "lambda", "match", "case", "nonlocal", "not", "or",
                    "pass", "raise", "return", "try", "while", "with", "yield",
                ],
                types: &[
                    "bool",
                    "bytes",
                    "dict",
                    "float",
                    "frozenset",
                    "int",
                    "list",
                    "object",
                    "set",
                    "str",
                    "tuple",
                    "type",
                    "Exception",
                ],
                constants: &["True", "False", "None", "self", "cls", "__name__"],
            },
            Lang::Js => LangSpec {
                line: &["//"],
                block: Some(("/*", "*/")),
                quotes: &['"', '\'', '`'],
                multiline_strings: true,
                keywords: &[
                    "async",
                    "await",
                    "break",
                    "case",
                    "catch",
                    "class",
                    "const",
                    "continue",
                    "debugger",
                    "default",
                    "delete",
                    "do",
                    "else",
                    "export",
                    "extends",
                    "finally",
                    "for",
                    "from",
                    "function",
                    "get",
                    "if",
                    "import",
                    "in",
                    "instanceof",
                    "let",
                    "new",
                    "of",
                    "return",
                    "set",
                    "static",
                    "super",
                    "switch",
                    "throw",
                    "try",
                    "typeof",
                    "var",
                    "void",
                    "while",
                    "with",
                    "yield",
                ],
                types: &[
                    "Array", "Boolean", "Date", "Error", "JSON", "Map", "Math", "Number", "Object",
                    "Promise", "RegExp", "Set", "String", "Symbol", "console", "document",
                    "window",
                ],
                constants: &[
                    "true",
                    "false",
                    "null",
                    "undefined",
                    "NaN",
                    "Infinity",
                    "this",
                ],
            },
            Lang::C => LangSpec {
                line: &["//"],
                block: Some(("/*", "*/")),
                quotes: &['"', '\''],
                multiline_strings: false,
                keywords: &[
                    "auto",
                    "break",
                    "case",
                    "catch",
                    "class",
                    "const",
                    "constexpr",
                    "continue",
                    "default",
                    "delete",
                    "do",
                    "else",
                    "enum",
                    "explicit",
                    "extern",
                    "for",
                    "friend",
                    "goto",
                    "if",
                    "inline",
                    "namespace",
                    "new",
                    "operator",
                    "private",
                    "protected",
                    "public",
                    "register",
                    "return",
                    "sizeof",
                    "static",
                    "struct",
                    "switch",
                    "template",
                    "this",
                    "throw",
                    "try",
                    "typedef",
                    "typename",
                    "union",
                    "using",
                    "virtual",
                    "volatile",
                    "while",
                ],
                types: &[
                    "bool", "char", "double", "float", "int", "long", "short", "signed", "size_t",
                    "string", "unsigned", "void", "wchar_t", "FILE", "vector", "map", "set",
                ],
                constants: &["true", "false", "NULL", "nullptr", "EOF"],
            },
            Lang::Java => LangSpec {
                line: &["//"],
                block: Some(("/*", "*/")),
                quotes: &['"', '\''],
                multiline_strings: false,
                keywords: &[
                    "abstract",
                    "assert",
                    "break",
                    "case",
                    "catch",
                    "class",
                    "continue",
                    "default",
                    "do",
                    "else",
                    "enum",
                    "extends",
                    "final",
                    "finally",
                    "for",
                    "if",
                    "implements",
                    "import",
                    "instanceof",
                    "interface",
                    "native",
                    "new",
                    "package",
                    "private",
                    "protected",
                    "public",
                    "record",
                    "return",
                    "sealed",
                    "static",
                    "strictfp",
                    "super",
                    "switch",
                    "synchronized",
                    "throw",
                    "throws",
                    "transient",
                    "try",
                    "volatile",
                    "while",
                    "var",
                    "sealed",
                    "permits",
                    "yield",
                ],
                types: &[
                    "String",
                    "Integer",
                    "Long",
                    "Double",
                    "Float",
                    "Boolean",
                    "Object",
                    "List",
                    "Map",
                    "Set",
                    "ArrayList",
                    "HashMap",
                    "Optional",
                    "Stream",
                    "byte",
                    "char",
                    "double",
                    "float",
                    "int",
                    "long",
                    "short",
                    "void",
                ],
                constants: &["true", "false", "null", "this", "super"],
            },
            Lang::Go => LangSpec {
                line: &["//"],
                block: Some(("/*", "*/")),
                quotes: &['"', '`'],
                multiline_strings: true,
                keywords: &[
                    "break",
                    "case",
                    "chan",
                    "const",
                    "continue",
                    "default",
                    "defer",
                    "else",
                    "fallthrough",
                    "for",
                    "func",
                    "go",
                    "goto",
                    "if",
                    "import",
                    "interface",
                    "map",
                    "package",
                    "range",
                    "return",
                    "select",
                    "struct",
                    "switch",
                    "type",
                    "var",
                ],
                types: &[
                    "any", "bool", "byte", "error", "float32", "float64", "int", "int8", "int16",
                    "int32", "int64", "rune", "string", "uint", "uint8", "uint16", "uint32",
                    "uint64", "uintptr",
                ],
                constants: &["true", "false", "nil", "iota"],
            },
            Lang::Shell => LangSpec {
                line: &["#"],
                block: None,
                quotes: &['"', '\''],
                multiline_strings: false,
                keywords: &[
                    "alias", "break", "case", "cd", "continue", "declare", "do", "done", "echo",
                    "elif", "else", "esac", "eval", "exec", "exit", "export", "fi", "for",
                    "function", "if", "in", "local", "printf", "read", "readonly", "return",
                    "select", "set", "shift", "source", "then", "time", "trap", "typeset", "unset",
                    "until", "while",
                ],
                types: &[],
                constants: &["true", "false"],
            },
            Lang::Json => LangSpec {
                line: &["//"],
                block: Some(("/*", "*/")),
                quotes: &['"'],
                multiline_strings: false,
                keywords: &[],
                types: &[],
                constants: &["true", "false", "null"],
            },
            Lang::Yaml => LangSpec {
                line: &["#"],
                block: None,
                quotes: &['"', '\''],
                multiline_strings: true,
                keywords: &[],
                types: &[],
                constants: &[
                    "true", "false", "null", "yes", "no", "on", "off", "True", "False", "Null",
                    "Yes", "No",
                ],
            },
            Lang::Toml => LangSpec {
                line: &["#"],
                block: None,
                quotes: &['"', '\''],
                multiline_strings: true,
                keywords: &[],
                types: &[],
                constants: &["true", "false"],
            },
            Lang::Markup => LangSpec {
                line: &[],
                block: Some(("<!--", "-->")),
                quotes: &['"', '\''],
                multiline_strings: false,
                keywords: &[
                    "html",
                    "head",
                    "body",
                    "title",
                    "meta",
                    "link",
                    "script",
                    "style",
                    "div",
                    "span",
                    "a",
                    "p",
                    "br",
                    "hr",
                    "img",
                    "ul",
                    "ol",
                    "li",
                    "table",
                    "tr",
                    "td",
                    "th",
                    "thead",
                    "tbody",
                    "form",
                    "input",
                    "button",
                    "select",
                    "option",
                    "label",
                    "section",
                    "header",
                    "footer",
                    "nav",
                    "main",
                    "article",
                    "svg",
                    "path",
                    "g",
                    "xml",
                    "component",
                    "template",
                    "router",
                    "slot",
                ],
                types: &[],
                constants: &[],
            },
            Lang::Css => LangSpec {
                line: &[],
                block: Some(("/*", "*/")),
                quotes: &['"', '\''],
                multiline_strings: false,
                keywords: &[
                    "important",
                    "inherit",
                    "initial",
                    "none",
                    "auto",
                    "block",
                    "flex",
                    "grid",
                    "absolute",
                    "relative",
                    "fixed",
                    "sticky",
                    "hidden",
                    "visible",
                    "solid",
                    "transparent",
                    "pointer",
                    "nowrap",
                    "center",
                    "middle",
                    "repeat",
                    "cover",
                    "contain",
                    "border-box",
                    "content-box",
                ],
                types: &[],
                constants: &[],
            },
            Lang::Sql => LangSpec {
                line: &["--"],
                block: Some(("/*", "*/")),
                quotes: &['\''],
                multiline_strings: false,
                keywords: &[
                    "add",
                    "all",
                    "alter",
                    "and",
                    "as",
                    "asc",
                    "begin",
                    "between",
                    "by",
                    "cascade",
                    "case",
                    "commit",
                    "constraint",
                    "create",
                    "delete",
                    "desc",
                    "distinct",
                    "drop",
                    "else",
                    "end",
                    "exists",
                    "foreign",
                    "from",
                    "full",
                    "group",
                    "having",
                    "in",
                    "index",
                    "inner",
                    "insert",
                    "into",
                    "is",
                    "join",
                    "key",
                    "left",
                    "like",
                    "limit",
                    "not",
                    "offset",
                    "on",
                    "or",
                    "order",
                    "outer",
                    "primary",
                    "references",
                    "returning",
                    "right",
                    "rollback",
                    "select",
                    "set",
                    "table",
                    "then",
                    "transaction",
                    "union",
                    "unique",
                    "update",
                    "values",
                    "view",
                    "when",
                    "where",
                    "with",
                ],
                types: &[
                    "bigint",
                    "boolean",
                    "char",
                    "date",
                    "decimal",
                    "int",
                    "integer",
                    "json",
                    "jsonb",
                    "numeric",
                    "serial",
                    "text",
                    "timestamp",
                    "uuid",
                    "varchar",
                ],
                constants: &["null", "true", "false"],
            },
            Lang::Ruby => LangSpec {
                line: &["#"],
                block: None,
                quotes: &['"', '\''],
                multiline_strings: false,
                keywords: &[
                    "and",
                    "begin",
                    "break",
                    "case",
                    "class",
                    "def",
                    "do",
                    "else",
                    "elsif",
                    "end",
                    "ensure",
                    "for",
                    "if",
                    "in",
                    "include",
                    "lambda",
                    "module",
                    "next",
                    "not",
                    "or",
                    "require",
                    "rescue",
                    "retry",
                    "return",
                    "self",
                    "super",
                    "then",
                    "unless",
                    "until",
                    "when",
                    "while",
                    "yield",
                    "attr_accessor",
                    "attr_reader",
                    "attr_writer",
                ],
                types: &[],
                constants: &["nil", "true", "false", "self"],
            },
            Lang::Php => LangSpec {
                line: &["//", "#"],
                block: Some(("/*", "*/")),
                quotes: &['"', '\''],
                multiline_strings: false,
                keywords: &[
                    "abstract",
                    "and",
                    "array",
                    "as",
                    "break",
                    "case",
                    "catch",
                    "class",
                    "clone",
                    "const",
                    "continue",
                    "declare",
                    "default",
                    "do",
                    "echo",
                    "else",
                    "elseif",
                    "empty",
                    "enddeclare",
                    "endfor",
                    "endforeach",
                    "endif",
                    "endswitch",
                    "endwhile",
                    "extends",
                    "final",
                    "finally",
                    "fn",
                    "for",
                    "foreach",
                    "function",
                    "global",
                    "goto",
                    "if",
                    "implements",
                    "include",
                    "include_once",
                    "instanceof",
                    "interface",
                    "isset",
                    "list",
                    "match",
                    "namespace",
                    "new",
                    "or",
                    "print",
                    "private",
                    "protected",
                    "public",
                    "require",
                    "require_once",
                    "return",
                    "static",
                    "switch",
                    "throw",
                    "trait",
                    "try",
                    "unset",
                    "use",
                    "while",
                    "xor",
                    "yield",
                ],
                types: &[],
                constants: &["true", "false", "null", "self", "parent"],
            },
            Lang::Lua => LangSpec {
                line: &["--"],
                block: Some(("--[[", "]]")),
                quotes: &['"', '\''],
                multiline_strings: false,
                keywords: &[
                    "and", "break", "do", "else", "elseif", "end", "for", "function", "goto", "if",
                    "in", "local", "not", "or", "repeat", "return", "then", "until", "while",
                ],
                types: &[],
                constants: &["true", "false", "nil", "self"],
            },
            Lang::Kotlin => LangSpec {
                line: &["//"],
                block: Some(("/*", "*/")),
                quotes: &['"', '\''],
                multiline_strings: false,
                keywords: &[
                    "abstract",
                    "actual",
                    "as",
                    "break",
                    "by",
                    "catch",
                    "class",
                    "companion",
                    "const",
                    "constructor",
                    "continue",
                    "data",
                    "do",
                    "else",
                    "enum",
                    "expect",
                    "final",
                    "finally",
                    "for",
                    "fun",
                    "get",
                    "if",
                    "in",
                    "infix",
                    "init",
                    "inline",
                    "interface",
                    "internal",
                    "is",
                    "lateinit",
                    "object",
                    "open",
                    "operator",
                    "out",
                    "override",
                    "package",
                    "private",
                    "protected",
                    "public",
                    "reified",
                    "return",
                    "sealed",
                    "set",
                    "super",
                    "suspend",
                    "tailrec",
                    "this",
                    "throw",
                    "try",
                    "typealias",
                    "val",
                    "var",
                    "vararg",
                    "when",
                    "where",
                    "while",
                    "import",
                ],
                types: &[
                    "Any", "Array", "Boolean", "Char", "Double", "Float", "Int", "List", "Long",
                    "Map", "Nothing", "Set", "String", "Unit",
                ],
                constants: &["true", "false", "null", "this", "super"],
            },
            Lang::Swift => LangSpec {
                line: &["//"],
                block: Some(("/*", "*/")),
                quotes: &['"'],
                multiline_strings: true,
                keywords: &[
                    "actor",
                    "any",
                    "as",
                    "associatedtype",
                    "async",
                    "await",
                    "break",
                    "case",
                    "catch",
                    "class",
                    "convenience",
                    "continue",
                    "default",
                    "defer",
                    "deinit",
                    "do",
                    "else",
                    "enum",
                    "extension",
                    "fallthrough",
                    "fileprivate",
                    "final",
                    "for",
                    "func",
                    "guard",
                    "if",
                    "import",
                    "in",
                    "init",
                    "inout",
                    "internal",
                    "is",
                    "lazy",
                    "let",
                    "mutating",
                    "nonmutating",
                    "open",
                    "operator",
                    "override",
                    "private",
                    "protocol",
                    "public",
                    "repeat",
                    "required",
                    "rethrows",
                    "return",
                    "self",
                    "some",
                    "static",
                    "struct",
                    "subscript",
                    "super",
                    "switch",
                    "throw",
                    "throws",
                    "try",
                    "typealias",
                    "var",
                    "weak",
                    "where",
                    "while",
                ],
                types: &[
                    "Any",
                    "AnyObject",
                    "Array",
                    "Bool",
                    "Character",
                    "Dictionary",
                    "Double",
                    "Float",
                    "Int",
                    "Optional",
                    "Set",
                    "String",
                    "Void",
                ],
                constants: &["nil", "true", "false", "self", "super"],
            },
            Lang::Csharp => LangSpec {
                line: &["//"],
                block: Some(("/*", "*/")),
                quotes: &['"', '\''],
                multiline_strings: false,
                keywords: &[
                    "abstract",
                    "as",
                    "async",
                    "await",
                    "base",
                    "break",
                    "case",
                    "catch",
                    "checked",
                    "class",
                    "const",
                    "continue",
                    "default",
                    "delegate",
                    "do",
                    "else",
                    "enum",
                    "event",
                    "explicit",
                    "extern",
                    "finally",
                    "fixed",
                    "for",
                    "foreach",
                    "get",
                    "goto",
                    "if",
                    "implicit",
                    "in",
                    "interface",
                    "internal",
                    "is",
                    "lock",
                    "namespace",
                    "new",
                    "operator",
                    "out",
                    "override",
                    "params",
                    "partial",
                    "private",
                    "protected",
                    "public",
                    "readonly",
                    "record",
                    "ref",
                    "return",
                    "sealed",
                    "set",
                    "sizeof",
                    "stackalloc",
                    "static",
                    "struct",
                    "switch",
                    "this",
                    "throw",
                    "try",
                    "typeof",
                    "unchecked",
                    "unsafe",
                    "using",
                    "value",
                    "virtual",
                    "when",
                    "where",
                    "while",
                    "yield",
                ],
                types: &[
                    "bool", "byte", "char", "decimal", "double", "float", "int", "long", "object",
                    "sbyte", "short", "string", "uint", "ulong", "ushort", "void",
                ],
                constants: &["null", "true", "false", "this", "base"],
            },
            Lang::R => LangSpec {
                line: &["#"],
                block: None,
                quotes: &['"', '\''],
                multiline_strings: false,
                keywords: &[
                    "break", "else", "for", "function", "if", "in", "library", "next", "repeat",
                    "require", "return", "while",
                ],
                types: &[],
                constants: &["TRUE", "FALSE", "NULL", "NA", "NaN", "Inf", "True", "False"],
            },
            Lang::Haskell => LangSpec {
                line: &["--"],
                block: Some(("{-", "-}")),
                quotes: &['"'],
                multiline_strings: false,
                keywords: &[
                    "as",
                    "case",
                    "class",
                    "data",
                    "default",
                    "deriving",
                    "do",
                    "else",
                    "forall",
                    "foreign",
                    "hiding",
                    "if",
                    "import",
                    "in",
                    "infix",
                    "infixl",
                    "infixr",
                    "instance",
                    "let",
                    "module",
                    "newtype",
                    "of",
                    "qualified",
                    "then",
                    "type",
                    "where",
                ],
                types: &[
                    "Bool", "Char", "Double", "Either", "Float", "IO", "Int", "Integer", "Maybe",
                    "String",
                ],
                constants: &["True", "False", "Nothing", "Just"],
            },
            Lang::Scala => LangSpec {
                line: &["//"],
                block: Some(("/*", "*/")),
                quotes: &['"', '\''],
                multiline_strings: false,
                keywords: &[
                    "abstract",
                    "case",
                    "catch",
                    "class",
                    "def",
                    "do",
                    "else",
                    "extends",
                    "final",
                    "finally",
                    "for",
                    "forSome",
                    "if",
                    "implicit",
                    "import",
                    "lazy",
                    "match",
                    "new",
                    "object",
                    "override",
                    "package",
                    "private",
                    "protected",
                    "return",
                    "sealed",
                    "super",
                    "this",
                    "throw",
                    "trait",
                    "try",
                    "type",
                    "val",
                    "var",
                    "while",
                    "with",
                    "yield",
                ],
                types: &[
                    "Any", "AnyRef", "Array", "Boolean", "Byte", "Char", "Double", "Either",
                    "Float", "Int", "List", "Long", "Map", "None", "Nothing", "Option", "Set",
                    "Short", "Some", "String", "Unit",
                ],
                constants: &["null", "true", "false", "this", "super"],
            },
            Lang::Dart => LangSpec {
                line: &["//"],
                block: Some(("/*", "*/")),
                quotes: &['"', '\''],
                multiline_strings: false,
                keywords: &[
                    "abstract",
                    "as",
                    "assert",
                    "async",
                    "await",
                    "break",
                    "case",
                    "catch",
                    "class",
                    "const",
                    "continue",
                    "covariant",
                    "default",
                    "do",
                    "dynamic",
                    "else",
                    "enum",
                    "extends",
                    "external",
                    "factory",
                    "final",
                    "finally",
                    "for",
                    "get",
                    "if",
                    "implements",
                    "import",
                    "in",
                    "is",
                    "late",
                    "library",
                    "mixin",
                    "new",
                    "operator",
                    "part",
                    "required",
                    "rethrow",
                    "return",
                    "set",
                    "static",
                    "super",
                    "switch",
                    "sync",
                    "this",
                    "throw",
                    "try",
                    "typedef",
                    "var",
                    "void",
                    "while",
                    "with",
                    "yield",
                ],
                types: &[
                    "bool", "double", "Dynamic", "Future", "int", "Iterable", "List", "Map", "num",
                    "Object", "Set", "Stream", "String",
                ],
                constants: &["null", "true", "false", "this", "super"],
            },
            Lang::Perl => LangSpec {
                line: &["#"],
                block: None,
                quotes: &['"', '\''],
                multiline_strings: false,
                keywords: &[
                    "bless", "chomp", "chop", "continue", "defined", "delete", "die", "do", "each",
                    "else", "elsif", "eval", "exists", "foreach", "for", "grep", "if", "join",
                    "keys", "last", "local", "map", "my", "next", "our", "package", "pop", "print",
                    "push", "redo", "ref", "require", "return", "reverse", "say", "shift", "sort",
                    "split", "sub", "undef", "unless", "unshift", "until", "use", "values", "warn",
                    "while",
                ],
                types: &[],
                constants: &["undef", "__FILE__", "__LINE__"],
            },
            Lang::Ini => LangSpec {
                line: &["#", ";"],
                block: None,
                quotes: &['"', '\''],
                multiline_strings: false,
                keywords: &[],
                types: &[],
                constants: &["true", "false", "yes", "no", "on", "off", "null"],
            },
            Lang::Make => LangSpec {
                line: &["#"],
                block: None,
                quotes: &['"', '\''],
                multiline_strings: false,
                keywords: &[
                    "define",
                    "else",
                    "endef",
                    "endif",
                    "export",
                    "ifdef",
                    "ifeq",
                    "ifndef",
                    "ifneq",
                    "include",
                    "override",
                    "unexport",
                    "vpath",
                    ".PHONY",
                    ".SUFFIXES",
                    ".DEFAULT",
                    ".PRECIOUS",
                ],
                types: &[],
                constants: &[],
            },
            Lang::Docker => LangSpec {
                line: &["#"],
                block: None,
                quotes: &['"', '\''],
                multiline_strings: false,
                keywords: &[
                    "ADD",
                    "ARG",
                    "AS",
                    "CMD",
                    "COPY",
                    "ENTRYPOINT",
                    "ENV",
                    "EXPOSE",
                    "FROM",
                    "HEALTHCHECK",
                    "LABEL",
                    "MAINTAINER",
                    "ONBUILD",
                    "RUN",
                    "SHELL",
                    "STOPSIGNAL",
                    "USER",
                    "VOLUME",
                    "WORKDIR",
                ],
                types: &[],
                constants: &[],
            },
            Lang::Nix => LangSpec {
                line: &["#"],
                block: Some(("/*", "*/")),
                quotes: &['"', '\''],
                multiline_strings: false,
                keywords: &[
                    "assert", "else", "if", "import", "in", "inherit", "let", "or", "rec", "then",
                    "with",
                ],
                types: &["builtins", "derivation", "mkDerivation"],
                constants: &["true", "false", "null"],
            },
            Lang::Elixir => LangSpec {
                line: &["#"],
                block: None,
                quotes: &['"', '\''],
                multiline_strings: true,
                keywords: &[
                    "after",
                    "alias",
                    "and",
                    "case",
                    "catch",
                    "cond",
                    "def",
                    "defguard",
                    "defimpl",
                    "defmacro",
                    "defmodule",
                    "defp",
                    "defprotocol",
                    "defstruct",
                    "do",
                    "else",
                    "end",
                    "fn",
                    "for",
                    "if",
                    "import",
                    "in",
                    "not",
                    "or",
                    "quote",
                    "raise",
                    "receive",
                    "require",
                    "rescue",
                    "super",
                    "try",
                    "unless",
                    "unquote",
                    "use",
                    "when",
                    "with",
                ],
                types: &[],
                constants: &["nil", "true", "false"],
            },
            Lang::Erlang => LangSpec {
                line: &["%"],
                block: None,
                quotes: &['"', '\''],
                multiline_strings: false,
                keywords: &[
                    "after", "and", "andalso", "band", "begin", "bnot", "bor", "bsl", "bsr",
                    "bxor", "case", "catch", "cond", "div", "end", "fun", "if", "let", "not", "of",
                    "or", "orelse", "receive", "rem", "try", "when", "xor",
                ],
                types: &[],
                constants: &["true", "false", "ok", "error", "undefined"],
            },
            Lang::Clojure => LangSpec {
                line: &[";"],
                block: None,
                quotes: &['"'],
                multiline_strings: false,
                keywords: &[
                    "and",
                    "binding",
                    "case",
                    "catch",
                    "comment",
                    "cond",
                    "condp",
                    "def",
                    "defmacro",
                    "defmethod",
                    "defmulti",
                    "defn",
                    "defonce",
                    "defprotocol",
                    "defrecord",
                    "deftype",
                    "do",
                    "doseq",
                    "dotimes",
                    "finally",
                    "fn",
                    "for",
                    "if",
                    "if-let",
                    "if-not",
                    "import",
                    "in-ns",
                    "let",
                    "letfn",
                    "loop",
                    "new",
                    "not",
                    "ns",
                    "or",
                    "quote",
                    "recur",
                    "refer",
                    "require",
                    "set!",
                    "throw",
                    "try",
                    "use",
                    "var",
                    "when",
                    "when-let",
                ],
                types: &[],
                constants: &["nil", "true", "false"],
            },
        }
    }
}

/// The language for a path, or `None` when it is not source code we highlight.
fn lang_for_path(path: &Path) -> Option<Lang> {
    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
        match name {
            "Makefile" | "makefile" | "GNUmakefile" | "Justfile" | "justfile" => {
                return Some(Lang::Make);
            }
            "Dockerfile" | "Containerfile" => return Some(Lang::Docker),
            "CMakeLists.txt" => return Some(Lang::C),
            ".bashrc" | ".zshrc" | ".bash_profile" | ".profile" => return Some(Lang::Shell),
            _ => {}
        }
    }
    Some(match ext_lower(path)?.as_str() {
        "rs" => Lang::Rust,
        "py" | "pyw" | "pyi" => Lang::Python,
        "js" | "mjs" | "cjs" | "jsx" | "ts" | "tsx" | "mts" | "cts" => Lang::Js,
        "c" | "h" | "cc" | "cpp" | "cxx" | "hpp" | "hh" | "hxx" | "ino" | "m" | "mm" => Lang::C,
        "java" => Lang::Java,
        "go" => Lang::Go,
        "sh" | "bash" | "zsh" | "fish" | "ksh" | "dash" => Lang::Shell,
        "json" | "jsonc" | "json5" | "geojson" => Lang::Json,
        "yml" | "yaml" => Lang::Yaml,
        "toml" => Lang::Toml,
        "html" | "htm" | "xhtml" | "xml" | "svg" | "vue" | "svelte" | "astro" => Lang::Markup,
        "css" | "scss" | "sass" | "less" | "styl" => Lang::Css,
        "sql" => Lang::Sql,
        "rb" | "rake" | "gemspec" => Lang::Ruby,
        "php" | "phtml" => Lang::Php,
        "lua" => Lang::Lua,
        "kt" | "kts" => Lang::Kotlin,
        "swift" => Lang::Swift,
        "cs" => Lang::Csharp,
        "r" | "rmd" => Lang::R,
        "hs" | "lhs" => Lang::Haskell,
        "scala" | "sc" => Lang::Scala,
        "dart" => Lang::Dart,
        "pl" | "pm" => Lang::Perl,
        "ini" | "cfg" | "conf" | "properties" | "editorconfig" | "env" | "service" => Lang::Ini,
        "mk" | "mak" => Lang::Make,
        "nix" => Lang::Nix,
        "ex" | "exs" => Lang::Elixir,
        "erl" | "hrl" => Lang::Erlang,
        "clj" | "cljs" | "cljc" | "edn" => Lang::Clojure,
        _ => return None,
    })
}

/// Render source code with lightweight syntax highlighting, or plain monospace
/// when the file is too big to colour.
fn code_preview(ui: &mut egui::Ui, t: &Theme, lang: Lang, text: &str) {
    if text.len() > MAX_HIGHLIGHT_BYTES {
        ui.label(
            egui::RichText::new(text)
                .monospace()
                .size(11.5)
                .color(t.dim),
        );
        return;
    }
    ui.label(highlight_code(text, lang, &t.syntax()));
}

/// Append `s` to the job in `color`, merging with the previous run when the
/// colour is unchanged so a mostly-plain file stays a handful of sections.
fn push_run(job: &mut egui::text::LayoutJob, s: &str, format: egui::TextFormat) {
    if s.is_empty() {
        return;
    }
    job.text.push_str(s);
    let end = job.text.len();
    if let Some(last) = job.sections.last_mut()
        && last.format.color == format.color
    {
        last.byte_range.end = end;
        return;
    }
    let start = end - s.len();
    job.sections.push(egui::text::LayoutSection {
        leading_space: 0.0,
        byte_range: start..end,
        format,
    });
}

/// Tokenise `text` into coloured runs. Best-effort by design: an unknown word
/// is plain, and an unterminated string or block comment simply runs to the end
/// of its line (or of the file) with no error recovery.
fn highlight_code(text: &str, lang: Lang, c: &SyntaxColors) -> egui::text::LayoutJob {
    let spec = lang.spec();
    let font = egui::FontId::monospace(11.5);
    let fmt = |color| egui::TextFormat {
        font_id: font.clone(),
        color,
        ..Default::default()
    };
    let mut job = egui::text::LayoutJob::default();
    let n = text.len();
    let mut i = 0;
    while i < n {
        let rest = &text[i..];
        let ch = rest.chars().next().unwrap();

        // Block comment.
        if let Some((open, close)) = spec.block
            && rest.starts_with(open)
        {
            let from = i + open.len();
            let end = text[from..]
                .find(close)
                .map_or(n, |p| from + p + close.len());
            push_run(&mut job, &text[i..end], fmt(c.comment));
            i = end;
            continue;
        }

        // Line comment.
        if spec.line.iter().any(|m| rest.starts_with(*m)) {
            let end = text[i..].find('\n').map_or(n, |p| i + p);
            push_run(&mut job, &text[i..end], fmt(c.comment));
            i = end;
            continue;
        }

        // String literal.
        if spec.quotes.contains(&ch) {
            let quote = ch;
            let mut j = i + quote.len_utf8();
            let mut escaped = false;
            while j < n {
                let cj = text[j..].chars().next().unwrap();
                if escaped {
                    escaped = false;
                } else if cj == '\\' {
                    escaped = true;
                } else if cj == quote {
                    let after = j + cj.len_utf8();
                    // A doubled quote is an escaped quote (SQL) and, in practice,
                    // harmless to treat the same way elsewhere.
                    if text[after..].starts_with(quote) {
                        j = after + quote.len_utf8();
                        continue;
                    }
                    j = after;
                    break;
                } else if cj == '\n' && !spec.multiline_strings {
                    break;
                }
                j += cj.len_utf8();
            }
            push_run(&mut job, &text[i..j], fmt(c.string));
            i = j;
            continue;
        }

        // `#directive` (C, Rust attributes), `#id` (CSS) and `@decorator`.
        if (ch == '@' || (ch == '#' && !spec.line.contains(&"#")))
            && let Some(next) = rest[ch.len_utf8()..].chars().next()
            && (next.is_alphabetic() || next == '_')
        {
            let mut j = i + ch.len_utf8();
            while j < n {
                let cj = text[j..].chars().next().unwrap();
                if cj.is_alphanumeric() || cj == '_' || cj == '-' || cj == '.' {
                    j += cj.len_utf8();
                } else {
                    break;
                }
            }
            push_run(&mut job, &text[i..j], fmt(c.type_));
            i = j;
            continue;
        }

        // Number (including hex/binary/float tails, crudely).
        if ch.is_ascii_digit() {
            let mut j = i;
            let mut prev = '\0';
            while j < n {
                let cj = text[j..].chars().next().unwrap();
                let exponent_sign =
                    (cj == '+' || cj == '-') && matches!(prev, 'e' | 'E' | 'p' | 'P');
                if cj.is_ascii_alphanumeric() || cj == '_' || cj == '.' || exponent_sign {
                    prev = cj;
                    j += cj.len_utf8();
                } else {
                    break;
                }
            }
            push_run(&mut job, &text[i..j], fmt(c.number));
            i = j;
            continue;
        }

        // Identifier: keyword, type, constant, call, or plain.
        if ch.is_alphabetic() || ch == '_' || ch == '$' || !ch.is_ascii() {
            let mut j = i;
            while j < n {
                let cj = text[j..].chars().next().unwrap();
                if cj.is_alphanumeric() || cj == '_' || cj == '$' || !cj.is_ascii() {
                    j += cj.len_utf8();
                } else {
                    break;
                }
            }
            let word = &text[i..j];
            let color = if spec.keywords.contains(&word) {
                c.keyword
            } else if spec.types.contains(&word) {
                c.type_
            } else if spec.constants.contains(&word) {
                c.number
            } else if is_call(&text[j..]) {
                c.func
            } else {
                c.text
            };
            push_run(&mut job, word, fmt(color));
            i = j;
            continue;
        }

        // Plain run: whitespace, operators and punctuation.
        let mut j = i + ch.len_utf8();
        while j < n {
            let cj = text[j..].chars().next().unwrap();
            if cj.is_ascii_digit()
                || cj.is_alphabetic()
                || cj == '_'
                || cj == '$'
                || cj == '@'
                || cj == '#'
                || !cj.is_ascii()
                || spec.quotes.contains(&cj)
            {
                break;
            }
            let r = &text[j..];
            if spec.block.is_some_and(|(o, _)| r.starts_with(o))
                || spec.line.iter().any(|m| r.starts_with(*m))
            {
                break;
            }
            j += cj.len_utf8();
        }
        push_run(&mut job, &text[i..j], fmt(c.text));
        i = j;
    }
    job
}

/// Whether the identifier ending at `rest` is immediately called (`name(`).
fn is_call(rest: &str) -> bool {
    rest.trim_start().starts_with('(')
}

/// A `##`..`######` heading's text, or `None` for anything else.
fn heading(line: &str) -> Option<&str> {
    let hashes = line.chars().take_while(|c| *c == '#').count();
    if (2..=6).contains(&hashes) && line[hashes..].starts_with(' ') {
        Some(line[hashes..].trim_start())
    } else {
        None
    }
}

/// Drop the inline emphasis markers without trying to style the runs.
fn strip_emphasis(text: &str) -> String {
    text.replace("**", "")
        .replace("__", "")
        .replace('`', "")
        .replace("~~", "")
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
            Sort::Name(asc)
            | Sort::Size(asc)
            | Sort::Mtime(asc)
            | Sort::Created(asc)
            | Sort::Relevance(asc) => {
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
            Sort::Created(true) => Some(Sort::Created(false)),
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
            | (Sort::Created(_), Sort::Created(_))
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
    use easysearch_core::matcher::fuzzy_score;
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
                easysearch_core::matcher::fuzzy_score(bare, &name, false).is_some()
                    || easysearch_core::matcher::fuzzy_score(bare, &full, false).is_some()
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
        Sort::Created(asc) => {
            // Birth time is read live (see `birth_secs`), so sorting by it costs
            // one `stat` per row — a one-off per sort action, not per frame.
            results.sort_by_cached_key(|r| {
                let t = birth_secs(&r.path);
                if asc { t } else { -t }
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
    fn short_dir_never_doubles_the_root_slash() {
        // A parent one level under the root used to render as `//home`.
        assert_eq!(short_dir(Path::new("/home/file.txt")), "/home");
        assert_eq!(short_dir(Path::new("/etc/hosts")), "/etc");
        // Two components fit whole; deeper paths are collapsed to the last two.
        assert_eq!(short_dir(Path::new("/home/a/file.txt")), "/home/a");
        assert_eq!(
            short_dir(Path::new("/home/a/Documents/file.txt")),
            "…/a/Documents"
        );
        // A file at the root shows just the root.
        assert_eq!(short_dir(Path::new("/file.txt")), "/");
    }

    #[test]
    fn file_uri_percent_encodes_specials() {
        assert_eq!(
            file_uri(Path::new("/home/a b/c.txt")),
            "file:///home/a%20b/c.txt"
        );
        assert_eq!(file_uri(Path::new("/tmp/x#y")), "file:///tmp/x%23y");
    }

    #[test]
    fn shell_escape_quotes_only_when_needed() {
        assert_eq!(shell_escape("/home/a/b.txt"), "/home/a/b.txt");
        assert_eq!(shell_escape("/home/My Docs"), "'/home/My Docs'");
        assert_eq!(shell_escape("it's"), "'it'\\''s'");
        assert_eq!(shell_escape(""), "''");
    }

    #[test]
    fn result_filter_matches_name_or_path() {
        assert!(path_contains(Path::new("/home/a/Report.pdf"), "report"));
        assert!(path_contains(Path::new("/home/a/Report.pdf"), "/home/"));
        assert!(path_contains(Path::new("/home/a/Report.pdf"), ""));
        assert!(!path_contains(Path::new("/home/a/Report.pdf"), "draft"));
    }

    #[test]
    fn hash_compare_is_case_insensitive_and_trims() {
        assert!(hash_eq("AABBCC", "aabbcc"));
        assert!(hash_eq("aabbcc", "  AABBCC  "));
        assert!(!hash_eq("aabbcc", "aabbcd"));
    }

    #[test]
    fn duplicates_ignore_hardlinks() {
        let dir = std::env::temp_dir().join(format!("easysearch-dup-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let a = dir.join("a.txt");
        std::fs::write(&a, b"same bytes here").unwrap();
        let b = dir.join("b.txt");
        std::fs::hard_link(&a, &b).unwrap();
        let c = dir.join("c.txt");
        std::fs::write(&c, b"same bytes here").unwrap();
        let report = find_duplicates(&[a, b, c], &mut |_, _| {});
        // a and b share an inode, so only one survives; it groups with c.
        assert_eq!(report.groups.len(), 1);
        assert_eq!(report.groups[0].paths.len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn empty_and_broken_detection() {
        let dir = std::env::temp_dir().join(format!("easysearch-empty-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let empty = dir.join("empty.txt");
        std::fs::write(&empty, b"").unwrap();
        let full = dir.join("full.txt");
        std::fs::write(&full, b"x").unwrap();
        let broken = dir.join("broken");
        std::os::unix::fs::symlink(dir.join("missing"), &broken).unwrap();
        assert!(is_empty_entry(&empty, false));
        assert!(!is_empty_entry(&full, false));
        assert!(is_broken_symlink(&broken));
        assert!(!is_broken_symlink(&full));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn export_covers_csv_tsv_and_json() {
        let rows = vec![ResultRow {
            path: PathBuf::from("/home/a/My Report, v2.pdf"),
            size: 10,
            mtime: 5,
            is_dir: false,
        }];
        let csv = export_text(&rows, ExportFormat::Csv);
        assert!(csv.starts_with("name,path,size,modified\n"));
        assert!(csv.contains("\"My Report, v2.pdf\""), "{csv}");
        let tsv = export_text(&rows, ExportFormat::Tsv);
        assert!(tsv.starts_with("name\tpath\tsize\tmodified\n"));
        // A comma inside a field must not be quoted in TSV.
        assert!(tsv.contains("My Report, v2.pdf"), "{tsv}");
        let json = export_text(&rows, ExportFormat::Json);
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed[0]["name"], "My Report, v2.pdf");
        assert_eq!(parsed[0]["size"], 10);
    }

    #[test]
    fn autostart_entry_runs_the_daemon() {
        let entry = autostart_entry();
        assert!(entry.starts_with("[Desktop Entry]\n"));
        assert!(entry.contains("Exec=easysearch --hidden\n"));
        assert!(entry.contains(&format!("Icon={APP_ID}\n")));
    }

    #[test]
    fn rename_names_are_validated() {
        assert_eq!(validate_rename(" report.pdf ").unwrap(), "report.pdf");
        assert!(validate_rename("").is_err());
        assert!(validate_rename("..").is_err());
        assert!(validate_rename("a/b").is_err());
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
    fn gtk_and_kde_font_names_are_parsed() {
        // GTK: `Family Size`, and the family may contain spaces.
        assert_eq!(
            gtk_family_from("[Settings]\ngtk-font-name=Noto Sans 10\n", "gtk-font-name").as_deref(),
            Some("Noto Sans")
        );
        assert_eq!(
            gtk_family_from(
                "gtk-monospace-font-name=JetBrains Mono 12\n",
                "gtk-monospace-font-name"
            )
            .as_deref(),
            Some("JetBrains Mono")
        );
        // Without a size the whole value is the family.
        assert_eq!(
            gtk_family_from("gtk-font-name=DejaVu Sans\n", "gtk-font-name").as_deref(),
            Some("DejaVu Sans")
        );
        assert_eq!(gtk_family_from("[Settings]\n", "gtk-font-name"), None);

        // KDE: `Family,size,weight,…`.
        assert_eq!(
            kde_family_from("[General]\nfont=Noto Sans,10,-1,5,50,0,0,0,0,0\n", "font").as_deref(),
            Some("Noto Sans")
        );
        assert_eq!(
            kde_family_from("[General]\nfixed=Hack,10\n", "fixed").as_deref(),
            Some("Hack")
        );
        assert_eq!(kde_family_from("[General]\n", "font"), None);
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

    #[test]
    fn created_sort_cycles_and_is_keyed_on_its_own_column() {
        // Clicking the Created header sorts ascending, again descending, a third
        // time clears it — the same contract as Name / Size / Modified.
        assert!(same_key(Sort::Created(true), Sort::Created(false)));
        assert!(!same_key(Sort::Created(true), Sort::Mtime(false)));
        assert_eq!(
            cycle_sort(None, Sort::Created(true)),
            Some(Sort::Created(true))
        );
        assert_eq!(
            cycle_sort(Some(Sort::Created(true)), Sort::Created(true)),
            Some(Sort::Created(false))
        );
        assert_eq!(
            cycle_sort(Some(Sort::Created(false)), Sort::Created(true)),
            None
        );
        // Switching to Created from another column starts ascending.
        assert_eq!(
            cycle_sort(Some(Sort::Mtime(true)), Sort::Created(true)),
            Some(Sort::Created(true))
        );
    }

    #[test]
    fn birth_time_is_zero_for_a_missing_path() {
        let missing = std::env::temp_dir().join("easysearch-birth-does-not-exist");
        let _ = std::fs::remove_file(&missing);
        assert_eq!(birth_secs(&missing), 0, "no file, no birth time");
        assert_eq!(file_created(&missing), "—");

        // A file we just wrote: where the filesystem records a birth time it
        // must be recent, and where it does not the cell falls back to `—`.
        let dir = std::env::temp_dir().join(format!("easysearch-birth-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("fresh.txt");
        std::fs::write(&file, b"x").unwrap();
        let secs = birth_secs(&file);
        if secs > 0 {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs() as i64;
            assert!(secs <= now + 2, "birth time {secs} is in the future");
            assert!(now - secs < 3600, "birth time {secs} looks stale");
        } else {
            assert_eq!(file_created(&file), "—");
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn created_sort_orders_by_birth_time_both_ways() {
        let dir =
            std::env::temp_dir().join(format!("easysearch-created-sort-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let older = dir.join("older.txt");
        std::fs::write(&older, b"a").unwrap();
        // Birth times have one-second resolution; separate the two files by more.
        std::thread::sleep(std::time::Duration::from_millis(1100));
        let newer = dir.join("newer.txt");
        std::fs::write(&newer, b"b").unwrap();

        let row = |p: &Path| ResultRow {
            path: p.to_path_buf(),
            size: 1,
            mtime: 0,
            is_dir: false,
        };
        let (a, b) = (birth_secs(&older), birth_secs(&newer));
        // Only meaningful where the filesystem records usable birth times.
        if a > 0 && b > a {
            let mut rows = vec![row(&newer), row(&older)];
            sort_results(&mut rows, Sort::Created(true), "", false);
            assert_eq!(rows.first().unwrap().path, older, "ascending: oldest first");
            assert_eq!(rows.last().unwrap().path, newer);
            sort_results(&mut rows, Sort::Created(false), "", false);
            assert_eq!(
                rows.first().unwrap().path,
                newer,
                "descending: newest first"
            );
        }
        let _ = std::fs::remove_dir_all(dir);
    }
}

/// The syntax highlighter is a pure function over `(text, language)`, so its
/// behaviour pins down cheaply: it never loses a byte, it colours the classes
/// it claims to, and an unterminated token cannot run away.
#[cfg(test)]
mod highlight_tests {
    use super::*;

    /// The run whose text contains `needle`, with its colour.
    fn run_containing(job: &egui::text::LayoutJob, needle: &str) -> (String, egui::Color32) {
        for s in &job.sections {
            let text = &job.text[s.byte_range.clone()];
            if text.contains(needle) {
                return (text.to_string(), s.format.color);
            }
        }
        panic!("no highlighted run contains {needle:?}");
    }

    fn highlight(text: &str, lang: Lang) -> egui::text::LayoutJob {
        highlight_code(text, lang, &Theme::DARK.syntax())
    }

    #[test]
    fn runs_tile_the_text_exactly() {
        let src = "fn main() {\n    let x = 1; // hi\n}\n";
        let job = highlight(src, Lang::Rust);
        assert_eq!(job.text, src, "no bytes are added or dropped");
        let mut at = 0;
        for s in &job.sections {
            assert_eq!(s.byte_range.start, at, "sections are contiguous");
            at = s.byte_range.end;
        }
        assert_eq!(at, src.len(), "sections cover the whole text");
    }

    #[test]
    fn rust_colours_keywords_comments_strings_and_numbers() {
        let c = Theme::DARK.syntax();
        assert_eq!(
            run_containing(&highlight("fn main() {}", Lang::Rust), "fn").1,
            c.keyword
        );
        let src = "let s = \"hi\"; let n = 42; // note";
        assert_eq!(
            run_containing(&highlight(src, Lang::Rust), "\"hi\"").1,
            c.string
        );
        assert_eq!(
            run_containing(&highlight(src, Lang::Rust), "42").1,
            c.number
        );
        assert_eq!(
            run_containing(&highlight(src, Lang::Rust), "// note").1,
            c.comment
        );
    }

    #[test]
    fn an_unterminated_string_stops_at_the_newline() {
        let src = "let s = \"unterminated\nlet y = 2;";
        let job = highlight(src, Lang::Rust);
        assert_eq!(job.text, src);
        assert_eq!(
            run_containing(&job, "\"unterminated").1,
            Theme::DARK.syntax().string
        );
        // `let y` is still recognised after the broken string.
        assert_eq!(run_containing(&job, "let").1, Theme::DARK.syntax().keyword);
    }

    #[test]
    fn python_string_escapes_do_not_end_the_string() {
        let src = "s = \"a\\\"b\"";
        let job = highlight(src, Lang::Python);
        assert_eq!(
            run_containing(&job, "a\\\"b").1,
            Theme::DARK.syntax().string
        );
    }

    #[test]
    fn hash_comments_and_css_ids_are_told_apart() {
        let c = Theme::DARK.syntax();
        // Python: `#` begins a comment.
        assert_eq!(
            run_containing(&highlight("x = 1  # hi", Lang::Python), "# hi").1,
            c.comment
        );
        // Rust: `#` never starts a comment (`//` does).
        let job = highlight("#[derive(Debug)]", Lang::Rust);
        assert!(job.sections.iter().all(|s| s.format.color != c.comment));
        // CSS: `#id` is a selector, coloured as a name.
        assert_eq!(
            run_containing(&highlight("#id { color: red }", Lang::Css), "#id").1,
            c.type_
        );
    }

    #[test]
    fn extensions_and_fence_tags_map_to_languages() {
        assert!(matches!(lang_for_path(Path::new("a.rs")), Some(Lang::Rust)));
        assert!(matches!(
            lang_for_path(Path::new("Makefile")),
            Some(Lang::Make)
        ));
        assert!(matches!(
            lang_for_path(Path::new("Dockerfile")),
            Some(Lang::Docker)
        ));
        assert!(lang_for_path(Path::new("notes.md")).is_none());
        assert!(matches!(Lang::from_tag("bash"), Some(Lang::Shell)));
        assert!(matches!(Lang::from_tag("TypeScript"), Some(Lang::Js)));
        assert!(Lang::from_tag("").is_none());
    }
}
