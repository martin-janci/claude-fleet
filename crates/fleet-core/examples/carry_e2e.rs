//! Cross-host check of the move carry engine: runs this build's REAL
//! generated scripts the way the app does — source side on this machine under
//! `bash -lc`, target side through `ssh <host> -- bash -lc '<script>'`,
//! payloads relayed through this process in chunks — against throwaway git
//! repositories in temp dirs on both ends. No app, no database, no session.
//!
//!   cargo run -p fleet-core --example carry_e2e -- <ssh-host>
//!
//! Everything it creates lives under a `mktemp -d /tmp/cf-e2e.*` dir on the
//! target, a temp dir here, and `~/.cache/claude-fleet/transfer/<fresh uuid>`
//! on both; all of it is removed at the end (also after a failed check).

use fleet_core::service::move_session::{self as mv, carry};
use fleet_core::service::safe_kill::parse_porcelain;
use fleet_core::shell::quote;
use std::collections::BTreeSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

struct Ctx {
    host: String,
    checks: Vec<(String, bool, String)>,
}

impl Ctx {
    fn check(&mut self, name: &str, ok: bool, detail: impl Into<String>) {
        let detail = detail.into();
        println!(
            "  [{}] {name}{}",
            if ok { "ok" } else { "FAIL" },
            if detail.is_empty() {
                String::new()
            } else {
                format!(" — {detail}")
            }
        );
        self.checks.push((name.to_string(), ok, detail));
    }
    /// Exactly the app's local spawn (`run_local_shell`): a login bash.
    fn local(&self, script: &str) -> Output {
        Command::new("bash")
            .args(["-lc", script])
            .output()
            .expect("bash")
    }
    /// Exactly the app's remote call shape: `ssh host -- bash -lc '<quoted script>'`.
    fn remote(&self, script: &str) -> Output {
        Command::new("ssh")
            .args([
                "-o",
                "BatchMode=yes",
                "-o",
                "ConnectTimeout=15",
                &self.host,
                "--",
                "bash",
                "-lc",
                &quote(script),
            ])
            .output()
            .expect("ssh")
    }
    /// `SshClient::upload_file`: the local file on stdin into `cat > <path>`.
    fn upload(&self, local: &Path, remote_path: &str) -> bool {
        let f = std::fs::File::open(local).expect("open upload");
        Command::new("ssh")
            .args([
                "-o",
                "BatchMode=yes",
                &self.host,
                "--",
                &format!("cat > {}", quote(remote_path)),
            ])
            .stdin(Stdio::from(f))
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }
    /// `download_chunked` on a LOCAL source: chunk script → payload → append.
    fn download_local(&self, path: &str, bytes: u64, to: &Path) -> Result<(), String> {
        let mut f = std::fs::File::create(to).map_err(|e| e.to_string())?;
        let mut got = 0u64;
        while got < bytes {
            let want = carry::CHUNK_BYTES.min(bytes - got);
            let out = self.local(&carry::chunk_script(path, got, want));
            let chunk = carry::payload(&out.stdout)
                .filter(|c| !c.is_empty())
                .ok_or_else(|| format!("empty chunk at {got}: {}", err(&out)))?;
            f.write_all(chunk).map_err(|e| e.to_string())?;
            got += chunk.len() as u64;
        }
        (got == bytes)
            .then_some(())
            .ok_or_else(|| format!("got {got} of {bytes}"))
    }
}

fn err(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).trim().to_string()
}
fn text(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}
fn git(dir: &Path, args: &[&str]) -> String {
    let o = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "user.name=cf-e2e",
            "-c",
            "user.email=cf-e2e@localhost",
        ])
        .args(args)
        .output()
        .expect("git");
    assert!(
        o.status.success(),
        "git {args:?} in {}: {}",
        dir.display(),
        err(&o)
    );
    text(&o)
}
fn uuid() -> String {
    text(&Command::new("uuidgen").output().expect("uuidgen"))
        .trim()
        .to_lowercase()
}
fn line_set(files: &[fleet_core::service::safe_kill::DirtyFile]) -> BTreeSet<String> {
    files
        .iter()
        .map(|d| format!("{}\t{}", d.status, d.path))
        .collect()
}

