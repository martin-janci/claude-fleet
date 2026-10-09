//! End-to-end passes: the real scripts run under `bash` against a real git
//! worktree in a temp directory (the "host"), and a second temp directory is
//! the local folder.

use super::*;
use crate::ssh::HostToolchain;
use std::process::Output;
use std::process::Stdio;

/// Runs every "remote" command on this machine through `bash -c`, the way
/// sshd hands it to the login shell. `remote_home` is a temp directory, so
/// the worktree the session's project resolves to lives inside it.
struct Bash {
    home: PathBuf,
}

#[async_trait::async_trait]
impl SshExec for Bash {
    /// `run_shell` (the paired desktop's pane lookup) lands here.
    async fn run(&self, _: &str, args: &[&str], _: Duration) -> Result<Output, IpcError> {
        Ok(crate::proc::command("bash")
            .arg("-c")
            .arg(args.join(" "))
            .env("HOME", &self.home)
            .env("TMUX_TMPDIR", self.home.join("tmux"))
            .env_remove("TMUX")
            .output()
            .await
            .unwrap())
    }
    async fn run_bounded(
        &self,
        _: &str,
        _: &[&str],
        _: Duration,
        _: Duration,
    ) -> Result<Output, IpcError> {
        unreachable!("local sync pipes stdin")
    }
    async fn run_cancellable(
        &self,
        _: &str,
        _: &[&str],
        _: Duration,
        _: CancellationToken,
    ) -> Result<Output, IpcError> {
        unreachable!("local sync pipes stdin")
    }
    async fn run_bounded_cancellable(
        &self,
        _: &str,
        _: &[&str],
        _: Duration,
        _: Duration,
        _: CancellationToken,
    ) -> Result<Output, IpcError> {
        unreachable!("local sync pipes stdin")
    }
    async fn upload_file(&self, _: &str, _: &Path, _: &str, _: Duration) -> Result<(), IpcError> {
        unreachable!("local sync uploads through a tar on stdin")
    }
    async fn remote_home(&self, _: &str) -> Result<String, IpcError> {
        Ok(self.home.to_string_lossy().into_owned())
    }
    async fn toolchain(&self, _: &str) -> Option<HostToolchain> {
        None
    }
    async fn run_with_stdin(
        &self,
        _host: &str,
        args: &[&str],
        stdin: Vec<u8>,
        _connect: Duration,
        _wall: Duration,
        max_output: usize,
    ) -> Result<Output, IpcError> {
        use tokio::io::AsyncWriteExt;
        let mut child = crate::proc::command("bash")
            .arg("-c")
            .arg(args.join(" "))
            .env("HOME", &self.home)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut pipe = child.stdin.take().unwrap();
        let writer = tokio::spawn(async move {
            let _ = pipe.write_all(&stdin).await;
            let _ = pipe.shutdown().await;
        });
        let mut out = child.wait_with_output().await.unwrap();
        writer.await.unwrap();
        out.stdout.truncate(max_output);
        Ok(out)
    }
}

struct Fixture {
    _home: tempfile::TempDir,
    _local_parent: tempfile::TempDir,
    remote: PathBuf,
    local: PathBuf,
    engine: Arc<LocalSync>,
    session_id: i64,
}

fn git(dir: &Path, args: &[&str]) {
    let ok = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap()
        .success();
    assert!(ok, "git {args:?}");
}

