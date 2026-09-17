//! Canonical identity for LOCAL filesystem paths.
//!
//! The same directory reaches fleet in several spellings. A symlinked root
//! (`~/projects -> /mnt/sda4/projects`) gives fleet-spawned panes a logical
//! `$PWD` but a physical cwd; tmux `pane_current_path` and `claude agents`
//! report physical paths; `git worktree list` reports each worktree in the
//! form it was added from. Comparing those strings raw splits one checkout
//! into several identities (duplicate rows, orphaned sessions), so every
//! place that ingests or compares a local path goes through here.
//!
//! Only meaningful for paths on THIS machine: a remote host's symlinks are
//! invisible here, so remote paths are matched by owner/repo instead
//! (`service::sessions::HostPaths`).

use std::path::{Component, Path, PathBuf};

/// Physical form of `path`: `fs::canonicalize` when it exists; otherwise the
/// nearest existing ancestor canonicalized, plus the missing remainder (a
/// worktree about to be created, a checkout that was deleted). A remainder
/// with a `..` component cannot be resolved without the filesystem, so such
/// a path comes back unchanged, as does one with no existing ancestor.
pub fn canonical(path: &Path) -> PathBuf {
    if let Ok(p) = std::fs::canonicalize(path) {
        return p;
    }
    for ancestor in path.ancestors().skip(1) {
        if ancestor.as_os_str().is_empty() {
            break;
        }
        let Ok(mut base) = std::fs::canonicalize(ancestor) else {
            continue;
        };
        let Ok(rest) = path.strip_prefix(ancestor) else {
            break;
        };
        for c in rest.components() {
            match c {
                Component::Normal(n) => base.push(n),
                Component::CurDir => {}
                _ => return path.to_path_buf(),
            }
        }
        return base;
    }
    path.to_path_buf()
}

/// [`canonical`] for a `&str` path, lossily back to a `String`.
pub fn canonical_str(path: &str) -> String {
    canonical(Path::new(path)).to_string_lossy().into_owned()
}

/// True when `path` is `root` or lies below it, compared by whole components:
/// root `/b/x` does not contain `/b/x-build`. Pure, with no filesystem access.
pub fn is_within(path: &Path, root: &Path) -> bool {
    !root.as_os_str().is_empty() && path.starts_with(root)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn component_prefix_compares_whole_components() {
        assert!(is_within(Path::new("/b/x/src"), Path::new("/b/x")));
        assert!(is_within(Path::new("/b/x"), Path::new("/b/x/")));
        assert!(!is_within(Path::new("/b/x-build"), Path::new("/b/x")));
        assert!(!is_within(Path::new("/b/x-build/src"), Path::new("/b/x")));
        assert!(!is_within(Path::new("/b/x"), Path::new("")));
    }

    #[test]
    fn missing_tail_resolves_through_the_nearest_existing_ancestor() {
        let tmp = TempDir::new().unwrap();
        let real = canonical(tmp.path());
        let p = tmp.path().join("not-yet").join("wt");
        assert_eq!(canonical(&p), real.join("not-yet").join("wt"));
        let dotdot = tmp.path().join("gone").join("..").join("x");
        assert_eq!(
            canonical(&dotdot),
            dotdot,
            "an unresolvable .. is left alone"
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_root_resolves_to_the_physical_path() {
        let tmp = TempDir::new().unwrap();
        let real = tmp.path().join("mnt").join("projects");
        std::fs::create_dir_all(real.join("o").join("r")).unwrap();
        let link = tmp.path().join("home-projects");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let logical = link.join("o").join("r");
        let physical = canonical(&real.join("o").join("r"));
        assert_eq!(canonical(&logical), physical);
        assert_eq!(
            canonical_str(&logical.to_string_lossy()),
            physical.to_string_lossy()
        );
        // A not-yet-created worktree below the link resolves too.
        assert_eq!(
            canonical(&logical.join(".worktrees").join("f")),
            physical.join(".worktrees").join("f")
        );
        // Logical and physical spellings are the same tree once canonical,
        // and the component rule still holds (`r-build` is not under `r`).
        assert!(is_within(
            &canonical(&logical.join("src")),
            &canonical(&real)
        ));
        assert!(!is_within(
            &canonical(&link.join("o").join("r-build")),
            &canonical(&real.join("o").join("r"))
        ));
    }
}
