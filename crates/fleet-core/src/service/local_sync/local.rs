//! The desktop's side of a link: walk the directory, hash files, and write or
//! delete one file under its guard. Blocking — the pass runs these on the
//! blocking pool.

use super::excludes::Excludes;
use super::MAX_FILE_BYTES;
use crate::ipc_error::{codes, IpcError};
use crate::store::FileStat;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};

/// Whether this machine syncs symlinks. Windows creates one only with a
/// privilege or developer mode, so there they stay left out, as before.
pub(super) const LINKS: bool = cfg!(unix);

/// A symlink travels as these bytes followed by its target: content no
/// text file starts with, so its hash never equals a file's, and the plan,
/// the BASE and the transfers treat it as one more version of the path. It
/// is never followed: the link itself is read, written and deleted.
pub(super) const LINK_PREFIX: &[u8] = b"\0fleet-symlink\0";

/// The bytes a link to `target` travels as.
pub(super) fn link_blob(target: &str) -> Vec<u8> {
    let mut v = LINK_PREFIX.to_vec();
    v.extend_from_slice(target.as_bytes());
    v
}

/// The target `bytes` say to link to, when they are a link's.
pub(super) fn link_target(bytes: &[u8]) -> Option<&str> {
    std::str::from_utf8(bytes.strip_prefix(LINK_PREFIX)?).ok()
}

/// A target the sync can carry both ways: UTF-8, no NUL, and not ending in
/// a newline (the host's `$(readlink)` would drop it).
pub(super) fn carriable_target(t: &str) -> bool {
    !t.is_empty() && !t.contains('\0') && !t.ends_with('\n')
}

/// What a walk found: regular files and (where [`LINKS`]) symlinks by
/// `/`-separated relative path, and the paths it left out on purpose (files
/// over the cap, anything else) — present, so never read as deleted.
#[derive(Debug, Default)]
pub(super) struct LocalScan {
    pub files: HashMap<String, FileStat>,
    pub blocked: HashSet<String>,
    /// Every symlink found: whatever lies below one is in another tree, so
    /// it is blocked.
    pub links: HashSet<String>,
    pub skipped: i64,
}

/// Whether `rel` is a plain relative path that stays below the root: no
/// empty, `.` or `..` component, no NUL, not absolute, no backslash or drive
/// colon (which Windows would read as a separator or a drive).
pub(crate) fn safe_rel(rel: &str) -> bool {
    !rel.is_empty()
        && !rel.starts_with('/')
        && !rel.contains(['\0', '\\', ':'])
        && rel
            .split('/')
            .all(|c| !c.is_empty() && c != "." && c != "..")
}

pub(super) fn abs(root: &Path, rel: &str) -> PathBuf {
    let mut p = root.to_path_buf();
    for c in rel.split('/') {
        p.push(c);
    }
    p
}

fn mtime_ns(m: &std::fs::Metadata) -> i64 {
    m.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos() as i64)
        .unwrap_or(0)
}

pub(super) fn stat_of(m: &std::fs::Metadata) -> FileStat {
    FileStat {
        size: m.len() as i64,
        mtime: mtime_ns(m),
    }
}

/// What sits at `rel`, looked at without following any link.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Probe {
    /// A regular file, reached through plain directories only.
    File(FileStat),
    /// A symlink (itself, not followed), likewise; only where [`LINKS`].
    Link(FileStat),
    /// Nothing: the path, or a directory above it, does not exist.
    Absent,
    /// Something that is not ours to read or replace: a link, a directory,
    /// a path below a link or a file, or one the OS would not tell us about
    /// (permission denied, an I/O error). Never read as deleted.
    Other,
}

/// Look at `rel` component by component: a directory above it that is a
/// link (into another tree the walk never entered) makes it `Other`, so a
/// write or delete cannot escape the root through it; only `NotFound`
/// means absent.
pub(super) fn probe(root: &Path, rel: &str) -> Probe {
    let mut p = root.to_path_buf();
    let parts: Vec<&str> = rel.split('/').collect();
    for (i, c) in parts.iter().enumerate() {
        p.push(c);
        let m = match std::fs::symlink_metadata(&p) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Probe::Absent,
            Err(_) => return Probe::Other,
        };
        let last = i + 1 == parts.len();
        if !last && !m.file_type().is_dir() {
            return Probe::Other;
        }
        if last {
            return if m.file_type().is_file() {
                Probe::File(stat_of(&m))
            } else if LINKS && m.file_type().is_symlink() {
                Probe::Link(stat_of(&m))
            } else {
                Probe::Other
            };
        }
    }
    Probe::Absent
}

