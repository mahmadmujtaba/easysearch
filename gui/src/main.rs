//! `everything-gui` — native egui frontend over the core engine.
//!
//! Search-as-you-type with Everything-style semantics:
//! - name mode (default): glob patterns or regex against basename/full path,
//! - content mode (toggle): the query is a regex searched inside files.

use eframe::egui;
use everything_core::{ContentIndexStatus, Engine, Query, ResultRow, SearchResponse, State, Status};
use std::process::Command;
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};

const DEBOUNCE_MS: u128 = 120;

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
            .with_inner_size([960.0, 640.0])
            .with_min_inner_size([480.0, 320.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Everything for Linux",
        options,
        Box::new(move |cc| Ok(Box::new(App::new(cc, engine)))),
    )
}

struct App {
    engine: Arc<Engine>,
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

        let status = engine.status_snapshot();
        let mut app = App {
            engine,
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
            status,
            last_sent: String::new(),
            pending: false,
            last_edit: Instant::now(),
            query_tx,
            result_rx,
            selected: 0,
        };
        app.send_query();
        let _ = cc; // style setup may come here later
        app
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
    }

    fn open(path: &std::path::Path) {
        let _ = Command::new("xdg-open").arg(path).spawn();
    }

    fn open_folder(path: &std::path::Path) {
        if let Some(parent) = path.parent() {
            let _ = Command::new("xdg-open").arg(parent).spawn();
        }
    }
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
                // A search is in flight; re-send when it lands (see drain above).
                self.last_sent = self.query.clone();
                self.pending = false;
                self.send_query();
            } else if self.last_edit.elapsed() >= Duration::from_millis(DEBOUNCE_MS as u64) {
                self.send_query();
            } else {
                ctx.request_repaint_after(Duration::from_millis(DEBOUNCE_MS as u64));
            }
        }

        // Keep the status bar live while indexing.
        ctx.request_repaint_after(Duration::from_millis(250));

        egui::TopBottomPanel::top("search").show(ctx, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let edit = egui::TextEdit::singleline(&mut self.query)
                    .hint_text(if self.content_mode {
                        "Search file contents (regex)…"
                    } else if self.regex_mode {
                        "Search filenames (regex)…"
                    } else {
                        "Search filenames (e.g. *.pdf, invoice !draft)…"
                    })
                    .desired_width(f32::INFINITY)
                    .font(egui::TextStyle::Heading);
                let resp = ui.add(edit);
                if resp.changed() {
                    self.last_edit = Instant::now();
                }
                if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    if let Some(first) = self.results.first() {
                        App::open(&first.path);
                    }
                }
            });
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.checkbox(&mut self.content_mode, "Match contents");
                ui.checkbox(&mut self.regex_mode, "Regex");
                ui.checkbox(&mut self.case_sensitive, "Case-sensitive");
                ui.checkbox(&mut self.hidden, "Hidden files");
                ui.checkbox(&mut self.full_path, "Full path");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let count = format!(
                        "{} result(s){}{}",
                        self.results.len(),
                        if self.truncated { "+" } else { "" },
                        if self.elapsed_ms > 0 {
                            format!(" · {} ms", self.elapsed_ms)
                        } else {
                            String::new()
                        }
                    );
                    ui.label(egui::RichText::new(count).weak());
                });
            });
            ui.add_space(6.0);
        });

        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.horizontal(|ui| {
                let (files, dirs) = self.engine.counts();
                let state = match self.status.state {
                    State::Starting => "starting…".to_string(),
                    State::Indexing => format!("indexing… {files} files"),
                    State::Live => format!("{files} files · {dirs} dirs"),
                };
                let watcher = if self.status.degraded {
                    "⚠ periodic rescan (watch limit hit)".to_string()
                } else {
                    "live (inotify)".to_string()
                };
                let ci = match &self.status.content_index {
                    ContentIndexStatus::Disabled => "content cache: off".to_string(),
                    ContentIndexStatus::Enabled { entries, bytes, pending } => format!(
                        "content cache: {entries} files · {} MiB{}",
                        bytes / (1024 * 1024),
                        if *pending > 0 { format!(" · {pending} pending") } else { String::new() }
                    ),
                };
                ui.label(egui::RichText::new(format!("{state} · {watcher} · {ci}")).weak());
            });
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            if let Some(err) = &self.error {
                ui.colored_label(ui.visuals().error_fg_color, format!("query error: {err}"));
                return;
            }
            if self.results.is_empty() && !self.query.is_empty() && !self.pending {
                ui.weak("No results.");
            }
            if self.results.is_empty() && self.query.is_empty() && self.status.state != State::Live {
                ui.weak("Building index…");
            }
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    let mut open: Option<usize> = None;
                    let mut open_folder: Option<usize> = None;
                    let mut copy: Option<usize> = None;
                    let mut select: Option<usize> = None;
                    for (i, r) in self.results.iter().enumerate() {
                        let name = r
                            .path
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_else(|| r.path.display().to_string());
                        let dir_mark = if r.is_dir { "📁 " } else { "" };
                        let size = if r.is_dir {
                            String::new()
                        } else {
                            format!("  {}", human_size(r.size))
                        };
                        let text = egui::RichText::new(format!("{dir_mark}{name}{size}"))
                            .strong()
                            .monospace();
                        let sub = egui::RichText::new(r.path.display().to_string())
                            .weak()
                            .small();
                        let resp = ui
                            .vertical(|ui| {
                                ui.add_sized([ui.available_width(), 18.0], egui::Label::new(text));
                                ui.add_sized([ui.available_width(), 14.0], egui::Label::new(sub));
                                ui.add_space(2.0);
                            })
                            .response;
                        if resp.clicked() {
                            select = Some(i);
                        }
                        if resp.double_clicked() {
                            open = Some(i);
                        }
                        resp.context_menu(|ui| {
                            if ui.button("Open").clicked() {
                                open = Some(i);
                                ui.close_menu();
                            }
                            if ui.button("Open containing folder").clicked() {
                                open_folder = Some(i);
                                ui.close_menu();
                            }
                            if ui.button("Copy path").clicked() {
                                copy = Some(i);
                                ui.close_menu();
                            }
                        });
                    }
                    if let Some(i) = select {
                        self.selected = i;
                    }
                    if let Some(i) = open {
                        if let Some(r) = self.results.get(i) {
                            App::open(&r.path);
                        }
                    }
                    if let Some(i) = open_folder {
                        if let Some(r) = self.results.get(i) {
                            App::open_folder(&r.path);
                        }
                    }
                    if let Some(i) = copy {
                        if let Some(r) = self.results.get(i) {
                            ui.ctx().copy_text(r.path.display().to_string());
                        }
                    }
                });
        });
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
