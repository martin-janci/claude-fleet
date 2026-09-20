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
//!
//! Two more halves ride alongside the git carry, both from
//! `service::move_session::claude_state` (see its module doc and
//! `docs/superpowers/specs/2026-09-20-move-carry-claude-state-design.md`):
//! **A. the per-session directory** uses EXPLICIT project dirs under this
//! run's own temp trees (`-cf-e2e-src-<scenario>` / `-cf-e2e-tgt-<scenario>`,
//! names beginning with `-` like every real encoded one) — never
//! `~/.claude/projects` — so cleanup is just the usual temp-tree teardown.
//! **B. the project's Claude memory** is keyed by the repo root, so it DOES
//! land in the real `~/.claude/projects/` on both hosts; that is only safe
//! because the repo root lives under a `cf-e2e` temp dir on each side, so its
//! encoded name always contains the literal substring `cf-e2e` (the encoding
//! turns every non-alphanumeric character into `-`, which cannot erase an
//! existing run of alphanumerics). Every removal of such a directory goes
//! through `guarded_projects_rm_script`, whose `case` pattern requires BOTH
//! the `$HOME/.claude/projects/` prefix AND the `cf-e2e` substring before it
//! will run `rm -rf` — a refusal exits non-zero instead, and teardown then
//! verifies the directory is actually gone.

use fleet_core::service::move_session::claude_state;
use fleet_core::service::move_session::{self as mv, carry};
use fleet_core::service::safe_kill::parse_porcelain;
use fleet_core::shell::quote;
use std::collections::BTreeSet;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

/// The source's `new.md`/`differs.md`/`MEMORY.md`, and the one line of that
/// index that must — and must only — travel with `new.md`.
const MEM_NEW_MD: &str = "the fact that only the source knows\n";
const MEM_SRC_DIFFERS_MD: &str = "the source's version of this note\n";
const MEM_SRC_INDEX: &str =
    "# Memory Index\n\n- [new](new.md) — a new fact\n- [differs](differs.md) — an old fact\n";
const MEM_NEW_LINE: &str = "- [new](new.md) — a new fact";
/// The target's own memory, seeded before the carry: a `differs.md` with
/// different content than the source's (so it is `kept_target`, never
/// overwritten), and an index with NO trailing newline — the append script
/// must add one itself before appending (see `memory_append_index_script`).
const MEM_TGT_DIFFERS_MD: &str = "the target's own version of this note\n";
const MEM_TGT_INDEX_SEED: &str = "- [differs](differs.md) — target's own note";

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

/// `cat` on the remote side — small text-file comparisons don't need the
/// chunked relay machinery, just its own bytes back for a direct comparison.
fn remote_read(c: &Ctx, path: &str) -> Vec<u8> {
    c.remote(&format!("cat -- {}", quote(path))).stdout
}
/// `stat -c %a` on the remote side (GNU stat; the target hosts this harness
/// runs against are Linux fleet hosts, same assumption the git-carry checks
/// below already make for `.env`/`mode.sh`).
fn remote_mode(c: &Ctx, path: &str) -> String {
    text(&c.remote(&format!("stat -c %a -- {}", quote(path))))
        .trim()
        .to_string()
}