fn write(root: &Path, rel: &str, body: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

fn read(root: &Path, rel: &str) -> Option<String> {
    std::fs::read_to_string(root.join(rel)).ok()
}

/// A host worktree with tracked, untracked, ignored and build-output files,
/// a session on it, and an empty local folder (not yet created).
fn fixture() -> Fixture {
    let home = tempfile::tempdir().unwrap();
    let remote = home.path().join("projects/github.com/acme/app");
    std::fs::create_dir_all(&remote).unwrap();
    git(&remote, &["init", "-q"]);
    write(&remote, ".gitignore", "*.log\n");
    write(&remote, "src/main.rs", "fn main() {}\n");
    write(&remote, "README.md", "hello\n");
    git(&remote, &["add", "."]);
    git(&remote, &["commit", "-qm", "init"]);
    write(&remote, "notes.txt", "untracked but not ignored\n");
    write(&remote, "debug.log", "ignored\n");
    write(&remote, "target/debug/app", "binary\n");
    let local_parent = tempfile::tempdir().unwrap();
    let local = local_parent.path().join("app");
    let store = Store::open_in_memory().unwrap();
    store.upsert_host("devbox").unwrap();
    let pid = store
        .upsert_project("acme", "app", "/nowhere/acme/app")
        .unwrap();
    let session_id = store
        .upsert_session("dev-app", "devbox", Some(pid), None, 1, 1, "running", None)
        .unwrap();
    let engine = LocalSync::new(
        Arc::new(Mutex::new(store)),
        Arc::new(Bash {
            home: home.path().to_path_buf(),
        }),
    );
    Fixture {
        _home: home,
        _local_parent: local_parent,
        remote,
        local,
        engine,
        session_id,
    }
}

impl Fixture {
    async fn enable(&self) -> LocalWorkspaceRow {
        let row = enable(
            &self.engine,
            EnableLocalWorkspaceArgs {
                session_id: self.session_id,
                local_path: self.local.to_string_lossy().into_owned(),
                excludes: vec![],
            },
        )
        .await
        .unwrap();
        // `enable` kicks a background pass; wait it out with one of our own.
        self.engine.sync_now(row.id).await.unwrap()
    }

    async fn pass(&self, id: i64) -> LocalWorkspaceRow {
        self.engine.sync_now(id).await.unwrap()
    }
}

#[tokio::test]
async fn the_first_pass_downloads_what_git_would_track_and_nothing_else() {
    let f = fixture();
    let row = f.enable().await;
    assert_eq!(row.state, "synced", "{row:?}");
    assert_eq!(row.worktree_key, "main");
    assert_eq!(row.remote_path, f.remote.to_string_lossy());
    assert_eq!(
        read(&f.local, "src/main.rs").as_deref(),
        Some("fn main() {}\n")
    );
    assert_eq!(
        read(&f.local, "notes.txt").as_deref(),
        Some("untracked but not ignored\n")
    );
    assert_eq!(read(&f.local, ".gitignore").as_deref(), Some("*.log\n"));
    assert!(read(&f.local, "debug.log").is_none(), "git-ignored");
    assert!(!f.local.join("target").exists(), "build output");
    assert!(
        !f.local.join(".git").exists(),
        "the local folder is not a checkout"
    );
    // A second pass over an unchanged pair carries nothing and stays synced.
    let again = f.pass(row.id).await;
    assert_eq!(again.state, "synced");
}

/// Redesign 8.1: Pause all stops the tick before it starts a pass, so an
/// edit on the host stays there until the pause lifts.
#[tokio::test]
async fn pause_all_stops_the_tick() {
    let f = fixture();
    f.enable().await;
    write(&f.remote, "README.md", "edited on the host\n");
    crate::service::settings::set(
        &f.engine.store.lock().unwrap(),
        crate::service::settings::AUTOMATION_PAUSED,
        "true",
    )
    .unwrap();
    assert!(!f.engine.tick(), "no pass while paused");
    assert_eq!(read(&f.local, "README.md").as_deref(), Some("hello\n"));
    crate::service::settings::set(
        &f.engine.store.lock().unwrap(),
        crate::service::settings::AUTOMATION_PAUSED,
        "false",
    )
    .unwrap();
    assert!(f.engine.tick(), "the tick runs again once the pause lifts");
}

#[tokio::test]
async fn edits_deletions_and_new_files_travel_both_ways() {
    let f = fixture();
    let row = f.enable().await;
    // Local → remote.
    write(
        &f.local,
        "src/main.rs",
        "fn main() { println!(\"local\"); }\n",
    );
    write(&f.local, "src/new.rs", "// new here\n");
    std::fs::remove_file(f.local.join("README.md")).unwrap();
    let r = f.pass(row.id).await;
    assert_eq!(r.state, "synced", "{r:?}");
    assert_eq!(
        read(&f.remote, "src/main.rs").as_deref(),
        Some("fn main() { println!(\"local\"); }\n")
    );
    assert_eq!(
        read(&f.remote, "src/new.rs").as_deref(),
        Some("// new here\n")
    );
    assert!(read(&f.remote, "README.md").is_none());
    // Nothing was committed: sync is not git.
    let status = std::process::Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(&f.remote)
        .output()
        .unwrap();
    let status = String::from_utf8_lossy(&status.stdout);
    assert!(status.contains(" M src/main.rs"), "{status}");
    assert!(status.contains(" D README.md"), "{status}");

    // Remote → local.
    write(&f.remote, "notes.txt", "the agent wrote this\n");
    write(&f.remote, "src/lib/deep.rs", "// agent\n");
    std::fs::remove_file(f.remote.join("src/new.rs")).unwrap();
    let r = f.pass(row.id).await;
    assert_eq!(r.state, "synced", "{r:?}");
    assert_eq!(
        read(&f.local, "notes.txt").as_deref(),
        Some("the agent wrote this\n")
    );
    assert_eq!(
        read(&f.local, "src/lib/deep.rs").as_deref(),
        Some("// agent\n")
    );
    assert!(read(&f.local, "src/new.rs").is_none());
}

#[tokio::test]
async fn both_sides_changing_a_file_is_a_conflict_that_overwrites_nothing() {
    let f = fixture();
    let row = f.enable().await;
    write(&f.local, "src/main.rs", "local version\n");
    write(&f.remote, "src/main.rs", "agent version\n");
    // The same edit on both sides is not a conflict.
    write(&f.local, "README.md", "same\n");
    write(&f.remote, "README.md", "same\n");
    let r = f.pass(row.id).await;
    assert_eq!(r.state, "conflict", "{r:?}");
    assert_eq!(r.conflicts.len(), 1);
    assert_eq!(r.conflicts[0].path, "src/main.rs");
    assert_eq!(r.conflicts[0].kind, "both_modified");
    assert_eq!(
        read(&f.local, "src/main.rs").as_deref(),
        Some("local version\n")
    );
    assert_eq!(
        read(&f.remote, "src/main.rs").as_deref(),
        Some("agent version\n")
    );
    // It stays put pass after pass…
    let r = f.pass(row.id).await;
    assert_eq!(r.state, "conflict");
    assert_eq!(
        read(&f.remote, "src/main.rs").as_deref(),
        Some("agent version\n")
    );
    // …while everything else keeps syncing.
    write(&f.remote, "notes.txt", "still flowing\n");
    f.pass(row.id).await;
    assert_eq!(
        read(&f.local, "notes.txt").as_deref(),
        Some("still flowing\n")
    );

    // Keep local.
    let r = resolve_conflict(
        &f.engine,
        ResolveLocalConflictArgs {
            id: row.id,
            path: "src/main.rs".into(),
            keep: "local".into(),
        },
    )
    .await
    .unwrap();
    assert_eq!(r.state, "synced", "{r:?}");
    assert!(r.conflicts.is_empty());
    assert_eq!(
        read(&f.remote, "src/main.rs").as_deref(),
        Some("local version\n")
    );
}

#[tokio::test]
async fn keep_remote_and_resolving_by_hand_both_clear_a_conflict() {
    let f = fixture();
    let row = f.enable().await;
    write(&f.local, "src/main.rs", "mine\n");
    write(&f.remote, "src/main.rs", "theirs\n");
    std::fs::remove_file(f.local.join("README.md")).unwrap();
    write(&f.remote, "README.md", "edited remotely\n");
    let r = f.pass(row.id).await;
    let mut kinds: Vec<(&str, &str)> = r
        .conflicts
        .iter()
        .map(|c| (c.path.as_str(), c.kind.as_str()))
        .collect();
    kinds.sort();
    assert_eq!(
        kinds,
        vec![
            ("README.md", "local_deleted"),
            ("src/main.rs", "both_modified")
        ]
    );
    let r = resolve_conflict(
        &f.engine,
        ResolveLocalConflictArgs {
            id: row.id,
            path: "README.md".into(),
            keep: "remote".into(),
        },
    )
    .await
    .unwrap();
    assert_eq!(
        read(&f.local, "README.md").as_deref(),
        Some("edited remotely\n")
    );
    assert_eq!(r.conflicts.len(), 1);
    // By hand: make both sides the same.
    write(&f.local, "src/main.rs", "theirs\n");
    let r = f.pass(row.id).await;
    assert_eq!(r.state, "synced", "{r:?}");
    assert!(r.conflicts.is_empty());
}

#[tokio::test]
async fn enabling_over_a_folder_with_files_adopts_copies_and_never_overwrites() {
    let f = fixture();
    write(&f.local, "README.md", "hello\n"); // identical: adopted
    write(&f.local, "src/main.rs", "a different main\n"); // differs: conflict
    write(&f.local, "local-only.txt", "mine\n"); // one-sided: copied up
    let row = f.enable().await;
    assert_eq!(row.state, "conflict", "{row:?}");
    assert_eq!(row.conflicts.len(), 1);
    assert_eq!(row.conflicts[0].kind, "both_added");
    assert_eq!(
        read(&f.local, "src/main.rs").as_deref(),
        Some("a different main\n")
    );
    assert_eq!(
        read(&f.remote, "src/main.rs").as_deref(),
        Some("fn main() {}\n")
    );
    assert_eq!(read(&f.remote, "local-only.txt").as_deref(), Some("mine\n"));
    assert_eq!(
        read(&f.local, "notes.txt").as_deref(),
        Some("untracked but not ignored\n")
    );
}

#[tokio::test]
async fn a_wiped_folder_pauses_the_link_instead_of_deleting_the_worktree() {
    let f = fixture();
    for i in 0..30 {
        write(&f.remote, &format!("gen/f{i}.txt"), "x\n");
    }
    let row = f.enable().await;
    assert_eq!(row.state, "synced");
    std::fs::remove_dir_all(f.local.join("gen")).unwrap();
    let r = f.pass(row.id).await;
    assert!(r.paused, "{r:?}");
    assert_eq!(r.state, "error");
    assert!(
        r.last_error.as_deref().unwrap().contains("disappeared"),
        "{r:?}"
    );
    assert!(
        f.remote.join("gen/f0.txt").exists(),
        "nothing deleted remotely"
    );
    // A missing local folder is an error too, never "everything deleted".
    let r = resume(&f.engine, row.id).unwrap();
    assert!(!r.paused);
    std::fs::remove_dir_all(&f.local).unwrap();
    let r = f.pass(row.id).await;
    assert_eq!(r.state, "error", "{r:?}");
    assert!(f.remote.join("README.md").exists());
}

#[tokio::test]
async fn pause_excludes_and_disconnect() {
    let f = fixture();
    let row = f.enable().await;
    pause(&f.engine, row.id).unwrap();
    write(&f.remote, "notes.txt", "while paused\n");
    let r = f.pass(row.id).await;
    assert_eq!(r.state, "paused");
    assert_eq!(
        read(&f.local, "notes.txt").as_deref(),
        Some("untracked but not ignored\n")
    );
    resume(&f.engine, row.id).unwrap();
    let r = f.pass(row.id).await;
    assert_eq!(r.state, "synced");
    assert_eq!(
        read(&f.local, "notes.txt").as_deref(),
        Some("while paused\n")
    );

    // A link's own exclude keeps a file out from then on.
    set_excludes(
        &f.engine,
        SetLocalWorkspaceExcludesArgs {
            id: row.id,
            excludes: vec!["*.csv".into()],
        },
    )
    .unwrap();
    write(&f.local, "data.csv", "a,b\n");
    f.pass(row.id).await;
    assert!(read(&f.remote, "data.csv").is_none());
    assert!(set_excludes(
        &f.engine,
        SetLocalWorkspaceExcludesArgs {
            id: row.id,
            excludes: (0..=excludes::MAX_USER_EXCLUDES)
                .map(|i| format!("x{i}"))
                .collect(),
        },
    )
    .is_err());

    disconnect(&f.engine, row.id).await.unwrap();
    assert!(list(&f.engine.store).unwrap().is_empty());
    // Both sides keep their files.
    assert!(f.local.join("src/main.rs").exists());
    assert!(f.remote.join("src/main.rs").exists());
}

#[tokio::test]
async fn enabling_refuses_a_second_link_and_a_folder_that_is_a_file() {
    let f = fixture();
    let row = f.enable().await;
    let again = enable(
        &f.engine,
        EnableLocalWorkspaceArgs {
            session_id: f.session_id,
            local_path: f
                ._local_parent
                .path()
                .join("other")
                .to_string_lossy()
                .into_owned(),
            excludes: vec![],
        },
    )
    .await;
    assert_eq!(again.unwrap_err().code, codes::E_CONFLICT);
    let file = f._local_parent.path().join("file");
    std::fs::write(&file, "x").unwrap();
    disconnect(&f.engine, row.id).await.unwrap();
    let bad = enable(
        &f.engine,
        EnableLocalWorkspaceArgs {
            session_id: f.session_id,
            local_path: file.to_string_lossy().into_owned(),
            excludes: vec![],
        },
    )
    .await;
    assert_eq!(bad.unwrap_err().code, codes::E_INVALID);
    let relative = enable(
        &f.engine,
        EnableLocalWorkspaceArgs {
            session_id: f.session_id,
            local_path: "relative/dir".into(),
            excludes: vec![],
        },
    )
    .await;
    assert_eq!(relative.unwrap_err().code, codes::E_INVALID);
}

#[tokio::test]
async fn the_upload_guard_refuses_a_remote_file_that_moved() {
    let f = fixture();
    let bash = Bash {
        home: f._home.path().to_path_buf(),
    };
    let root = f.remote.to_string_lossy().into_owned();
    let ops = vec![
        PushOp::Write {
            path: "README.md".into(),
            bytes: b"over\n".to_vec(),
            executable: false,
            mtime_secs: 1,
            expect: Some(local::sha256_hex(b"not what is there\n")),
        },
        PushOp::Write {
            path: "fresh/x.sh".into(),
            bytes: b"#!/bin/sh\n".to_vec(),
            executable: true,
            mtime_secs: 1,
            expect: None,
        },
        PushOp::Delete {
            path: "notes.txt".into(),
            expect: local::sha256_hex(b"untracked but not ignored\n"),
        },
    ];
    let r = remote::push(&bash, "devbox", &root, &ops).await.unwrap();
    assert_eq!(r["README.md"], PushResult::Moved);
    assert!(matches!(r["fresh/x.sh"], PushResult::Done(Some(_))));
    assert_eq!(r["notes.txt"], PushResult::Done(None));
    assert_eq!(read(&f.remote, "README.md").as_deref(), Some("hello\n"));
    assert!(read(&f.remote, "notes.txt").is_none());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(f.remote.join("fresh/x.sh"))
            .unwrap()
            .permissions()
            .mode();
        assert!(mode & 0o111 != 0);
    }
    // No temp directory is left in the worktree.
    let leftovers: Vec<_> = std::fs::read_dir(&f.remote)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().starts_with(".fleet-sync"))
        .collect();
    assert!(leftovers.is_empty());
}

