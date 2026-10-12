//! The catalog repo on the controller: git operations via the `git` CLI,
//! and loading / writing IR assets on disk.

use super::model::{Asset, Kind, Problem, Resource};
use super::{E_ASSET_EXISTS, E_ASSET_NOT_FOUND, E_CATALOG_GIT, E_CATALOG_PARSE};
use crate::ipc_error::codes::E_INVALID;
use crate::ipc_error::IpcError;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

pub const SCHEMA_VERSION: u64 = 1;

/// A pointer to the catalog an asset actually came from, on a composed
/// (effective / union) catalog — see `Catalog::origin`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogRef {
    pub id: i64,
    pub name: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Catalog {
    /// `catalogs.id`; 0 in tests that build one by hand and never install it
    /// under the registry. `#[serde(default)]` because `Catalog` travels the
    /// wire nested in `resolve::Resolution` (`catalog_resolve_preview`'s
    /// answer): a hub older than Assets S1b never sends this field, and a
    /// desktop newer than its hub must still parse the answer rather than
    /// error with `E_PARSE`.
    #[serde(default)]
    pub id: i64,
    /// `catalogs.name` ("personal", …); default `""` until `load` sets it.
    #[serde(default)]
    pub name: String,
    /// `catalogs.org_id`; `None` is the personal catalog.
    #[serde(default)]
    pub org_id: Option<i64>,
    pub assets: Vec<Asset>,
    pub problems: Vec<Problem>,
    pub head: String,
    pub loaded_at: i64,
    /// Layer definitions from `layers/*.yaml`. Empty ⇒ no layering, and
    /// every host resolves to the whole catalog (backward compatibility).
    pub layers: crate::service::catalog::layer::LayerSet,
    /// `<kind>/<name>` → the catalog that asset came from, on a composed
    /// (effective / union) catalog. Empty on a catalog loaded from one repo:
    /// then every asset is from `self`. `#[serde(default)]` because `Catalog`
    /// travels the wire nested in `resolve::Resolution`: a hub older than
    /// Assets M2 never sends this field.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub origin: BTreeMap<String, CatalogRef>,
    /// Assets M3: why this catalog could not be loaded — a registry *problem
    /// entry* (no assets, one `Problem`), kept so the other catalogs still
    /// load (spec, Runtime) and so a sync never reads its absence as "every
    /// asset dropped" (Rulings R5, R6). `None` on every loaded catalog.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub load_error: Option<String>,
    /// Assets M3 (PF13): the store row's `repo_path`+`remote_url` this
    /// problem entry was stamped against (NUL-joined; never shown to a
    /// user — display uses `problems[0].path`/`message`), so `ensure_fresh`
    /// retries a broken catalog once `catalog add` re-points it even when
    /// `head_commit`/`last_loaded_at` stay `NULL` before and after (true of
    /// a catalog that has never successfully loaded). `None` on every
    /// loaded catalog. Fix round 1, item 4: purely internal bookkeeping for
    /// `ensure_fresh`'s own freshness check — `#[serde(skip)]`, not
    /// `#[serde(default)]`, so it never travels the wire at all (in
    /// particular, never inside `resolve::Resolution`, which nests a
    /// `Catalog`); deserializing always lands on `Catalog::default()`'s
    /// `None` regardless of what a peer sends.
    #[serde(skip)]
    pub load_error_stamp: Option<String>,
}

impl Catalog {
    pub fn find(&self, kind: Kind, name: &str) -> Option<&Asset> {
        self.assets
            .iter()
            .find(|a| a.kind() == kind && a.header.name == name)
    }

    /// Which catalog `kind/name` came from: `origin`, else this catalog
    /// itself (a catalog loaded straight from one repo has no `origin`
    /// entries — every asset on it is its own).
    pub fn origin_of(&self, kind: Kind, name: &str) -> CatalogRef {
        self.origin
            .get(&format!("{}/{name}", kind.as_str()))
            .cloned()
            .unwrap_or_else(|| CatalogRef {
                id: self.id,
                name: self.name.clone(),
            })
    }
}

/// What a catalog's load problems put in doubt (Assets M4, carry 2,
/// Rulings R24). `load_dir` records a file that did not parse at
/// `<kind dir>/<name>/asset.yaml` (skills, agents) or `<kind dir>/<name>.yaml`
/// (the rest) — as it does a folder entry it could not stat or a symlink
/// that does not resolve to a folder (final review I6) — and a kind
/// directory, or an entry in it, it could not read at `<kind dir>`. The
/// first holds that one asset, the second every asset of the kind. Layer
/// and catalog-file problems hold nothing. A sync must never read a held
/// asset's absence as "the catalog dropped it" (`sync::plan::KeepRules`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProblemHolds {
    /// Kind → why its whole directory could not be read.
    pub kinds: BTreeMap<Kind, String>,
    /// `(kind, name)` → why its file did not load.
    pub assets: BTreeMap<(Kind, String), String>,
}

impl ProblemHolds {
    pub fn from_problems(problems: &[Problem]) -> ProblemHolds {
        let mut out = ProblemHolds::default();
        for p in problems {
            let parts: Vec<&str> = p.path.split(['/', '\\']).collect();
            let Some(kind) = parts.first().and_then(|d| Kind::from_dir(d)) else {
                continue;
            };
            match parts.as_slice() {
                [_] => {
                    out.kinds.insert(kind, p.message.clone());
                }
                [_, name, "asset.yaml"] if kind.is_folder() => {
                    out.assets
                        .insert((kind, (*name).to_string()), p.message.clone());
                }
                [_, file] if !kind.is_folder() => {
                    if let Some(name) = file.strip_suffix(".yaml") {
                        out.assets
                            .insert((kind, name.to_string()), p.message.clone());
                    }
                }
                _ => {}
            }
        }
        out
    }

    /// Why `kind/name` is held, if it is.
    pub fn reason(&self, kind: Kind, name: &str) -> Option<&str> {
        self.assets
            .get(&(kind, name.to_string()))
            .or_else(|| self.kinds.get(&kind))
            .map(String::as_str)
    }

    pub fn is_empty(&self) -> bool {
        self.kinds.is_empty() && self.assets.is_empty()
    }
}

#[derive(serde::Deserialize)]
struct CatalogFile {
    schema_version: u64,
}

/// The git verbs that reach the catalog's remote.
const NETWORK_VERBS: [&str; 5] = ["clone", "fetch", "pull", "push", "ls-remote"];

/// Run git and hand back the raw `Output`, exit status included. Only
/// callers that give a non-zero status its own meaning (`has_staged`) use
/// this directly; everything else goes through `git`, which turns a non-zero
/// status into an `E_CATALOG_GIT`.
fn git_output(dir: &Path, args: &[&str]) -> Result<std::process::Output, IpcError> {
    let mut cmd = crate::proc::std_command("git");
    // Every path this module hands git is a literal file or directory
    // name, never a pattern: `layers/[a].yaml` must not also match
    // `layers/a.yaml` (Assets M4, Task 6 review round 3).
    cmd.args(args)
        .current_dir(dir)
        .env("GIT_LITERAL_PATHSPECS", "1")
        // Nobody is at a terminal to answer a credential prompt: on a hub or
        // a desktop launched from the dock, git asking for a password would
        // wait forever while holding the authoring lock. Fail instead.
        .env("GIT_TERMINAL_PROMPT", "0");
    // The verbs that talk to the remote run on the caller's thread while the
    // authoring lock is held: a remote that stops answering must end them,
    // not hold every later authoring action and card apply behind it
    // (review r06). ssh gives up on a dead host and never asks; https gives
    // up on a transfer that stalls. A person's own GIT_SSH_COMMAND wins.
    if args.first().is_some_and(|v| NETWORK_VERBS.contains(v)) {
        if std::env::var_os("GIT_SSH_COMMAND").is_none() {
            cmd.env(
                "GIT_SSH_COMMAND",
                "ssh -o BatchMode=yes -o ConnectTimeout=15 -o ServerAliveInterval=15 -o ServerAliveCountMax=2",
            );
        }
        cmd.env("GIT_HTTP_LOW_SPEED_LIMIT", "1000")
            .env("GIT_HTTP_LOW_SPEED_TIME", "30");
    }
    // Tests must not depend on (or be broken by) the host's own global git
    // config or identity environment: isolate every git invocation the
    // production code makes from both. This has no effect on release builds
    // — `git config user.email` there still resolves the normal local ->
    // global -> system chain, so a real global identity is honoured and the
    // claude-fleet fallback only kicks in when git itself has none.
    //
    // Per COMMAND, never `std::env::set_var`: this is one multi-threaded
    // test binary with ~158 `Command::new` sites (each `fork`/`exec` reads
    // `environ`) and dozens of `env::var` reads running in parallel, so
    // mutating the process environment from a test thread is a real
    // use-after-free race on glibc — the rule `service/projects.rs` already
    // writes down. It also leaked `GIT_CONFIG_GLOBAL=/dev/null` into every
    // later test in the binary that shells out to git (round 20, F17).
    #[cfg(test)]
    test_git_isolation(&mut cmd);
    cmd.output()
        .map_err(|e| IpcError::new(E_CATALOG_GIT, format!("spawn git: {e}")))
}

/// Make one `git` invocation ignore whoever is running it, scoped to that
/// `Command` and nothing else. Two layers can contribute an identity, and
/// the second is the one that is easy to miss:
///
///  - **config.** `has_identity` asks `git config user.email`, which falls
///    back to the global file. On a clean runner a fresh repo has none and
///    `commit` takes its fallback branch; on a developer's machine it has
///    one and that branch is unreachable. `/dev/null` reads as an empty
///    config; both platforms this is built on have it, and nothing here
///    runs on Windows.
///  - **environment.** `GIT_AUTHOR_EMAIL` and friends override *every*
///    config layer, including a repository's own `--local` setting. An agent
///    harness sets them so its commits are attributed correctly, and with
///    them set `commit_keeps_configured_identity` fails even though the test
///    had just written a local identity — which is what makes this the wrong
///    thing to diagnose as "the global config leaked in".
///
/// Green in CI and red locally is the worse of the two failures, because it
/// is the pattern that teaches people to ignore a red local suite.
#[cfg(test)]
fn test_git_isolation(cmd: &mut std::process::Command) {
    cmd.env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1");
    for name in [
        "GIT_CONFIG_SYSTEM",
        "GIT_AUTHOR_NAME",
        "GIT_AUTHOR_EMAIL",
        "GIT_AUTHOR_DATE",
        "GIT_COMMITTER_NAME",
        "GIT_COMMITTER_EMAIL",
        "GIT_COMMITTER_DATE",
        // Git's last resort before it gives up guessing.
        "EMAIL",
    ] {
        cmd.env_remove(name);
    }
}

fn git(dir: &Path, args: &[&str]) -> Result<String, IpcError> {
    let out = git_output(dir, args)?;
    if !out.status.success() {
        // Final review I1: a clone's remote may carry `user:token@`, and this
        // message becomes a problem entry's `load_error` and an asset tool's
        // error — never echo the userinfo.
        let message = redact_url_userinfo(&format!("git {}: failed", args.join(" ")));
        let stderr = redact_url_userinfo(String::from_utf8_lossy(&out.stderr).trim());
        return Err(IpcError::new(E_CATALOG_GIT, message)
            .with_details(serde_json::json!({ "stderr": stderr })));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// `text` with the userinfo of every `scheme://user[:pass]@host` URL in it
/// replaced by `***` (`scheme://***@host`), so an error that echoes a remote
/// never echoes its credentials. Anything that is not such a URL is left as
/// it is.
pub(crate) fn redact_url_userinfo(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find("://") {
        let (head, tail) = rest.split_at(i + 3);
        out.push_str(head);
        // The authority ends at the first `/`, whitespace or quote.
        let end = tail
            .find(|c: char| c == '/' || c.is_whitespace() || c == '\'' || c == '"')
            .unwrap_or(tail.len());
        let authority = &tail[..end];
        match authority.rfind('@') {
            Some(at) => {
                out.push_str("***");
                out.push_str(&authority[at..]);
            }
            None => out.push_str(authority),
        }
        rest = &tail[end..];
    }
    out.push_str(rest);
    out
}

/// The directory to run `git clone` from for a clone target of `path`: the
/// parent directory, or `.` when `path` is relative with no parent segment
/// (`Path::parent` returns `Some("")` for a single relative segment like
/// `foo`, not `None`, so that empty-string case must be normalised too).
fn clone_parent(path: &Path) -> &Path {
    match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    }
}

/// Clone `remote` into `path` when `path` has no `.git`. With no remote, the
/// directory must already be a git repo.
///
/// A checkout this process may not read is reported as that, with its path:
/// `exists()` answers `false` for it, and taking that for "no checkout" sent
/// the clone into the same unreadable directory, where it failed with a bare
/// `Permission denied (os error 13)` naming nothing — the usual cause being a
/// path set by one user (`sudo fleet-hub catalog set ~/…`) and read by the
/// hub's own (`fleet`).
pub fn ensure_repo(path: &Path, remote: Option<&str>) -> Result<(), IpcError> {
    let git_dir = path.join(".git");
    match git_dir.try_exists() {
        Ok(true) => return Ok(()),
        Ok(false) => {}
        Err(e) => return Err(unreadable(path, &e)),
    }
    match remote {
        Some(url) => {
            let parent = clone_parent(path);
            // Named as the checkout, like a failed probe: Windows reads a
            // path through a file as absent rather than failing the probe,
            // so this is where such a checkout surfaces there.
            std::fs::create_dir_all(parent).map_err(|e| {
                let e =
                    std::io::Error::new(e.kind(), format!("creating {}: {e}", parent.display()));
                unreadable(path, &e)
            })?;
            // A URL that begins with `-` would be parsed by git as an option
            // (`--upload-pack=<cmd>` runs a command); refused outright, and
            // `--` ends option parsing for anything that slips past.
            if url.trim_start().starts_with('-') {
                return Err(IpcError::new(
                    E_CATALOG_GIT,
                    format!("{url:?} is not a repository URL"),
                ));
            }
            let target = path.to_string_lossy().to_string();
            git(parent, &["clone", "-q", "--", url, &target])?;
            Ok(())
        }
        None => Err(IpcError::new(
            E_CATALOG_GIT,
            format!(
                "{} is not a git repository and no remote URL is configured",
                path.display()
            ),
        )),
    }
}

/// `path` could not be read or created by this process: an `E_IO` that says
/// which path, and — for a permission error — whose permission is missing.
fn unreadable(path: &Path, e: &std::io::Error) -> IpcError {
    let hint = if e.kind() == std::io::ErrorKind::PermissionDenied {
        "; the catalog checkout and every directory above it must be readable \
         by the user fleet runs as (on a hub: `fleet`), so set a path it owns, \
         e.g. under its data directory"
    } else {
        ""
    };
    IpcError::new(
        crate::ipc_error::codes::E_IO,
        format!("catalog checkout {}: {e}{hint}", path.display()),
    )
}

pub fn pull(path: &Path) -> Result<(), IpcError> {
    git(path, &["pull", "-q", "--ff-only"]).map(|_| ())
}

pub fn head(path: &Path) -> Result<String, IpcError> {
    git(path, &["rev-parse", "HEAD"])
}

/// Repo working-tree status: dirty file count plus ahead/behind versus the
/// upstream (`None` for both when there is no upstream).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoStatus {
    pub head: String,
    pub dirty: usize,
    pub ahead: Option<u64>,
    pub behind: Option<u64>,
    pub has_upstream: bool,
    /// What the uncommitted changes touch, one row per asset (gap plan
    /// G2.6, "Commit 3 asset changes"). Absent from an older hub.
    #[serde(default)]
    pub changes: Vec<RepoChange>,
}

