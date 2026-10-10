//! Page actions (declarative pages P5): a button on a settings or data page
//! that runs one existing desktop command with no arguments, then re-reads
//! the page's data items. Like a resource's actions, an action names a
//! command, never code, so that command's hub verdict applies unchanged; a
//! read-only page (a paired desktop) shows no action.

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct PageAction {
    /// Dotted, e.g. `work.retention_sweep`: what a page's `action` item names.
    pub id: &'static str,
    /// The button's words.
    pub label: &'static str,
    /// The desktop command it runs, with no arguments.
    pub command: &'static str,
    /// One sentence under the button: what it does.
    pub help: &'static str,
    /// Asked before it runs, when set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confirm: Option<&'static str>,
}

/// Every page action.
pub const PAGE_ACTIONS: &[PageAction] = &[
    PageAction {
        id: "work.retention_sweep",
        label: "Sweep now",
        command: "work_retention_sweep",
        help: "Delete what is past its window now, as the GC tick would: at most 2,000 rows per table, the rest over later sweeps.",
        confirm: None,
    },
    // Gap plan G4.6: the repair buttons Settings shows beside their
    // settings.
    PageAction {
        id: "repair.now",
        label: "Repair now",
        command: "repair_workspaces_now",
        help: "Re-add deleted worktree folders on every host now, as the automatic repair would: at most five per run, and an unmounted volume is never touched.",
        confirm: None,
    },
    PageAction {
        id: "restore.lost",
        label: "Restore lost sessions",
        command: "restore_all_lost_sessions",
        help: "Resume every host's lost sessions now, in batches with the pause between them set above.",
        confirm: Some("Resume every lost session on every host now? Each one starts its agent again on its host."),
    },
    // Gap plan G4.5: Debug devices' list, every host at once (a record's
    // own "Rescan host" scans one).
    PageAction {
        id: "debug_devices.scan_all",
        label: "Scan all hosts",
        command: "scan_debug_devices",
        help: "Look for attached phones, emulators and simulators on every host now.",
        confirm: None,
    },
];

pub fn action(id: &str) -> Option<&'static PageAction> {
    PAGE_ACTIONS.iter().find(|a| a.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_actions_are_well_formed() {
        let mut ids = std::collections::BTreeSet::new();
        for a in PAGE_ACTIONS {
            assert!(ids.insert(a.id), "duplicate page action {}", a.id);
            assert!(a.id.contains('.'), "{}: a dotted id", a.id);
            assert!(a.help.ends_with('.'), "{}: help is a sentence", a.id);
            if let Some(c) = a.confirm {
                assert!(c.ends_with('.'), "{}: a confirm is a sentence", a.id);
            }
        }
    }
}
