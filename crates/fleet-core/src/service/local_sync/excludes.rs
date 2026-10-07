//! What a link never carries, on either side: the built-in defaults (VCS
//! metadata, build outputs, IDE metadata) plus the link's own patterns, in
//! gitignore syntax. `.gitignore` itself is handled separately — by git on
//! the host and by the local walk — so this matcher holds only the patterns
//! fleet adds.

use crate::ipc_error::{codes, IpcError};
use ignore::gitignore::{Gitignore, GitignoreBuilder};

/// Always left out, whatever a link's own patterns say: VCS metadata, the
/// sync's own temp files, and the local version of a conflicting file that
/// "Ask AI to resolve" puts next to it on the host (`<path>.fleet-local`).
pub const FIXED_EXCLUDES: &[&str] = &[".git", ".fleet-sync-*", "*.fleet-local"];

/// Left out by default. A link's own patterns come after these, so
/// `!build/` brings a default back.
pub const DEFAULT_EXCLUDES: &[&str] = &[
    "target/",
    "build/",
    ".gradle/",
    "node_modules/",
    ".idea/",
    "*.iml",
    ".vscode/",
    ".DS_Store",
    "dist/",
    "out/",
    ".next/",
    "__pycache__/",
    ".venv/",
];

/// A link's own patterns, at most.
pub const MAX_USER_EXCLUDES: usize = 100;

#[derive(Clone)]
pub(crate) struct Excludes {
    fixed: Gitignore,
    rest: Gitignore,
}

impl Excludes {
    /// `E_INVALID` for a pattern gitignore syntax cannot parse, or too many.
    pub(crate) fn new(user: &[String]) -> Result<Self, IpcError> {
        if user.len() > MAX_USER_EXCLUDES {
            return Err(IpcError::new(
                codes::E_INVALID,
                format!("at most {MAX_USER_EXCLUDES} exclude patterns"),
            ));
        }
        let mut f = GitignoreBuilder::new("");
        for p in FIXED_EXCLUDES {
            f.add_line(None, p)
                .map_err(|e| IpcError::new(codes::E_INTERNAL, format!("fixed exclude: {e}")))?;
        }
        let fixed = f
            .build()
            .map_err(|e| IpcError::new(codes::E_INTERNAL, format!("fixed excludes: {e}")))?;
        let mut b = GitignoreBuilder::new("");
        for p in DEFAULT_EXCLUDES {
            b.add_line(None, p)
                .map_err(|e| IpcError::new(codes::E_INTERNAL, format!("default exclude: {e}")))?;
        }
        for p in user {
            let p = p.trim();
            if p.is_empty() || p.starts_with('#') {
                continue;
            }
            b.add_line(None, p).map_err(|e| {
                IpcError::new(codes::E_INVALID, format!("exclude pattern {p:?}: {e}"))
            })?;
        }
        let g = b
            .build()
            .map_err(|e| IpcError::new(codes::E_INVALID, format!("exclude patterns: {e}")))?;
        Ok(Excludes { fixed, rest: g })
    }

    /// Whether `rel` (a `/`-separated path below the root), or any directory
    /// above it, is left out.
    pub(crate) fn excluded(&self, rel: &str, is_dir: bool) -> bool {
        self.fixed
            .matched_path_or_any_parents(rel, is_dir)
            .is_ignore()
            || self
                .rest
                .matched_path_or_any_parents(rel, is_dir)
                .is_ignore()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_cover_vcs_build_and_ide_metadata_at_any_depth() {
        let x = Excludes::new(&[]).unwrap();
        for p in [
            ".git/config",
            "target/debug/app",
            "app/build/classes/A.class",
            "android/.gradle/x",
            "web/node_modules/a/index.js",
            ".idea/workspace.xml",
            "mod/app.iml",
            "src/.DS_Store",
            ".fleet-sync-ab12/f/x",
        ] {
            assert!(x.excluded(p, false), "{p} should be excluded");
        }
        for p in [
            "src/main.rs",
            "build.gradle",
            "docs/target.md",
            ".gitignore",
        ] {
            assert!(!x.excluded(p, false), "{p} should sync");
        }
        // A worktree's `.git` is a file, not a directory.
        assert!(x.excluded(".git", false));
    }

    #[test]
    fn a_links_own_patterns_add_to_and_override_the_defaults() {
        let x = Excludes::new(&["*.log".into(), "!build/".into(), "# note".into()]).unwrap();
        assert!(x.excluded("logs/run.log", false));
        assert!(!x.excluded("build/gen.rs", false));
        assert!(x.excluded("target/x", false));
    }

    #[test]
    fn no_pattern_brings_vcs_metadata_or_temp_files_back() {
        let x = Excludes::new(&["!.git".into(), "!.git/".into(), "!.fleet-sync-*".into()]).unwrap();
        assert!(x.excluded(".git", false));
        assert!(x.excluded(".git/config", false));
        assert!(x.excluded(".fleet-sync-ab12/f/x", false));
    }

    #[test]
    fn too_many_patterns_are_refused() {
        let many: Vec<String> = (0..=MAX_USER_EXCLUDES).map(|i| format!("x{i}")).collect();
        assert_eq!(Excludes::new(&many).err().unwrap().code, codes::E_INVALID);
    }
}