#[tokio::test]
async fn an_emptied_folder_pauses_even_when_the_repo_is_small() {
    let f = fixture();
    let row = f.enable().await;
    assert_eq!(row.state, "synced");
    // An unmounted volume: the mount point is still there, and empty.
    for e in std::fs::read_dir(&f.local).unwrap() {
        let p = e.unwrap().path();
        if p.is_dir() {
            std::fs::remove_dir_all(p).unwrap();
        } else {
            std::fs::remove_file(p).unwrap();
        }
    }
    let r = f.pass(row.id).await;
    assert!(r.paused, "{r:?}");
    assert!(f.remote.join("README.md").exists());
    assert!(f.remote.join("src/main.rs").exists());
}

#[tokio::test]
async fn a_symlinked_directory_is_never_written_or_deleted_through() {
    let f = fixture();
    let row = f.enable().await;
    // Locally, `docs` points at a folder outside the link.
    let outside_local = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(outside_local.path(), f.local.join("docs")).unwrap();
    write(&f.remote, "docs/a.md", "from the host\n");
    // On the host, `vendor` points outside the worktree.
    let outside_remote = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(outside_remote.path(), f.remote.join("vendor")).unwrap();
    std::fs::create_dir_all(f.local.join("vendor")).unwrap();
    write(&f.local, "vendor/x.txt", "from the desktop\n");
    f.pass(row.id).await;
    f.pass(row.id).await;
    assert!(
        !outside_local.path().join("a.md").exists(),
        "pulled through a local link"
    );
    assert!(
        !outside_remote.path().join("x.txt").exists(),
        "pushed through a host link"
    );
}