/// The source: a LINKED WORKTREE (claude-fleet's real shape) on `feat`, two
/// commits ahead of origin, with every kind of state a move must reproduce.
fn make_source(lt: &Path, origin_url: &str) -> PathBuf {
    let main = lt.join("repo");
    std::fs::create_dir_all(&main).unwrap();
    git(&main, &["init", "-q", "-b", "main"]);
    git(&main, &["remote", "add", "origin", origin_url]);
    for (f, body) in [
        ("keep.txt", "keep\n"),
        ("mod.txt", "v1\n"),
        ("del.txt", "bye\n"),
        ("both.txt", "v1\n"),
        ("mode.sh", "#!/bin/sh\n"),
    ] {
        std::fs::write(main.join(f), body).unwrap();
    }
    std::fs::write(
        main.join(".gitignore"),
        ".env\nnode_modules/\nlocal.cfg\nconf.d/\n",
    )
    .unwrap();
    git(&main, &["add", "-A"]);
    git(&main, &["commit", "-q", "-m", "base"]);
    git(&main, &["branch", "feat"]);
    git(&main, &["push", "-q", "-u", "origin", "main", "feat"]);
    let wt = lt.join("wt");
    git(
        &main,
        &["worktree", "add", "-q", wt.to_str().unwrap(), "feat"],
    );
    git(&wt, &["branch", "--set-upstream-to=origin/feat", "feat"]);
    for n in ["one", "two"] {
        std::fs::write(wt.join(format!("{n}.txt")), n).unwrap();
        git(&wt, &["add", "-A"]);
        git(&wt, &["commit", "-q", "-m", n]); // unpushed
    }
    std::fs::write(wt.join("mod.txt"), "v2\n").unwrap();
    std::fs::write(wt.join("staged new.txt"), "new\n").unwrap();
    git(&wt, &["add", "staged new.txt"]);
    std::fs::write(wt.join("both.txt"), "staged\n").unwrap();
    git(&wt, &["add", "both.txt"]);
    std::fs::write(wt.join("both.txt"), "then modified\n").unwrap();
    std::fs::write(wt.join("it's untracked.txt"), "u\n").unwrap();
    std::fs::write(wt.join("ünïcödé.txt"), "diacritics\n").unwrap();
    std::fs::create_dir_all(wt.join("sub")).unwrap();
    std::fs::write(wt.join("sub/nested.txt"), "n\n").unwrap();
    std::fs::remove_file(wt.join("del.txt")).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(wt.join("mode.sh"), std::fs::Permissions::from_mode(0o755)).unwrap();
    std::os::unix::fs::symlink("keep.txt", wt.join("link")).unwrap();
    // git-ignored: travels by tar, not by git
    std::fs::write(wt.join(".env"), "SECRET=1\n").unwrap();
    std::fs::set_permissions(wt.join(".env"), std::fs::Permissions::from_mode(0o600)).unwrap();
    let _ = Command::new("xattr")
        .args(["-w", "com.example.cf-e2e", "1"])
        .arg(wt.join(".env"))
        .status(); // provoke AppleDouble
    std::fs::write(wt.join("local.cfg"), "cfg\n").unwrap();
    std::fs::create_dir_all(wt.join("conf.d")).unwrap();
    std::fs::write(wt.join("conf.d/a b.conf"), "a\n").unwrap();
    std::fs::create_dir_all(wt.join("node_modules/pkg")).unwrap();
    std::fs::write(wt.join("node_modules/pkg/index.js"), "x").unwrap();
    wt
}