/// One uncommitted change, grouped to the asset it belongs to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoChange {
    /// `skills/release-notes`, `hooks/stop`, or the file for anything that
    /// is not an asset (`catalog.yaml`, a layer file).
    pub path: String,
    /// `A` added, `M` modified, `D` deleted.
    pub status: String,
}

/// PURE: `git status --porcelain` lines grouped per asset. A folder kind
/// groups by its folder, a single-file kind by its file stem; a path that
/// is not under a kind directory stays as it is. An asset with any added
/// file and nothing else is `A`, one whose every file went is `D`, the rest
/// `M`.
pub fn changes_of(porcelain: &str) -> Vec<RepoChange> {
    let mut out: Vec<(String, std::collections::BTreeSet<char>)> = Vec::new();
    for line in porcelain.lines() {
        // `XY path`; `git()` trims the output, so the first line may have
        // lost X's leading space: split at the first space instead.
        let Some((xy, rest)) = line.trim_start().split_once(' ') else {
            continue;
        };
        let rest = rest.trim_start();
        let path = rest.rsplit(" -> ").next().unwrap_or(rest).trim_matches('"');
        if path.is_empty() {
            continue;
        }
        let code = if xy.starts_with("??") || xy.contains('A') {
            'A'
        } else if xy.contains('D') {
            'D'
        } else {
            'M'
        };
        let mut parts = path.splitn(3, '/');
        let key = match (parts.next(), parts.next(), parts.next()) {
            (Some(dir), Some(name), rest) => match Kind::from_dir(dir) {
                Some(k) if k.is_folder() && rest.is_some() => format!("{dir}/{name}"),
                Some(k) if !k.is_folder() && rest.is_none() => {
                    format!("{dir}/{}", name.strip_suffix(".yaml").unwrap_or(name))
                }
                _ => path.to_string(),
            },
            _ => path.to_string(),
        };
        match out.iter_mut().find(|(k, _)| *k == key) {
            Some((_, codes)) => {
                codes.insert(code);
            }
            None => out.push((key, std::collections::BTreeSet::from([code]))),
        }
    }
    out.into_iter()
        .map(|(path, codes)| {
            let status = if codes.len() == 1 {
                codes.into_iter().next().unwrap_or('M')
            } else {
                'M'
            };
            RepoChange {
                path,
                status: status.to_string(),
            }
        })
        .collect()
}

/// PURE: the commit message the Commit form starts from, written from the
/// changes by rule (no model): `catalog: update skills/a, hooks/b (+2)`.
pub fn commit_message_for(changes: &[RepoChange]) -> String {
    if changes.is_empty() {
        return "catalog: commit pending changes".to_string();
    }
    let verb = if changes.iter().all(|c| c.status == "A") {
        "add"
    } else if changes.iter().all(|c| c.status == "D") {
        "remove"
    } else {
        "update"
    };
    let named: Vec<&str> = changes.iter().take(3).map(|c| c.path.as_str()).collect();
    let more = changes.len().saturating_sub(named.len());
    let tail = if more > 0 {
        format!(" (+{more})")
    } else {
        String::new()
    };
    format!("catalog: {verb} {}{tail}", named.join(", "))
}

/// Backs `author::repo_status` (the toolbar's dirty / ahead / behind badge).
pub fn git_status(root: &Path) -> Result<RepoStatus, IpcError> {
    let head_sha = head(root)?;
    let porcelain = git(root, &["status", "--porcelain"])?;
    let dirty = porcelain.lines().filter(|l| !l.is_empty()).count();
    // Every untracked file, not just its new folder, so a new single-file
    // asset (`hooks/x.yaml` in a new `hooks/`) groups by its own name.
    let changes = if dirty == 0 {
        Vec::new()
    } else {
        changes_of(&git(
            root,
            &["status", "--porcelain", "--untracked-files=all"],
        )?)
    };
    let has_upstream = git(root, &["rev-parse", "--abbrev-ref", "@{u}"]).is_ok();
    let (behind, ahead) = if has_upstream {
        let out = git(
            root,
            &["rev-list", "--left-right", "--count", "@{u}...HEAD"],
        )?;
        let mut parts = out.split_whitespace();
        let behind = parts.next().and_then(|s| s.parse::<u64>().ok());
        let ahead = parts.next().and_then(|s| s.parse::<u64>().ok());
        (behind, ahead)
    } else {
        (None, None)
    };
    Ok(RepoStatus {
        head: head_sha,
        dirty,
        ahead,
        behind,
        has_upstream,
        changes,
    })
}

/// Whether git can resolve an identity for this repo (`git config
/// user.email` succeeds) — the normal local -> global -> system resolution,
/// so a user's own global identity is honoured and the claude-fleet fallback
/// only applies when git itself has none configured anywhere. In test
/// builds, `git()` isolates every invocation from the host's global/system
/// config (see its doc comment) so this is deterministic regardless of the
/// machine running the tests.
pub fn has_identity(root: &Path) -> bool {
    git(root, &["config", "user.email"]).is_ok()
}

/// `git add -A -- <rel_paths>`, or `git add -A` for the whole tree when
/// `rel_paths` is empty. `rel_paths` are trusted here (`author.rs` validates
/// any path that originates from the frontend before it reaches this
/// function); the `--` before them still defuses flag injection (a path that
/// happens to start with `-`) regardless.
pub fn stage_paths(root: &Path, rel_paths: &[String]) -> Result<(), IpcError> {
    if rel_paths.is_empty() {
        git(root, &["add", "-A"]).map(|_| ())
    } else {
        let mut args: Vec<&str> = vec!["add", "-A", "--"];
        args.extend(rel_paths.iter().map(String::as_str));
        git(root, &args).map(|_| ())
    }
}

/// Final review I2: which of `rel_paths` (relative FILE paths, present or
/// not) the checkout's ignore rules match — `.gitignore` files, `.git/info/
/// exclude` and the user's `core.excludesFile` — so a caller never names an
/// ignored path to `git add` (which exits 1 on one). A tracked path is never
/// reported (git does not ignore what it tracks). Paths go over stdin, NUL
/// separated: `check-ignore` takes no pathspec magic, so this one command
/// runs without `GIT_LITERAL_PATHSPECS`, and a name is matched as a name.
pub fn ignored_paths(
    root: &Path,
    rel_paths: &std::collections::BTreeSet<String>,
) -> Result<std::collections::BTreeSet<String>, IpcError> {
    use std::io::Write;
    if rel_paths.is_empty() {
        return Ok(Default::default());
    }
    let mut cmd = crate::proc::std_command("git");
    cmd.args(["check-ignore", "--stdin", "-z"])
        .current_dir(root)
        .env_remove("GIT_LITERAL_PATHSPECS")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    #[cfg(test)]
    test_git_isolation(&mut cmd);
    let mut child = cmd
        .spawn()
        .map_err(|e| IpcError::new(E_CATALOG_GIT, format!("spawn git: {e}")))?;
    let mut input = Vec::new();
    for p in rel_paths {
        input.extend_from_slice(p.as_bytes());
        input.push(0);
    }
    let writer = child.stdin.take().map(|mut stdin| {
        std::thread::spawn(move || {
            let _ = stdin.write_all(&input);
        })
    });
    let out = child
        .wait_with_output()
        .map_err(|e| IpcError::new(E_CATALOG_GIT, format!("git check-ignore: {e}")))?;
    if let Some(w) = writer {
        let _ = w.join();
    }
    // 0: some are ignored; 1: none are; anything else is a real failure.
    match out.status.code() {
        Some(0) | Some(1) => {}
        _ => {
            return Err(
                IpcError::new(E_CATALOG_GIT, "git check-ignore: failed").with_details(
                    serde_json::json!({
                        "stderr": redact_url_userinfo(String::from_utf8_lossy(&out.stderr).trim())
                    }),
                ),
            )
        }
    }
    Ok(String::from_utf8_lossy(&out.stdout)
        .split('\0')
        .filter(|p| !p.is_empty() && rel_paths.contains(*p))
        .map(str::to_string)
        .collect())
}

/// Whether the index differs from HEAD for `rel_paths` (for the whole index
/// when empty) — i.e. whether a commit limited to those paths would record
/// anything. `git diff --cached --quiet` exits 0 when there is nothing
/// staged and 1 when there is (also in a repo with no commits yet, where it
/// compares against the empty tree); any other status is a real failure.
/// Lets a caller skip an empty commit without inspecting the whole working
/// tree, which may legitimately be dirty elsewhere.
pub fn has_staged(root: &Path, rel_paths: &[String]) -> Result<bool, IpcError> {
    let mut args: Vec<&str> = vec!["diff", "--cached", "--quiet"];
    if !rel_paths.is_empty() {
        args.push("--");
        args.extend(rel_paths.iter().map(String::as_str));
    }
    let out = git_output(root, &args)?;
    match out.status.code() {
        Some(0) => Ok(false),
        Some(1) => Ok(true),
        _ => Err(
            IpcError::new(E_CATALOG_GIT, "git diff --cached failed").with_details(
                serde_json::json!({ "stderr": String::from_utf8_lossy(&out.stderr).trim() }),
            ),
        ),
    }
}

/// Commit whatever is currently staged (see `stage_paths`), falling back to
/// a synthetic identity when the repo has none configured locally. Returns
/// the new HEAD. `E_CATALOG_GIT` ("nothing to commit") when the working tree
/// has no changes at all.
pub fn commit(root: &Path, message: &str) -> Result<String, IpcError> {
    let porcelain = git(root, &["status", "--porcelain"])?;
    if porcelain.trim().is_empty() {
        return Err(IpcError::new(E_CATALOG_GIT, "nothing to commit"));
    }
    let mut args: Vec<&str> = Vec::new();
    if !has_identity(root) {
        args.extend([
            "-c",
            "user.name=claude-fleet",
            "-c",
            "user.email=fleet@localhost",
        ]);
    }
    args.extend(["commit", "-q", "-m", message]);
    git(root, &args)?;
    head(root)
}

/// `git push`; requires an existing upstream (git surfaces its own stderr
/// through the shared `git()` helper on failure).
pub fn push(root: &Path) -> Result<(), IpcError> {
    git(root, &["push"]).map(|_| ())
}

/// One asset straight from the checkout (Assets M4: a card's scope edit and
/// take_host read the file just written, before any reload).
pub fn read_asset(root: &Path, kind: Kind, name: &str) -> Result<Asset, IpcError> {
    let yaml = asset_path(root, kind, name);
    load_one(root, kind, &yaml, name)
        .map_err(|m| IpcError::new(E_CATALOG_PARSE, format!("{}: {m}", rel(root, &yaml))))
}

/// `git status` for the M4 guards: every untracked file listed one by one,
/// submodules included, whatever `status.showUntrackedFiles` or
/// `diff.ignoreSubmodules` the repo or the user configured — a config that
/// hides untracked files must never make someone's work look clean.
const STATUS_ALL: [&str; 6] = [
    "status",
    "--porcelain=v1",
    "-z",
    "--untracked-files=all",
    "--ignore-submodules=none",
    "--no-renames",
];

/// Every path `git status` reports changed, staged or untracked (each
/// untracked file on its own), relative to `root` with `/` separators.
pub fn changed_paths(root: &Path) -> Result<Vec<String>, IpcError> {
    let out = git_output(root, &STATUS_ALL)?;
    if !out.status.success() {
        return Err(
            IpcError::new(E_CATALOG_GIT, "git status failed").with_details(
                serde_json::json!({ "stderr": String::from_utf8_lossy(&out.stderr).trim() }),
            ),
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout)
        .split('\0')
        .filter(|e| e.len() > 3)
        .map(|e| e[3..].to_string())
        .collect())
}

