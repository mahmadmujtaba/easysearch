//! System tray icon (StatusNotifierItem over D-Bus).
//!
//! SNI is the shared tray protocol: KDE/Qt hosts it natively, and GTK-based
//! desktops (GNOME + AppIndicator extension, XFCE, Cinnamon, MATE) do too, so
//! one implementation covers both worlds. Pure Rust via `ksni`/`zbus` — no
//! system dependencies beyond the session D-Bus.

use ksni::blocking::TrayMethods;
use ksni::menu::{StandardItem, SubMenu};
use ksni::{Icon, MenuItem, ToolTip, Tray};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug, PartialEq)]
pub enum TrayMsg {
    /// Left-click on the icon: show/hide the window.
    Toggle,
    /// Menu item "Open": show and focus the window.
    Open,
    /// Re-run a query picked from the tray's "Recent searches" submenu.
    Search(String),
    /// Menu item "Quit": close the app for real.
    Quit,
}

pub struct AppTray {
    pub tx: Sender<TrayMsg>,
    pub title: String,
    /// Shared snapshot of recent searches (mirrored from the GUI prefs).
    pub history: Arc<Mutex<Vec<String>>>,
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
            description: "Realtime file & content search".into(),
            icon_name: String::new(),
            icon_pixmap: self.icon_pixmap(),
        }
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        let title = self.title.clone();
        let history: Vec<String> = self.history.lock().map(|h| h.clone()).unwrap_or_default();
        let mut items = vec![
            MenuItem::Standard(StandardItem {
                label: format!("Open {title}"),
                activate: Box::new(|t: &mut Self| {
                    let _ = t.tx.send(TrayMsg::Open);
                }),
                ..Default::default()
            }),
            MenuItem::Separator,
        ];

        if history.is_empty() {
            items.push(MenuItem::Standard(StandardItem {
                label: "Recent searches".into(),
                enabled: false,
                ..Default::default()
            }));
        } else {
            let mut children: Vec<MenuItem<Self>> = Vec::new();
            for q in history.iter().take(12) {
                let q_owned = q.clone();
                let label = if q_owned.chars().count() > 48 {
                    format!("{}…", q_owned.chars().take(48).collect::<String>())
                } else {
                    q_owned.clone()
                };
                children.push(MenuItem::Standard(StandardItem {
                    label,
                    activate: Box::new(move |t: &mut Self| {
                        let _ = t.tx.send(TrayMsg::Search(q_owned.clone()));
                    }),
                    ..Default::default()
                }));
            }
            items.push(MenuItem::SubMenu(SubMenu {
                label: "Recent searches".into(),
                submenu: children,
                ..Default::default()
            }));
        }

        items.push(MenuItem::Separator);
        items.push(MenuItem::Standard(StandardItem {
            label: "Quit".into(),
            activate: Box::new(|t: &mut Self| {
                let _ = t.tx.send(TrayMsg::Quit);
            }),
            ..Default::default()
        }));
        items
    }

    /// Left-click on the icon toggles the main window.
    fn activate(&mut self, _x: i32, _y: i32) {
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
/// It is the same mark as the window icon and the desktop entry — drawn from
/// [`crate::logo`] — so the three cannot drift apart.
pub fn tray_icon_pixmap() -> Icon {
    Icon {
        width: SIZE as i32,
        height: SIZE as i32,
        data: crate::logo::argb32(SIZE),
    }
}
