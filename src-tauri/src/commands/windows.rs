//! Pop-out terminal windows (Orbit Fleet redesign step 5.4): one session's
//! agent pane or shell terminal in a window of its own.
//!
//! A pop-out is a second webview on the same `index.html`. It learns what to
//! show from its own window label, `term-<session id>-agent` or
//! `term-<session id>-sh<N>` ([`popout_label`]), so the URL carries nothing,
//! and it attaches through the same `pty_open` as the main window under a
//! pty id equal to that label: a second `tmux attach` to the same tmux
//! session, never the main window's PTY. Closing the window closes that PTY
//! (`lib.rs`'s window-event handler, [`is_popout_label`]); it never stops the
//! session or the terminal.

use fleet_core::ipc_error::{codes, IpcError};
use serde::Deserialize;
use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

/// The prefix every pop-out window's label starts with.
pub const POPOUT_PREFIX: &str = "term-";

/// The most shell terminals a session keeps (`tmux::MAX_SHELL_TERMINALS`).
const MAX_SHELL: u8 = 9;

/// A window title is shown, never parsed; this only keeps a hostile one
/// short and on one line.
const MAX_TITLE: usize = 200;

/// The label of `session_id`'s pop-out for `shell` (`None`: the agent's
/// pane). The frontend parses it back with `parsePopoutLabel` in
/// `src/lib/terminal_popout.ts`.
pub fn popout_label(session_id: i64, shell: Option<u8>) -> String {
    match shell {
        None => format!("{POPOUT_PREFIX}{session_id}-agent"),
        Some(n) => format!("{POPOUT_PREFIX}{session_id}-sh{n}"),
    }
}

/// The main window's label: Tauri's default for the one window
/// `tauri.conf.json` declares without a label.
pub const MAIN_LABEL: &str = "main";

/// Is `label` a pop-out's? The main window is `main`; closing it shuts the
/// app's connections down, closing a pop-out closes only its own PTY.
pub fn is_popout_label(label: &str) -> bool {
    label.starts_with(POPOUT_PREFIX)
}

/// What a window's `Destroyed` event does, by its label.
#[derive(Debug, PartialEq, Eq)]
pub enum OnDestroyed {
    /// The main window: close every ssh master, tunnel and PTY.
    ShutDown,
    /// A pop-out: close its own PTY and nothing else.
    ClosePty,
    /// Any other window: nothing. Only the main window's going away may
    /// take the app's connections down (r18).
    Nothing,
}

pub fn on_destroyed(label: &str) -> OnDestroyed {
    if label == MAIN_LABEL {
        OnDestroyed::ShutDown
    } else if is_popout_label(label) {
        OnDestroyed::ClosePty
    } else {
        OnDestroyed::Nothing
    }
}

fn clean_title(title: &str) -> String {
    let one_line: String = title
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(MAX_TITLE)
        .collect();
    let t = one_line.trim();
    if t.is_empty() {
        "Orbit Fleet".to_owned()
    } else {
        t.to_owned()
    }
}

#[derive(Debug, Deserialize)]
pub struct OpenTerminalWindowArgs {
    pub session_id: i64,
    /// The shell terminal to pop out; absent for the agent's pane.
    #[serde(default)]
    pub shell: Option<u8>,
    /// The window's title, as the pane names the session.
    pub title: String,
}

fn validate(args: &OpenTerminalWindowArgs) -> Result<(), IpcError> {
    if args.session_id <= 0 {
        return Err(IpcError::new(
            codes::E_INVALID,
            "session_id must be positive",
        ));
    }
    if let Some(n) = args.shell {
        if n == 0 || n > MAX_SHELL {
            return Err(IpcError::new(
                codes::E_INVALID,
                format!("shell must be 1-{MAX_SHELL}"),
            ));
        }
    }
    Ok(())
}

/// Pop a terminal out into its own window, or bring that window forward
/// when it is already open. Async: building a window from a sync command
/// deadlocks on Windows.
#[tauri::command]
pub async fn open_terminal_window(
    app: tauri::AppHandle,
    args: OpenTerminalWindowArgs,
) -> Result<String, IpcError> {
    validate(&args)?;
    let label = popout_label(args.session_id, args.shell);
    if let Some(w) = app.get_webview_window(&label) {
        let _ = w.unminimize();
        let _ = w.set_focus();
        return Ok(label);
    }
    WebviewWindowBuilder::new(&app, &label, WebviewUrl::App("index.html".into()))
        .title(clean_title(&args.title))
        .inner_size(900.0, 560.0)
        .min_inner_size(420.0, 240.0)
        .build()
        .map_err(|e| IpcError::new(codes::E_INTERNAL, format!("open window: {e}")))?;
    Ok(label)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_main_window_going_away_shuts_the_app_down() {
        assert_eq!(on_destroyed("main"), OnDestroyed::ShutDown);
        assert_eq!(on_destroyed(&popout_label(4, None)), OnDestroyed::ClosePty);
        assert_eq!(
            on_destroyed(&popout_label(4, Some(2))),
            OnDestroyed::ClosePty
        );
        assert_eq!(on_destroyed("settings"), OnDestroyed::Nothing);
        assert_eq!(on_destroyed(""), OnDestroyed::Nothing);
    }

    /// The config's one window takes Tauri's default label, `main`; a label
    /// given there must be `main` too, or no window would ever shut down.
    #[test]
    fn the_configured_window_is_the_main_one() {
        let conf = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tauri.conf.json");
        let conf: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(conf).unwrap()).unwrap();
        let windows = conf["app"]["windows"].as_array().unwrap();
        assert_eq!(windows.len(), 1);
        let label = windows[0]["label"].as_str().unwrap_or("main");
        assert_eq!(label, MAIN_LABEL);
    }

    #[test]
    fn a_label_names_the_session_and_the_terminal() {
        assert_eq!(popout_label(12, None), "term-12-agent");
        assert_eq!(popout_label(12, Some(3)), "term-12-sh3");
        assert!(is_popout_label(&popout_label(1, Some(1))));
        assert!(!is_popout_label("main"));
    }

    /// The label doubles as the pop-out's pty id, so it must pass
    /// `pty::validate_pty_id`.
    #[test]
    fn a_label_is_a_valid_pty_id() {
        for shell in [None, Some(1), Some(MAX_SHELL)] {
            crate::pty::validate_pty_id(&popout_label(i64::MAX, shell)).expect("valid pty id");
        }
    }

    #[test]
    fn a_bad_shell_or_session_is_refused() {
        let a = |session_id, shell| OpenTerminalWindowArgs {
            session_id,
            shell,
            title: "t".into(),
        };
        assert!(validate(&a(1, None)).is_ok());
        assert!(validate(&a(1, Some(9))).is_ok());
        assert!(validate(&a(1, Some(0))).is_err());
        assert!(validate(&a(1, Some(10))).is_err());
        assert!(validate(&a(0, None)).is_err());
    }

    #[test]
    fn a_title_is_one_short_line() {
        assert_eq!(clean_title("api\nShell 2"), "api Shell 2");
        assert_eq!(clean_title("   "), "Orbit Fleet");
        assert_eq!(clean_title(&"x".repeat(500)).len(), MAX_TITLE);
    }
}