/// Whether the working tree has nothing to commit — untracked files count,
/// whatever the config says (Rulings R11: what a failed apply's reset would
/// undo must not be someone's work).
pub fn is_clean(root: &Path) -> Result<bool, IpcError> {
    Ok(changed_paths(root)?.is_empty())
}

/// An asset's path relative to the catalog root, as `git` names it: its
/// folder for skills and agents, its file for the rest.
pub fn asset_rel_path(kind: Kind, name: &str) -> String {
    if kind.is_folder() {
        format!("{}/{name}", kind.dir())
    } else {
        format!("{}/{name}.yaml", kind.dir())
    }
}

/// One commit that touched an asset (Assets M5, the Inspector's History).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommitEntry {
    pub sha: String,
    /// Committer time, Unix seconds.
    pub at: i64,
    pub author: String,
    pub subject: String,
}

/// The newest `limit` commits that touched `kind/name` — its folder or its
/// file ([`asset_rel_path`], a literal pathspec) — newest first. Only a
/// repository with no commit yet has none: a missing, non-git or broken
/// checkout is `E_CATALOG_GIT`, as [`git_status`] answers it.
/// `--no-show-signature` keeps a user's `log.showSignature` from
/// interleaving lines the parser would drop.
pub fn asset_log(
    root: &Path,
    kind: Kind,
    name: &str,
    limit: usize,
) -> Result<Vec<CommitEntry>, IpcError> {
    // A missing directory fails to spawn; a non-git or broken one fails here.
    git(root, &["rev-parse", "--git-dir"])?;
    let head = git_output(root, &["rev-parse", "--verify", "-q", "HEAD"])?;
    if !head.status.success() {
        // `-q`: exit 1 and silence for an unborn HEAD; anything else is a
        // real failure, reported the way `git` reports one.
        if head.status.code() == Some(1) && head.stderr.is_empty() {
            return Ok(Vec::new());
        }
        git(root, &["rev-parse", "--verify", "HEAD"])?;
    }
    let rel = asset_rel_path(kind, name);
    let n = format!("-n{}", limit.max(1));
    let out = git(
        root,
        &[
            "log",
            "--no-show-signature",
            &n,
            "--format=%H%x1f%ct%x1f%an%x1f%s",
            "--",
            &rel,
        ],
    )?;
    Ok(out
        .lines()
        .filter_map(|line| {
            let mut p = line.splitn(4, '\u{1f}');
            Some(CommitEntry {
                sha: p.next()?.to_string(),
                at: p.next()?.parse().ok()?,
                author: p.next()?.to_string(),
                subject: p.next().unwrap_or("").to_string(),
            })
        })
        .collect())
}

fn check_rev(rev: &str) -> Result<(), IpcError> {
    if rev.is_empty() || !rev.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(IpcError::new(E_INVALID, format!("not a commit id: {rev}")));
    }
    Ok(())
}

/// The files tracked in `rev`'s tree at or under each of `paths` (relative,
/// `/`-separated). An empty `paths` answers nothing (never the whole tree).
pub fn tracked_files(
    root: &Path,
    rev: &str,
    paths: &[&str],
) -> Result<std::collections::BTreeSet<String>, IpcError> {
    if paths.is_empty() {
        return Ok(Default::default());
    }
    let mut args: Vec<&str> = vec!["ls-tree", "-r", "--name-only", "-z", rev, "--"];
    args.extend(paths.iter().copied());
    let out = git_output(root, &args)?;
    if !out.status.success() {
        return Err(
            IpcError::new(E_CATALOG_GIT, "git ls-tree failed").with_details(
                serde_json::json!({ "stderr": String::from_utf8_lossy(&out.stderr).trim() }),
            ),
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout)
        .split('\0')
        .filter(|f| !f.is_empty())
        .map(str::to_string)
        .collect())
}

/// Whether the file `rel` is tracked at `rev` and the working tree's copy
/// is still exactly that blob (Assets M4 round 3: the only state in which an
/// apply may claim and rewrite or delete an existing file). A symlink,
/// a directory or an untracked or ignored file is never "unchanged".
pub fn unchanged_since(root: &Path, rev: &str, rel: &str) -> Result<bool, IpcError> {
    check_rev(rev)?;
    let entry = git(root, &["ls-tree", rev, "--", rel])?;
    let Some((meta, _)) = entry.lines().next().and_then(|l| l.split_once('\t')) else {
        return Ok(false);
    };
    let mut parts = meta.split_whitespace();
    let (Some(mode), Some(kind), Some(blob)) = (parts.next(), parts.next(), parts.next()) else {
        return Ok(false);
    };
    if kind != "blob" || mode == "120000" {
        return Ok(false);
    }
    match std::fs::symlink_metadata(root.join(rel)) {
        Ok(m) if m.is_file() => {}
        _ => return Ok(false),
    }
    Ok(git(root, &["hash-object", "--", rel])? == blob)
}

/// The first parent of commit `sha`.
pub fn parent_of(root: &Path, sha: &str) -> Result<String, IpcError> {
    check_rev(sha)?;
    git(root, &["rev-parse", &format!("{sha}^")])
}

/// Every file on disk under `rel_dir` (relative to `root`), whatever git
/// thinks of it — untracked, ignored or tracked. A symlink is listed as a
/// file and never followed. A missing directory has no files.
pub fn files_on_disk(root: &Path, rel_dir: &str) -> Result<Vec<String>, IpcError> {
    let mut out = Vec::new();
    let mut stack = vec![root.join(rel_dir)];
    while let Some(d) = stack.pop() {
        let entries = match std::fs::read_dir(&d) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e.into()),
        };
        for entry in entries {
            let p = entry?.path();
            let meta = std::fs::symlink_metadata(&p)?;
            if meta.is_dir() {
                stack.push(p);
            } else {
                out.push(rel(root, &p));
            }
        }
    }
    out.sort();
    Ok(out)
}

/// Assets M4 guard (PF7, Task 6 review rounds 1–2): what in `root` is NOT
/// this operation's own work. `ours` is the exact set of FILES it wrote or
/// deleted (relative, `/`-separated); `heads` the HEAD it recorded before
/// writing and the commit it made. Foreign is: HEAD anywhere else; every
/// changed path git reports (untracked included) that is not one of `ours`;
/// and every file on disk in a directory holding one of `ours` that is
/// neither ours nor tracked-and-unchanged — which is how a file git
/// ignores, next to ours, is seen. Empty means [`reset_paths`] can undo the
/// operation without touching anyone else's work. Apply and undo share it.
pub fn foreign_changes(
    root: &Path,
    heads: &[&str],
    ours: &std::collections::BTreeSet<String>,
) -> Result<Vec<String>, IpcError> {
    let mut foreign = std::collections::BTreeSet::new();
    let mut moved = None;
    let now = head(root)?;
    if !heads.contains(&now.as_str()) {
        moved = Some(format!(
            "HEAD moved to {} (expected {})",
            &now[..now.len().min(12)],
            heads
                .iter()
                .map(|h| &h[..h.len().min(12)])
                .collect::<Vec<_>>()
                .join(" or ")
        ));
    }
    let changed: std::collections::BTreeSet<String> = changed_paths(root)?.into_iter().collect();
    foreign.extend(changed.iter().filter(|p| !ours.contains(*p)).cloned());
    let dirs: std::collections::BTreeSet<&str> = ours
        .iter()
        .filter_map(|f| f.rsplit_once('/').map(|(d, _)| d))
        .collect();
    let dir_list: Vec<&str> = dirs.iter().copied().collect();
    let in_index: std::collections::BTreeSet<String> = {
        let mut args: Vec<&str> = vec!["ls-files", "-z", "--"];
        args.extend(dir_list.iter().copied());
        if dir_list.is_empty() {
            Default::default()
        } else {
            git(root, &args)?
                .split('\0')
                .filter(|f| !f.is_empty())
                .map(str::to_string)
                .collect()
        }
    };
    for d in &dir_list {
        for f in files_on_disk(root, d)? {
            if !ours.contains(&f) && (changed.contains(&f) || !in_index.contains(&f)) {
                foreign.insert(f);
            }
        }
    }
    Ok(moved.into_iter().chain(foreign).collect())
}

/// Commit exactly the FILES `files` (relative; a deleted tracked file
/// commits as a deletion) — never a directory pathspec, never anything else
/// staged or lying in the tree. `None` when they hold no change.
pub fn commit_paths(
    root: &Path,
    message: &str,
    files: &std::collections::BTreeSet<String>,
) -> Result<Option<String>, IpcError> {
    let all: Vec<&str> = files.iter().map(String::as_str).collect();
    let tracked = tracked_files(root, "HEAD", &all)?;
    let known: Vec<String> = files
        .iter()
        .filter(|f| {
            let p = root.join(f.as_str());
            (p.is_file() || p.is_symlink()) || tracked.contains(*f)
        })
        .cloned()
        .collect();
    if known.is_empty() {
        return Ok(None);
    }
    stage_paths(root, &known)?;
    if !has_staged(root, &known)? {
        return Ok(None);
    }
    let mut args: Vec<&str> = Vec::new();
    if !has_identity(root) {
        args.extend([
            "-c",
            "user.name=claude-fleet",
            "-c",
            "user.email=fleet@localhost",
        ]);
    }
    args.extend(["commit", "-q", "--only", "-m", message, "--"]);
    args.extend(known.iter().map(String::as_str));
    git(root, &args)?;
    head(root).map(Some)
}

/// Undo an operation's own writes, file by file: HEAD back to `pre` (soft —
/// the index keeps every other path); each of `files` that `pre` has is
/// restored from it (index and tree), each it does not have is unstaged and
/// deleted; then `created_dirs` (the directories the operation made, in the
/// order it made them) are removed, deepest first, only when now EMPTY.
/// Never a directory-wide delete, never `git clean`, so ignored and foreign
/// files are never touched. Call it only once [`foreign_changes`] came back
/// empty.
pub fn reset_paths(
    root: &Path,
    pre: &str,
    files: &std::collections::BTreeSet<String>,
    created_dirs: &[String],
) -> Result<(), IpcError> {
    check_rev(pre)?;
    if head(root)? != pre {
        git(root, &["reset", "-q", "--soft", pre])?;
    }
    let all: Vec<&str> = files.iter().map(String::as_str).collect();
    let in_pre = tracked_files(root, pre, &all)?;
    let new: Vec<&str> = all
        .iter()
        .copied()
        .filter(|f| !in_pre.contains(*f))
        .collect();
    if !new.is_empty() {
        let mut args: Vec<&str> = vec!["rm", "-q", "--cached", "--ignore-unmatch", "--"];
        args.extend(new.iter().copied());
        git(root, &args)?;
        for f in &new {
            match std::fs::remove_file(root.join(f)) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
        }
    }
    if !in_pre.is_empty() {
        let mut args: Vec<&str> = vec!["checkout", "-q", pre, "--"];
        args.extend(in_pre.iter().map(String::as_str));
        git(root, &args)?;
    }
    for d in created_dirs.iter().rev() {
        // Only an empty directory goes; one still holding anything (an
        // ignored file, someone's file) stays, silently.
        let _ = std::fs::remove_dir(root.join(d));
    }
    Ok(())
}

/// Put the whole tree back at `rev`, deleting what is untracked but never
/// what git ignores (`clean -fd`, no `-x`). NOT for a changeset apply or
/// undo: those must keep to their own files ([`foreign_changes`],
/// [`reset_paths`]) — a blanket reset would take another writer's work.
pub fn reset_hard(root: &Path, rev: &str) -> Result<(), IpcError> {
    git(root, &["reset", "-q", "--hard", rev])?;
    git(root, &["clean", "-q", "-fd"])?;
    Ok(())
}

/// `git revert` one commit (Assets M4 undo, SB5), with the synthetic
/// identity `commit` falls back to. A revert that fails (a conflict) is
/// aborted (`git revert --abort`), which puts HEAD, index and tree back,
/// and is the error; when the abort fails too and a revert is still in
/// progress the error says so. Callers verify the tree themselves
/// ([`revert_in_progress`], [`head`], [`is_clean`]) — this never forces
/// anything. Answers the new HEAD.
pub fn revert(root: &Path, sha: &str) -> Result<String, IpcError> {
    check_rev(sha)?;
    let mut args: Vec<&str> = Vec::new();
    if !has_identity(root) {
        args.extend([
            "-c",
            "user.name=claude-fleet",
            "-c",
            "user.email=fleet@localhost",
        ]);
    }
    args.extend(["revert", "--no-edit", sha]);
    if let Err(e) = git(root, &args) {
        if let Err(a) = git(root, &["revert", "--abort"]) {
            if revert_in_progress(root) {
                let why = a
                    .details
                    .as_ref()
                    .and_then(|d| d.get("stderr"))
                    .and_then(|s| s.as_str())
                    .unwrap_or_default()
                    .to_string();
                let mut both = IpcError::new(
                    &e.code,
                    format!("{}; `git revert --abort` failed too: {why}", e.message),
                );
                both.details = e.details.clone();
                return Err(both);
            }
        }
        return Err(e);
    }
    head(root)
}

/// Whether commit `ancestor` is in `of`'s history (both hex shas). A
/// commit the repository no longer has (pruned, never fetched) is in no
/// history: `false`, not an error.
pub fn is_ancestor(root: &Path, ancestor: &str, of: &str) -> Result<bool, IpcError> {
    check_rev(ancestor)?;
    check_rev(of)?;
    let out = git_output(root, &["merge-base", "--is-ancestor", ancestor, of])?;
    match out.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ if !git_output(root, &["cat-file", "-e", &format!("{ancestor}^{{commit}}")])?
            .status
            .success() =>
        {
            Ok(false)
        }
        _ => Err(
            IpcError::new(E_CATALOG_GIT, "git merge-base --is-ancestor failed").with_details(
                serde_json::json!({ "stderr": String::from_utf8_lossy(&out.stderr).trim() }),
            ),
        ),
    }
}