/// Removes a directory under the real `~/.claude/projects/` iff its name
/// contains `cf-e2e` AND it is really under that prefix — the `case` pattern
/// is the guard, not the Rust caller: a path that fails to match falls to the
/// `*)` branch and refuses (non-zero exit) instead of ever reaching `rm -rf`.
/// Never used for anything but the memory-half fixtures (the session-half
/// fixtures live under this run's own temp trees, never here).
fn guarded_projects_rm_script(path: &str) -> String {
    format!(
        r#"set +e
d={d}
case "$d" in
  "$HOME"/.claude/projects/*cf-e2e*) rm -rf -- "$d" ;;
  *) printf 'refused: %s\n' "$d" >&2; exit 1 ;;
esac
"#,
        d = quote(path)
    )
}
/// [`guarded_projects_rm_script`]'s companion: reports `gone` only once the
/// same guard passes and the path is confirmed absent.
fn guarded_projects_gone_script(path: &str) -> String {
    format!(
        r#"set +e
d={d}
case "$d" in
  "$HOME"/.claude/projects/*cf-e2e*) if [ -e "$d" ]; then echo present; else echo gone; fi ;;
  *) echo refused ;;
esac
"#,
        d = quote(path)
    )
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

/// The `<id>/…` fixture a real per-session directory carries: one subagent
/// transcript deliberately sized to 500 B (so a `cloned` target's own bigger
/// copy wins the merge), its `.meta.json`, an out-of-line tool result and a
/// custom title — every file `0600` like the real ones, so the packed
/// archive (which preserves on-disk modes, umask does not touch it) actually
/// exercises the "arrives 0600" contract instead of whatever the process
/// umask happened to leave from a plain `fs::write`.
fn write_session_fixture(project_dir: &Path, id: &str) {
    let dir = project_dir.join(id);
    for (rel, body) in [
        ("subagents/agent-aa.jsonl", "a\n".repeat(250)), // 500 B
        ("subagents/agent-aa.meta.json", "{}".to_string()),
        ("tool-results/o.txt", "output\n".to_string()),
        ("custom-title.json", "{\"title\":\"t\"}".to_string()),
    ] {
        let f = dir.join(rel);
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        std::fs::write(&f, &body).unwrap();
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
}

/// Half A of the Claude-side carry: list → select → pack → chunked relay →
/// merge, run against the EXPLICIT `src_proj`/`tgt_proj` dirs the caller
/// fabricated (never `~/.claude/projects`), then check the result against
/// `want_seed` (the `cloned` scenario pre-populates a bigger `agent-aa.jsonl`
/// on the target; `initialized` starts the target empty).
fn check_session_state(
    c: &mut Ctx,
    src_proj: &Path,
    tgt_proj: &str,
    id: &str,
    tgt_dir: &str,
    want_seed: carry::TargetSeed,
    pre_existing_aa: Option<&str>,
) {
    let src_proj_s = src_proj.to_str().unwrap();
    let o = c.local(&claude_state::session_list_script(src_proj_s, id));
    let listed = claude_state::parse_file_list(&o.stdout);
    let mut got: Vec<&str> = listed.iter().map(|f| f.path.as_str()).collect();
    got.sort();
    c.check(
        "session-state: list (the 4 fixture files, nothing else)",
        got == [
            "custom-title.json",
            "subagents/agent-aa.jsonl",
            "subagents/agent-aa.meta.json",
            "tool-results/o.txt",
        ],
        format!("{got:?}"),
    );
    let sel = claude_state::select_session_files(
        listed,
        claude_state::DEFAULT_MAX_SESSION_STATE_MB * 1024 * 1024,
    );
    c.check(
        "session-state: select (well under the cap, nothing excluded)",
        sel.skip.is_none() && sel.left.is_empty() && sel.exclude.is_empty() && sel.carry.len() == 4,
        format!("{sel:?}"),
    );
    let o = c.local(&claude_state::session_pack_script(
        src_proj_s,
        id,
        &sel.exclude,
    ));
    let Ok((bytes, archive)) = carry::parse_pack(&text(&o)) else {
        c.check("session-state: pack", false, err(&o));
        return;
    };
    let local_tgz = std::env::temp_dir().join(format!("cf-e2e-state-{id}.tgz"));
    let dl = c.download_local(&archive, bytes, &local_tgz);
    let staged = format!("{tgt_dir}/state.tgz");
    let uploaded = dl.is_ok() && c.upload(&local_tgz, &staged);
    c.check(
        "session-state: pack + relay",
        uploaded,
        dl.err().unwrap_or_default(),
    );
    let _ = std::fs::remove_file(&local_tgz);
    if !uploaded {
        return;
    }
    let o = c.remote(&claude_state::session_merge_script(tgt_proj, id, &staged));
    let Some(merged) = claude_state::parse_merge(&text(&o)) else {
        c.check("session-state: merge parses", false, err(&o));
        return;
    };
    c.check(
        "session-state: nothing failed to place",
        merged.failed.is_empty(),
        format!("{:?}", merged.failed),
    );
    let mut carried_names: Vec<&str> = merged.carried.iter().map(|e| e.path.as_str()).collect();
    carried_names.sort();
    match want_seed {
        carry::TargetSeed::Cloned => {
            c.check(
                "session-state: cloned — the larger target copy is kept, the rest carried",
                merged.kept == ["subagents/agent-aa.jsonl"]
                    && carried_names
                        == [
                            "custom-title.json",
                            "subagents/agent-aa.meta.json",
                            "tool-results/o.txt",
                        ],
                format!("carried={carried_names:?} kept={:?}", merged.kept),
            );
            let after = remote_read(c, &format!("{tgt_proj}/{id}/subagents/agent-aa.jsonl"));
            c.check(
                "session-state: the kept file is byte-identical to before the merge",
                Some(after.as_slice()) == pre_existing_aa.map(str::as_bytes),
                "",
            );
        }
        carry::TargetSeed::Initialized => {
            c.check(
                "session-state: initialized — everything carried, nothing kept",
                merged.kept.is_empty()
                    && carried_names
                        == [
                            "custom-title.json",
                            "subagents/agent-aa.jsonl",
                            "subagents/agent-aa.meta.json",
                            "tool-results/o.txt",
                        ],
                format!("carried={carried_names:?} kept={:?}", merged.kept),
            );
        }
        carry::TargetSeed::Existing => unreachable!("no scenario uses this seed"),
    }
    for rel in &carried_names {
        let src_bytes = std::fs::read(src_proj.join(id).join(rel)).unwrap();
        let tgt_bytes = remote_read(c, &format!("{tgt_proj}/{id}/{rel}"));
        let mode = remote_mode(c, &format!("{tgt_proj}/{id}/{rel}"));
        c.check(
            &format!("session-state: {rel} identical on the target, mode 0600"),
            src_bytes == tgt_bytes && mode == "600",
            format!("mode={mode}"),
        );
    }
    for d in ["subagents", "tool-results"] {
        let mode = remote_mode(c, &format!("{tgt_proj}/{id}/{d}"));
        c.check(
            &format!("session-state: {d}/ created on the target, mode 0700"),
            mode == "700",
            mode,
        );
    }
}

/// The SOURCE's real Claude memory fixture — created ONCE, before any
/// scenario runs: memory is keyed by the repo root (the same `main` checkout
/// for every scenario, since they all move the same worktree), not by the
/// worktree, so there is exactly one source memory dir for the whole run.
/// Uses the app's own `memory_list_script` to learn the real path rather
/// than re-deriving Claude Code's encoding by hand — the same lookup
/// `carry_memory` performs. Asserts the discovered dir is a `cf-e2e` temp
/// dir under `~/.claude/projects/` before writing anything into it.
fn setup_source_memory(c: &mut Ctx, wts: &str) -> String {
    let o = c.local(&claude_state::memory_list_script(wts, Some(wts)));
    let listing = claude_state::parse_memory_list(&o.stdout)
        .unwrap_or_else(|| panic!("memory-list on the source did not parse: {}", err(&o)));
    let home = std::env::var("HOME").unwrap_or_default();
    c.check(
        "memory: source dir is a cf-e2e temp dir under ~/.claude/projects",
        listing.dir.contains("cf-e2e")
            && listing
                .dir
                .starts_with(&format!("{home}/.claude/projects/")),
        listing.dir.clone(),
    );
    std::fs::create_dir_all(&listing.dir).unwrap();
    std::fs::set_permissions(&listing.dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    for (name, body) in [
        ("new.md", MEM_NEW_MD),
        ("differs.md", MEM_SRC_DIFFERS_MD),
        ("MEMORY.md", MEM_SRC_INDEX),
    ] {
        std::fs::write(Path::new(&listing.dir).join(name), body).unwrap();
    }
    listing.dir
}

/// Half B of the Claude-side carry, for one scenario's target: discover the
/// target's real memory dir (`memory_list_script(root, None)`, exactly the
/// app's own lookup for `tgt_project_root`), pre-populate it with the
/// target's own `differs.md` and a one-line index with NO trailing newline,
/// then run the real list-both → decide → pack → relay → extract →
/// read-both-indexes → merge → append flow and check the result. Returns the
/// target's memory PROJECT dir (the parent of `memory/`) for teardown, best
/// effort even when a later step fails, so a partial run never leaks a
/// `cf-e2e` directory on the target.
fn check_memory(c: &mut Ctx, root: &str, tgt_dir: &str, wts: &str, id: &str) -> Option<String> {
    let o = c.remote(&claude_state::memory_list_script(root, None));
    let Some(discover) = claude_state::parse_memory_list(&o.stdout) else {
        c.check("memory: target listing parses", false, err(&o));
        return None;
    };
    let home_ok = discover.dir.contains("cf-e2e") && discover.dir.contains("/.claude/projects/");
    c.check(
        "memory: target dir is a cf-e2e temp dir under ~/.claude/projects",
        home_ok,
        discover.dir.clone(),
    );
    let project_dir = Path::new(&discover.dir)
        .parent()
        .map(|p| p.to_string_lossy().into_owned());
    if !home_ok {
        // Nothing here is safe to touch, let alone guard-delete later.
        return None;
    }

    let o = c.remote(&format!(
        "umask 077 && mkdir -p -- {d} && printf '%s' {differs} > {df} && printf '%s' {idx} > {idxf}",
        d = quote(&discover.dir),
        differs = quote(MEM_TGT_DIFFERS_MD),
        df = quote(&format!("{}/differs.md", discover.dir)),
        idx = quote(MEM_TGT_INDEX_SEED),
        idxf = quote(&format!("{}/MEMORY.md", discover.dir)),
    ));
    c.check(
        "memory: target pre-populated (its own differs.md + index, no trailing newline)",
        o.status.success(),
        err(&o),
    );

    let o = c.local(&claude_state::memory_list_script(wts, Some(wts)));
    let source = claude_state::parse_memory_list(&o.stdout);
    let o = c.remote(&claude_state::memory_list_script(root, None));
    let target = claude_state::parse_memory_list(&o.stdout);
    let (Some(source), Some(target)) = (source, target) else {
        c.check("memory: re-listing both sides parses", false, "");
        return project_dir;
    };
    let decided = claude_state::decide_memory(&source.files, &target.files);
    let mut carried_names: Vec<&str> = decided.carry.iter().map(|e| e.path.as_str()).collect();
    carried_names.sort();
    c.check(
        "memory: decide (new.md carries, differs.md is kept_target)",
        carried_names == ["new.md"]
            && decided.kept_target == ["differs.md"]
            && decided.identical == 0
            && decided.left.is_empty(),
        format!("{decided:?}"),
    );
    if decided.carry.is_empty() {
        return project_dir;
    }
    let names: Vec<String> = decided.carry.iter().map(|e| e.path.clone()).collect();

    let o = c.local(&carry::pack_script(
        &source.dir,
        id,
        claude_state::MEMORY_ARCHIVE,
        &names,
    ));
    let Ok((bytes, archive)) = carry::parse_pack(&text(&o)) else {
        c.check("memory: pack", false, err(&o));
        return project_dir;
    };
    let local_tgz = std::env::temp_dir().join(format!("cf-e2e-memory-{id}.tgz"));
    let dl = c.download_local(&archive, bytes, &local_tgz);
    let staged = format!("{tgt_dir}/{}", claude_state::MEMORY_ARCHIVE);
    let uploaded = dl.is_ok() && c.upload(&local_tgz, &staged);
    c.check(
        "memory: pack + relay",
        uploaded,
        dl.err().unwrap_or_default(),
    );
    let _ = std::fs::remove_file(&local_tgz);
    if !uploaded {
        return project_dir;
    }
    let o = c.remote(&carry::extract_keep_existing_script(
        &target.dir,
        &staged,
        true,
    ));
    c.check(
        "memory: extract (keep-existing — target's own differs.md must survive)",
        o.status.success(),
        err(&o),
    );

    let o = c.local(&claude_state::memory_read_index_script(&source.dir));
    let source_index = match claude_state::parse_index(&text(&o)) {
        Some(Some(t)) => t,
        other => {
            c.check(
                "memory: read the source index",
                false,
                format!("{other:?} / {}", err(&o)),
            );
            return project_dir;
        }
    };
    let o = c.remote(&claude_state::memory_read_index_script(&target.dir));
    let target_index = match claude_state::parse_index(&text(&o)) {
        Some(Some(t)) => t,
        other => {
            c.check(
                "memory: read the target index",
                false,
                format!("{other:?} / {}", err(&o)),
            );
            return project_dir;
        }
    };
    let merged = claude_state::merge_index(&source_index, Some(&target_index), &names);
    c.check(
        "memory: index merge produces exactly the new.md line",
        merged.lines == 1 && merged.append == format!("{MEM_NEW_LINE}\n"),
        format!("{merged:?}"),
    );
    if merged.lines > 0 {
        let o = c.remote(&claude_state::memory_append_index_script(
            &target.dir,
            &merged.append,
        ));
        c.check(
            "memory: append the index on the target",
            o.status.success(),
            err(&o),
        );
    }

    let new_on_target = remote_read(c, &format!("{}/new.md", target.dir));
    c.check(
        "memory: new.md arrived byte-identical",
        new_on_target == MEM_NEW_MD.as_bytes(),
        "",
    );
    let differs_on_target = remote_read(c, &format!("{}/differs.md", target.dir));
    c.check(
        "memory: the target's own differs.md is untouched (byte-identical to before)",
        differs_on_target == MEM_TGT_DIFFERS_MD.as_bytes(),
        "",
    );
    let index_on_target = remote_read(c, &format!("{}/MEMORY.md", target.dir));
    let want_index = format!("{MEM_TGT_INDEX_SEED}\n{MEM_NEW_LINE}\n");
    c.check(
        "memory: target index = old bytes + newline + exactly the new.md line",
        index_on_target == want_index.as_bytes(),
        format!("{:?}", String::from_utf8_lossy(&index_on_target)),
    );

    project_dir
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

/// What one scenario leaves behind for teardown: the target's transcript
/// project dir (removed with a plain `rmdir` — the transcript itself was
/// never written, so it must be empty) and its memory project dir (removed
/// with the guarded `rm -rf`, since it holds real files).
struct ScenarioLeftovers {
    transcript_project_dir: Option<String>,
    memory_project_dir: Option<String>,
}

/// One carried move and its two Claude-side halves.
fn scenario(c: &mut Ctx, env: &Env, s: &Scenario) -> ScenarioLeftovers {
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
            return ScenarioLeftovers {
                transcript_project_dir: None,
                memory_project_dir: None,
            };
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
            return ScenarioLeftovers {
                transcript_project_dir: None,
                memory_project_dir: None,
            };
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
            return ScenarioLeftovers {
                transcript_project_dir: None,
                memory_project_dir: None,
            };
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

    // 6. Claude-side state, half A: the per-session directory. EXPLICIT
    //    project dirs under this run's own temp trees — never
    //    ~/.claude/projects (see the module doc comment).
    let src_proj = lt.join("projects").join(format!("-cf-e2e-src-{name}"));
    write_session_fixture(&src_proj, &id);
    let tgt_proj = format!("{rt}/projects/-cf-e2e-tgt-{name}");
    let pre_existing_aa = if want_seed == carry::TargetSeed::Cloned {
        let body = "b\n".repeat(400); // 800 B — bigger than the source's 500 B
        let o = c.remote(&format!(
            "umask 077 && mkdir -p -- {d} && printf '%s' {body} > {f}",
            d = quote(&format!("{tgt_proj}/{id}/subagents")),
            body = quote(&body),
            f = quote(&format!("{tgt_proj}/{id}/subagents/agent-aa.jsonl")),
        ));
        c.check(
            "session-state: target pre-populated with a larger agent-aa.jsonl (cloned)",
            o.status.success(),
            err(&o),
        );
        Some(body)
    } else {
        None
    };
    check_session_state(
        c,
        &src_proj,
        &tgt_proj,
        &id,
        &tgt_dir,
        want_seed,
        pre_existing_aa.as_deref(),
    );

    // 7. Claude-side state, half B: the project's Claude memory. Real
    //    ~/.claude/projects on both hosts — guarded, see `check_memory`.
    let memory_project_dir = check_memory(c, &root, &tgt_dir, wts, &id);

    // 8. what arrived (the git carry)
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

    // 9. cleanup, both hosts
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
    let transcript_project_dir = prep.ok().and_then(|p| {
        Path::new(&p.path)
            .parent()
            .map(|d| d.to_string_lossy().into_owned())
    });
    ScenarioLeftovers {
        transcript_project_dir,
        memory_project_dir,
    }
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
    let wts = wt.to_str().unwrap();

    // The memory half's source fixture: one dir, shared by every scenario
    // (the repo root — this worktree's main checkout — never changes).
    let source_memory_dir = setup_source_memory(&mut c, wts);

    let mut transcript_dirs = Vec::new();
    let mut memory_dirs = Vec::new();
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
        let leftovers = scenario(&mut c, &env, &s);
        transcript_dirs.extend(leftovers.transcript_project_dir);
        memory_dirs.extend(leftovers.memory_project_dir);
    }

    // teardown: only what this run created
    for d in &transcript_dirs {
        if d.contains("/.claude/projects/-tmp-cf-e2e-") {
            let _ = c.remote(&format!("rmdir -- {} 2>/dev/null", quote(d)));
        }
    }
    // Memory: guarded rm -rf on both hosts, then verify each is gone. The
    // guard lives in the shell script itself (see `guarded_projects_rm_script`),
    // not in this Rust-side filtering — this loop only decides WHICH paths to
    // ask the script to remove.
    for d in &memory_dirs {
        let o = c.remote(&guarded_projects_rm_script(d));
        c.check(
            "teardown: target memory project dir removed",
            o.status.success(),
            format!("{d}: {}", err(&o)),
        );
        let gone = text(&c.remote(&guarded_projects_gone_script(d)));
        c.check(
            "teardown: target memory project dir gone",
            gone.trim() == "gone",
            format!("{d}: {}", gone.trim()),
        );
    }
    if let Some(source_memory_project) = Path::new(&source_memory_dir)
        .parent()
        .map(|p| p.to_string_lossy().into_owned())
    {
        let o = c.local(&guarded_projects_rm_script(&source_memory_project));
        c.check(
            "teardown: source memory project dir removed",
            o.status.success(),
            format!("{source_memory_project}: {}", err(&o)),
        );
        let gone = text(&c.local(&guarded_projects_gone_script(&source_memory_project)));
        c.check(
            "teardown: source memory project dir gone",
            gone.trim() == "gone",
            format!("{source_memory_project}: {}", gone.trim()),
        );
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