fn link_of(root: &Path, rel: &str) -> Option<String> {
    let p = root.join(rel);
    std::fs::symlink_metadata(&p)
        .ok()
        .filter(|m| m.file_type().is_symlink())?;
    Some(std::fs::read_link(p).ok()?.to_string_lossy().into_owned())
}

#[tokio::test]
async fn symlinks_sync_both_ways_as_links() {
    let f = fixture();
    // On the host before the first pass: a link git tracks.
    std::os::unix::fs::symlink("src/main.rs", f.remote.join("main-link")).unwrap();
    git(&f.remote, &["add", "main-link"]);
    let row = f.enable().await;
    assert_eq!(row.state, "synced", "{row:?}");
    assert_eq!(
        link_of(&f.local, "main-link").as_deref(),
        Some("src/main.rs")
    );

    // A link made on the desktop, pointing outside: carried, not followed.
    std::os::unix::fs::symlink("../../elsewhere", f.local.join("src/out")).unwrap();
    let r = f.pass(row.id).await;
    assert_eq!(r.state, "synced", "{r:?}");
    assert_eq!(
        link_of(&f.remote, "src/out").as_deref(),
        Some("../../elsewhere")
    );

    // Retargeted on the host: the desktop's link follows suit.
    std::fs::remove_file(f.remote.join("main-link")).unwrap();
    std::os::unix::fs::symlink("README.md", f.remote.join("main-link")).unwrap();
    let r = f.pass(row.id).await;
    assert_eq!(r.state, "synced", "{r:?}");
    assert_eq!(link_of(&f.local, "main-link").as_deref(), Some("README.md"));
    assert_eq!(
        read(&f.local, "src/main.rs").as_deref(),
        Some("fn main() {}\n"),
        "the file the link used to name is untouched"
    );

    // Replaced by a file on the desktop: the host's link becomes that file.
    std::fs::remove_file(f.local.join("main-link")).unwrap();
    write(&f.local, "main-link", "now a file\n");
    f.pass(row.id).await;
    assert_eq!(link_of(&f.remote, "main-link"), None);
    assert_eq!(
        read(&f.remote, "main-link").as_deref(),
        Some("now a file\n")
    );
    assert_eq!(read(&f.remote, "README.md").as_deref(), Some("hello\n"));

    // Deleted on the host: only the link goes.
    std::fs::remove_file(f.remote.join("src/out")).unwrap();
    let r = f.pass(row.id).await;
    assert_eq!(r.state, "synced", "{r:?}");
    assert!(!local::exists(&f.local, "src/out"));
}