/// Whether a `git revert` is stopped half-way in `root` (`REVERT_HEAD`
/// exists).
pub fn revert_in_progress(root: &Path) -> bool {
    git(root, &["rev-parse", "-q", "--verify", "REVERT_HEAD"]).is_ok()
}

/// The files that differ between commits `from` and `to` (relative,
/// `/`-separated, no rename detection): what a commit on `from` that made
/// `to` wrote or deleted.
pub fn files_between(
    root: &Path,
    from: &str,
    to: &str,
) -> Result<std::collections::BTreeSet<String>, IpcError> {
    diff_tree(root, from, to, None)
}

/// The files `from` has and `to` does not (relative, `/`-separated): what a
/// commit on `from` that made `to` deleted — so what reverting it
/// re-creates.
pub fn files_deleted_between(
    root: &Path,
    from: &str,
    to: &str,
) -> Result<std::collections::BTreeSet<String>, IpcError> {
    diff_tree(root, from, to, Some("--diff-filter=D"))
}

fn diff_tree(
    root: &Path,
    from: &str,
    to: &str,
    filter: Option<&str>,
) -> Result<std::collections::BTreeSet<String>, IpcError> {
    check_rev(from)?;
    check_rev(to)?;
    let mut args = vec!["diff-tree", "-r", "--name-only", "-z", "--no-renames"];
    args.extend(filter);
    args.extend([from, to]);
    let out = git_output(root, &args)?;
    if !out.status.success() {
        return Err(
            IpcError::new(E_CATALOG_GIT, "git diff-tree failed").with_details(
                serde_json::json!({ "stderr": String::from_utf8_lossy(&out.stderr).trim() }),
            ),
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout)
        .split('\0')
        .filter(|f| !f.is_empty())
        .map(str::to_string)
        .collect())
}

pub fn asset_path(root: &Path, kind: Kind, name: &str) -> PathBuf {
    if kind.is_folder() {
        root.join(kind.dir()).join(name).join("asset.yaml")
    } else {
        root.join(kind.dir()).join(format!("{name}.yaml"))
    }
}

fn body_file(kind: Kind) -> &'static str {
    match kind {
        Kind::Skill => "body.md",
        Kind::Agent | Kind::Command => "prompt.md",
        _ => "",
    }
}

/// A `Resource.rel_path` is safe to join onto an asset dir and write only
/// when it starts with `resources/`, stays within `[A-Za-z0-9._/-]`, and has
/// no empty or `..` segment (which would otherwise let it escape the asset
/// directory or the `resources/` subtree). Shared with `author.rs`, which
/// applies the same rule to resource paths arriving from the frontend
/// before anything is read or written.
pub fn valid_resource_rel_path(rel_path: &str) -> bool {
    rel_path.starts_with("resources/")
        && rel_path
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '/' | '-'))
        && rel_path
            .split('/')
            .all(|seg| !seg.is_empty() && seg != "..")
}

/// `p` relative to `root`, with `/` separators on every platform: these
/// strings are resource names and keep-lists that travel to (POSIX) hosts
/// and are checked by [`valid_resource_rel_path`], so a Windows `\` must never reach
/// them.
fn rel(root: &Path, p: &Path) -> String {
    let s = p
        .strip_prefix(root)
        .unwrap_or(p)
        .to_string_lossy()
        .to_string();
    if cfg!(windows) {
        s.replace('\\', "/")
    } else {
        s
    }
}

/// Refuse `p` when it is a symlink. An asset's own files (`asset.yaml`, its
/// body, `resources/`) come from a catalog repo that other people push to,
/// and a committed symlink is checked out as one: `SKILL.md ->
/// ~/.ssh/id_ed25519` would otherwise be read on the hub and shipped as the
/// skill's body to every host. A symlinked asset FOLDER stays allowed (a
/// local catalog may link one in); what is inside it is read as files.
fn refuse_symlink(p: &Path) -> Result<(), String> {
    match std::fs::symlink_metadata(p) {
        Ok(m) if m.file_type().is_symlink() => Err(format!(
            "{} is a symlink; an asset's files must be regular files",
            p.file_name().unwrap_or_default().to_string_lossy()
        )),
        _ => Ok(()),
    }
}

fn read_resources(dir: &Path) -> Result<Vec<Resource>, String> {
    let res = dir.join("resources");
    refuse_symlink(&res)?;
    if !res.is_dir() {
        return Ok(Vec::new());
    }
    read_resource_tree(dir, res).map_err(|e| e.to_string())
}

fn read_resource_tree(dir: &Path, res: PathBuf) -> std::io::Result<Vec<Resource>> {
    let mut out = Vec::new();
    let mut stack = vec![res];
    while let Some(d) = stack.pop() {
        let mut entries: Vec<_> = std::fs::read_dir(&d)?.collect::<Result<_, _>>()?;
        entries.sort_by_key(|e| e.path());
        for e in entries {
            let p = e.path();
            // `symlink_metadata` does not follow the link, so a symlink here
            // (including one that cycles back into an ancestor directory) is
            // detected and skipped rather than walked.
            let meta = std::fs::symlink_metadata(&p)?;
            if meta.file_type().is_symlink() {
                continue;
            }
            if meta.is_dir() {
                stack.push(p);
            } else {
                out.push(Resource {
                    rel_path: rel(dir, &p),
                    bytes: std::fs::read(&p)?,
                });
            }
        }
    }
    out.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    Ok(out)
}

fn load_one(root: &Path, kind: Kind, yaml_path: &Path, stem: &str) -> Result<Asset, String> {
    refuse_symlink(yaml_path)?;
    let text = std::fs::read_to_string(yaml_path).map_err(|e| e.to_string())?;
    let mut asset = Asset::from_yaml(Some(kind), &text)?;
    if asset.header.name != stem {
        return Err(format!(
            "name '{}' does not match the file/folder stem '{stem}'",
            asset.header.name
        ));
    }
    let problems = asset.validate();
    if !problems.is_empty() {
        return Err(problems.join("; "));
    }
    if kind.is_folder() {
        let dir = yaml_path.parent().unwrap_or(root);
        let body = dir.join(body_file(kind));
        refuse_symlink(&body)?;
        asset.body =
            std::fs::read_to_string(&body).map_err(|_| format!("missing {}", body_file(kind)))?;
        asset.resources = read_resources(dir)?;
    }
    Ok(asset)
}

/// Parse a catalog working tree. Never fails on a bad asset: those become
/// `problems`. Fails only when `catalog.yaml` is missing or has the wrong
/// schema version.
pub fn load_dir(root: &Path) -> Result<Catalog, IpcError> {
    let cat_file = root.join("catalog.yaml");
    let text = std::fs::read_to_string(&cat_file)
        .map_err(|e| IpcError::new(E_CATALOG_PARSE, format!("{}: {e}", cat_file.display())))?;
    let cf: CatalogFile = serde_yaml::from_str(&text)
        .map_err(|e| IpcError::new(E_CATALOG_PARSE, format!("catalog.yaml: {e}")))?;
    if cf.schema_version != SCHEMA_VERSION {
        return Err(IpcError::new(
            E_CATALOG_PARSE,
            format!(
                "catalog.yaml schema_version {} is not supported (want {SCHEMA_VERSION})",
                cf.schema_version
            ),
        ));
    }
    let mut cat = Catalog {
        loaded_at: super::now_secs(),
        ..Default::default()
    };
    for kind in Kind::ALL {
        let dir = root.join(kind.dir());
        if !dir.is_dir() {
            continue;
        }
        // Final review I6: an entry that cannot be read is a Problem, never
        // silently skipped — an asset whose entry vanished from the load
        // would read as "the catalog dropped it" and plan a Remove.
        let mut entries: Vec<PathBuf> = match std::fs::read_dir(&dir) {
            Ok(rd) => rd
                .filter_map(|e| match e {
                    Ok(e) => Some(e.path()),
                    Err(e) => {
                        // No name to hold: the whole kind is held.
                        cat.problems.push(Problem {
                            path: rel(root, &dir),
                            message: format!("an entry could not be read: {e}"),
                        });
                        None
                    }
                })
                .collect(),
            Err(e) => {
                cat.problems.push(Problem {
                    path: rel(root, &dir),
                    message: e.to_string(),
                });
                continue;
            }
        };
        entries.sort();
        for p in entries {
            let (yaml_path, stem) = if kind.is_folder() {
                let yaml_path = p.join("asset.yaml");
                // A symlink must resolve to a directory; a plain stray file
                // (a README, a `.DS_Store`) is not an asset and is skipped.
                let unreadable = match std::fs::symlink_metadata(&p) {
                    Err(e) => Some(e.to_string()),
                    Ok(m) if m.file_type().is_symlink() => match std::fs::metadata(&p) {
                        Ok(t) if t.is_dir() => None,
                        Ok(_) => Some("a symlink to something that is not a folder".to_string()),
                        Err(e) => Some(format!("a symlink that does not resolve: {e}")),
                    },
                    Ok(m) if !m.is_dir() => continue,
                    Ok(_) => None,
                };
                if let Some(message) = unreadable {
                    cat.problems.push(Problem {
                        path: rel(root, &yaml_path),
                        message,
                    });
                    continue;
                }
                (
                    yaml_path,
                    p.file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string(),
                )
            } else {
                if p.extension().and_then(|e| e.to_str()) != Some("yaml") {
                    continue;
                }
                (
                    p.clone(),
                    p.file_stem()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string(),
                )
            };
            match load_one(root, kind, &yaml_path, &stem) {
                Ok(a) => cat.assets.push(a),
                Err(message) => cat.problems.push(Problem {
                    path: rel(root, &yaml_path),
                    message,
                }),
            }
        }
    }

    // Layers are NOT a Kind: they must never reach compute_host_plan as an
    // asset. Own directory, own pass.
    let layer_dir = root.join("layers");
    if layer_dir.is_dir() {
        let mut parsed: Vec<crate::service::catalog::layer::Layer> = Vec::new();
        let mut entries: Vec<PathBuf> = match std::fs::read_dir(&layer_dir) {
            Ok(rd) => rd
                .filter_map(|e| match e {
                    Ok(e) => Some(e.path()),
                    Err(e) => {
                        cat.problems.push(Problem {
                            path: rel(root, &layer_dir),
                            message: format!("an entry could not be read: {e}"),
                        });
                        None
                    }
                })
                .collect(),
            Err(e) => {
                cat.problems.push(Problem {
                    path: rel(root, &layer_dir),
                    message: e.to_string(),
                });
                Vec::new()
            }
        };
        entries.sort();
        for p in entries {
            if p.extension().and_then(|e| e.to_str()) != Some("yaml") {
                continue;
            }
            let stem = p
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            match std::fs::read_to_string(&p)
                .map_err(|e| e.to_string())
                .and_then(|t| crate::service::catalog::layer::Layer::from_yaml(&t))
            {
                Ok(l) if l.name != stem => cat.problems.push(Problem {
                    path: rel(root, &p),
                    message: format!("layer name '{}' does not match file stem '{stem}'", l.name),
                }),
                Ok(l) => parsed.push(l),
                Err(message) => cat.problems.push(Problem {
                    path: rel(root, &p),
                    message,
                }),
            }
        }
        let (set, errors) = crate::service::catalog::layer::LayerSet::from_layers(parsed);
        for message in errors {
            cat.problems.push(Problem {
                path: "layers".to_string(),
                message,
            });
        }
        // A member, exclude or override key naming an unknown asset is a
        // WARNING: the layer still resolves, matching load_dir's existing
        // tolerance elsewhere. Exclude is checked too — a typo there would
        // otherwise silently exclude nothing.
        for l in set.iter() {
            for key in l
                .members
                .iter()
                .chain(l.exclude.iter())
                .chain(l.overrides.keys())
            {
                if let Some((kind, name)) = crate::service::catalog::layer::split_key(key) {
                    if cat.find(kind, &name).is_none() {
                        cat.problems.push(Problem {
                            path: format!("layers/{}.yaml", l.name),
                            message: format!("'{key}' is not in the catalog"),
                        });
                    }
                }
            }
        }
        // `extends` is flattened at load, not by `resolve`, so a cycle (or a
        // missing/cross-axis parent) is caught here as a Problem instead of
        // surfacing only when a host that happens to be assigned the bad
        // layer is planned — or never, if nobody is assigned it.
        for l in set.iter() {
            if let Err(message) = set.chain_for(&l.name) {
                cat.problems.push(Problem {
                    path: format!("layers/{}.yaml", l.name),
                    message,
                });
            }
        }
        cat.layers = set;
    }

    Ok(cat)
}

/// Write an asset into the working tree (asset.yaml + body + resources). On
/// `overwrite`, prunes files under `<dir>/resources/` that are no longer
/// listed in `asset.resources`, removing directories left empty.
pub fn write_asset(root: &Path, asset: &Asset, overwrite: bool) -> Result<(), IpcError> {
    for r in &asset.resources {
        if !valid_resource_rel_path(&r.rel_path) {
            return Err(IpcError::new(
                E_INVALID,
                format!("invalid resource path: {}", r.rel_path),
            ));
        }
    }
    let kind = asset.kind();
    let yaml_path = asset_path(root, kind, &asset.header.name);
    if yaml_path.exists() && !overwrite {
        return Err(IpcError::new(
            E_ASSET_EXISTS,
            format!(
                "{} {} already exists in the catalog",
                kind.as_str(),
                asset.header.name
            ),
        ));
    }
    let dir = yaml_path.parent().unwrap_or(root);
    std::fs::create_dir_all(dir)?;
    std::fs::write(&yaml_path, asset.to_yaml())?;
    if kind.is_folder() {
        std::fs::write(dir.join(body_file(kind)), &asset.body)?;
        for r in &asset.resources {
            let p = dir.join(&r.rel_path);
            if let Some(parent) = p.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(p, &r.bytes)?;
        }
        if overwrite {
            let keep: HashSet<&str> = asset
                .resources
                .iter()
                .map(|r| r.rel_path.as_str())
                .collect();
            prune_resources(dir, &keep)?;
        }
    }
    Ok(())
}

