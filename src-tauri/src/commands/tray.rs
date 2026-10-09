//! The tray and menu-bar icon (Orbit Fleet redesign step 3.17, motion map
//! "Tray and menu bar"). Four states, each a still frame of its loader:
//! Breathe (idle and connected), Chase (a session is working), Halo
//! (something needs you) and Signal lost (the hub is gone). The frontend
//! works the state out from the rows it already has (`src/lib/tray_state.ts`)
//! and sets it with `set_tray_state`.
//!
//! The icons are `icons/tray/*.svg`, rendered to 64 px PNGs with
//! `rsvg-convert -w 64 -h 64 <name>.svg -o <name>.png`.
//!
//! Step 3.14's Halo on the macOS dock: the same call carries the Inbox count
//! the tray's Halo follows, and the dock icon wears it as its badge
//! ([`dock_badge`]), cleared when nothing waits. Only macOS has that dock;
//! elsewhere the tray is the whole story.

use serde::Deserialize;
use tauri::image::Image;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::Manager;

use fleet_core::ipc_error::{codes, IpcError};

const TRAY_ID: &str = "main";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrayState {
    Idle,
    Working,
    NeedsYou,
    Lost,
}

impl TrayState {
    fn png(self) -> &'static [u8] {
        match self {
            Self::Idle => include_bytes!("../../icons/tray/idle.png"),
            Self::Working => include_bytes!("../../icons/tray/working.png"),
            Self::NeedsYou => include_bytes!("../../icons/tray/needs_you.png"),
            Self::Lost => include_bytes!("../../icons/tray/lost.png"),
        }
    }

    fn tooltip(self) -> &'static str {
        match self {
            Self::Idle => "Orbit Fleet",
            // The manual's status words, as the status bar says them.
            Self::Working => "Orbit Fleet · Working",
            Self::NeedsYou => "Orbit Fleet · Needs you",
            Self::Lost => "Orbit Fleet · Hub unavailable",
        }
    }

    fn icon(self) -> tauri::Result<Image<'static>> {
        Image::from_bytes(self.png())
    }
}

fn show_main_window(app: &tauri::AppHandle) {
    // The main window by name: a pop-out terminal (step 5.4) is a window
    // too, and "Open Orbit Fleet" means the app.
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

/// Builds the tray at startup, idle. A desktop with no tray (a Linux session
/// without an indicator host) logs it and carries on.
pub fn install(app: &tauri::App) {
    let build = || -> tauri::Result<()> {
        let open = MenuItem::with_id(app, "tray-open", "Open Orbit Fleet", true, None::<&str>)?;
        let quit = MenuItem::with_id(app, "tray-quit", "Quit", true, None::<&str>)?;
        let menu = Menu::with_items(app, &[&open, &quit])?;
        TrayIconBuilder::with_id(TRAY_ID)
            .icon(TrayState::Idle.icon()?)
            .tooltip(TrayState::Idle.tooltip())
            .menu(&menu)
            .show_menu_on_left_click(false)
            .on_menu_event(|app, event| match event.id.as_ref() {
                "tray-open" => show_main_window(app),
                "tray-quit" => app.exit(0),
                _ => {}
            })
            .on_tray_icon_event(|tray, event| {
                if let TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                } = event
                {
                    show_main_window(tray.app_handle());
                }
            })
            .build(app)?;
        Ok(())
    };
    if let Err(e) = build() {
        tracing::warn!(error = %e, "tray icon unavailable");
    }
}

/// The dock badge for this many sessions waiting for you: the count, or
/// none at all when nothing waits (a `0` badge would still read as "look").
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn dock_badge(needs_you: Option<u32>) -> Option<i64> {
    needs_you.filter(|n| *n > 0).map(i64::from)
}

/// Halo on the dock: the main window's badge (macOS: the app's dock icon).
#[cfg(target_os = "macos")]
fn set_dock_badge(app: &tauri::AppHandle, needs_you: Option<u32>) {
    if let Some(w) = app.get_webview_window("main") {
        if let Err(e) = w.set_badge_count(dock_badge(needs_you)) {
            tracing::debug!(error = %e, "dock badge unavailable");
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn set_dock_badge(_app: &tauri::AppHandle, _needs_you: Option<u32>) {}

/// Switches the tray icon and its tooltip, and the dock's Halo badge to
/// `needs_you` (the Inbox count; absent from an older frontend, which leaves
/// no badge). Same in both modes: the tray and the dock are this window's.
/// No tray (see `install`) is not an error.
#[tauri::command]
pub async fn set_tray_state(
    app: tauri::AppHandle,
    state: TrayState,
    needs_you: Option<u32>,
) -> Result<(), IpcError> {
    set_dock_badge(&app, needs_you);
    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        return Ok(());
    };
    let icon = state
        .icon()
        .map_err(|e| IpcError::new(codes::E_INTERNAL, e.to_string()))?;
    tray.set_icon(Some(icon))
        .and_then(|()| tray.set_tooltip(Some(state.tooltip())))
        .map_err(|e| IpcError::new(codes::E_INTERNAL, e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tray_switches_through_all_four_states() {
        let mut seen_png = std::collections::HashSet::new();
        let mut seen_tip = std::collections::HashSet::new();
        use TrayState::*;
        for state in [Idle, Working, NeedsYou, Lost] {
            let icon = state.icon().unwrap_or_else(|e| panic!("{state:?}: {e}"));
            assert_eq!((icon.width(), icon.height()), (64, 64), "{state:?}");
            assert!(
                seen_png.insert(state.png()),
                "{state:?} reuses another state's icon"
            );
            assert!(
                seen_tip.insert(state.tooltip()),
                "{state:?} reuses another state's tooltip"
            );
        }
    }

    #[test]
    fn the_dock_badge_follows_the_inbox_count() {
        assert_eq!(dock_badge(None), None);
        assert_eq!(dock_badge(Some(0)), None, "nothing waiting: no badge");
        assert_eq!(dock_badge(Some(1)), Some(1));
        assert_eq!(dock_badge(Some(4)), Some(4));
        assert_eq!(dock_badge(Some(u32::MAX)), Some(i64::from(u32::MAX)));
    }

    #[test]
    fn state_names_match_the_frontend() {
        for (name, state) in [
            ("idle", TrayState::Idle),
            ("working", TrayState::Working),
            ("needs_you", TrayState::NeedsYou),
            ("lost", TrayState::Lost),
        ] {
            let parsed: TrayState = serde_json::from_value(serde_json::json!(name)).unwrap();
            assert_eq!(parsed, state);
        }
    }
}