#[tokio::test]
async fn a_host_link_to_a_directory_is_never_written_through_on_push() {
    let f = fixture();
    let row = f.enable().await;
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("keep.txt"), "theirs\n").unwrap();
    // The desktop makes `shared` a link; the host has a link of the same
    // name to a directory elsewhere, made meanwhile.
    std::os::unix::fs::symlink("src", f.local.join("shared")).unwrap();
    std::os::unix::fs::symlink(outside.path(), f.remote.join("shared")).unwrap();
    let r = f.pass(row.id).await;
    // Both added differently: a conflict, and nothing moved into `outside`.
    assert_eq!(r.state, "conflict", "{r:?}");
    let names: Vec<_> = std::fs::read_dir(outside.path())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(names, vec![std::ffi::OsString::from("keep.txt")]);
    // Keep local replaces the host's link itself, not what it points at.
    resolve_conflict(
        &f.engine,
        ResolveLocalConflictArgs {
            id: row.id,
            path: "shared".into(),
            keep: "local".into(),
        },
    )
    .await
    .unwrap();
    f.pass(row.id).await;
    assert_eq!(link_of(&f.remote, "shared").as_deref(), Some("src"));
    let names: Vec<_> = std::fs::read_dir(outside.path())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(names, vec![std::ffi::OsString::from("keep.txt")]);
}

#[tokio::test]
async fn a_pushed_file_lands_with_its_arrival_time() {
    let f = fixture();
    let row = f.enable().await;
    write(&f.local, "src/main.rs", "fn main() { edited(); }\n");
    let hour_ago = std::time::SystemTime::now() - Duration::from_secs(3600);
    std::fs::File::options()
        .write(true)
        .open(f.local.join("src/main.rs"))
        .unwrap()
        .set_modified(hour_ago)
        .unwrap();
    let r = f.pass(row.id).await;
    assert_eq!(r.state, "synced", "{r:?}");
    let landed = std::fs::metadata(f.remote.join("src/main.rs"))
        .unwrap()
        .modified()
        .unwrap();
    assert!(
        landed > hour_ago + Duration::from_secs(1800),
        "a build on the host would think its outputs are newer"
    );
}

#[tokio::test]
async fn files_deleted_on_both_sides_are_agreement_not_a_wipe() {
    let f = fixture();
    let row = f.enable().await;
    for root in [&f.local, &f.remote] {
        for rel in ["README.md", "notes.txt", "src/main.rs", ".gitignore"] {
            let _ = std::fs::remove_file(root.join(rel));
        }
    }
    let r = f.pass(row.id).await;
    assert!(!r.paused, "{r:?}");
    assert_eq!(r.state, "synced", "{r:?}");
}

#[test]
fn only_a_link_blocks_what_lies_below_it() {
    let blocked: HashSet<String> = ["config".to_string(), "docs".to_string()].into();
    let links: HashSet<String> = ["docs".to_string()].into();
    assert!(is_blocked(&blocked, &links, "config"));
    assert!(!is_blocked(&blocked, &links, "config/app.toml"));
    assert!(is_blocked(&blocked, &links, "docs/a/b.md"));
    assert!(!is_blocked(&blocked, &links, "docsx/a.md"));
}

// ---------------------------------------------------------------------------
// Phases 2 and 3: activity log, git on the worktree, conflicts, prompts.
// ---------------------------------------------------------------------------

use handoff::{
    AskAiArgs, CommitLocalWorkspaceArgs, DismissLocalActivityArgs, LocalWorkspacePathArgs,
    LocalWorkspacePathsArgs, SetLocalDriverArgs,
};

fn activity(f: &Fixture, id: i64) -> Vec<(String, String, String)> {
    let mut v: Vec<_> = lock(&f.engine.store)
        .unwrap()
        .local_workspace_activity(id)
        .unwrap()
        .into_iter()
        .map(|a| (a.path, a.origin, a.change))
        .collect();
    v.sort();
    v
}

fn t(p: &str, o: &str, c: &str) -> (String, String, String) {
    (p.into(), o.into(), c.into())
}

#[tokio::test]
async fn the_activity_log_says_which_side_changed_what_but_not_the_first_copy() {
    let f = fixture();
    let row = f.enable().await;
    assert!(
        activity(&f, row.id).is_empty(),
        "the initial copy is nobody's change"
    );
    write(&f.local, "src/main.rs", "fn main() { /* mine */ }\n");
    write(&f.local, "src/new.rs", "// new\n");
    std::fs::remove_file(f.local.join("README.md")).unwrap();
    write(&f.remote, "notes.txt", "the agent wrote this\n");
    let r = f.pass(row.id).await;
    assert_eq!((r.local_activity, r.remote_activity), (3, 1), "{r:?}");
    assert_eq!(
        activity(&f, row.id),
        vec![
            t("README.md", "local", "deleted"),
            t("notes.txt", "remote", "modified"),
            t("src/main.rs", "local", "modified"),
            t("src/new.rs", "local", "added"),
        ]
    );
    // The latest carry wins: the agent then edits a file the developer did.
    write(&f.remote, "src/new.rs", "// agent took it over\n");
    f.pass(row.id).await;
    assert!(activity(&f, row.id).contains(&t("src/new.rs", "remote", "modified")));
    // Dismissing one side leaves the other.
    let r = handoff::dismiss(
        &f.engine,
        DismissLocalActivityArgs {
            id: row.id,
            origin: Some("remote".into()),
        },
    )
    .unwrap();
    assert_eq!((r.local_activity, r.remote_activity), (2, 0));
}

