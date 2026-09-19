//! The carry engine of `move_session`: script builders and output parsers
//! that take a worktree's state — unpushed commits, staged, modified and
//! untracked files, small git-ignored files — from the source host to the
//! target without origin. Pure: no I/O, no `async`; `mod.rs` runs the
//! scripts. See `docs/superpowers/specs/2026-09-19-move-carry-engine-design.md`.

use crate::service::safe_kill::DirtyFile;
use serde::{Deserialize, Serialize};

/// `settings` key: largest git bundle (MiB) a move relays.
pub const SETTING_MAX_BUNDLE_MB: &str = "move.max_bundle_mb";
pub const DEFAULT_MAX_BUNDLE_MB: u64 = 500;
/// `settings` key: largest single git-ignored entry (KiB) a move carries.
pub const SETTING_IGNORED_ENTRY_KB: &str = "move.ignored_entry_kb";
pub const DEFAULT_IGNORED_ENTRY_KB: u64 = 1024;
/// `settings` key: total git-ignored payload (MiB) a move carries.
pub const SETTING_IGNORED_TOTAL_MB: &str = "move.ignored_total_mb";
pub const DEFAULT_IGNORED_TOTAL_MB: u64 = 20;

/// Final path components that are never carried: rebuildable, often huge,
/// frequently platform-specific. `worktrees` / `.worktrees`: nested git
/// worktrees never travel.
pub const DENYLIST: &[&str] = &[
    "node_modules",
    "target",
    ".venv",
    "venv",
    "dist",
    "build",
    "out",
    ".next",
    ".nuxt",
    ".svelte-kit",
    "__pycache__",
    ".gradle",
    ".cache",
    ".turbo",
    "coverage",
    "worktrees",
    ".worktrees",
];
/// Bound on carried ignored entries: they reach `tar` as argv.
pub const MAX_IGNORED_ENTRIES: usize = 500;

