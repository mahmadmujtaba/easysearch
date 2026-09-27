//! The engine protocol: one JSON object per line, over the engine's
//! stdin/stdout.
//!
//! The engine runs as a **child process of the app**, not as a networked
//! service: the app spawns it, owns its pipes and kills it on exit. There is no
//! listening socket, no port and no HTTP, so nothing about the index is reachable
//! from outside the pair of processes that share these pipes.
//!
//! Framing is newline-delimited JSON. A JSON string escapes its own newlines, so
//! one object per line is unambiguous, and the reader needs no length prefix.
//!
//! ```text
//! → {"id":1,"op":"search","payload":{…Query…}}
//! ← {"id":1,"data":{…SearchResponseDto…}}
//! ← {"id":2,"error":"bad regex: …"}
//! ```

use serde::{Deserialize, Serialize};

/// What the app asks the engine to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Op {
    /// Version and uptime.
    Health,
    /// Engine status plus the exact index counts.
    Status,
    /// Run a query.
    Search,
    /// Count matches without returning rows.
    Count,
    /// Rebuild the on-disk index in the background.
    Rebuild,
    /// Change live index settings (and optionally rebuild).
    Config,
    /// Return freed heap pages to the OS (`malloc_trim`) — sent when the UI
    /// leaves content search, whose live-scan buffers are the largest transient
    /// allocation it makes.
    Trim,
    /// Stop the engine process.
    Shutdown,
}

/// One request. `payload` carries the op's arguments (a `Query`, a
/// [`ConfigPatch`], or nothing).
#[derive(Debug, Serialize, Deserialize)]
pub struct Request {
    pub id: u64,
    pub op: Op,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<serde_json::Value>,
}

/// One reply. Exactly one of `data` / `error` is present.
#[derive(Debug, Serialize, Deserialize)]
pub struct Response {
    pub id: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Response {
    pub fn ok(id: u64, data: serde_json::Value) -> Response {
        Response {
            id,
            data: Some(data),
            error: None,
        }
    }

    /// A successful reply with no body (`rebuild`, `shutdown`, …).
    pub fn done(id: u64) -> Response {
        Response {
            id,
            data: None,
            error: None,
        }
    }

    pub fn failed(id: u64, error: impl Into<String>) -> Response {
        Response {
            id,
            data: None,
            error: Some(error.into()),
        }
    }

    /// The reply's body as JSON, or the error it carried.
    pub fn into_data(self) -> Result<serde_json::Value, String> {
        match (self.data, self.error) {
            (_, Some(error)) => Err(error),
            (Some(data), None) => Ok(data),
            (None, None) => Ok(serde_json::Value::Null),
        }
    }
}

/// A message the engine host pushes to a window *without being asked*.
///
/// When the window is a child of a background host (the tray + engine process),
/// the host drives the window over the same pipes: `Show`/`Hide`/`Quit` mirror
/// the tray and `--toggle`/`--quit`, `Search` carries a query from a shortcut or
/// the tray's recent-searches menu, and the remaining variants are the tray's
/// other actions (`FocusSearch`, `NewSearch`, `ClearResults`, `Settings`,
/// `About`, `OpenIndexFolder`). It is written as one line, like a [`Response`],
/// and distinguished by carrying `event` instead of `id`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    /// Bring the window to the front.
    Show,
    /// Close the window; the host keeps running in the background.
    Hide,
    /// Stop the window *and* the host.
    Quit,
    /// Show the window and run this query.
    Search { query: String },
    /// Show the window and put the cursor in the search box.
    FocusSearch,
    /// Show the window, clear the current query and results, and focus the
    /// search box — a fresh start (the tray's *New search*).
    NewSearch,
    /// Empty the open window's result list, leaving the window as it is.
    ClearResults,
    /// Show the window with the Settings dialog open.
    Settings,
    /// Show the window with the About dialog open.
    About,
    /// Show the window and reveal the index folder in the file manager.
    OpenIndexFolder,
    /// Drop the recent-search history.
    ClearHistory,
}