/// The stat of the regular file or symlink at `rel`, or `None` when nothing
/// (or something else) is there.
pub(super) fn stat_file(root: &Path, rel: &str) -> Option<FileStat> {
    match probe(root, rel) {
        Probe::File(st) | Probe::Link(st) => Some(st),
        _ => None,
    }
}

/// The link at `rel` as the bytes it travels as, `None` when it is not a
/// link the sync can carry.
fn read_link_blob(root: &Path, rel: &str) -> Option<Vec<u8>> {
    let t = std::fs::read_link(abs(root, rel)).ok()?;
    let t = t.to_str()?;
    carriable_target(t).then(|| link_blob(t))
}

/// The bytes of the file or link at `rel`, provided it is still the kind
/// `probe` saw.
fn read_entry(root: &Path, rel: &str, link: bool) -> Option<Vec<u8>> {
    if link {
        read_link_blob(root, rel)
    } else {
        std::fs::read(abs(root, rel)).ok()
    }
}

/// Whether anything at all (a file, a link, a directory, or something the
/// OS would not show us) sits at `rel`.
pub(super) fn exists(root: &Path, rel: &str) -> bool {
    probe(root, rel) != Probe::Absent
}

/// Walk `root`, honouring the `.gitignore` files in it and `ex`. `also`
/// are paths to stat even when the walk passed them by (the BASE and open
/// conflicts: a file that became ignored keeps syncing rather than reading
/// as deleted).
pub(super) fn scan(
    root: &Path,
    ex: &Excludes,
    also: &HashSet<String>,
) -> Result<LocalScan, IpcError> {
    if !root.is_dir() {
        return Err(IpcError::new(
            codes::E_NOTFOUND,
            format!("the local folder {} is missing", root.display()),
        ));
    }
    let mut out = LocalScan::default();
    let root_owned = root.to_path_buf();
    // Prune excluded directories (`target/`, `node_modules/`) instead of
    // walking them and dropping their files one by one.
    let prune = (ex.clone(), root_owned.clone());
    let walker = ignore::WalkBuilder::new(root)
        .hidden(false)
        .ignore(false)
        .git_ignore(true)
        .git_global(false)
        .git_exclude(false)
        .require_git(false)
        .parents(false)
        .follow_links(false)
        .filter_entry(move |e| {
            let (ex, root) = &prune;
            let is_dir = e.file_type().is_some_and(|t| t.is_dir());
            match rel_of(root, e.path()) {
                Some(rel) => !ex.excluded(&rel, is_dir),
                None => true,
            }
        })
        .build();
    for entry in walker {
        let Ok(entry) = entry else { continue };
        let Some(rel) = rel_of(&root_owned, entry.path()) else {
            continue;
        };
        let Some(ft) = entry.file_type() else {
            continue;
        };
        if ft.is_dir() {
            continue;
        }
        if ex.excluded(&rel, false) {
            continue;
        }
        if ft.is_symlink() {
            out.links.insert(rel.clone());
            if LINKS && safe_rel(&rel) && read_link_blob(root, &rel).is_some() {
                if let Ok(m) = std::fs::symlink_metadata(entry.path()) {
                    out.files.insert(rel, stat_of(&m));
                    continue;
                }
            }
        }
        if ft.is_symlink() || !ft.is_file() || !safe_rel(&rel) {
            out.skipped += 1;
            out.blocked.insert(rel);
            continue;
        }
        let Ok(m) = entry.metadata() else { continue };
        if m.len() > MAX_FILE_BYTES {
            out.skipped += 1;
            out.blocked.insert(rel);
            continue;
        }
        out.files.insert(rel, stat_of(&m));
    }
    for rel in also {
        if out.files.contains_key(rel) || !safe_rel(rel) || ex.excluded(rel, false) {
            continue;
        }
        match probe(root, rel) {
            Probe::File(st) if st.size as u64 <= MAX_FILE_BYTES => {
                out.files.insert(rel.clone(), st);
            }
            Probe::Link(st) if read_link_blob(root, rel).is_some() => {
                out.links.insert(rel.clone());
                out.files.insert(rel.clone(), st);
            }
            Probe::Link(_) => {
                out.links.insert(rel.clone());
                out.blocked.insert(rel.clone());
            }
            // Too big, no longer a regular file (a link, a directory), or
            // unreadable: present either way, so never a deletion.
            Probe::File(_) | Probe::Other => {
                out.blocked.insert(rel.clone());
            }
            Probe::Absent => {}
        }
    }
    Ok(out)
}

