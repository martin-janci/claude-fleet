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
    async fn run(&self, _: &str, _: &[&str], _: Duration) -> Result<Output, IpcError> {
        unreachable!("local sync pipes stdin")
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