/// Deletes files under `<dir>/resources/` whose rel_path (relative to `dir`,
/// e.g. `resources/x.txt`) is not in `keep`, then removes any directory
/// under `resources/` (`resources/` itself included) left empty. Symlinks
/// are left untouched (and count as occupying their parent), matching
/// `read_resources`'s treatment of them.
fn prune_resources(dir: &Path, keep: &HashSet<&str>) -> std::io::Result<()> {
    let res = dir.join("resources");
    if !res.is_dir() {
        return Ok(());
    }
    if prune_dir(&res, dir, keep)? {
        std::fs::remove_dir(&res)?;
    }
    Ok(())
}

/// Recursively prunes `current`, returning whether it ended up empty (so the
/// caller can remove it too).
fn prune_dir(current: &Path, root_dir: &Path, keep: &HashSet<&str>) -> std::io::Result<bool> {
    let mut is_empty = true;
    for entry in std::fs::read_dir(current)? {
        let p = entry?.path();
        let meta = std::fs::symlink_metadata(&p)?;
        if meta.file_type().is_symlink() {
            is_empty = false;
            continue;
        }
        if meta.is_dir() {
            if prune_dir(&p, root_dir, keep)? {
                std::fs::remove_dir(&p)?;
            } else {
                is_empty = false;
            }
        } else if keep.contains(rel(root_dir, &p).as_str()) {
            is_empty = false;
        } else {
            std::fs::remove_file(&p)?;
        }
    }
    Ok(is_empty)
}

/// Repo-relative directory (folder kinds) or file path (single-file kinds)
/// for an asset: `"skills/<name>"` or `"hooks/<name>.yaml"`.
pub fn asset_rel_dir(kind: Kind, name: &str) -> String {
    if kind.is_folder() {
        format!("{}/{name}", kind.dir())
    } else {
        format!("{}/{name}.yaml", kind.dir())
    }
}

/// Deletes an asset's folder (folder kinds) or file (single-file kinds),
/// returning the repo-relative paths removed. `E_ASSET_NOT_FOUND` when the
/// asset does not exist on disk.
pub fn remove_asset(root: &Path, kind: Kind, name: &str) -> Result<Vec<String>, IpcError> {
    let not_found = || {
        IpcError::new(
            E_ASSET_NOT_FOUND,
            format!("{} {} not found in the catalog", kind.as_str(), name),
        )
    };
    if kind.is_folder() {
        let dir = root.join(kind.dir()).join(name);
        guard_removable(root, &dir, true, &not_found)?;
        let removed = list_files_rel(root, &dir)?;
        std::fs::remove_dir_all(&dir)?;
        Ok(removed)
    } else {
        let path = asset_path(root, kind, name);
        guard_removable(root, &path, false, &not_found)?;
        std::fs::remove_file(&path)?;
        Ok(vec![rel(root, &path)])
    }
}

/// Checks `path` is safe for `remove_asset` to delete: absent -> `not_found`
/// (via `E_ASSET_NOT_FOUND`); a symlink (whether it targets a directory,
/// file, or nothing) -> `E_INVALID`, never followed or unlinked; present but
/// the wrong type (a plain file where a directory was expected, or vice
/// versa) -> `not_found` as well. `symlink_metadata` (not `metadata`) is
/// essential here: it reports on the link itself rather than following it,
/// which is what lets a symlink be detected before anything is removed.
fn guard_removable(
    root: &Path,
    path: &Path,
    want_dir: bool,
    not_found: &dyn Fn() -> IpcError,
) -> Result<(), IpcError> {
    match std::fs::symlink_metadata(path) {
        Err(_) => Err(not_found()),
        Ok(meta) if meta.file_type().is_symlink() => Err(IpcError::new(
            E_INVALID,
            format!("{} is a symlink; refusing to remove", rel(root, path)),
        )),
        Ok(meta) if meta.is_dir() != want_dir => Err(not_found()),
        Ok(_) => Ok(()),
    }
}