/// `path` below `root` as a `/`-separated relative path.
fn rel_of(root: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(root).ok()?;
    let parts: Option<Vec<&str>> = rel.components().map(|c| c.as_os_str().to_str()).collect();
    let parts = parts?;
    if parts.is_empty() {
        return None;
    }
    Some(parts.join("/"))
}

pub(super) fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// The content hash of the file at `rel` and the stat it had while being
/// read; `None` when it is gone or changed under the read.
pub(super) fn hash_file(root: &Path, rel: &str) -> Option<(String, FileStat)> {
    let path = abs(root, rel);
    if let Probe::Link(before) = probe(root, rel) {
        let blob = read_link_blob(root, rel)?;
        let after = stat_file(root, rel)?;
        return (before == after).then(|| (sha256_hex(&blob), after));
    }
    let before = stat_file(root, rel)?;
    let mut f = std::fs::File::open(&path).ok()?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    let after = stat_file(root, rel)?;
    (before == after).then(|| (hex::encode(h.finalize()), after))
}

/// The file's bytes, provided it still has `expect`.
pub(super) fn read_guarded(root: &Path, rel: &str, expect: FileStat) -> Option<Vec<u8>> {
    let link = match probe(root, rel) {
        Probe::File(st) if st == expect => false,
        Probe::Link(st) if st == expect => true,
        _ => return None,
    };
    let bytes = read_entry(root, rel, link)?;
    (stat_file(root, rel)? == expect).then_some(bytes)
}

#[cfg(unix)]
pub(super) fn is_executable(root: &Path, rel: &str) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::symlink_metadata(abs(root, rel))
        .map(|m| m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
pub(super) fn is_executable(_root: &Path, _rel: &str) -> bool {
    false
}

/// How a guarded write or delete ended.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Guarded {
    /// Done; the file's stat now (`None` after a delete).
    Done(Option<FileStat>),
    /// The file was not what the plan saw: left alone for the next pass.
    Moved,
}

/// Write `bytes` at `rel` provided the file still has `expect` (`None` =
/// still absent): through a temp file beside it, renamed into place.
pub(super) fn write_guarded(
    root: &Path,
    rel: &str,
    bytes: &[u8],
    executable: bool,
    expect: Option<FileStat>,
) -> Result<Guarded, IpcError> {
    if !safe_rel(rel) {
        return Err(IpcError::new(
            codes::E_INVALID,
            format!("unsafe path {rel:?}"),
        ));
    }
    let target = abs(root, rel);
    let current = stat_file(root, rel);
    if current != expect || (expect.is_none() && exists(root, rel)) {
        return Ok(Guarded::Moved);
    }
    let dir = target.parent().unwrap_or(root);
    std::fs::create_dir_all(dir).map_err(|e| io(&target, e))?;
    let tmp = dir.join(format!(".fleet-sync-{}.tmp", uuid::Uuid::new_v4().simple()));
    match link_target(bytes) {
        // The rename below replaces the link itself, never what it points at.
        Some(t) => make_link(t, &tmp)?,
        None => {
            std::fs::write(&tmp, bytes).map_err(|e| io(&tmp, e))?;
            set_mode(&tmp, &target, executable);
        }
    }
    // Last look before the rename: an editor that saved meanwhile wins.
    if stat_file(root, rel) != expect {
        let _ = std::fs::remove_file(&tmp);
        return Ok(Guarded::Moved);
    }
    if let Err(e) = std::fs::rename(&tmp, &target) {
        let _ = std::fs::remove_file(&tmp);
        return Err(io(&target, e));
    }
    Ok(Guarded::Done(stat_file(root, rel)))
}

/// Delete the file at `rel` provided it still has `expect`, then any
/// directories above it that this left empty (never `root` itself).
pub(super) fn delete_guarded(
    root: &Path,
    rel: &str,
    expect: FileStat,
) -> Result<Guarded, IpcError> {
    if !safe_rel(rel) {
        return Err(IpcError::new(
            codes::E_INVALID,
            format!("unsafe path {rel:?}"),
        ));
    }
    if stat_file(root, rel) != Some(expect) {
        return Ok(Guarded::Moved);
    }
    let target = abs(root, rel);
    std::fs::remove_file(&target).map_err(|e| io(&target, e))?;
    let mut dir = target.parent();
    while let Some(d) = dir {
        if d == root || !d.starts_with(root) || std::fs::remove_dir(d).is_err() {
            break;
        }
        dir = d.parent();
    }
    Ok(Guarded::Done(None))
}

#[cfg(unix)]
fn make_link(target: &str, at: &Path) -> Result<(), IpcError> {
    if !carriable_target(target) {
        return Err(IpcError::new(
            codes::E_INVALID,
            format!("a link to {target:?} cannot be carried"),
        ));
    }
    std::os::unix::fs::symlink(target, at).map_err(|e| io(at, e))
}

