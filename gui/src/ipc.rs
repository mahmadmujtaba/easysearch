//! Control socket — driving a *running* GUI from the command line.
//!
//! Wayland deliberately has no global-hotkey API, and every desktop invents its
//! own (KGlobalAccel, GNOME custom keybindings, the GlobalShortcuts portal). The
//! portable answer is to let the desktop run a command and have that command
//! talk to the instance that is already up:
//!
//! ```text
//! everything-linux --toggle      # show/hide the window
//! everything-linux --show        # bring it to the front
//! everything-linux --search TODO # run a search from a shortcut
//! everything-linux --quit        # ask it to exit
//! ```
//!
//! The GUI listens on `$XDG_RUNTIME_DIR/everything-linux.sock` (mode `0600`, so
//! only the same user can talk to it) and each invocation is one line of text.
//! A shortcut in the desktop's own settings is all that is needed — see
//! `docs/ui.md`.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::sync::mpsc;

/// A command sent to the running GUI.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    /// Show the window if hidden, hide it if visible.
    Toggle,
    Show,
    Hide,
    Quit,
    /// Show the window and run this query.
    Search(String),
}

impl Command {
    /// Parse one line of the wire format.
    pub fn parse(line: &str) -> Option<Command> {
        let line = line.trim();
        let (head, rest) = match line.split_once(char::is_whitespace) {
            Some((head, rest)) => (head, rest.trim()),
            None => (line, ""),
        };
        Some(match head.to_ascii_lowercase().as_str() {
            "toggle" => Command::Toggle,
            "show" => Command::Show,
            "hide" => Command::Hide,
            "quit" | "exit" => Command::Quit,
            "search" => Command::Search(rest.to_string()),
            _ => return None,
        })
    }

    pub fn encode(&self) -> String {
        match self {
            Command::Toggle => "toggle".to_string(),
            Command::Show => "show".to_string(),
            Command::Hide => "hide".to_string(),
            Command::Quit => "quit".to_string(),
            Command::Search(query) => format!("search {query}"),
        }
    }
}

/// `$XDG_RUNTIME_DIR/everything-linux.sock`, falling back to the temp dir.
pub fn socket_path() -> PathBuf {
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    dir.join("everything-linux.sock")
}

/// Send one command to a running instance.
///
/// `Ok(false)` means nothing is listening — which is a normal state (no GUI is
/// running yet), not an error. A stale socket left by a crashed instance is
/// cleaned up.
pub fn send(command: &Command) -> std::io::Result<bool> {
    send_to(&socket_path(), command)
}

fn send_to(path: &std::path::Path, command: &Command) -> std::io::Result<bool> {
    use std::io::ErrorKind;
    use std::os::unix::net::UnixStream;

    let mut stream = match UnixStream::connect(path) {
        Ok(stream) => stream,
        Err(e) if matches!(e.kind(), ErrorKind::NotFound | ErrorKind::ConnectionRefused) => {
            if e.kind() == ErrorKind::ConnectionRefused {
                // The listener is gone but its socket file lingers.
                let _ = std::fs::remove_file(path);
            }
            return Ok(false);
        }
        Err(e) => return Err(e),
    };
    stream.write_all(command.encode().as_bytes())?;
    stream.write_all(b"\n")?;
    stream.flush()?;
    Ok(true)
}

/// Bind the control socket and forward commands to `tx` until the process ends.
///
/// Does nothing when another instance is already serving it, so a second GUI
/// (or the standalone `everything-gui` binary) can never steal the socket.
pub fn spawn_listener(tx: mpsc::Sender<Command>) {
    spawn_listener_at(&socket_path(), tx);
}

fn spawn_listener_at(path: &std::path::Path, tx: mpsc::Sender<Command>) {
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::UnixListener;

    if std::os::unix::net::UnixStream::connect(path).is_ok() {
        return; // a live instance owns it
    }
    let _ = std::fs::remove_file(path); // stale file
    let Ok(listener) = UnixListener::bind(path) else {
        return; // no writable runtime dir: shortcuts just won't work
    };
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));

    std::thread::Builder::new()
        .name("ipc".into())
        .spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else {
                    continue;
                };
                let mut line = String::new();
                if BufReader::new(&stream).read_line(&mut line).is_ok()
                    && let Some(command) = Command::parse(&line)
                {
                    let ack = format!("ok {}\n", command.encode());
                    let _ = tx.send(command);
                    let mut stream = &stream;
                    let _ = stream.write_all(ack.as_bytes());
                }
            }
        })
        .ok();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn commands_round_trip_through_the_wire_format() {
        for command in [
            Command::Toggle,
            Command::Show,
            Command::Hide,
            Command::Quit,
            Command::Search("invoice 2026 !draft".to_string()),
        ] {
            assert_eq!(Command::parse(&command.encode()), Some(command.clone()));
        }
    }

    #[test]
    fn parsing_is_lenient_about_case_and_noise() {
        assert_eq!(Command::parse("  TOGGLE \n"), Some(Command::Toggle));
        assert_eq!(Command::parse("exit"), Some(Command::Quit));
        assert_eq!(
            Command::parse("search\n"),
            Some(Command::Search(String::new()))
        );
        assert_eq!(
            Command::parse("search   a b  "),
            Some(Command::Search("a b".into()))
        );
        assert_eq!(Command::parse("nonsense"), None);
        assert_eq!(Command::parse(""), None);
    }

    #[test]
    fn the_socket_path_is_in_the_runtime_dir() {
        let path = socket_path();
        assert!(path.ends_with("everything-linux.sock"));
    }

    #[test]
    fn commands_reach_a_listener_and_only_one_listener_serves() {
        let dir = std::env::temp_dir().join(format!(
            "evfl-ipc-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("ctrl.sock");

        let (tx, rx) = mpsc::channel();
        spawn_listener_at(&path, tx);
        assert!(path.exists(), "the listener binds synchronously");

        assert!(send_to(&path, &Command::Toggle).unwrap());
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            Command::Toggle
        );

        assert!(send_to(&path, &Command::Search("hello world".into())).unwrap());
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            Command::Search("hello world".into())
        );

        // A second listener must not steal the socket from the live one.
        let (tx2, rx2) = mpsc::channel();
        spawn_listener_at(&path, tx2);
        assert!(send_to(&path, &Command::Show).unwrap());
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            Command::Show
        );
        assert!(
            rx2.try_recv().is_err(),
            "the second listener must stay idle"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sending_with_no_listener_is_not_an_error() {
        let path = std::env::temp_dir().join(format!(
            "evfl-ipc-missing-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        assert!(!send_to(&path, &Command::Show).unwrap());
    }
}