/// All non-symlink files under `dir`, as paths relative to `root`, sorted.
fn list_files_rel(root: &Path, dir: &Path) -> std::io::Result<Vec<String>> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(&d)? {
            let p = entry?.path();
            let meta = std::fs::symlink_metadata(&p)?;
            if meta.file_type().is_symlink() {
                continue;
            }
            if meta.is_dir() {
                stack.push(p);
            } else {
                out.push(rel(root, &p));
            }
        }
    }
    out.sort();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::catalog::model::{Asset, Kind, Resource};
    use std::fs;

    /// Assets M5 (R19): the commits that touched one asset, newest first;
    /// a repo with no commit has none; `limit` caps them; another asset's
    /// commit and a sibling whose name only starts the same are not its.
    #[test]
    fn asset_log_lists_the_commits_that_touched_one_asset_newest_first() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        git(root, &["init", "-q", "-b", "main"]).unwrap();
        assert!(asset_log(root, Kind::Skill, "w", 10).unwrap().is_empty());
        fs::create_dir_all(root.join("skills/w")).unwrap();
        fs::write(root.join("skills/w/asset.yaml"), "kind: skill\nname: w\n").unwrap();
        stage_paths(root, &[]).unwrap();
        let first = commit(root, "add w").unwrap();
        fs::write(root.join("other.txt"), "x\n").unwrap();
        fs::create_dir_all(root.join("skills/w2")).unwrap();
        fs::write(root.join("skills/w2/asset.yaml"), "kind: skill\nname: w2\n").unwrap();
        stage_paths(root, &[]).unwrap();
        commit(root, "unrelated").unwrap();
        fs::write(
            root.join("skills/w/asset.yaml"),
            "kind: skill\nname: w\ndescription: d\n",
        )
        .unwrap();
        stage_paths(root, &[]).unwrap();
        let second = commit(root, "edit w").unwrap();

        let log = asset_log(root, Kind::Skill, "w", 10).unwrap();
        assert_eq!(
            log.iter()
                .map(|c| (c.sha.as_str(), c.subject.as_str()))
                .collect::<Vec<_>>(),
            [(second.as_str(), "edit w"), (first.as_str(), "add w")]
        );
        assert!(log.iter().all(|c| c.at > 0 && !c.author.is_empty()));
        assert_eq!(asset_log(root, Kind::Skill, "w", 1).unwrap().len(), 1);
        assert!(asset_log(root, Kind::Agent, "w", 10).unwrap().is_empty());
    }

    /// Fix round 1: only a repository with no commit yet has an empty
    /// history; a missing, non-git or broken checkout is `E_CATALOG_GIT`,
    /// as `git_status` (repo_status) answers it.
    #[test]
    fn asset_log_refuses_a_missing_or_broken_checkout() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("gone");
        let err = asset_log(&missing, Kind::Skill, "w", 10).unwrap_err();
        assert_eq!(err.code, E_CATALOG_GIT, "{}", err.message);
        let plain = dir.path().join("plain");
        fs::create_dir_all(&plain).unwrap();
        let err = asset_log(&plain, Kind::Skill, "w", 10).unwrap_err();
        assert_eq!(err.code, E_CATALOG_GIT, "{}", err.message);
        let broken = dir.path().join("broken");
        fs::create_dir_all(&broken).unwrap();
        git(&broken, &["init", "-q", "-b", "main"]).unwrap();
        assert!(asset_log(&broken, Kind::Skill, "w", 10).unwrap().is_empty());
        fs::write(broken.join(".git/HEAD"), "garbage\n").unwrap();
        let err = asset_log(&broken, Kind::Skill, "w", 10).unwrap_err();
        assert_eq!(err.code, E_CATALOG_GIT, "{}", err.message);
    }

    /// Final review I1: a remote's userinfo never survives into a git error.
    #[test]
    fn redact_url_userinfo_hides_credentials_and_keeps_the_rest() {
        assert_eq!(
            redact_url_userinfo("git clone -q https://u:secret@host/r.git /srv/acme: failed"),
            "git clone -q https://***@host/r.git /srv/acme: failed"
        );
        assert_eq!(
            redact_url_userinfo("fatal: unable to access 'https://tok@h:8443/x/': 401"),
            "fatal: unable to access 'https://***@h:8443/x/': 401"
        );
        // Two URLs, one without userinfo; an scp-style remote has no scheme.
        assert_eq!(
            redact_url_userinfo("a ssh://git:pw@g.example/r b https://plain.example/r c"),
            "a ssh://***@g.example/r b https://plain.example/r c"
        );
        assert_eq!(
            redact_url_userinfo("git@github.com:o/r.git"),
            "git@github.com:o/r.git"
        );
        assert_eq!(redact_url_userinfo("https://u:p@h"), "https://***@h");
        assert_eq!(redact_url_userinfo(""), "");
    }

    fn tmp(name: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("fleet-catalog-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    fn write(p: &std::path::Path, rel: &str, content: &str) {
        let f = p.join(rel);
        fs::create_dir_all(f.parent().unwrap()).unwrap();
        fs::write(f, content).unwrap();
    }

    #[test]
    fn load_dir_reads_every_kind_and_collects_problems() {
        let root = tmp("load");
        write(&root, "catalog.yaml", "schema_version: 1\nname: test\n");
        write(
            &root,
            "skills/worktree/asset.yaml",
            "kind: skill\nname: worktree\ndescription: d\n",
        );
        write(&root, "skills/worktree/body.md", "# body\n");
        write(&root, "skills/worktree/resources/scripts/go.sh", "echo\n");
        write(
            &root,
            "agents/pm/asset.yaml",
            "kind: agent\nname: pm\ndescription: d\n",
        );
        write(&root, "agents/pm/prompt.md", "prompt\n");
        write(&root, "hooks/stop.yaml", "kind: hook\nname: stop\ndescription: d\nevent: stop\naction: { type: command, command: x }\n");
        write(
            &root,
            "mcp/fleet.yaml",
            "kind: mcp_server\nname: fleet\ndescription: d\ntransport: http\nurl: u\n",
        );
        write(&root, "plugins/sp.yaml", "kind: plugin_ref\nname: sp\ndescription: d\nharness: claude\nmarketplace: { name: m, source: github, repo: o/r }\nplugin: sp\nversion: latest\n");
        write(&root, "hooks/bad.yaml", "kind: hook\nname: WRONG NAME\ndescription: d\nevent: stop\naction: { type: command, command: x }\n");
        write(&root, "mcp/garbage.yaml", ": : not yaml [\n");
        write(
            &root,
            "skills/mismatch/asset.yaml",
            "kind: skill\nname: other\ndescription: d\n",
        );

        let cat = load_dir(&root).unwrap();
        let names: Vec<(Kind, String)> = cat
            .assets
            .iter()
            .map(|a| (a.kind(), a.header.name.clone()))
            .collect();
        assert_eq!(
            names,
            vec![
                (Kind::Skill, "worktree".into()),
                (Kind::Agent, "pm".into()),
                (Kind::Hook, "stop".into()),
                (Kind::McpServer, "fleet".into()),
                (Kind::PluginRef, "sp".into()),
            ]
        );
        let skill = cat.find(Kind::Skill, "worktree").unwrap();
        assert_eq!(skill.body, "# body\n");
        assert_eq!(skill.resources.len(), 1);
        assert_eq!(skill.resources[0].rel_path, "resources/scripts/go.sh");
        assert_eq!(cat.find(Kind::Agent, "pm").unwrap().body, "prompt\n");
        assert_eq!(cat.problems.len(), 3, "{:?}", cat.problems);
        assert!(cat
            .problems
            .iter()
            .any(|p| p.path.ends_with("hooks/bad.yaml") && p.message.contains("name")));
        assert!(cat
            .problems
            .iter()
            .any(|p| p.path.ends_with("mcp/garbage.yaml")));
        assert!(cat
            .problems
            .iter()
            .any(|p| p.path.ends_with("skills/mismatch/asset.yaml") && p.message.contains("stem")));
    }

    #[test]
    fn load_dir_rejects_missing_or_wrong_schema() {
        let root = tmp("schema");
        let err = load_dir(&root).unwrap_err();
        assert_eq!(err.code, "E_CATALOG_PARSE");
        write(&root, "catalog.yaml", "schema_version: 99\n");
        assert_eq!(load_dir(&root).unwrap_err().code, "E_CATALOG_PARSE");
    }

    /// An `extends` cycle must be caught at load (Fix 2), regardless of
    /// whether any host is assigned the offending layer — otherwise it is
    /// invisible until (or unless) a host happens to be planned with it.
    #[test]
    fn load_dir_reports_an_extends_cycle_as_a_problem_and_does_not_hang() {
        let root = tmp("layer-cycle");
        write(&root, "catalog.yaml", "schema_version: 1\nname: test\n");
        write(
            &root,
            "layers/a.yaml",
            "kind: layer\nname: a\naxis: role\nextends: b\n",
        );
        write(
            &root,
            "layers/b.yaml",
            "kind: layer\nname: b\naxis: role\nextends: a\n",
        );

        let cat = load_dir(&root).unwrap();
        assert!(
            cat.problems
                .iter()
                .any(|p| p.path == "layers/a.yaml" && p.message.contains("cycle")),
            "{:?}",
            cat.problems
        );
        assert!(
            cat.problems
                .iter()
                .any(|p| p.path == "layers/b.yaml" && p.message.contains("cycle")),
            "{:?}",
            cat.problems
        );
        // The layers still load into the set — only the chain is unusable.
        assert!(cat.layers.get("a").is_some());
        assert!(cat.layers.get("b").is_some());
    }

    fn catalog_with_skill_s(label: &str) -> std::path::PathBuf {
        let root = tmp(label);
        write(&root, "catalog.yaml", "schema_version: 1\nname: test\n");
        write(
            &root,
            "skills/s/asset.yaml",
            "kind: skill\nname: s\ndescription: d\n",
        );
        write(&root, "skills/s/body.md", "b\n");
        root
    }

    fn has_problem(cat: &Catalog, path: &str, needle: &str) -> bool {
        cat.problems
            .iter()
            .any(|p| p.path == path && p.message.contains(needle))
    }

    #[test]
    fn load_dir_reports_each_bad_layer_and_keeps_the_good_ones() {
        let root = catalog_with_skill_s("layer-problems");
        // File stem and `name` disagree: the loader would otherwise register
        // a layer under a name no file carries.
        write(
            &root,
            "layers/wrong.yaml",
            "kind: layer\nname: right\naxis: role\n",
        );
        // An unknown member is a warning only — the layer still loads.
        write(
            &root,
            "layers/warn.yaml",
            "kind: layer\nname: warn\naxis: role\nmembers:\n  - skill/s\n  - skill/ghost\n",
        );
        // A key in both members and exclude of one layer is rejected.
        write(
            &root,
            "layers/clash.yaml",
            "kind: layer\nname: clash\naxis: role\nmembers:\n  - skill/s\nexclude:\n  - skill/s\n",
        );

        let cat = load_dir(&root).unwrap();
        assert!(
            has_problem(&cat, "layers/wrong.yaml", "does not match file stem"),
            "{:?}",
            cat.problems
        );
        assert!(cat.layers.get("right").is_none());
        assert!(
            has_problem(&cat, "layers/warn.yaml", "skill/ghost"),
            "{:?}",
            cat.problems
        );
        assert!(cat.layers.get("warn").is_some());
        assert!(
            has_problem(&cat, "layers", "both members and exclude"),
            "{:?}",
            cat.problems
        );
        assert!(cat.layers.get("clash").is_none());
    }

    #[test]
    fn load_dir_warns_about_an_excluded_key_the_catalog_does_not_have() {
        // The spec checks members, exclude and overrides alike; a typo'd
        // exclude would otherwise silently exclude nothing.
        let root = catalog_with_skill_s("layer-exclude-typo");
        write(
            &root,
            "layers/r.yaml",
            "kind: layer\nname: r\naxis: role\nexclude:\n  - skill/typo\n",
        );

        let cat = load_dir(&root).unwrap();
        assert!(
            has_problem(&cat, "layers/r.yaml", "skill/typo"),
            "{:?}",
            cat.problems
        );
        assert!(cat.layers.get("r").is_some());
    }

    #[test]
    fn write_asset_round_trips_and_refuses_overwrite() {
        let root = tmp("write");
        write(&root, "catalog.yaml", "schema_version: 1\n");
        let mut a = Asset::from_yaml(None, "kind: skill\nname: s\ndescription: d\n").unwrap();
        a.body = "b\n".into();
        a.resources.push(Resource {
            rel_path: "resources/x.txt".into(),
            bytes: b"x".to_vec(),
        });
        write_asset(&root, &a, false).unwrap();
        assert!(root.join("skills/s/asset.yaml").exists());
        assert_eq!(
            fs::read_to_string(root.join("skills/s/body.md")).unwrap(),
            "b\n"
        );
        assert_eq!(
            fs::read(root.join("skills/s/resources/x.txt")).unwrap(),
            b"x"
        );
        let err = write_asset(&root, &a, false).unwrap_err();
        assert_eq!(err.code, "E_ASSET_EXISTS");
        write_asset(&root, &a, true).unwrap();
        let hook = Asset::from_yaml(None, "kind: hook\nname: h\ndescription: d\nevent: stop\naction: { type: command, command: x }\n").unwrap();
        write_asset(&root, &hook, false).unwrap();
        assert!(root.join("hooks/h.yaml").exists());
        let cat = load_dir(&root).unwrap();
        assert_eq!(cat.assets.len(), 2);
        assert_eq!(cat.find(Kind::Skill, "s").unwrap().resources[0].bytes, b"x");
    }

    #[test]
    fn git_ensure_head_and_pull_on_a_local_repo() {
        let origin = tmp("origin");
        let run = git_run;
        run(&origin, &["init", "-q", "-b", "main"]);
        run(&origin, &["config", "user.email", "t@t"]);
        run(&origin, &["config", "user.name", "t"]);
        write(&origin, "catalog.yaml", "schema_version: 1\n");
        run(&origin, &["add", "."]);
        run(&origin, &["commit", "-q", "-m", "init"]);

        let clone =
            std::env::temp_dir().join(format!("fleet-catalog-clone-{}", std::process::id()));
        let _ = fs::remove_dir_all(&clone);
        ensure_repo(&clone, Some(origin.to_str().unwrap())).unwrap();
        assert!(clone.join(".git").exists());
        let h1 = head(&clone).unwrap();
        assert_eq!(h1.len(), 40);
        ensure_repo(&clone, Some(origin.to_str().unwrap())).unwrap(); // idempotent

        write(&origin, "hooks/x.yaml", "kind: hook\nname: x\ndescription: d\nevent: stop\naction: { type: command, command: x }\n");
        run(&origin, &["add", "."]);
        run(&origin, &["commit", "-q", "-m", "two"]);
        pull(&clone).unwrap();
        assert_ne!(head(&clone).unwrap(), h1);

        let missing = std::env::temp_dir().join("fleet-catalog-does-not-exist");
        let _ = fs::remove_dir_all(&missing);
        let err = ensure_repo(&missing, None).unwrap_err();
        assert_eq!(err.code, "E_CATALOG_GIT");
    }

    /// A checkout that cannot even be probed is reported as that, with its
    /// path — never taken for "no checkout" and cloned over. A file where a
    /// directory should be stands in for an unreadable one, which the root
    /// test runner could read anyway. Unix fails the probe itself
    /// (`NotADirectory`); Windows reads the path as absent and fails creating
    /// the clone's parent — both name the checkout, and neither touches it.
    #[test]
    fn ensure_repo_names_a_checkout_it_cannot_probe() {
        let base = tmp("ensure-repo-unprobeable");
        let file = base.join("file");
        fs::write(&file, "x").unwrap();
        let path = file.join("agent-assets");
        let err = ensure_repo(&path, Some("/nonexistent/remote.git")).unwrap_err();
        assert_eq!(err.code, "E_IO", "{}", err.message);
        assert!(
            err.message.contains(&*path.to_string_lossy()),
            "{}",
            err.message
        );
        assert_eq!(fs::read_to_string(&file).unwrap(), "x");
    }

    /// A checkout that probes as absent but whose directory cannot be made
    /// names the checkout and the directory that failed. A dangling symlink
    /// in the way does this on Unix, as a path under a file does on Windows.
    #[cfg(unix)]
    #[test]
    fn ensure_repo_names_a_checkout_it_cannot_create() {
        let base = tmp("ensure-repo-uncreatable");
        let dangling = base.join("dangling");
        std::os::unix::fs::symlink(base.join("nowhere"), &dangling).unwrap();
        let path = dangling.join("agent-assets");
        let err = ensure_repo(&path, Some("/nonexistent/remote.git")).unwrap_err();
        assert_eq!(err.code, "E_IO");
        assert!(
            err.message.contains(&*path.to_string_lossy())
                && err
                    .message
                    .contains(&format!("creating {}", dangling.display())),
            "{}",
            err.message
        );
    }

    #[test]
    fn a_permission_error_says_whose_permission() {
        let e = std::io::Error::from(std::io::ErrorKind::PermissionDenied);
        let err = unreadable(std::path::Path::new("/home/me/src/agent-assets"), &e);
        assert!(
            err.message.contains("/home/me/src/agent-assets"),
            "{}",
            err.message
        );
        assert!(
            err.message.contains("the user fleet runs as"),
            "{}",
            err.message
        );
        let other = unreadable(
            std::path::Path::new("/x"),
            &std::io::Error::from(std::io::ErrorKind::NotFound),
        );
        assert!(!other.message.contains("runs as"), "{}", other.message);
    }

    #[test]
    fn clone_parent_normalises_relative_single_segment_paths() {
        assert_eq!(
            clone_parent(std::path::Path::new("foo")),
            std::path::Path::new(".")
        );
        assert_eq!(
            clone_parent(std::path::Path::new("/tmp/x/foo")),
            std::path::Path::new("/tmp/x")
        );
        assert_eq!(
            clone_parent(std::path::Path::new("/foo")),
            std::path::Path::new("/")
        );
    }

    #[cfg(unix)]
    #[test]
    fn load_dir_skips_symlinks_in_resources_and_terminates() {
        let root = tmp("symlink");
        write(&root, "catalog.yaml", "schema_version: 1\n");
        write(
            &root,
            "skills/x/asset.yaml",
            "kind: skill\nname: x\ndescription: d\n",
        );
        write(&root, "skills/x/body.md", "b\n");
        write(&root, "skills/x/resources/real.txt", "hi\n");
        // A symlink back up the tree: following it as a directory would
        // recurse forever. It must be skipped entirely, not walked.
        std::os::unix::fs::symlink("..", root.join("skills/x/resources/loop")).unwrap();

        let cat = load_dir(&root).unwrap();
        let skill = cat.find(Kind::Skill, "x").unwrap();
        assert_eq!(skill.resources.len(), 1);
        assert_eq!(skill.resources[0].rel_path, "resources/real.txt");
    }

    #[test]
    fn a_remote_that_looks_like_an_option_is_not_cloned() {
        let root = tmp("dash-url");
        let path = root.join("checkout");
        let marker = root.join("ran");
        let url = format!("--upload-pack=touch {}", marker.display());
        let err = ensure_repo(&path, Some(&url)).unwrap_err();
        assert_eq!(err.code, E_CATALOG_GIT);
        assert!(!marker.exists());
        assert!(!path.exists());
    }

    /// A catalog repo is pushed to by other people, and a committed symlink
    /// is checked out as one. The asset's own files must not reach outside
    /// it: the body, `asset.yaml` and `resources/` are refused as symlinks,
    /// and the asset becomes a problem rather than shipping the target.
    #[cfg(unix)]
    #[test]
    fn load_dir_refuses_an_asset_whose_own_files_are_symlinks() {
        let root = tmp("symlink-own");
        let outside = tmp("symlink-outside");
        write(&outside, "id_ed25519", "PRIVATE KEY\n");
        write(&outside, "secrets/token", "t0ken\n");
        write(
            &outside,
            "asset.yaml",
            "kind: skill\nname: y\ndescription: d\n",
        );
        write(&root, "catalog.yaml", "schema_version: 1\n");
        // Body links out.
        write(
            &root,
            "skills/a/asset.yaml",
            "kind: skill\nname: a\ndescription: d\n",
        );
        std::os::unix::fs::symlink(outside.join("id_ed25519"), root.join("skills/a/body.md"))
            .unwrap();
        // resources/ links out.
        write(
            &root,
            "skills/b/asset.yaml",
            "kind: skill\nname: b\ndescription: d\n",
        );
        write(&root, "skills/b/body.md", "b\n");
        std::os::unix::fs::symlink(outside.join("secrets"), root.join("skills/b/resources"))
            .unwrap();
        // asset.yaml links out.
        fs::create_dir_all(root.join("skills/y")).unwrap();
        write(&root, "skills/y/body.md", "b\n");
        std::os::unix::fs::symlink(outside.join("asset.yaml"), root.join("skills/y/asset.yaml"))
            .unwrap();
        // A plain asset beside them still loads.
        write(
            &root,
            "skills/ok/asset.yaml",
            "kind: skill\nname: ok\ndescription: d\n",
        );
        write(&root, "skills/ok/body.md", "fine\n");

        let cat = load_dir(&root).unwrap();
        assert!(cat.find(Kind::Skill, "ok").is_some());
        for name in ["a", "b", "y"] {
            assert!(cat.find(Kind::Skill, name).is_none(), "{name} loaded");
            assert!(
                cat.problems
                    .iter()
                    .any(|p| p.path.starts_with(&format!("skills/{name}/"))
                        && p.message.contains("symlink")),
                "{name}: {:?}",
                cat.problems
            );
        }
        let all = format!("{cat:?}");
        assert!(!all.contains("PRIVATE KEY") && !all.contains("t0ken"));
    }

    #[cfg(unix)]
    #[test]
    fn load_dir_records_problem_when_kind_dir_unreadable_and_continues() {
        use std::os::unix::fs::PermissionsExt;
        let root = tmp("unreadable");
        write(&root, "catalog.yaml", "schema_version: 1\n");
        write(
            &root,
            "agents/pm/asset.yaml",
            "kind: agent\nname: pm\ndescription: d\n",
        );
        write(&root, "agents/pm/prompt.md", "prompt\n");
        let hooks_dir = root.join("hooks");
        fs::create_dir_all(&hooks_dir).unwrap();
        if crate::service::move_session::carry::tests::skip_as_root(
            "mode 000 does not stop uid 0 reading the directory",
        ) {
            return;
        }
        fs::set_permissions(&hooks_dir, fs::Permissions::from_mode(0o000)).unwrap();

        let result = load_dir(&root);

        // Restore permissions unconditionally so tmp cleanup on a later run
        // (and this test's own `tmp()` helper) can remove the directory.
        fs::set_permissions(&hooks_dir, fs::Permissions::from_mode(0o755)).unwrap();

        let cat = result.unwrap();
        assert_eq!(cat.assets.len(), 1);
        assert_eq!(cat.find(Kind::Agent, "pm").unwrap().body, "prompt\n");
        assert!(
            cat.problems.iter().any(|p| p.path.ends_with("hooks")),
            "{:?}",
            cat.problems
        );
    }

    /// A `git` the tests drive themselves, isolated exactly like the one
    /// `git_output` runs (`test_git_isolation`) — per command, so nothing
    /// here touches the process environment other threads are reading.
    fn test_git(dir: &std::path::Path, args: &[&str]) -> std::process::Command {
        let mut cmd = std::process::Command::new("git");
        cmd.args(args).current_dir(dir);
        super::test_git_isolation(&mut cmd);
        cmd
    }

    fn git_run(dir: &std::path::Path, args: &[&str]) {
        let out = test_git(dir, args).output().unwrap();
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn init_repo(root: &std::path::Path) {
        git_run(root, &["init", "-q", "-b", "main"]);
    }

    fn commit_author_email(root: &std::path::Path) -> String {
        let out = test_git(root, &["log", "-1", "--format=%ae"])
            .output()
            .unwrap();
        assert!(out.status.success());
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    #[test]
    fn commit_uses_fallback_identity_when_unset() {
        let root = tmp("commit-fallback");
        init_repo(&root);
        write(&root, "catalog.yaml", "schema_version: 1\n");
        assert!(!has_identity(&root));

        stage_paths(&root, &[]).unwrap();
        let head_sha = commit(&root, "catalog: init").unwrap();
        assert_eq!(head_sha.len(), 40);
        assert_eq!(commit_author_email(&root), "fleet@localhost");
    }

    #[test]
    fn commit_keeps_configured_identity() {
        let root = tmp("commit-configured");
        init_repo(&root);
        git_run(&root, &["config", "user.email", "dev@example.com"]);
        git_run(&root, &["config", "user.name", "Dev"]);
        write(&root, "catalog.yaml", "schema_version: 1\n");
        assert!(has_identity(&root));

        stage_paths(&root, &[]).unwrap();
        commit(&root, "catalog: init").unwrap();
        assert_eq!(commit_author_email(&root), "dev@example.com");
    }

    #[test]
    fn stage_paths_then_commit_returns_head() {
        let root = tmp("stage-commit");
        init_repo(&root);
        git_run(&root, &["config", "user.email", "dev@example.com"]);
        git_run(&root, &["config", "user.name", "Dev"]);
        write(&root, "catalog.yaml", "schema_version: 1\n");
        write(&root, "hooks/keep.yaml", "kind: hook\n");
        write(&root, "hooks/ignored.yaml", "kind: hook\n");

        stage_paths(
            &root,
            &["catalog.yaml".to_string(), "hooks/keep.yaml".to_string()],
        )
        .unwrap();
        let head_sha = commit(&root, "catalog: partial").unwrap();
        assert_eq!(head_sha.len(), 40);
        assert_eq!(head(&root).unwrap(), head_sha);

        // Only the staged paths were committed; the unstaged file is still
        // untracked, so the tree remains dirty.
        let status = git_status(&root).unwrap();
        assert_eq!(status.dirty, 1);

        // Nothing left to stage/commit for the already-committed paths.
        let err = commit(&root, "catalog: nothing").unwrap_err();
        assert_eq!(err.code, "E_CATALOG_GIT");
    }

    /// `has_staged` is what lets a caller tell "this write changed nothing"
    /// from "the tree is dirty somewhere else": it looks only at the index,
    /// and only at the paths it is given.
    #[test]
    fn has_staged_is_scoped_to_its_paths() {
        let root = tmp("has-staged");
        init_repo(&root);
        git_run(&root, &["config", "user.email", "dev@example.com"]);
        git_run(&root, &["config", "user.name", "Dev"]);

        // A repo with no commits at all: an empty index stages nothing.
        assert!(!has_staged(&root, &[]).unwrap());
        write(&root, "catalog.yaml", "schema_version: 1\n");
        stage_paths(&root, &[]).unwrap();
        assert!(has_staged(&root, &[]).unwrap());
        commit(&root, "catalog: init").unwrap();
        assert!(!has_staged(&root, &[]).unwrap());

        // Re-writing identical content stages nothing…
        write(&root, "catalog.yaml", "schema_version: 1\n");
        write(&root, "hooks/other.yaml", "kind: hook\n");
        stage_paths(&root, &["catalog.yaml".to_string()]).unwrap();
        assert!(!has_staged(&root, &["catalog.yaml".to_string()]).unwrap());
        // …even though another path is genuinely dirty and unstaged.
        assert_eq!(git_status(&root).unwrap().dirty, 1);
        assert!(!has_staged(&root, &[]).unwrap());

        // A real change to the scoped path shows up; an unrelated one does not.
        write(&root, "catalog.yaml", "schema_version: 1\n# changed\n");
        stage_paths(&root, &["catalog.yaml".to_string()]).unwrap();
        assert!(has_staged(&root, &["catalog.yaml".to_string()]).unwrap());
        assert!(!has_staged(&root, &["hooks/other.yaml".to_string()]).unwrap());
        assert!(!has_staged(&root, &["nosuch".to_string()]).unwrap());
    }

    #[test]
    fn changes_group_per_asset_and_draft_a_message() {
        let porcelain = "M skills/notes/body.md\n M skills/notes/asset.yaml\n?? commands/ship/\n D hooks/stop.yaml\n M catalog.yaml\nR  mcp/a.yaml -> mcp/b.yaml";
        let c = changes_of(porcelain);
        let got: Vec<(&str, &str)> = c
            .iter()
            .map(|c| (c.path.as_str(), c.status.as_str()))
            .collect();
        assert_eq!(
            got,
            vec![
                ("skills/notes", "M"),
                ("commands/ship", "A"),
                ("hooks/stop", "D"),
                ("catalog.yaml", "M"),
                ("mcp/b", "M"),
            ]
        );
        assert_eq!(
            commit_message_for(&c),
            "catalog: update skills/notes, commands/ship, hooks/stop (+2)"
        );
        assert_eq!(commit_message_for(&c[1..2]), "catalog: add commands/ship");
        assert_eq!(commit_message_for(&[]), "catalog: commit pending changes");
    }

    #[test]
    fn git_status_counts_dirty_and_ahead() {
        let root = tmp("status");
        init_repo(&root);
        git_run(&root, &["config", "user.email", "dev@example.com"]);
        git_run(&root, &["config", "user.name", "Dev"]);
        write(&root, "catalog.yaml", "schema_version: 1\n");
        stage_paths(&root, &[]).unwrap();
        commit(&root, "catalog: init").unwrap();

        let status = git_status(&root).unwrap();
        assert!(!status.has_upstream);
        assert_eq!(status.ahead, None);
        assert_eq!(status.behind, None);
        assert_eq!(status.dirty, 0);

        write(&root, "hooks/x.yaml", "kind: hook\n");
        let status = git_status(&root).unwrap();
        assert_eq!(status.dirty, 1);
        assert_eq!(
            status.changes,
            vec![RepoChange {
                path: "hooks/x".into(),
                status: "A".into()
            }]
        );

        let remote = tmp("status-remote");
        git_run(&remote, &["init", "-q", "--bare", "-b", "main"]);
        git_run(
            &root,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        );
        git_run(&root, &["push", "-q", "-u", "origin", "main"]);

        stage_paths(&root, &[]).unwrap();
        commit(&root, "catalog: second").unwrap();
        let status = git_status(&root).unwrap();
        assert!(status.has_upstream);
        assert_eq!(status.ahead, Some(1));
        assert_eq!(status.behind, Some(0));
        assert_eq!(status.dirty, 0);
    }

    #[test]
    fn remove_asset_deletes_folder_and_file_kinds() {
        let root = tmp("remove");
        write(
            &root,
            "skills/s/asset.yaml",
            "kind: skill\nname: s\ndescription: d\n",
        );
        write(&root, "skills/s/body.md", "b\n");
        write(&root, "skills/s/resources/x.txt", "x");
        write(&root, "hooks/h.yaml", "kind: hook\nname: h\ndescription: d\nevent: stop\naction: { type: command, command: x }\n");

        let mut removed = remove_asset(&root, Kind::Skill, "s").unwrap();
        removed.sort();
        assert!(!root.join("skills/s").exists());
        assert_eq!(
            removed,
            vec![
                "skills/s/asset.yaml".to_string(),
                "skills/s/body.md".to_string(),
                "skills/s/resources/x.txt".to_string(),
            ]
        );

        let removed = remove_asset(&root, Kind::Hook, "h").unwrap();
        assert!(!root.join("hooks/h.yaml").exists());
        assert_eq!(removed, vec!["hooks/h.yaml".to_string()]);

        let err = remove_asset(&root, Kind::Skill, "missing").unwrap_err();
        assert_eq!(err.code, "E_ASSET_NOT_FOUND");
        let err = remove_asset(&root, Kind::Hook, "missing").unwrap_err();
        assert_eq!(err.code, "E_ASSET_NOT_FOUND");
    }

    #[cfg(unix)]
    #[test]
    fn remove_asset_refuses_symlinked_targets() {
        let root = tmp("remove-symlink");
        let outside = tmp("remove-symlink-outside");
        write(&outside, "keep.txt", "keep");

        // A folder-kind asset whose directory is actually a symlink out of
        // the repo: must be refused, never unlinked or walked into.
        fs::create_dir_all(root.join("skills")).unwrap();
        std::os::unix::fs::symlink(&outside, root.join("skills/link")).unwrap();
        let err = remove_asset(&root, Kind::Skill, "link").unwrap_err();
        assert_eq!(err.code, "E_INVALID");
        assert!(root.join("skills/link").exists());
        assert!(outside.join("keep.txt").exists());

        // Same guard for a single-file kind whose yaml is a symlink.
        let outside_file = outside.join("keep.txt");
        fs::create_dir_all(root.join("hooks")).unwrap();
        std::os::unix::fs::symlink(&outside_file, root.join("hooks/h.yaml")).unwrap();
        let err = remove_asset(&root, Kind::Hook, "h").unwrap_err();
        assert_eq!(err.code, "E_INVALID");
        assert!(root.join("hooks/h.yaml").exists());
        assert!(outside_file.exists());
    }

    #[test]
    fn write_asset_rejects_invalid_resource_paths() {
        let root = tmp("invalid-resource");
        let mut a = Asset::from_yaml(None, "kind: skill\nname: s\ndescription: d\n").unwrap();
        a.body = "b\n".into();

        for bad in [
            "not-resources/x.txt",     // must live under resources/
            "resources/../escape.txt", // .. segment
            "resources//x.txt",        // empty segment
            "resources/x y.txt",       // space: outside [A-Za-z0-9._/-]
            "resources/héllo.txt",     // non-ASCII: outside [A-Za-z0-9._/-]
        ] {
            a.resources = vec![Resource {
                rel_path: bad.into(),
                bytes: b"x".to_vec(),
            }];
            let err = write_asset(&root, &a, false).unwrap_err();
            assert_eq!(err.code, "E_INVALID", "path: {bad}");
        }
        // None of the rejected writes touched disk.
        assert!(!root.join("skills/s").exists());

        a.resources = vec![Resource {
            rel_path: "resources/ok.txt".into(),
            bytes: b"ok".to_vec(),
        }];
        write_asset(&root, &a, false).unwrap();
        assert!(root.join("skills/s/resources/ok.txt").exists());
    }

    #[test]
    fn write_asset_overwrite_prunes_stale_resources() {
        let root = tmp("prune");
        write(&root, "catalog.yaml", "schema_version: 1\n");
        let mut a = Asset::from_yaml(None, "kind: skill\nname: s\ndescription: d\n").unwrap();
        a.body = "b\n".into();
        a.resources = vec![
            Resource {
                rel_path: "resources/keep.txt".into(),
                bytes: b"keep".to_vec(),
            },
            Resource {
                rel_path: "resources/sub/drop.txt".into(),
                bytes: b"drop".to_vec(),
            },
        ];
        write_asset(&root, &a, false).unwrap();
        assert!(root.join("skills/s/resources/sub/drop.txt").exists());

        a.resources = vec![Resource {
            rel_path: "resources/keep.txt".into(),
            bytes: b"keep2".to_vec(),
        }];
        write_asset(&root, &a, true).unwrap();

        assert!(root.join("skills/s/resources/keep.txt").exists());
        assert!(!root.join("skills/s/resources/sub/drop.txt").exists());
        assert!(!root.join("skills/s/resources/sub").exists());

        let cat = load_dir(&root).unwrap();
        let skill = cat.find(Kind::Skill, "s").unwrap();
        assert_eq!(skill.resources.len(), 1);
        assert_eq!(skill.resources[0].rel_path, "resources/keep.txt");
        assert_eq!(
            fs::read(root.join("skills/s/resources/keep.txt")).unwrap(),
            b"keep2"
        );
    }

    /// Carry 2 (Rulings R24): a problem holds the asset its path names, or a
    /// whole kind when the kind's directory could not be read; layer,
    /// catalog-file and absolute (problem-entry) paths hold nothing.
    #[test]
    fn problem_holds_name_the_asset_or_the_kind_a_problem_is_about() {
        let p = |path: &str| Problem {
            path: path.into(),
            message: format!("bad {path}"),
        };
        let h = ProblemHolds::from_problems(&[
            p("skills/broken/asset.yaml"),
            p("hooks/stop.yaml"),
            p("agents"),
            p("layers/core.yaml"),
            p("layers"),
            p("/abs/repo"),
        ]);
        assert_eq!(
            h.reason(Kind::Skill, "broken"),
            Some("bad skills/broken/asset.yaml")
        );
        assert_eq!(h.reason(Kind::Hook, "stop"), Some("bad hooks/stop.yaml"));
        assert_eq!(h.reason(Kind::Agent, "anything"), Some("bad agents"));
        assert_eq!(h.reason(Kind::Skill, "fine"), None);
        assert_eq!(h.assets.len() + h.kinds.len(), 3);
    }

    /// The loader's own paths: a skill whose asset.yaml does not parse.
    #[test]
    fn load_dir_problems_become_holds() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("catalog.yaml"), "schema_version: 1\n").unwrap();
        std::fs::create_dir_all(root.path().join("skills/broken")).unwrap();
        std::fs::write(
            root.path().join("skills/broken/asset.yaml"),
            "kind: skill\nname: [\n",
        )
        .unwrap();
        let cat = load_dir(root.path()).unwrap();
        let h = ProblemHolds::from_problems(&cat.problems);
        assert!(
            h.reason(Kind::Skill, "broken").is_some(),
            "{:?}",
            cat.problems
        );
    }

    /// Final review I6: an entry of a folder kind that cannot be read as a
    /// folder — a dangling symlink, a symlink to a file — is a Problem at
    /// `<kind dir>/<name>/asset.yaml`, so it holds that one asset instead
    /// of vanishing; a plain stray file there is still skipped.
    #[cfg(unix)]
    #[test]
    fn a_dangling_skill_symlink_is_a_problem_that_holds_the_skill() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path();
        std::fs::write(root.join("catalog.yaml"), "schema_version: 1\n").unwrap();
        std::fs::create_dir_all(root.join("skills")).unwrap();
        std::os::unix::fs::symlink(root.join("nowhere"), root.join("skills/dead")).unwrap();
        std::fs::write(root.join("a-file"), "x").unwrap();
        std::os::unix::fs::symlink(root.join("a-file"), root.join("skills/to-file")).unwrap();
        std::fs::write(root.join("skills/README.md"), "notes").unwrap();
        let cat = load_dir(root).unwrap();
        let h = ProblemHolds::from_problems(&cat.problems);
        assert!(
            h.reason(Kind::Skill, "dead").is_some(),
            "{:?}",
            cat.problems
        );
        assert!(
            h.reason(Kind::Skill, "to-file").is_some(),
            "{:?}",
            cat.problems
        );
        assert!(h.kinds.is_empty(), "{:?}", cat.problems);
        assert_eq!(cat.problems.len(), 2, "{:?}", cat.problems);
    }

    /// Assets M4: the apply engine's git steps — a clean check that counts
    /// untracked files, a reset that drops this apply's commit and strays,
    /// and a revert that keeps history and takes only a hex sha.
    #[test]
    fn is_clean_reset_hard_and_revert() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        git(root, &["init", "-q", "-b", "main"]).unwrap();
        std::fs::write(root.join("catalog.yaml"), "schema_version: 1\n").unwrap();
        stage_paths(root, &[]).unwrap();
        let base = commit(root, "init").unwrap();
        assert!(is_clean(root).unwrap());
        std::fs::write(root.join("a.txt"), "a\n").unwrap();
        assert!(!is_clean(root).unwrap(), "an untracked file is not clean");
        stage_paths(root, &[]).unwrap();
        commit(root, "add a").unwrap();
        std::fs::write(root.join("stray.txt"), "x\n").unwrap();
        reset_hard(root, &base).unwrap();
        assert_eq!(head(root).unwrap(), base);
        assert!(is_clean(root).unwrap());
        assert!(!root.join("a.txt").exists() && !root.join("stray.txt").exists());

        std::fs::write(root.join("a.txt"), "a\n").unwrap();
        stage_paths(root, &[]).unwrap();
        let added = commit(root, "add a").unwrap();
        let reverted = revert(root, &added).unwrap();
        assert_ne!(reverted, added);
        assert!(!root.join("a.txt").exists());
        assert!(
            revert(root, "--help").is_err(),
            "only a hex sha reaches git"
        );
    }

    /// Assets M4 undo's git helpers: the files a commit wrote, ancestry,
    /// and a conflicting revert aborted with no revert left in progress.
    #[test]
    fn files_between_ancestry_and_an_aborted_revert() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        git(root, &["init", "-q", "-b", "main"]).unwrap();
        std::fs::write(root.join("catalog.yaml"), "schema_version: 1\n").unwrap();
        stage_paths(root, &[]).unwrap();
        let base = commit(root, "init").unwrap();
        std::fs::create_dir_all(root.join("skills/w")).unwrap();
        std::fs::write(root.join("skills/w/body.md"), "a\n").unwrap();
        stage_paths(root, &[]).unwrap();
        let added = commit(root, "add w").unwrap();
        assert_eq!(
            files_between(root, &base, &added)
                .unwrap()
                .into_iter()
                .collect::<Vec<_>>(),
            ["skills/w/body.md"]
        );
        assert!(is_ancestor(root, &base, &added).unwrap());
        assert!(!is_ancestor(root, &added, &base).unwrap());
        assert!(
            !is_ancestor(root, &"d".repeat(40), &added).unwrap(),
            "a commit the repo does not have is in no history"
        );
        assert!(files_deleted_between(root, &base, &added)
            .unwrap()
            .is_empty());
        assert_eq!(
            files_deleted_between(root, &added, &base)
                .unwrap()
                .into_iter()
                .collect::<Vec<_>>(),
            ["skills/w/body.md"]
        );
        assert!(
            is_ancestor(root, "--help", &added).is_err(),
            "only hex shas"
        );

        std::fs::write(root.join("skills/w/body.md"), "b\n").unwrap();
        stage_paths(root, &[]).unwrap();
        let edited = commit(root, "edit w").unwrap();
        assert!(revert(root, &added).is_err(), "modify/delete conflicts");
        assert!(!revert_in_progress(root));
        assert_eq!(head(root).unwrap(), edited);
        assert!(is_clean(root).unwrap());
    }

    /// The failure reset never deletes what git ignores (controller ruling
    /// on R12: `clean -fd`, never `-x`).
    #[test]
    fn reset_hard_keeps_ignored_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        git(root, &["init", "-q", "-b", "main"]).unwrap();
        std::fs::write(root.join(".gitignore"), "*.local\n").unwrap();
        stage_paths(root, &[]).unwrap();
        let base = commit(root, "init").unwrap();
        std::fs::write(root.join("keep.local"), "mine\n").unwrap();
        assert!(is_clean(root).unwrap(), "an ignored file is not a change");
        reset_hard(root, &base).unwrap();
        assert!(root.join("keep.local").is_file());
    }

    #[test]
    fn read_asset_reads_one_asset_back() {
        let dir = tempfile::tempdir().unwrap();
        let a = Asset::from_yaml(None, "kind: skill\nname: w\ndescription: d\n").unwrap();
        write_asset(dir.path(), &a, false).unwrap();
        assert_eq!(
            read_asset(dir.path(), Kind::Skill, "w")
                .unwrap()
                .header
                .name,
            "w"
        );
        assert!(read_asset(dir.path(), Kind::Skill, "nope").is_err());
    }

    /// Task 6 review (Critical): a repo or user config that hides untracked
    /// files must not make a tree with someone's new file look clean.
    #[test]
    fn is_clean_sees_untracked_files_whatever_the_config_says() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        git(root, &["init", "-q", "-b", "main"]).unwrap();
        std::fs::write(root.join("catalog.yaml"), "schema_version: 1\n").unwrap();
        stage_paths(root, &[]).unwrap();
        commit(root, "init").unwrap();
        git(root, &["config", "status.showUntrackedFiles", "no"]).unwrap();
        std::fs::create_dir_all(root.join("skills/w")).unwrap();
        std::fs::write(root.join("skills/w/asset.yaml"), "x\n").unwrap();
        assert!(
            git(root, &["status", "--porcelain"]).unwrap().is_empty(),
            "the config hides it from a plain status"
        );
        assert!(!is_clean(root).unwrap());
        assert_eq!(changed_paths(root).unwrap(), ["skills/w/asset.yaml"]);
    }

    /// Task 6 review (PF7 "never", rounds 1–2): an operation commits only
    /// the FILES it wrote; the guard names every other file — a stray next
    /// to ours and an ignored one included — and the reset puts back only
    /// our files, removing a directory only once it is empty.
    #[test]
    fn commit_paths_foreign_changes_and_reset_paths_keep_to_our_files() {
        use std::collections::BTreeSet;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        git(root, &["init", "-q", "-b", "main"]).unwrap();
        std::fs::write(root.join("catalog.yaml"), "schema_version: 1\n").unwrap();
        std::fs::write(root.join(".gitignore"), "*.local\n").unwrap();
        std::fs::create_dir_all(root.join("skills/w")).unwrap();
        std::fs::write(root.join("skills/w/asset.yaml"), "old\n").unwrap();
        std::fs::write(root.join("skills/w/gone.md"), "tracked\n").unwrap();
        stage_paths(root, &[]).unwrap();
        let base = commit(root, "init").unwrap();

        let set = |v: &[&str]| -> BTreeSet<String> { v.iter().map(|s| s.to_string()).collect() };
        let ours = set(&[
            "skills/w/asset.yaml",
            "skills/w/gone.md",
            "skills/w/body.md",
            "layers/core.yaml",
        ]);
        std::fs::write(root.join("skills/w/asset.yaml"), "new\n").unwrap();
        std::fs::remove_file(root.join("skills/w/gone.md")).unwrap();
        std::fs::write(root.join("skills/w/body.md"), "b\n").unwrap();
        std::fs::create_dir_all(root.join("layers")).unwrap();
        std::fs::write(root.join("layers/core.yaml"), "l\n").unwrap();
        std::fs::write(root.join("foreign.txt"), "theirs\n").unwrap();
        std::fs::write(root.join("skills/w/notes.md"), "theirs too\n").unwrap();
        let mine = commit_paths(root, "fleet: x", &ours).unwrap().unwrap();
        let files = git(root, &["show", "--name-status", "--format=", "HEAD"]).unwrap();
        assert!(files.contains("D\tskills/w/gone.md"), "{files}");
        assert!(files.contains("A\tskills/w/body.md") && files.contains("A\tlayers/core.yaml"));
        assert!(
            !files.contains("notes.md") && !files.contains("foreign.txt"),
            "{files}"
        );

        std::fs::write(root.join("skills/w/cache.local"), "ignored\n").unwrap();
        let foreign = foreign_changes(root, &[&base, &mine], &ours).unwrap();
        assert_eq!(
            foreign,
            ["foreign.txt", "skills/w/cache.local", "skills/w/notes.md"]
        );
        let moved = foreign_changes(root, &[&base], &ours).unwrap();
        assert!(moved[0].starts_with("HEAD moved"), "{moved:?}");

        // A reset is only for a clean guard; even forced, it keeps to our
        // files: the stray, the ignored file and the foreign file stay.
        reset_paths(root, &base, &ours, &["layers".to_string()]).unwrap();
        assert_eq!(head(root).unwrap(), base);
        assert_eq!(
            std::fs::read_to_string(root.join("skills/w/asset.yaml")).unwrap(),
            "old\n"
        );
        assert!(
            root.join("skills/w/gone.md").is_file(),
            "a deleted file is back"
        );
        assert!(!root.join("skills/w/body.md").exists());
        assert!(!root.join("layers").exists(), "an emptied created dir goes");
        assert!(
            root.join("skills/w/notes.md").is_file(),
            "the stray is kept"
        );
        assert!(
            root.join("skills/w/cache.local").is_file(),
            "ignored is kept"
        );
        assert_eq!(
            changed_paths(root).unwrap(),
            ["foreign.txt", "skills/w/notes.md"]
        );
        assert!(commit_paths(root, "noop", &ours).unwrap().is_none());
    }

    /// Round 3: an existing file is "unchanged" only while it is the blob
    /// `rev` tracks; paths are literal, never patterns; a commit's parent.
    #[test]
    fn unchanged_since_literal_pathspecs_and_parent_of() {
        use std::collections::BTreeSet;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        git(root, &["init", "-q", "-b", "main"]).unwrap();
        std::fs::write(root.join("catalog.yaml"), "schema_version: 1\n").unwrap();
        std::fs::write(root.join(".gitignore"), "*.local\n").unwrap();
        stage_paths(root, &[]).unwrap();
        let base = commit(root, "init").unwrap();
        assert!(unchanged_since(root, &base, "catalog.yaml").unwrap());
        std::fs::write(root.join("catalog.yaml"), "edited\n").unwrap();
        assert!(!unchanged_since(root, &base, "catalog.yaml").unwrap());
        std::fs::write(root.join("x.local"), "ignored\n").unwrap();
        assert!(!unchanged_since(root, &base, "x.local").unwrap());
        assert!(!unchanged_since(root, &base, "absent.txt").unwrap());
        std::fs::write(root.join("catalog.yaml"), "schema_version: 1\n").unwrap();

        std::fs::create_dir_all(root.join("layers")).unwrap();
        std::fs::write(root.join("layers/[a].yaml"), "ours\n").unwrap();
        std::fs::write(root.join("layers/a.yaml"), "theirs\n").unwrap();
        let ours: BTreeSet<String> = ["layers/[a].yaml".to_string()].into();
        let mine = commit_paths(root, "fleet: x", &ours).unwrap().unwrap();
        let files = git(root, &["show", "--name-only", "--format=", "HEAD"]).unwrap();
        assert_eq!(files, "layers/[a].yaml", "a bracket is not a glob");
        assert_eq!(parent_of(root, &mine).unwrap(), base);
        assert!(parent_of(root, "--help").is_err());
    }
}