#[cfg(not(unix))]
fn make_link(_target: &str, at: &Path) -> Result<(), IpcError> {
    Err(IpcError::new(
        codes::E_INVALID,
        format!("{}: symlinks are not synced on this OS", at.display()),
    ))
}

/// The temp file's mode before it replaces `target`: the target's own
/// (a `0600` file stays `0600`), else the default; the executable bits
/// then follow the other side.
#[cfg(unix)]
fn set_mode(tmp: &Path, target: &Path, executable: bool) {
    use std::os::unix::fs::PermissionsExt;
    let base = std::fs::symlink_metadata(target)
        .ok()
        .filter(|m| m.file_type().is_file())
        .or_else(|| std::fs::metadata(tmp).ok());
    if let Some(m) = base {
        let mode = m.permissions().mode() & 0o777;
        let mode = if executable {
            // Execute where read is granted, as git checks out a 0755.
            mode | ((mode & 0o444) >> 2)
        } else {
            mode & !0o111
        };
        let _ = std::fs::set_permissions(tmp, std::fs::Permissions::from_mode(mode));
    }
}

#[cfg(not(unix))]
fn set_mode(_tmp: &Path, _target: &Path, _executable: bool) {}

fn io(path: &Path, e: std::io::Error) -> IpcError {
    IpcError::new(codes::E_IO, format!("{}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_rel_refuses_anything_that_could_leave_the_root() {
        for ok in ["a", "a/b.txt", ".gitignore", "x/.y/z"] {
            assert!(safe_rel(ok), "{ok}");
        }
        for bad in [
            "",
            "/etc/passwd",
            "../x",
            "a/../../x",
            "a//b",
            "./a",
            "a\\b",
            "C:x",
            "a\0b",
        ] {
            assert!(!safe_rel(bad), "{bad:?}");
        }
    }

    #[test]
    fn the_walk_honours_gitignore_and_excludes_and_stats_base_paths_it_skipped() {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        std::fs::create_dir_all(r.join("src")).unwrap();
        std::fs::create_dir_all(r.join("target/debug")).unwrap();
        std::fs::write(r.join(".gitignore"), "*.tmp\n").unwrap();
        std::fs::write(r.join("src/a.rs"), "fn a() {}").unwrap();
        std::fs::write(r.join("src/x.tmp"), "scratch").unwrap();
        std::fs::write(r.join("forced.tmp"), "tracked though ignored").unwrap();
        std::fs::write(r.join("target/debug/app"), "bin").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("src/a.rs", r.join("link")).unwrap();
        let ex = Excludes::new(&[]).unwrap();
        let also: HashSet<String> = ["forced.tmp".to_string()].into();
        let s = scan(r, &ex, &also).unwrap();
        let mut got: Vec<&str> = s.files.keys().map(String::as_str).collect();
        got.sort();
        // The link is carried as a link.
        #[cfg(unix)]
        assert_eq!(got, vec![".gitignore", "forced.tmp", "link", "src/a.rs"]);
        #[cfg(not(unix))]
        assert_eq!(got, vec![".gitignore", "forced.tmp", "src/a.rs"]);
        assert_eq!(s.skipped, 0);
    }

    #[cfg(unix)]
    #[test]
    fn a_link_is_hashed_read_written_and_deleted_as_itself() {
        let d = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let r = d.path();
        std::fs::write(outside.path().join("f"), b"theirs").unwrap();
        let target = outside.path().join("f").to_string_lossy().into_owned();
        // Written as a link, pointing outside: never followed.
        assert!(matches!(
            write_guarded(r, "l", &link_blob(&target), false, None).unwrap(),
            Guarded::Done(Some(_))
        ));
        assert_eq!(
            std::fs::read_link(r.join("l")).unwrap().to_string_lossy(),
            target
        );
        let Probe::Link(st) = probe(r, "l") else {
            panic!("not a link")
        };
        let (sha, _) = hash_file(r, "l").unwrap();
        assert_eq!(sha, sha256_hex(&link_blob(&target)));
        assert_eq!(read_guarded(r, "l", st).unwrap(), link_blob(&target));
        // Replaced by a file: the link goes, its target is untouched.
        write_guarded(r, "l", b"mine", false, Some(st)).unwrap();
        assert_eq!(std::fs::read(r.join("l")).unwrap(), b"mine");
        assert_eq!(std::fs::read(outside.path().join("f")).unwrap(), b"theirs");
        // And a link again, then deleted: only the link.
        let st = stat_file(r, "l").unwrap();
        write_guarded(r, "l", &link_blob(&target), false, Some(st)).unwrap();
        let st = stat_file(r, "l").unwrap();
        assert_eq!(delete_guarded(r, "l", st).unwrap(), Guarded::Done(None));
        assert!(!exists(r, "l"));
        assert_eq!(std::fs::read(outside.path().join("f")).unwrap(), b"theirs");
    }

    #[test]
    fn a_missing_root_is_an_error_not_an_empty_scan() {
        let d = tempfile::tempdir().unwrap();
        let ex = Excludes::new(&[]).unwrap();
        let err = scan(&d.path().join("gone"), &ex, &HashSet::new()).unwrap_err();
        assert_eq!(err.code, codes::E_NOTFOUND);
    }

    #[test]
    fn writes_and_deletes_hold_to_their_guard() {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        // A new file lands, directories and all.
        let w = write_guarded(r, "a/b/c.txt", b"one", false, None).unwrap();
        let Guarded::Done(Some(st)) = w else {
            panic!("{w:?}")
        };
        assert_eq!(std::fs::read(r.join("a/b/c.txt")).unwrap(), b"one");
        // Writing "into absence" over a file that appeared is refused.
        assert_eq!(
            write_guarded(r, "a/b/c.txt", b"two", false, None).unwrap(),
            Guarded::Moved
        );
        // A stale expectation is refused; the right one goes through.
        let stale = FileStat {
            size: 99,
            mtime: st.mtime,
        };
        assert_eq!(
            write_guarded(r, "a/b/c.txt", b"two", false, Some(stale)).unwrap(),
            Guarded::Moved
        );
        // A different size, so the stat moves even where two writes land in
        // the same clock tick (Windows' file times often do).
        let Guarded::Done(Some(st2)) =
            write_guarded(r, "a/b/c.txt", b"three", true, Some(st)).unwrap()
        else {
            panic!()
        };
        assert_eq!(std::fs::read(r.join("a/b/c.txt")).unwrap(), b"three");
        #[cfg(unix)]
        assert!(is_executable(r, "a/b/c.txt"));
        assert_eq!(delete_guarded(r, "a/b/c.txt", st).unwrap(), Guarded::Moved);
        assert_eq!(
            delete_guarded(r, "a/b/c.txt", st2).unwrap(),
            Guarded::Done(None)
        );
        assert!(!r.join("a").exists(), "emptied directories are removed");
        assert!(r.exists());
        // No temp file is left behind.
        assert_eq!(std::fs::read_dir(r).unwrap().count(), 0);
    }

    #[cfg(unix)]
    #[test]
    fn a_replaced_file_keeps_its_mode_and_takes_the_executable_bit() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        let p = r.join("secret.env");
        std::fs::write(&p, b"a=1").unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600)).unwrap();
        let st = stat_file(r, "secret.env").unwrap();
        let Guarded::Done(Some(st)) =
            write_guarded(r, "secret.env", b"a=22", false, Some(st)).unwrap()
        else {
            panic!()
        };
        assert_eq!(
            std::fs::metadata(&p).unwrap().permissions().mode() & 0o777,
            0o600
        );
        write_guarded(r, "secret.env", b"a=333", true, Some(st)).unwrap();
        assert_eq!(
            std::fs::metadata(&p).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_path_below_a_link_is_neither_absent_nor_writable() {
        let d = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("f"), b"theirs").unwrap();
        std::os::unix::fs::symlink(outside.path(), d.path().join("l")).unwrap();
        assert_eq!(probe(d.path(), "l/f"), Probe::Other);
        assert_eq!(probe(d.path(), "l/new"), Probe::Other);
        assert_eq!(probe(d.path(), "nothing/here"), Probe::Absent);
        assert_eq!(
            write_guarded(d.path(), "l/new", b"x", false, None).unwrap(),
            Guarded::Moved
        );
        assert!(!outside.path().join("new").exists());
        let also: HashSet<String> = ["l/f".to_string()].into();
        let s = scan(d.path(), &Excludes::new(&[]).unwrap(), &also).unwrap();
        assert!(s.blocked.contains("l/f"), "{s:?}");
        assert!(!s.files.contains_key("l/f"));
    }

    #[test]
    fn hashing_reports_the_stat_it_read_under() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("f"), b"abc").unwrap();
        let (sha, st) = hash_file(d.path(), "f").unwrap();
        assert_eq!(sha, sha256_hex(b"abc"));
        assert_eq!(st.size, 3);
        assert!(hash_file(d.path(), "missing").is_none());
    }
}