#[tokio::test]
async fn changes_and_diff_read_the_worktree_with_each_sides_origin() {
    let f = fixture();
    let row = f.enable().await;
    write(&f.local, "src/main.rs", "fn main() { /* mine */ }\n");
    write(&f.local, "brand/new.txt", "fresh\n");
    // Not yet carried: `changes` runs a pass first.
    let c = handoff::changes(&f.engine, row.id).await.unwrap();
    let mut got: Vec<(String, String, Option<String>)> = c
        .files
        .iter()
        .map(|x| (x.file.path.clone(), x.file.status.clone(), x.origin.clone()))
        .collect();
    got.sort();
    assert_eq!(
        got,
        vec![
            (
                "brand/new.txt".into(),
                "untracked".into(),
                Some("local".into())
            ),
            // Untracked before the link: git knows it, the log does not.
            ("notes.txt".into(), "untracked".into(), None),
            (
                "src/main.rs".into(),
                "modified".into(),
                Some("local".into())
            ),
        ]
    );
    assert!(!c.branch.is_empty());
    let d = handoff::diff(
        &f.engine,
        LocalWorkspacePathArgs {
            id: row.id,
            path: "src/main.rs".into(),
        },
    )
    .await
    .unwrap();
    assert!(d.diff.contains("-fn main() {}"), "{}", d.diff);
    assert!(d.diff.contains("+fn main() { /* mine */ }"), "{}", d.diff);
    let d = handoff::diff(
        &f.engine,
        LocalWorkspacePathArgs {
            id: row.id,
            path: "brand/new.txt".into(),
        },
    )
    .await
    .unwrap();
    assert!(
        d.diff.contains("+fresh"),
        "an untracked file is all added: {}",
        d.diff
    );
    let bad = handoff::diff(
        &f.engine,
        LocalWorkspacePathArgs {
            id: row.id,
            path: "../etc/passwd".into(),
        },
    )
    .await
    .unwrap_err();
    assert_eq!(bad.code, codes::E_INVALID);
}

#[tokio::test]
async fn commit_takes_exactly_the_chosen_files_and_discard_comes_back_to_the_folder() {
    let f = fixture();
    let row = f.enable().await;
    write(&f.local, "src/main.rs", "fn main() { /* mine */ }\n");
    write(&f.local, "keep.txt", "commit me\n");
    write(&f.local, "README.md", "not this one\n");
    let c = handoff::commit(
        &f.engine,
        CommitLocalWorkspaceArgs {
            id: row.id,
            message: "Local edits".into(),
            paths: vec!["src/main.rs".into(), "keep.txt".into()],
        },
    )
    .await
    .unwrap();
    assert_eq!(c.commit.len(), 40, "{c:?}");
    let out = std::process::Command::new("git")
        .args(["show", "--stat", "--format=%s", "HEAD"])
        .current_dir(&f.remote)
        .output()
        .unwrap();
    let show = String::from_utf8_lossy(&out.stdout);
    assert!(show.starts_with("Local edits"), "{show}");
    assert!(
        show.contains("src/main.rs") && show.contains("keep.txt"),
        "{show}"
    );
    assert!(!show.contains("README.md"), "{show}");
    // What was committed left the log; the rest stays.
    assert_eq!(
        activity(&f, row.id),
        vec![t("README.md", "local", "modified")]
    );

    // Discard: back to HEAD on the host, then carried to the folder.
    write(&f.local, "scratch.txt", "throw away\n");
    let r = handoff::discard(
        &f.engine,
        LocalWorkspacePathsArgs {
            id: row.id,
            paths: vec!["README.md".into(), "scratch.txt".into()],
        },
    )
    .await
    .unwrap();
    assert_eq!(r.state, "synced", "{r:?}");
    assert_eq!(read(&f.remote, "README.md").as_deref(), Some("hello\n"));
    assert_eq!(read(&f.local, "README.md").as_deref(), Some("hello\n"));
    assert!(read(&f.remote, "scratch.txt").is_none());
    assert!(read(&f.local, "scratch.txt").is_none());
    assert!(
        activity(&f, row.id).is_empty(),
        "{:?}",
        activity(&f, row.id)
    );
    // Nothing else moved.
    assert_eq!(
        read(&f.local, "src/main.rs").as_deref(),
        Some("fn main() { /* mine */ }\n")
    );

    // A paused link refuses a discard: the folder would bring it back.
    pause(&f.engine, row.id).unwrap();
    let e = handoff::discard(
        &f.engine,
        LocalWorkspacePathsArgs {
            id: row.id,
            paths: vec!["src/main.rs".into()],
        },
    )
    .await
    .unwrap_err();
    assert_eq!(e.code, codes::E_INVALID_STATE);
}

async fn conflicted(f: &Fixture) -> LocalWorkspaceRow {
    let row = f.enable().await;
    write(&f.local, "README.md", "local version\n");
    write(&f.remote, "README.md", "remote version\n");
    let r = f.pass(row.id).await;
    assert_eq!(r.conflicts.len(), 1, "{r:?}");
    r
}