fn fingerprint(wt: &Path) -> (String, Vec<u8>, String) {
    let idx = git(
        wt,
        &["rev-parse", "--path-format=absolute", "--git-path", "index"],
    );
    (
        git(wt, &["--no-optional-locks", "status", "--porcelain=v1"]),
        std::fs::read(idx.trim()).unwrap(),
        git(
            wt,
            &["for-each-ref", "refs/heads", "refs/remotes", "refs/tags"],
        ),
    )
}

/// Where one run lives: the source worktree, and the temp dirs on both ends.
struct Env<'a> {
    wt: &'a Path,
    lt: &'a Path,
    rt: &'a str,
}

/// One carried move and what it must end in.
struct Scenario<'a> {
    name: &'a str,
    /// What the TARGET clones from (unreachable → the `git init` seed).
    clone_url: &'a str,
    want_seed: carry::TargetSeed,
    want_upstream: bool,
}

/// One carried move. Returns the `~/.claude/projects/<enc>` dir the prep
/// script created on the target, for teardown.
fn scenario(c: &mut Ctx, env: &Env, s: &Scenario) -> Option<String> {
    let (wt, lt, rt) = (env.wt, env.lt, env.rt);
    let (name, clone_url, want_seed, want_upstream) =
        (s.name, s.clone_url, s.want_seed, s.want_upstream);
    println!("\n== scenario: {name}");
    let id = uuid();
    let root = format!("{rt}/clone-{name}");
    let cwd = format!("{rt}/wt-{name}");
    let wts = wt.to_str().unwrap();
    let before = fingerprint(wt);

    // 1. inspect (source)
    let o = c.local(&mv::inspect_script("", Some(wts), "feat"));
    let state = match mv::parse_inspection(&text(&o)) {
        Ok(s) => s,
        Err(e) => {
            c.check(
                "inspect parses",
                false,
                format!("{} / {}", e.message, err(&o)),
            );
            return None;
        }
    };
    c.check(
        "inspect: verdict accepts dirty + unpushed",
        mv::carry_verdict(&state, "feat").is_ok(),
        format!("{} dirty entries, ahead={}", state.dirty.len(), state.ahead),
    );

    // 2. seed + haves (target)
    let o = c.remote(&carry::seed_script(&root, clone_url));
    let seeded = carry::parse_seed(&text(&o));
    c.check(
        "seed",
        seeded.as_ref().ok() == Some(&want_seed),
        format!("{seeded:?} {}", err(&o)),
    );
    let o = c.remote(&carry::haves_script(&root, &id, "feat"));
    let (tgt_dir, haves) = match carry::parse_haves(&text(&o)) {
        Ok(v) => v,
        Err(e) => {
            c.check(
                "haves parses",
                false,
                format!("{} / {}", e.message, err(&o)),
            );
            return None;
        }
    };
    c.check(
        "haves",
        tgt_dir.ends_with(&id),
        format!("{} haves, dir {tgt_dir}", haves.len()),
    );

    // 3. snapshot + bundle (source), relay, fetch (target)
    let o = c.local(&carry::snapshot_script(wts, &id, &haves, 500 * 1024 * 1024));
    let bundle = match carry::parse_snapshot(&text(&o)) {
        Ok(b) => b,
        Err(e) => {
            c.check(
                "snapshot parses",
                false,
                format!("{} / {}", e.message, err(&o)),
            );
            return None;
        }
    };
    let want_commits = if want_seed == carry::TargetSeed::Cloned {
        2
    } else {
        3
    };
    c.check(
        "snapshot + bundle",
        bundle.commits == want_commits,
        format!(
            "{} bytes, {} commits (want {want_commits})",
            bundle.bytes, bundle.commits
        ),
    );
    c.check(
        "source untouched by the snapshot (porcelain, index BYTES, refs)",
        fingerprint(wt) == before,
        "",
    );
    let local_bundle = lt.join(format!("{name}.bundle"));
    let dl = c.download_local(&bundle.path, bundle.bytes, &local_bundle);
    c.check(
        "relay: chunked download",
        dl.is_ok(),
        dl.err().unwrap_or_default(),
    );
    let tgt_bundle = format!("{tgt_dir}/carry.bundle");
    c.check("relay: upload", c.upload(&local_bundle, &tgt_bundle), "");
    let o = c.remote(&carry::fetch_script(&root, &tgt_bundle, &id, "feat"));
    c.check("fetch into the target clone", o.status.success(), err(&o));

    // 4. worktree (what repair does for a local branch), prep, apply, verify
    let o = c.remote(&format!(
        "git -C {} worktree add -q -- {} feat",
        quote(&root),
        quote(&cwd)
    ));
    c.check(
        "worktree add from the LOCAL branch (no origin needed)",
        o.status.success(),
        err(&o),
    );
    let o = c.remote(&mv::target_prep_script(&cwd, &state.head, "feat", &id));
    let prep = mv::parse_target_prep(&text(&o), &id);
    c.check(
        "prep: target at the source HEAD (unpushed commits arrived)",
        prep.as_ref().map(|p| p.head == state.head).unwrap_or(false),
        err(&o),
    );
    let o = c.remote(&carry::apply_script(&cwd, &id, &state.head));
    let so = text(&o);
    match carry::parse_apply(&so) {
        Ok(p) => {
            let (want, got) = (line_set(&state.dirty), line_set(&parse_porcelain(p)));
            let diff: Vec<_> = want.symmetric_difference(&got).cloned().collect();
            c.check(
                "VERIFY: target porcelain == source porcelain",
                want == got,
                if diff.is_empty() {
                    format!("{} entries", got.len())
                } else {
                    format!("differs: {diff:?}")
                },
            );
        }
        Err(e) => c.check("apply", false, format!("{} / {}", e.message, err(&o))),
    }

    // 5. ignored files: list + select (source), pack, relay, extract (target)
    let o = c.local(&carry::ignored_list_script(wts));
    let sel = carry::select_ignored(carry::parse_ignored_list(&o.stdout), 1024, 20 * 1024);
    let mut carried: Vec<String> = sel.carry.iter().map(|e| e.path.clone()).collect();
    carried.sort();
    c.check(
        "ignored: selection",
        carried == [".env", "conf.d/", "local.cfg"]
            && sel
                .left
                .iter()
                .any(|l| l.path == "node_modules/" && l.reason == carry::LeftReason::Denylisted),
        format!(
            "carry {carried:?}, left {:?}",
            sel.left.iter().map(|l| &l.path).collect::<Vec<_>>()
        ),
    );
    let o = c.local(&carry::ignored_pack_script(wts, &id, &carried));
    match carry::parse_pack(&text(&o)) {
        Ok((bytes, archive)) => {
            let local_tgz = lt.join(format!("{name}.tgz"));
            let dl = c.download_local(&archive, bytes, &local_tgz);
            c.check(
                "ignored: pack (BSD tar) + relay",
                dl.is_ok() && c.upload(&local_tgz, &format!("{tgt_dir}/ignored.tgz")),
                dl.err().unwrap_or_default(),
            );
            let o = c.remote(&carry::ignored_extract_script(
                &cwd,
                &format!("{tgt_dir}/ignored.tgz"),
            ));
            c.check("ignored: extract (GNU tar)", o.status.success(), err(&o));
        }
        Err(e) => c.check(
            "ignored: pack",
            false,
            format!("{} / {}", e.message, err(&o)),
        ),
    }

    // 6. what arrived
    let files = [
        "mod.txt",
        "staged new.txt",
        "both.txt",
        "it's untracked.txt",
        "ünïcödé.txt",
        "sub/nested.txt",
        "one.txt",
        ".env",
        "local.cfg",
        "conf.d/a b.conf",
    ];
    let hashes = |paths: &[&str]| {
        paths
            .iter()
            .map(|p| format!("git hash-object -- {}", quote(p)))
            .collect::<Vec<_>>()
            .join("; ")
    };
    let src_h = text(&c.local(&format!("cd {} && {{ {}; }}", quote(wts), hashes(&files))));
    let tgt_h = text(&c.remote(&format!("cd {} && {{ {}; }}", quote(&cwd), hashes(&files))));
    c.check(
        "contents identical (10 files incl. space, quote, diacritic, nested, ignored)",
        !src_h.is_empty() && src_h == tgt_h,
        "",
    );
    let facts = text(&c.remote(&format!(
        "cd {cwd} && printf 'env=%s\\n' \"$(stat -c %a .env)\" && printf 'appledouble=%s\\n' \"$(find . -name '._*' -not -path './.git/*' | wc -l | tr -d ' ')\" && printf 'mode=%s\\n' \"$(stat -c %a mode.sh)\" && printf 'link=%s\\n' \"$(readlink link)\" && printf 'del=%s\\n' \"$([ -e del.txt ] && echo present || echo gone)\" && printf 'nm=%s\\n' \"$([ -e node_modules ] && echo present || echo absent)\" && printf 'staged=%s\\n' \"$(git diff --cached --name-only | sort | tr '\\n' ',')\" && printf 'upstream=%s\\n' \"$(git rev-parse --abbrev-ref 'feat@{{upstream}}' 2>/dev/null)\" && git push --dry-run >/dev/null 2>&1; printf 'pushdry=%s\\n' \"$?\"",
        cwd = quote(&cwd)
    )));
    let fact = |k: &str| {
        facts
            .lines()
            .find_map(|l| l.strip_prefix(&format!("{k}=")))
            .unwrap_or("?")
            .to_string()
    };
    c.check(".env arrived 0600", fact("env") == "600", fact("env"));
    c.check(
        "no AppleDouble ._* files on the target",
        fact("appledouble") == "0",
        fact("appledouble"),
    );
    // git tracks only the executable bit; the rest is 0777 & ~umask on the target
    // (mefistos: umask 002 -> 775), so assert the bit, as the in-repo round trip does.
    let exec = u32::from_str_radix(&fact("mode"), 8)
        .map(|m| m & 0o111 == 0o111)
        .unwrap_or(false);
    c.check(
        "exec bit travelled (mode & 0111)",
        exec,
        format!("{} on the target, umask-dependent", fact("mode")),
    );
    c.check(
        "symlink travelled",
        fact("link") == "keep.txt",
        fact("link"),
    );
    c.check(
        "deletion travelled; node_modules did not",
        fact("del") == "gone" && fact("nm") == "absent",
        format!("del={} nm={}", fact("del"), fact("nm")),
    );
    let src_staged = git(wt, &["diff", "--cached", "--name-only"])
        .lines()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .map(|s| format!("{s},"))
        .collect::<String>();
    c.check(
        "same files staged",
        fact("staged") == src_staged,
        format!("target {:?} / source {src_staged:?}", fact("staged")),
    );
    if want_upstream {
        c.check(
            "moved branch has its upstream; `git push --dry-run` works from the target",
            fact("upstream") == "origin/feat" && fact("pushdry") == "0",
            format!(
                "upstream={} push exit={}",
                fact("upstream"),
                fact("pushdry")
            ),
        );
    } else {
        c.check(
            "init-seeded target: no upstream (origin was unreachable)",
            fact("upstream").is_empty(),
            fact("upstream"),
        );
    }

    // 7. cleanup, both hosts
    c.check(
        "cleanup: source",
        c.local(&carry::cleanup_script(wts, &id)).status.success(),
        "",
    );
    c.check(
        "cleanup: target",
        c.remote(&carry::cleanup_script(&root, &id))
            .status
            .success(),
        "",
    );
    let left_src = git(wt, &["for-each-ref", "refs/fleet"]);
    let left_tgt = text(&c.remote(&format!("git -C {} for-each-ref refs/fleet; ls -d \"$HOME/.cache/claude-fleet/transfer/{id}\" 2>/dev/null", quote(&root))));
    let home = std::env::var("HOME").unwrap_or_default();
    c.check(
        "nothing left: no refs/fleet, no transfer dir, on either host",
        left_src.is_empty()
            && left_tgt.trim().is_empty()
            && !Path::new(&format!("{home}/.cache/claude-fleet/transfer/{id}")).exists(),
        format!("{left_src}{left_tgt}"),
    );
    c.check(
        "source still untouched after the whole move",
        fingerprint(wt) == before,
        "",
    );
    prep.ok().and_then(|p| {
        Path::new(&p.path)
            .parent()
            .map(|d| d.to_string_lossy().into_owned())
    })
}

