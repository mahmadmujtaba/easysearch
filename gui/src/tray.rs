//! System tray icon (StatusNotifierItem over D-Bus).
//!
//! The tray lives in the **background host process**, alongside the engine and
//! the control socket: the host outlives every window, so it is what owns the
//! icon that represents the running app. Closing a window leaves the host (with
//! the tray and the index) running; *Quit* stops the host (and its engine).
//!
//! SNI is the shared tray protocol: KDE/Qt hosts it natively, and GTK-based
//! desktops (GNOME + AppIndicator extension, XFCE, Cinnamon, MATE) do too, so
//! one implementation covers both worlds. Pure Rust via `ksni`/`zbus` — no
//! system dependencies beyond the session D-Bus.
//!
//! Clicking:
//! - **left click** flips the window — shows it ready to search, or hides it
//!   again ([`TrayMsg::Toggle`]);
//! - **middle click** does the same ([`TrayMsg::Toggle`]); and
//! - **right click** opens the full menu below.

use ksni::blocking::TrayMethods;
use ksni::menu::{StandardItem, SubMenu};
use ksni::{Icon, MenuItem, ToolTip, Tray};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug, PartialEq)]
pub enum TrayMsg {
    /// Show the window and focus the search box — left-click, and the menu's
    /// *Open EasySearch*.
    Open,
    /// Show / hide the window (the menu's *Show / Hide window*, middle-click).
    Toggle,
    /// Show the window, clear the current results and focus the search box.
    NewSearch,
    /// Empty the open window's result list.
    ClearResults,
    /// Re-run a query picked from the tray's *Recent searches* submenu.
    Search(String),
    /// Drop every entry from the recent-search history.
    ClearHistory,
    /// Rebuild the on-disk index.
    RebuildIndex,
    /// Pause or resume live indexing.
    TogglePause,
    /// Reveal the index folder in the file manager.
    OpenIndexFolder,
    /// Open the Settings dialog.
    Settings,
    /// Open the About dialog.
    About,
    /// Stop the app (window and engine).
    Quit,
}

pub struct AppTray {
    pub tx: Sender<TrayMsg>,
    pub title: String,
    /// Shared snapshot of recent searches (mirrored from the GUI prefs).
    pub history: Arc<Mutex<Vec<String>>>,
}

/// A plain menu item that sends `msg` when chosen.
fn msg_item(label: impl Into<String>, msg: TrayMsg) -> MenuItem<AppTray> {
    MenuItem::Standard(StandardItem {
        label: label.into(),
        activate: Box::new(move |t: &mut AppTray| {
            let _ = t.tx.send(msg.clone());
        }),
        ..Default::default()
    })
}

/// A menu item that runs `f` when chosen (used for the recent-search entries,
/// which each carry their own query).
fn callback_item(
    label: impl Into<String>,
    f: impl Fn(&mut AppTray) + Send + 'static,
) -> MenuItem<AppTray> {
    MenuItem::Standard(StandardItem {
        label: label.into(),
        activate: Box::new(f),
        ..Default::default()
    })
}

impl Tray for AppTray {
    fn id(&self) -> String {
        "easysearch-tray".into()
    }

    fn title(&self) -> String {
        self.title.clone()
    }

    fn icon_pixmap(&self) -> Vec<Icon> {
        vec![tray_icon_pixmap()]
    }

    fn tool_tip(&self) -> ToolTip {
        ToolTip {
            title: self.title.clone(),
            // No version here: the build stamp belongs to the app window only.
            description: "Realtime file & content search".to_string(),
            icon_name: String::new(),
            icon_pixmap: self.icon_pixmap(),
        }
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        let title = self.title.clone();
        let history: Vec<String> = self.history.lock().map(|h| h.clone()).unwrap_or_default();

        let mut items = vec![
            msg_item(format!("Open {title}"), TrayMsg::Open),
            msg_item("Show / Hide window", TrayMsg::Toggle),
            MenuItem::Separator,
            msg_item("New search", TrayMsg::NewSearch),
            msg_item("Clear results", TrayMsg::ClearResults),
            MenuItem::Separator,
        ];

        // Recent searches, always ending with *Clear history* so the action is
        // reachable even before anything has been searched for.
        let mut children: Vec<MenuItem<Self>> = Vec::new();
        for q in history.iter().take(12) {
            let q_owned = q.clone();
            let label = if q_owned.chars().count() > 48 {
                format!("{}…", q_owned.chars().take(48).collect::<String>())
            } else {
                q_owned.clone()
            };
            children.push(callback_item(label, move |t: &mut AppTray| {
                let _ = t.tx.send(TrayMsg::Search(q_owned.clone()));
            }));
        }
        if children.is_empty() {
            children.push(MenuItem::Standard(StandardItem {
                label: "Nothing yet".into(),
                enabled: false,
                ..Default::default()
            }));
        } else {
            children.push(MenuItem::Separator);
        }
        children.push(msg_item("Clear history", TrayMsg::ClearHistory));
        items.push(MenuItem::SubMenu(SubMenu {
            label: "Recent searches".into(),
            submenu: children,
            ..Default::default()
        }));

        items.push(MenuItem::Separator);
        items.push(msg_item("Rebuild index", TrayMsg::RebuildIndex));
        items.push(msg_item("Pause / resume indexing", TrayMsg::TogglePause));
        items.push(msg_item("Open index folder", TrayMsg::OpenIndexFolder));
        items.push(MenuItem::Separator);
        items.push(msg_item("Settings…", TrayMsg::Settings));
        items.push(msg_item(format!("About {title}"), TrayMsg::About));
        items.push(MenuItem::Separator);
        items.push(msg_item(format!("Quit {title}"), TrayMsg::Quit));
        items
    }