#[tokio::test]
async fn compare_shows_a_conflict_from_local_to_remote() {
    let f = fixture();
    let row = conflicted(&f).await;
    let d = handoff::compare_conflict(
        &f.engine,
        LocalWorkspacePathArgs {
            id: row.id,
            path: "README.md".into(),
        },
    )
    .await
    .unwrap();
    assert!(d.diff.contains("-local version"), "{}", d.diff);
    assert!(d.diff.contains("+remote version"), "{}", d.diff);
    let e = handoff::compare_conflict(
        &f.engine,
        LocalWorkspacePathArgs {
            id: row.id,
            path: "src/main.rs".into(),
        },
    )
    .await
    .unwrap_err();
    assert_eq!(e.code, codes::E_NOTFOUND, "not in conflict");
}

#[tokio::test]
async fn keep_both_saves_the_local_version_aside_and_takes_the_remote() {
    let f = fixture();
    let row = conflicted(&f).await;
    let r = handoff::keep_both(
        &f.engine,
        LocalWorkspacePathArgs {
            id: row.id,
            path: "README.md".into(),
        },
    )
    .await
    .unwrap();
    assert!(r.conflicts.is_empty(), "{r:?}");
    assert_eq!(
        read(&f.local, "README.md").as_deref(),
        Some("remote version\n")
    );
    assert_eq!(
        read(&f.local, "README.md.local-copy").as_deref(),
        Some("local version\n")
    );
    assert_eq!(
        read(&f.remote, "README.md.local-copy").as_deref(),
        Some("local version\n"),
        "the copy syncs like any new file"
    );
}

#[tokio::test]
async fn ask_ai_targets_the_worktrees_session_and_resolve_stages_the_local_copy() {
    let f = fixture();
    let row = conflicted(&f).await;
    let plan = handoff::prepare_ask(
        &f.engine,
        AskAiArgs {
            id: row.id,
            intent: "resolve".into(),
            question: None,
            paths: Some(vec!["README.md".into()]),
        },
        None,
    )
    .await
    .unwrap();
    assert_eq!(plan.session_id, f.session_id);
    assert_eq!(plan.tmux_name, "dev-app");
    assert!(
        plan.prompt.contains("README.md.fleet-local"),
        "{}",
        plan.prompt
    );
    assert_eq!(
        read(&f.remote, "README.md.fleet-local").as_deref(),
        Some("local version\n")
    );
    // The staged copy never comes back to the folder.
    f.pass(row.id).await;
    assert!(read(&f.local, "README.md.fleet-local").is_none());

    // `continue` hands over the local changes and clears them once sent.
    write(&f.local, "src/main.rs", "fn main() { /* mine */ }\n");
    f.pass(row.id).await;
    let plan = handoff::prepare_ask(
        &f.engine,
        AskAiArgs {
            id: row.id,
            intent: "continue".into(),
            question: None,
            paths: None,
        },
        None,
    )
    .await
    .unwrap();
    assert!(plan.prompt.contains("- M src/main.rs"), "{}", plan.prompt);
    assert_eq!(plan.clear, vec!["src/main.rs".to_string()]);
    let r = handoff::finish_ask(&f.engine, row.id, &plan.clear).unwrap();
    assert_eq!(r.local_activity, 0);
}

#[tokio::test]
async fn ask_ai_without_a_live_session_says_to_start_one() {
    let f = fixture();
    let row = f.enable().await;
    lock(&f.engine.store)
        .unwrap()
        .mark_host_sessions_lost("devbox", "host_rebooted", &[], 10, 0)
        .unwrap();
    let e = handoff::prepare_ask(
        &f.engine,
        AskAiArgs {
            id: row.id,
            intent: "review".into(),
            question: None,
            paths: None,
        },
        None,
    )
    .await
    .unwrap_err();
    assert_eq!(e.code, codes::E_NOTFOUND);
    // Taking over with nobody there sets the driver and says nothing.
    let args = SetLocalDriverArgs {
        id: row.id,
        driver: "developer".into(),
    };
    let plan = handoff::prepare_driver(&f.engine, &args, None)
        .await
        .unwrap();
    assert!(plan.is_none());
    let r = handoff::finish_driver(&f.engine, &args, None).unwrap();
    assert_eq!(r.driver, "developer");
    assert!(r.driver_since.is_some());
}

#[tokio::test]
async fn handing_back_gives_the_agent_the_developers_changes() {
    let f = fixture();
    let row = f.enable().await;
    let take = SetLocalDriverArgs {
        id: row.id,
        driver: "developer".into(),
    };
    let plan = handoff::prepare_driver(&f.engine, &take, None)
        .await
        .unwrap()
        .unwrap();
    assert!(
        plan.prompt.contains("Do not change files here"),
        "{}",
        plan.prompt
    );
    handoff::finish_driver(&f.engine, &take, Some(&plan)).unwrap();
    write(&f.local, "src/main.rs", "fn main() { /* mine */ }\n");
    let back = SetLocalDriverArgs {
        id: row.id,
        driver: "agent".into(),
    };
    let plan = handoff::prepare_driver(&f.engine, &back, None)
        .await
        .unwrap()
        .unwrap();
    assert!(
        plan.prompt
            .starts_with("The developer hands this worktree back"),
        "{}",
        plan.prompt
    );
    assert!(plan.prompt.contains("- M src/main.rs"), "{}", plan.prompt);
    let r = handoff::finish_driver(&f.engine, &back, Some(&plan)).unwrap();
    assert_eq!(r.driver, "agent");
    assert_eq!(r.local_activity, 0);
    let bad = SetLocalDriverArgs {
        id: row.id,
        driver: "robot".into(),
    };
    assert_eq!(
        handoff::prepare_driver(&f.engine, &bad, None)
            .await
            .unwrap_err()
            .code,
        codes::E_INVALID
    );
}

