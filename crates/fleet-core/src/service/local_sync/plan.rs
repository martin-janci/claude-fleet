//! The decision for one path, from the BASE, an open conflict (if any) and
//! what each side holds now. Pure: the pass gathers the inputs and carries
//! the answer out under its write guards. The table is the design's
//! (`docs/superpowers/specs/2026-10-07-local-workspace-sync-design.md`).

use crate::store::{BaseEntry, FileStat, LocalConflictRow, SideSeen};

/// One side of a path as this pass sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Now {
    Gone,
    Has { sha: String, stat: FileStat },
}

impl Now {
    fn sha(&self) -> Option<&str> {
        match self {
            Now::Gone => None,
            Now::Has { sha, .. } => Some(sha),
        }
    }

    pub(super) fn stat(&self) -> Option<FileStat> {
        match self {
            Now::Gone => None,
            Now::Has { stat, .. } => Some(*stat),
        }
    }

    pub(super) fn seen(&self) -> Option<SideSeen> {
        match self {
            Now::Gone => None,
            Now::Has { sha, stat } => Some(SideSeen {
                sha256: sha.clone(),
                stat: *stat,
            }),
        }
    }

    /// The same content as a conflict recorded for this side.
    fn same_as(&self, seen: Option<&SideSeen>) -> bool {
        self.sha() == seen.map(|s| s.sha256.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Action {
    /// Nothing to carry and nothing to record.
    Nothing,
    /// Both sides hold the same content: record it as the BASE (and drop an
    /// open conflict on the path).
    Record,
    /// Both sides are gone: drop the BASE entry and any conflict.
    Forget,
    /// Write the local file over the remote one, which must still hold
    /// `expect_remote` (`None` = must still be absent).
    Push { expect_remote: Option<String> },
    /// Write the remote file over the local one, which must still have
    /// `expect_local` (`None` = must still be absent).
    Pull { expect_local: Option<FileStat> },
    /// Delete the remote file, which must still hold this content.
    DeleteRemote { expect: String },
    /// Delete the local file, which must still have this stat.
    DeleteLocal { expect: FileStat },
    /// Record (or refresh) a conflict of this kind; write nothing.
    Conflict(&'static str),
}

/// The conflict kind two differing sides make.
fn conflict_kind(base: Option<&BaseEntry>, local: &Now, remote: &Now) -> &'static str {
    match (local, remote) {
        (Now::Gone, _) => "local_deleted",
        (_, Now::Gone) => "remote_deleted",
        _ if base.is_some() => "both_modified",
        _ => "both_added",
    }
}

pub(super) fn decide(
    base: Option<&BaseEntry>,
    conflict: Option<&LocalConflictRow>,
    local: &Now,
    remote: &Now,
) -> Action {
    // Both sides agree: whatever happened, it is the new BASE.
    if local.sha() == remote.sha() {
        return match local {
            Now::Gone if base.is_none() && conflict.is_none() => Action::Nothing,
            Now::Gone => Action::Forget,
            Now::Has { .. } => Action::Record,
        };
    }
    if let Some(c) = conflict {
        return decide_conflict(base, c, local, remote);
    }
    let Some(base) = base else {
        // First sight of the path on one side: copy it across.
        return match (local, remote) {
            (Now::Has { .. }, Now::Gone) => Action::Push {
                expect_remote: None,
            },
            (Now::Gone, Now::Has { .. }) => Action::Pull { expect_local: None },
            _ => Action::Conflict("both_added"),
        };
    };
    let l_same = local.sha() == Some(base.sha256.as_str());
    let r_same = remote.sha() == Some(base.sha256.as_str());
    match (l_same, r_same) {
        (true, true) => Action::Record, // unreachable: the shas would agree
        (false, true) => match local {
            Now::Has { .. } => Action::Push {
                expect_remote: Some(base.sha256.clone()),
            },
            Now::Gone => Action::DeleteRemote {
                expect: base.sha256.clone(),
            },
        },
        (true, false) => match remote {
            Now::Has { .. } => Action::Pull {
                expect_local: local.stat(),
            },
            Now::Gone => match local.stat() {
                Some(expect) => Action::DeleteLocal { expect },
                None => Action::Forget,
            },
        },
        (false, false) => Action::Conflict(conflict_kind(Some(base), local, remote)),
    }
}

/// A path with an open conflict (and sides that still differ).
fn decide_conflict(
    base: Option<&BaseEntry>,
    c: &LocalConflictRow,
    local: &Now,
    remote: &Now,
) -> Action {
    let l_moved = !local.same_as(c.local.as_ref());
    let r_moved = !remote.same_as(c.remote.as_ref());
    match c.resolution.as_deref() {
        // Keep local: carry the local side out, unless the remote moved
        // since the person looked — then ask again.
        Some("local") if !r_moved => match local {
            Now::Has { .. } => Action::Push {
                expect_remote: remote.sha().map(str::to_string),
            },
            Now::Gone => Action::DeleteRemote {
                expect: remote.sha().unwrap_or_default().to_string(),
            },
        },
        Some("remote") if !l_moved => match remote {
            Now::Has { .. } => Action::Pull {
                expect_local: local.stat(),
            },
            Now::Gone => match local.stat() {
                Some(expect) => Action::DeleteLocal { expect },
                None => Action::Forget,
            },
        },
        _ if l_moved || r_moved || c.resolution.is_some() => {
            Action::Conflict(conflict_kind(base, local, remote))
        }
        _ => Action::Nothing,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn st(n: i64) -> FileStat {
        FileStat { size: n, mtime: n }
    }
    fn has(sha: &str) -> Now {
        Now::Has {
            sha: sha.into(),
            stat: st(sha.len() as i64),
        }
    }
    fn base(sha: &str) -> BaseEntry {
        BaseEntry {
            sha256: sha.into(),
            local: Some(st(1)),
            remote: Some(st(1)),
        }
    }
    fn conflict(l: Option<&str>, r: Option<&str>, resolution: Option<&str>) -> LocalConflictRow {
        let side = |s: Option<&str>| {
            s.map(|s| SideSeen {
                sha256: s.into(),
                stat: st(s.len() as i64),
            })
        };
        LocalConflictRow {
            path: "p".into(),
            kind: "both_modified".into(),
            detected_at: 0,
            resolution: resolution.map(str::to_string),
            local: side(l),
            remote: side(r),
        }
    }

    #[test]
    fn one_sided_changes_are_carried_with_the_base_as_the_guard() {
        let b = base("b");
        assert_eq!(
            decide(Some(&b), None, &has("l"), &has("b")),
            Action::Push {
                expect_remote: Some("b".into())
            }
        );
        assert_eq!(
            decide(Some(&b), None, &has("b"), &has("rr")),
            Action::Pull {
                expect_local: Some(st(1))
            }
        );
        assert_eq!(
            decide(Some(&b), None, &Now::Gone, &has("b")),
            Action::DeleteRemote { expect: "b".into() }
        );
        assert_eq!(
            decide(Some(&b), None, &has("b"), &Now::Gone),
            Action::DeleteLocal { expect: st(1) }
        );
    }

    #[test]
    fn new_files_copy_across_and_never_overwrite() {
        assert_eq!(
            decide(None, None, &has("l"), &Now::Gone),
            Action::Push {
                expect_remote: None
            }
        );
        assert_eq!(
            decide(None, None, &Now::Gone, &has("r")),
            Action::Pull { expect_local: None }
        );
        assert_eq!(
            decide(None, None, &has("l"), &has("r")),
            Action::Conflict("both_added")
        );
        assert_eq!(decide(None, None, &has("x"), &has("x")), Action::Record);
        assert_eq!(decide(None, None, &Now::Gone, &Now::Gone), Action::Nothing);
    }

    #[test]
    fn both_sides_changing_differently_is_a_conflict_of_the_right_kind() {
        let b = base("b");
        assert_eq!(
            decide(Some(&b), None, &has("l"), &has("rr")),
            Action::Conflict("both_modified")
        );
        assert_eq!(
            decide(Some(&b), None, &Now::Gone, &has("rr")),
            Action::Conflict("local_deleted")
        );
        assert_eq!(
            decide(Some(&b), None, &has("l"), &Now::Gone),
            Action::Conflict("remote_deleted")
        );
        // The same edit on both sides is not a conflict.
        assert_eq!(decide(Some(&b), None, &has("x"), &has("x")), Action::Record);
        assert_eq!(
            decide(Some(&b), None, &Now::Gone, &Now::Gone),
            Action::Forget
        );
    }

    #[test]
    fn an_open_conflict_waits_until_the_sides_agree_or_someone_picks() {
        let b = base("b");
        let c = conflict(Some("l"), Some("rr"), None);
        assert_eq!(
            decide(Some(&b), Some(&c), &has("l"), &has("rr")),
            Action::Nothing
        );
        // A side moved: refresh it (which clears any pick).
        assert_eq!(
            decide(Some(&b), Some(&c), &has("l2"), &has("rr")),
            Action::Conflict("both_modified")
        );
        // Resolved by hand: same content on both sides.
        assert_eq!(
            decide(Some(&b), Some(&c), &has("rr"), &has("rr")),
            Action::Record
        );
        assert_eq!(
            decide(Some(&b), Some(&c), &Now::Gone, &Now::Gone),
            Action::Forget
        );
    }

    #[test]
    fn a_pick_is_carried_out_only_while_the_other_side_holds_still() {
        let b = base("b");
        let keep_local = conflict(Some("l"), Some("rr"), Some("local"));
        assert_eq!(
            decide(Some(&b), Some(&keep_local), &has("l"), &has("rr")),
            Action::Push {
                expect_remote: Some("rr".into())
            }
        );
        // The local side may move on (that is what "keep local" means)…
        assert_eq!(
            decide(Some(&b), Some(&keep_local), &has("l22"), &has("rr")),
            Action::Push {
                expect_remote: Some("rr".into())
            }
        );
        // …but a remote that moved after the pick asks again.
        assert_eq!(
            decide(Some(&b), Some(&keep_local), &has("l"), &has("r3")),
            Action::Conflict("both_modified")
        );
        let keep_remote = conflict(Some("l"), Some("rr"), Some("remote"));
        assert_eq!(
            decide(Some(&b), Some(&keep_remote), &has("l"), &has("rr")),
            Action::Pull {
                expect_local: Some(st(1))
            }
        );
        let keep_remote_gone = conflict(Some("l"), None, Some("remote"));
        assert_eq!(
            decide(Some(&b), Some(&keep_remote_gone), &has("l"), &Now::Gone),
            Action::DeleteLocal { expect: st(1) }
        );
        let keep_local_gone = conflict(None, Some("rr"), Some("local"));
        assert_eq!(
            decide(Some(&b), Some(&keep_local_gone), &Now::Gone, &has("rr")),
            Action::DeleteRemote {
                expect: "rr".into()
            }
        );
    }
}