    /// Left-click: flip the window — show it ready to search, or hide it again.
    fn activate(&mut self, _x: i32, _y: i32) {
        let _ = self.tx.send(TrayMsg::Toggle);
    }

    /// Middle-click: flip the window on and off. Hosts differ on which click they
    /// deliver as `Activate`, so this keeps a one-click hide/show either way.
    fn secondary_activate(&mut self, _x: i32, _y: i32) {
        let _ = self.tx.send(TrayMsg::Toggle);
    }
}

/// Start the tray service on a background thread. Returns a message channel
/// and the handle that keeps the service registered (drop it to unregister).
pub fn spawn_tray(
    title: &str,
    history: Arc<Mutex<Vec<String>>>,
) -> Result<
    (
        std::sync::mpsc::Receiver<TrayMsg>,
        ksni::blocking::Handle<AppTray>,
    ),
    ksni::Error,
> {
    let (tx, rx) = std::sync::mpsc::channel();
    let tray = AppTray {
        tx,
        title: title.to_string(),
        history,
    };
    let handle = tray.spawn()?;
    Ok((rx, handle))
}

// --- icon rendering -------------------------------------------------------

/// Pixmap side, in pixels. Hosts scale this down, and 64 keeps it sharp on a
/// HiDPI tray.
const SIZE: u32 = 64;

/// `SIZE`×`SIZE` ARGB32 (network byte order) pixmap for the StatusNotifierItem.
/// It is the same transparent mark as the window icon and the desktop entry —
/// drawn from [`easysearch_core::logo`] — so the three cannot drift apart.
pub fn tray_icon_pixmap() -> Icon {
    Icon {
        width: SIZE as i32,
        height: SIZE as i32,
        data: easysearch_core::logo::argb32(SIZE),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tray(history: Vec<String>) -> AppTray {
        let (tx, _rx) = std::sync::mpsc::channel();
        AppTray {
            tx,
            title: "EasySearch".into(),
            history: Arc::new(Mutex::new(history)),
        }
    }

    /// The labels of a menu, flattening every item kind to its text.
    fn labels(items: Vec<MenuItem<AppTray>>) -> Vec<String> {
        items
            .iter()
            .filter_map(|m| match m {
                MenuItem::Standard(s) => Some(s.label.clone()),
                MenuItem::SubMenu(s) => Some(s.label.clone()),
                _ => None,
            })
            .collect()
    }

    fn recent_submenu(t: &AppTray) -> Vec<MenuItem<AppTray>> {
        t.menu()
            .into_iter()
            .find_map(|m| match m {
                MenuItem::SubMenu(s) if s.label == "Recent searches" => Some(s.submenu),
                _ => None,
            })
            .expect("a Recent searches submenu")
    }

    #[test]
    fn the_menu_exposes_the_full_action_set() {
        let top = labels(tray(vec!["report".into()]).menu());
        for want in [
            "Open EasySearch",
            "Show / Hide window",
            "New search",
            "Clear results",
            "Recent searches",
            "Rebuild index",
            "Pause / resume indexing",
            "Open index folder",
            "Settings…",
            "About EasySearch",
            "Quit EasySearch",
        ] {
            assert!(top.iter().any(|l| l == want), "missing {want:?} in {top:?}");
        }
    }

    #[test]
    fn clearing_history_is_always_offered() {
        // Even with no history the item is present, so the action stays reachable.
        for history in [Vec::new(), vec!["report".to_string()]] {
            let inner = labels(recent_submenu(&tray(history)));
            assert!(
                inner.iter().any(|l| l == "Clear history"),
                "missing Clear history in {inner:?}"
            );
        }
    }

    #[test]
    fn the_tray_never_shows_the_version() {
        let t = tray(vec![]);
        assert_eq!(t.tool_tip().description, "Realtime file & content search");
        // The menu labels carry the name only, never a build stamp.
        for label in labels(t.menu()) {
            assert!(!label.contains("v0."), "{label:?} leaked the version");
        }
    }

    #[test]
    fn clicking_flips_the_window() {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut t = AppTray {
            tx,
            title: "EasySearch".into(),
            history: Arc::new(Mutex::new(Vec::new())),
        };
        t.activate(0, 0);
        t.secondary_activate(0, 0);
        // Both a left and a middle click flip the window, so a second click on a
        // visible window hides it.
        assert_eq!(rx.try_recv().unwrap(), TrayMsg::Toggle);
        assert_eq!(rx.try_recv().unwrap(), TrayMsg::Toggle);
    }
}