/// The body of an [`Op::Config`] request; every field is optional, so a caller
/// changes only what it means to.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct ConfigPatch {
    /// Honour `.gitignore` / `.ignore` files.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub respect: Option<bool>,
    /// Follow symbolic links while walking.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub follow_symlinks: Option<bool>,
    /// Replace the excluded-directory list (resolved by the engine; `~` expands
    /// to the engine's `$HOME`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exclude_dirs: Option<Vec<String>>,
    /// Background content cache on/off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_index: Option<bool>,
    /// Rebuild the index after a walk setting changed.
    #[serde(default)]
    pub rebuild: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_survives_a_line_of_json() {
        let request = Request {
            id: 7,
            op: Op::Search,
            payload: Some(serde_json::json!({"name": "a\nb"})),
        };
        let line = serde_json::to_string(&request).unwrap();
        assert!(!line.contains('\n'), "a frame is one line: {line}");
        let back: Request = serde_json::from_str(&line).unwrap();
        assert_eq!(back.id, 7);
        assert_eq!(back.op, Op::Search);
        assert_eq!(back.payload.unwrap()["name"], "a\nb");
    }

    #[test]
    fn the_ops_are_lower_snake_case_on_the_wire() {
        assert_eq!(
            serde_json::to_string(&Op::Shutdown).unwrap(),
            "\"shutdown\""
        );
        assert_eq!(
            serde_json::from_str::<Op>("\"content_index\"").ok(),
            None,
            "unknown ops are rejected"
        );
    }

    #[test]
    fn a_response_carries_either_data_or_an_error() {
        assert_eq!(
            Response::ok(1, serde_json::json!({"count": 3}))
                .into_data()
                .unwrap()["count"],
            3
        );
        assert!(Response::done(2).into_data().is_ok());
        assert_eq!(
            Response::failed(3, "no engine").into_data().unwrap_err(),
            "no engine"
        );
    }

    #[test]
    fn a_config_patch_only_sends_what_changed() {
        let patch = ConfigPatch {
            respect: Some(false),
            ..Default::default()
        };
        let json = serde_json::to_string(&patch).unwrap();
        assert!(json.contains("respect"));
        assert!(!json.contains("follow_symlinks"), "{json}");
        assert!(!json.contains("content_index"), "{json}");
    }

    #[test]
    fn an_event_is_tagged_and_has_no_id() {
        let show = serde_json::to_string(&Event::Show).unwrap();
        assert_eq!(show, r#"{"event":"show"}"#);
        let search = serde_json::to_string(&Event::Search {
            query: "a b".into(),
        })
        .unwrap();
        assert!(search.contains(r#""event":"search""#), "{search}");
        assert!(!search.contains('\n'), "an event is one line: {search}");
        assert_eq!(serde_json::from_str::<Event>(&show).unwrap(), Event::Show);
        // A response is not an event, and vice versa.
        assert!(serde_json::from_str::<Event>(r#"{"id":1,"data":null}"#).is_err());
    }

    #[test]
    fn the_tray_events_round_trip() {
        // Every tray action survives a line of JSON, tagged lower-snake-case.
        for (event, tag) in [
            (Event::FocusSearch, r#"{"event":"focus_search"}"#),
            (Event::NewSearch, r#"{"event":"new_search"}"#),
            (Event::ClearResults, r#"{"event":"clear_results"}"#),
            (Event::Settings, r#"{"event":"settings"}"#),
            (Event::About, r#"{"event":"about"}"#),
            (Event::OpenIndexFolder, r#"{"event":"open_index_folder"}"#),
            (Event::ClearHistory, r#"{"event":"clear_history"}"#),
        ] {
            let line = serde_json::to_string(&event).unwrap();
            assert_eq!(line, tag);
            assert_eq!(serde_json::from_str::<Event>(&line).unwrap(), event);
        }
    }
}