// ---------------------------------------------------------------------------
// A desktop paired with a hub.
// ---------------------------------------------------------------------------

fn hub_worktree(tmux_name: &str) -> HubSessionWorktree {
    HubSessionWorktree {
        host_alias: "devbox".into(),
        owner: "acme".into(),
        repo: "app".into(),
        worktree_key: "main".into(),
        tmux_name: tmux_name.into(),
    }
}

#[tokio::test]
async fn a_paired_desktop_finds_the_worktree_through_the_sessions_pane() {
    let has_tmux = std::process::Command::new("tmux")
        .arg("-V")
        .output()
        .is_ok_and(|o| o.status.success());
    if !has_tmux {
        return;
    }
    let f = fixture();
    let sock = f._home.path().join("tmux");
    std::fs::create_dir_all(&sock).unwrap();
    let tmux = |args: &[&str]| {
        std::process::Command::new("tmux")
            .args(args)
            .env("TMUX_TMPDIR", &sock)
            .env_remove("TMUX")
            .status()
            .unwrap()
    };
    let remote = f.remote.to_string_lossy().into_owned();
    assert!(tmux(&["new-session", "-d", "-s", "hub-app", "-c", &remote]).success());
    let r = enable_on_hub_session(
        &f.engine,
        hub_worktree("hub-app"),
        &f.local.to_string_lossy(),
        vec![],
    )
    .await;
    let missing = enable_on_hub_session(
        &f.engine,
        hub_worktree("no-such-session"),
        &f.local.to_string_lossy(),
        vec![],
    )
    .await;
    tmux(&["kill-server"]);
    let row = r.unwrap();
    assert_eq!(
        Path::new(&row.remote_path),
        std::fs::canonicalize(&f.remote).unwrap()
    );
    // The hub's session id is not this database's.
    assert_eq!(row.session_id, None);
    let row = f.pass(row.id).await;
    assert_eq!(row.state, "synced", "{row:?}");
    assert_eq!(read(&f.local, "README.md").as_deref(), Some("hello\n"));
    assert!(missing.is_err());
}

#[tokio::test]
async fn a_paired_desktop_does_not_link_the_hubs_own_machine() {
    let f = fixture();
    let mut w = hub_worktree("dev-app");
    w.host_alias = crate::service::projects::LOCAL_HOST.into();
    let e = enable_on_hub_session(&f.engine, w, &f.local.to_string_lossy(), vec![])
        .await
        .unwrap_err();
    assert_eq!(e.code, codes::E_INVALID);
}

#[tokio::test]
async fn a_paired_desktop_asks_the_hubs_session() {
    let f = fixture();
    let row = f.enable().await;
    let mut theirs = lock(&f.engine.store)
        .unwrap()
        .get_session_by_id(f.session_id)
        .unwrap()
        .unwrap();
    // The hub numbers its rows its own way.
    theirs.id = 7001;
    theirs.project_id = Some(55);
    theirs.tmux_name = "hub-app".into();
    let hub = handoff::HubSessions {
        sessions: vec![theirs],
        project_id: Some(55),
    };
    let plan = handoff::prepare_ask(
        &f.engine,
        AskAiArgs {
            id: row.id,
            intent: "review".into(),
            question: None,
            paths: None,
        },
        Some(&hub),
    )
    .await
    .unwrap();
    assert_eq!(
        (plan.session_id, plan.tmux_name.as_str()),
        (7001, "hub-app")
    );
    let other = handoff::HubSessions {
        project_id: Some(56),
        ..hub
    };
    let e = handoff::prepare_ask(
        &f.engine,
        AskAiArgs {
            id: row.id,
            intent: "review".into(),
            question: None,
            paths: None,
        },
        Some(&other),
    )
    .await
    .unwrap_err();
    assert_eq!(e.code, codes::E_NOTFOUND);
}

/// The home folder, anything above it and the disk root are refused: the
/// first pass would copy all of it into the remote worktree.
#[test]
fn a_folder_that_is_or_holds_home_is_refused() {
    let base = tempfile::tempdir().unwrap();
    let home = base.path().join("users/me");
    let project = home.join("code/app");
    std::fs::create_dir_all(&project).unwrap();
    let refuse = |p: &std::path::Path| super::refuse_broad_folder(p, Some(&home));

    assert!(refuse(&project).is_ok());
    for bad in [
        home.clone(),
        base.path().join("users"),
        std::path::PathBuf::from("/"),
    ] {
        assert_eq!(
            refuse(&bad).unwrap_err().code,
            codes::E_INVALID,
            "{}",
            bad.display()
        );
    }
    // `..` back up to home is refused before anything resolves it.
    assert_eq!(
        refuse(&project.join("../..")).unwrap_err().code,
        codes::E_INVALID
    );
    #[cfg(unix)]
    {
        let link = base.path().join("link-to-home");
        std::os::unix::fs::symlink(&home, &link).unwrap();
        assert_eq!(refuse(&link).unwrap_err().code, codes::E_INVALID);
    }
    // No home known: only the root and `..` are refused.
    assert!(super::refuse_broad_folder(&home, None).is_ok());
}

/// Review r18: `~\proj` is the home folder on Windows, and only there.
#[test]
fn a_backslash_after_the_tilde_is_home_on_windows_only() {
    assert_eq!(home_relative("~/proj", false), Some("proj"));
    assert_eq!(home_relative("~/proj", true), Some("proj"));
    assert_eq!(home_relative("~\\proj", true), Some("proj"));
    assert_eq!(home_relative("~\\proj", false), None);
    assert_eq!(home_relative("/abs", true), None);
}