fn main() {
    let host = std::env::args()
        .nth(1)
        .expect("usage: carry_e2e <ssh-host>");
    let mut c = Ctx {
        host,
        checks: Vec::new(),
    };
    let info = text(&c.remote("uname -sr; git --version; tar --version | head -1"));
    println!("target {}: {}", c.host, info.trim().replace('\n', " | "));
    println!(
        "source: {} | {} | {}",
        text(&c.local("uname -sr")).trim(),
        text(&c.local("git --version")).trim(),
        text(&c.local("tar --version | head -1")).trim()
    );

    let rt = text(&c.remote("mktemp -d /tmp/cf-e2e.XXXXXX"))
        .trim()
        .to_string();
    assert!(
        rt.starts_with("/tmp/cf-e2e."),
        "unexpected remote temp dir {rt:?}"
    );
    let lt = std::env::temp_dir().join(format!("cf-e2e-{}", uuid()));
    std::fs::create_dir_all(&lt).unwrap();
    assert!(c
        .remote(&format!(
            "git init -q --bare {}",
            quote(&format!("{rt}/origin.git"))
        ))
        .status
        .success());
    let wt = make_source(&lt, &format!("{}:{rt}/origin.git", c.host));

    let mut project_dirs = Vec::new();
    let env = Env {
        wt: &wt,
        lt: &lt,
        rt: &rt,
    };
    let origin = format!("{rt}/origin.git");
    for s in [
        // An origin both hosts reach: a thin bundle, and the branch gets its upstream.
        Scenario {
            name: "cloned",
            clone_url: &origin,
            want_seed: carry::TargetSeed::Cloned,
            want_upstream: true,
        },
        // No route to origin from the target: `git init` + the whole branch in the bundle.
        Scenario {
            name: "initialized",
            clone_url: "/nonexistent/origin.git",
            want_seed: carry::TargetSeed::Initialized,
            want_upstream: false,
        },
    ] {
        project_dirs.extend(scenario(&mut c, &env, &s));
    }

    // teardown: only what this run created
    for d in &project_dirs {
        if d.contains("/.claude/projects/-tmp-cf-e2e-") {
            let _ = c.remote(&format!("rmdir -- {} 2>/dev/null", quote(d)));
        }
    }
    let _ = c.remote(&format!("rm -rf -- {}", quote(&rt)));
    let _ = std::fs::remove_dir_all(&lt);
    let gone = text(&c.remote(&format!(
        "[ -e {} ] && echo present || echo gone",
        quote(&rt)
    )));
    c.check(
        "teardown: remote temp dir removed",
        gone.trim() == "gone",
        rt.clone(),
    );

    let failed: Vec<_> = c.checks.iter().filter(|(_, ok, _)| !ok).collect();
    println!("\n{} checks, {} failed", c.checks.len(), failed.len());
    for (n, _, d) in &failed {
        println!("  FAILED: {n} — {d}");
    }
    std::process::exit(if failed.is_empty() { 0 } else { 1 });
}