/// How the target's main clone came to exist.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetSeed {
    #[default]
    Existing,
    Cloned,
    /// `git init` + the bundle: origin was unreachable from the target.
    Initialized,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LeftReason {
    Denylisted,
    OverCap,
    /// The path is not valid UTF-8.
    UnsupportedName,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IgnoredEntry {
    pub path: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeftBehind {
    pub path: String,
    /// `None` for a deny-listed entry: it is never walked, so never sized.
    pub bytes: Option<u64>,
    pub reason: LeftReason,
}

/// What a move carried besides the transcript.
#[derive(Debug, Clone, Default, Serialize)]
pub struct CarryReport {
    /// Commits in the bundle besides the two snapshot commits.
    pub commits: u32,
    pub bundle_bytes: u64,
    /// The porcelain rows restored on the target.
    pub dirty_entries: Vec<DirtyFile>,
    pub ignored_carried: Vec<IgnoredEntry>,
    pub ignored_left_behind: Vec<LeftBehind>,
    pub target_seeded: TargetSeed,
}

/// One record of the ignored-list script.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListedIgnored {
    pub path: String,
    /// `None`: the script recognised a deny-listed name and did not size it.
    pub kb: Option<u64>,
    /// `false` when the path was not valid UTF-8 (`path` is then lossy).
    pub valid_name: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IgnoredSelection {
    pub carry: Vec<IgnoredEntry>,
    pub left: Vec<LeftBehind>,
}

/// Parse `<kb>\t<path>\0` records; a record without a tab is dropped.
pub fn parse_ignored_list(stdout: &[u8]) -> Vec<ListedIgnored> {
    stdout
        .split(|b| *b == 0)
        .filter(|rec| !rec.is_empty())
        .filter_map(|rec| {
            let tab = rec.iter().position(|b| *b == b'\t')?;
            let kb = std::str::from_utf8(&rec[..tab])
                .ok()?
                .trim()
                .parse::<i64>()
                .ok()?;
            let raw = &rec[tab + 1..];
            let (path, valid_name) = match std::str::from_utf8(raw) {
                Ok(p) => (p.to_string(), true),
                Err(_) => (String::from_utf8_lossy(raw).into_owned(), false),
            };
            Some(ListedIgnored {
                path,
                kb: u64::try_from(kb).ok(),
                valid_name,
            })
        })
        .collect()
}

fn denylisted(path: &str) -> bool {
    let base = path.trim_end_matches('/').rsplit('/').next().unwrap_or("");
    DENYLIST.contains(&base)
}

/// The carry policy: deny-list, per-entry cap, then smallest-first up to the
/// total cap and [`MAX_IGNORED_ENTRIES`].
pub fn select_ignored(
    listed: Vec<ListedIgnored>,
    entry_kb: u64,
    total_kb: u64,
) -> IgnoredSelection {
    let mut sel = IgnoredSelection::default();
    let mut candidates: Vec<(u64, String)> = Vec::new();
    for l in listed {
        let bytes = l.kb.map(|k| k.saturating_mul(1024));
        if !l.valid_name {
            sel.left.push(LeftBehind {
                path: l.path,
                bytes,
                reason: LeftReason::UnsupportedName,
            });
        } else if l.kb.is_none() || denylisted(&l.path) {
            sel.left.push(LeftBehind {
                path: l.path,
                bytes: None,
                reason: LeftReason::Denylisted,
            });
        } else if l.kb.unwrap_or(0) > entry_kb {
            sel.left.push(LeftBehind {
                path: l.path,
                bytes,
                reason: LeftReason::OverCap,
            });
        } else {
            candidates.push((l.kb.unwrap_or(0), l.path));
        }
    }
    candidates.sort();
    let mut used = 0u64;
    for (kb, path) in candidates {
        let bytes = kb.saturating_mul(1024);
        if sel.carry.len() < MAX_IGNORED_ENTRIES && used.saturating_add(kb) <= total_kb {
            used += kb;
            sel.carry.push(IgnoredEntry { path, bytes });
        } else {
            sel.left.push(LeftBehind {
                path,
                bytes: Some(bytes),
                reason: LeftReason::OverCap,
            });
        }
    }
    sel
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listed(path: &str, kb: Option<u64>) -> ListedIgnored {
        ListedIgnored {
            path: path.into(),
            kb,
            valid_name: true,
        }
    }

    #[test]
    fn ignored_list_parses_nul_records_and_flags_bad_names() {
        let mut out = b"4\t.env\0-1\tnode_modules/\0".to_vec();
        out.extend_from_slice(b"1\tbad\xff.txt\0");
        out.extend_from_slice(b"garbage-without-tab\0");
        let got = parse_ignored_list(&out);
        assert_eq!(got.len(), 3, "the record without a tab is dropped");
        assert_eq!(got[0].path, ".env");
        assert_eq!(got[0].kb, Some(4));
        assert_eq!(got[1].path, "node_modules/");
        assert_eq!(got[1].kb, None, "-1 means the script never walked it");
        assert!(!got[2].valid_name, "non-UTF-8 path");
    }

    #[test]
    fn selection_applies_denylist_caps_and_smallest_first() {
        let sel = select_ignored(
            vec![
                listed("big.bin", Some(2048)),        // over the 1024 entry cap
                listed("web/node_modules/", Some(1)), // deny-listed by basename
                listed("target/", None),              // deny-listed by the script
                listed("c.cfg", Some(600)),
                listed(".env", Some(4)),
                listed("b.cfg", Some(500)),
                ListedIgnored {
                    path: "bad\u{fffd}".into(),
                    kb: Some(1),
                    valid_name: false,
                },
            ],
            1024,
            1000, // total cap: .env(4) + b.cfg(500) fit, c.cfg(600) does not
        );
        let carried: Vec<&str> = sel.carry.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(carried, vec![".env", "b.cfg"], "smallest first");
        assert_eq!(sel.carry[0].bytes, 4 * 1024);
        let left = |p: &str| {
            sel.left
                .iter()
                .find(|l| l.path == p)
                .unwrap_or_else(|| panic!("{p}"))
        };
        assert_eq!(left("big.bin").reason, LeftReason::OverCap);
        assert_eq!(left("big.bin").bytes, Some(2048 * 1024));
        assert_eq!(left("c.cfg").reason, LeftReason::OverCap);
        assert_eq!(left("web/node_modules/").reason, LeftReason::Denylisted);
        assert_eq!(left("target/").reason, LeftReason::Denylisted);
        assert_eq!(left("target/").bytes, None, "never walked, size unknown");
        assert_eq!(left("bad\u{fffd}").reason, LeftReason::UnsupportedName);
    }

    #[test]
    fn selection_stops_at_the_entry_count_bound() {
        let many: Vec<_> = (0..MAX_IGNORED_ENTRIES + 3)
            .map(|i| listed(&format!("f{i:04}"), Some(1)))
            .collect();
        let sel = select_ignored(many, 1024, u64::MAX);
        assert_eq!(sel.carry.len(), MAX_IGNORED_ENTRIES);
        assert_eq!(sel.left.len(), 3);
        assert!(sel.left.iter().all(|l| l.reason == LeftReason::OverCap));
    }

    #[test]
    fn carry_report_serializes_snake_case() {
        let v = serde_json::to_value(CarryReport {
            target_seeded: TargetSeed::Initialized,
            ignored_left_behind: vec![LeftBehind {
                path: "target/".into(),
                bytes: None,
                reason: LeftReason::Denylisted,
            }],
            ..Default::default()
        })
        .unwrap();
        assert_eq!(v["target_seeded"], "initialized");
        assert_eq!(v["ignored_left_behind"][0]["reason"], "denylisted");
        assert!(v["ignored_left_behind"][0]["bytes"].is_null());
    }
}
