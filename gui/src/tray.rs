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
) -> Result<(
    std::sync::mpsc::Receiver<TrayMsg>,
    ksni::blocking::Handle<AppTray>,
), ksni::Error> {
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

const SIZE: i32 = 64;
// Lightning bolt polygon (classic zig-zag), coordinates in 0..64 space.
const BOLT: [(f32, f32); 7] = [
    (35.0, 6.0),
    (19.0, 36.0),
    (29.0, 36.0),
    (25.0, 58.0),
    (47.0, 26.0),
    (35.0, 26.0),
    (42.0, 6.0),
];
const BOLT_COLOR: (u8, u8, u8) = (122, 162, 247); // #7aa2f7 accent
const BG_COLOR: (u8, u8, u8) = (36, 40, 59); // #24283b widget bg

/// 64×64 ARGB32 (network byte order) pixmap for the StatusNotifierItem.
pub fn tray_icon_pixmap() -> Icon {
    let mut data = Vec::with_capacity((SIZE * SIZE * 4) as usize);
    for y in 0..SIZE {
        for x in 0..SIZE {
            let (r, g, b, a) = pixel(x as f32 + 0.5, y as f32 + 0.5);
            data.push(a);
            data.push(r);
            data.push(g);
            data.push(b);
        }
    }
    Icon { width: SIZE, height: SIZE, data }
}

fn pixel(x: f32, y: f32) -> (u8, u8, u8, u8) {
    if point_in_poly(x, y, &BOLT) {
        return (BOLT_COLOR.0, BOLT_COLOR.1, BOLT_COLOR.2, 255);
    }
    if inside_rounded_rect(x, y, 2.0, 2.0, SIZE as f32 - 4.0, SIZE as f32 - 4.0, 14.0) {
        return (BG_COLOR.0, BG_COLOR.1, BG_COLOR.2, 255);
    }
    (0, 0, 0, 0)
}

fn inside_rounded_rect(x: f32, y: f32, rx: f32, ry: f32, w: f32, h: f32, r: f32) -> bool {
    if x < rx || y < ry || x > rx + w || y > ry + h {
        return false;
    }
    // Distance from the inner rect's corners.
    let cx = x.clamp(rx + r, rx + w - r);
    let cy = y.clamp(ry + r, ry + h - r);
    let dx = x - cx;
    let dy = y - cy;
    dx * dx + dy * dy <= r * r
}

/// Ray-casting point-in-polygon test.
fn point_in_poly(x: f32, y: f32, poly: &[(f32, f32)]) -> bool {
    let mut inside = false;
    let mut j = poly.len() - 1;
    for i in 0..poly.len() {
        let (xi, yi) = poly[i];
        let (xj, yj) = poly[j];
        if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
            inside = !inside;
        }
        j = i;
    }
    inside
}
