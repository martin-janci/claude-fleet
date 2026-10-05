//! File downloads: a file a session made on its host, copied to this
//! machine so a phone or desktop can fetch it
//! (`docs/superpowers/specs/2026-10-03-file-downloads-design.md`).
//!
//! [`send`] checks the file over SSH (or the agent), keeps the byte budget,
//! inserts the row in state `fetching` and returns it; [`spawn_fetch`]
//! pulls the bytes in [`CHUNK_BYTES`] slices — the move carry's
//! `chunk_script` / `payload`, so a login banner is never part of the file
//! — into `<data dir>/downloads/<id>`, hashing as it writes. A client reads
//! the rows with [`list`] and the bytes with `GET /downloads/<id>`
//! ([`open_ready`]). [`sweep`] drops what is past `downloads.keep_secs`.
//!
//! Scope: the master and an unbound client see every row; a client bound
//! to an org sees its org's; a per-host token sees, and sends from, its own
//! host only — a host's Claude can read any file there already, but never
//! one on another host.

use crate::ipc_error::{codes, IpcError};
use crate::service::move_session::carry;
use crate::service::settings;
use crate::service::view_scope::ViewScope;
use crate::shell::quote;
use crate::ssh::SshExec;
use crate::store::{DownloadRow, NewDownload, Store};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

/// `downloads.source` for a host's own token or the control API.
pub const SOURCE_AGENT: &str = "agent";
/// `downloads.source` for a person's paired device or the desktop.
pub const SOURCE_PERSON: &str = "person";

/// One chunk of the copy (the move carry's size).
pub const CHUNK_BYTES: u64 = carry::CHUNK_BYTES;
/// Wall clock for one chunk's round trip.
const CHUNK_TIMEOUT: Duration = Duration::from_secs(180);
/// Wall clock for the `stat` before anything is inserted.
const STAT_TIMEOUT: Duration = Duration::from_secs(30);
/// A note longer than this is cut.
pub const NOTE_MAX_CHARS: usize = 500;
/// The longest path `send` accepts.
pub const PATH_MAX_BYTES: usize = 4096;
/// The default and largest page `list` returns.
pub const LIST_LIMIT: usize = 200;

const MIB: u64 = 1024 * 1024;

static DIR: OnceLock<PathBuf> = OnceLock::new();

/// Make `<data_dir>/downloads` (mode 0700) the place copies live, and fail
/// every row a previous process left `fetching`. Once per process; a second
/// call keeps the first directory.
pub fn init(data_dir: &Path, store: &Store) -> std::io::Result<PathBuf> {
    let dir = data_dir.join("downloads");
    std::fs::create_dir_all(&dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    }
    let dir = DIR.get_or_init(|| dir).clone();
    if let Err(e) = store.fail_interrupted_downloads() {
        tracing::warn!(error = %e, "[downloads] could not fail interrupted copies");
    }
    Ok(dir)
}

fn dir() -> Result<&'static Path, IpcError> {
    DIR.get().map(PathBuf::as_path).ok_or_else(|| {
        IpcError::new(
            codes::E_UNSUPPORTED,
            "downloads are not set up on this machine",
        )
    })
}

/// Where download `id`'s bytes live once it is ready.
pub fn file_of(id: i64) -> Result<PathBuf, IpcError> {
    Ok(dir()?.join(id.to_string()))
}

fn part_of(id: i64) -> Result<PathBuf, IpcError> {
    Ok(dir()?.join(format!("{id}.part")))
}

/// Whether `scope` may see `row`: its org boundary, a per-host token's own
/// host only, and — multi-user M1 — the PERSON half, asked of the session
/// the file came out of **at the `own` tier**.
///
/// **Why `may_own` and not `sees_session_row`.** A download is a file read
/// off the session's host at an **unconstrained absolute path**: [`send`]
/// resolves a RELATIVE path against the pane's `pwd`, but an ABSOLUTE one is
/// taken as given — [`parse_stat`]'s success condition is
/// `path.starts_with('/')`, with no canonicalisation against a root and no
/// `starts_with(worktree)` check. So a download may be
/// `~/.claude/.credentials.json` as easily as `out/report.pdf`, and
/// [`crate::mcp::downloads_route`] hands its BYTES to whoever passes this
/// predicate. An unconstrained read of the owner's host is a subset of what
/// a terminal gives, and spec §4.3 invariant 5 — *sharing never confers a
/// terminal* — exists to refuse that class, so a `watch` or even a `drive`
/// grantee must not reach it. `list_downloads` is the INDEX into those bytes
/// (its rows carry the absolute path) and `remove_download` destroys the
/// owner's copy, so all three sit on the same tier.
///
/// **`repo_file` is not the precedent it looks like**: that one is confined
/// to the worktree and this is not. If somebody who owns downloads later
/// confines the path to the session's tree, `Reach::Read` becomes defensible
/// and this predicate should be revisited. The reason it is not done here is
/// that confining the path redesigns main's feature rather than fencing it,
/// and an owner sending `/var/log/...` off their own machine is a use that
/// survives intact under `own`.
///
/// When the session row is GONE (reaped, or a `session_id` of `None`) there
/// is no person to ask and `DownloadRow.org_id` — recorded for exactly this
/// case — is the whole of the fence: the org answer alone. The residue is
/// BOUNDED rather than merely acknowledged: [`send`] is the `own` tier too,
/// so every row that ever existed was created by the session's owner or by
/// its own Claude, never by a grantee. A person's own device keeps seeing
/// files it was sent; nobody inherits a file a grantee extracted, because a
/// grantee can extract none.
pub fn visible(s: &Store, scope: &ViewScope, row: &DownloadRow) -> bool {
    if scope.host.as_deref().is_some_and(|h| h != row.host_alias) {
        return false;
    }
    match row
        .session_id
        .and_then(|id| s.get_session_by_id(id).ok().flatten())
    {
        // The composition: `may_own` opens with `sees_session_row`, which
        // opens with the org clause, so this one call is the org boundary,
        // the person fence and the tier.
        Some(sess) => scope.may_own(&sess),
        // ORG-AUTHORITY question only, and the org claim is honest: nothing
        // is left that names a person. `not a privacy fence` — the person
        // half is `may_own` in the arm above, and a reaped session leaves
        // none to ask.
        None => scope.org.sees_session_org_only(&row.host_alias, row.org_id),
    }
}

/// The budget, as `list` reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Budget {
    pub max_file: u64,
    pub max_total: u64,
    /// 0 keeps a download until it is removed.
    pub keep_secs: u64,
}

impl Budget {
    pub fn from_store(s: &Store) -> Self {
        let mb = |k| settings::get_string(s, k).parse::<u64>().unwrap_or(1);
        Budget {
            max_file: mb(settings::DOWNLOADS_MAX_FILE_MB) * MIB,
            max_total: mb(settings::DOWNLOADS_MAX_TOTAL_MB) * MIB,
            keep_secs: settings::get_secs(s, settings::DOWNLOADS_KEEP_SECS),
        }
    }

    fn expires_at(&self, row: &DownloadRow) -> Option<i64> {
        if self.keep_secs == 0 || row.state == "fetching" {
            return None;
        }
        Some(row.ready_at.unwrap_or(row.at) + self.keep_secs as i64)
    }
}

/// One row as a client receives it: `expires_at` filled in.
fn present(b: &Budget, mut row: DownloadRow) -> DownloadRow {
    row.expires_at = b.expires_at(&row);
    row
}

// ---- send ------------------------------------------------------------------

/// Arguments of `send_file`.
#[derive(Debug, Clone, serde::Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct SendFileArgs {
    /// The session whose host holds the file (yours: `whoami`).
    pub session_id: i64,
    /// Absolute, or relative to the session's worktree root.
    pub path: String,
    /// One line for the person: what the file is.
    #[serde(default)]
    pub note: Option<String>,
}

/// The file, as the host reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stat {
    pub size: u64,
    /// Absolute, with the directory's symlinks resolved.
    pub path: String,
}

const STAT_ERR: &str = "__CF_DL_ERR__";

/// Print the file's size and absolute path after the carry's marker, or a
/// one-word refusal. A relative path is taken from the session's worktree
/// root — the git top level of the pane's directory, as `repo_file` reads
/// it — or the pane's directory itself outside git.
pub fn stat_script(tmux_name: &str, path: &str) -> String {
    let target = quote(&crate::tmux::exact_pane(tmux_name));
    format!(
        r#"set +e
p={p}
case "$p" in
  /*) ;;
  *) d=$(tmux display-message -t {target} -p '#{{pane_current_path}}' 2>/dev/null)
     [ -n "$d" ] || {{ printf '{STAT_ERR} nocwd\n'; exit 0; }}
     r=$(cd "$d" 2>/dev/null && git rev-parse --show-toplevel 2>/dev/null) || r=$d
     [ -n "$r" ] || r=$d
     p="$r/$p" ;;
esac
[ -d "$p" ] && {{ printf '{STAT_ERR} dir\n'; exit 0; }}
[ -f "$p" ] && [ -r "$p" ] || {{ printf '{STAT_ERR} missing\n'; exit 0; }}
n=$(wc -c < "$p" | tr -d ' ')
a=$(cd "$(dirname -- "$p")" && pwd -P)/$(basename -- "$p")
printf '\n{marker}\n%s\n%s\n' "$n" "$a"
"#,
        p = quote(path),
        marker = carry::OUT_MARKER,
    )
}

/// Read [`stat_script`]'s answer.
pub fn parse_stat(stdout: &[u8], asked: &str) -> Result<Stat, IpcError> {
    let text = String::from_utf8_lossy(stdout);
    if let Some(i) = text.find(STAT_ERR) {
        let why = text[i + STAT_ERR.len()..]
            .split_whitespace()
            .next()
            .unwrap_or("");
        return Err(match why {
            "dir" => IpcError::new(
                codes::E_INVALID,
                format!("{asked} is a folder; zip it and send the archive"),
            ),
            "nocwd" => IpcError::new(
                codes::E_INVALID,
                format!(
                    "{asked} is relative and the session's working directory is unknown; give an absolute path"
                ),
            ),
            _ => IpcError::new(
                codes::E_NOTFOUND,
                format!("{asked} is not a readable file"),
            ),
        });
    }
    let body = carry::payload(stdout)
        .map(String::from_utf8_lossy)
        .ok_or_else(|| carry::parse_err("stat", &text))?;
    let mut lines = body.lines();
    let size = lines.next().and_then(|n| n.trim().parse::<u64>().ok());
    let path = lines.next().map(str::to_string);
    match (size, path) {
        (Some(size), Some(path)) if path.starts_with('/') => Ok(Stat { size, path }),
        _ => Err(carry::parse_err("stat", &text)),
    }
}

fn check_path(path: &str) -> Result<(), IpcError> {
    if path.trim().is_empty() {
        return Err(IpcError::new(codes::E_INVALID, "path is empty"));
    }
    if path.contains('\0') || path.contains('\n') {
        return Err(IpcError::new(
            codes::E_INVALID,
            "path has a control character",
        ));
    }
    if path.len() > PATH_MAX_BYTES {
        return Err(IpcError::new(codes::E_INVALID, "path is too long"));
    }
    Ok(())
}

fn clean_note(note: Option<&str>) -> Option<String> {
    let n: String = note?
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(NOTE_MAX_CHARS)
        .collect();
    let n = n.trim();
    (!n.is_empty()).then(|| n.to_string())
}

fn base_name(path: &str) -> String {
    path.rsplit('/')
        .find(|s| !s.is_empty())
        .unwrap_or("file")
        .to_string()
}

fn too_large(size: u64, cap: u64, what: &str) -> IpcError {
    IpcError::new(
        codes::E_LIMIT,
        format!(
            "the file is {} — {what} is {}",
            crate::service::attachments::fmt_bytes(size),
            crate::service::attachments::fmt_bytes(cap)
        ),
    )
}

/// Make room for `size` more bytes: refuse a file over the per-file cap,
/// then drop the oldest ready (and failed) downloads until everything kept
/// fits. Refused when even that is not enough — copies still in flight
/// count and are never dropped.
fn make_room(s: &Store, b: &Budget, size: u64) -> Result<Vec<i64>, IpcError> {
    if size > b.max_file {
        return Err(too_large(size, b.max_file, "the limit per file"));
    }
    let rows = s.downloads()?;
    let mut used: u64 = rows
        .iter()
        .filter(|r| r.state != "failed")
        .map(|r| r.size.max(0) as u64)
        .sum();
    let mut evict = Vec::new();
    // Oldest first; `downloads()` is newest first.
    for r in rows.iter().rev() {
        if used.saturating_add(size) <= b.max_total {
            break;
        }
        if r.state == "ready" {
            used = used.saturating_sub(r.size.max(0) as u64);
            evict.push(r.id);
        }
    }
    if used.saturating_add(size) > b.max_total {
        return Err(too_large(size, b.max_total, "the space left for downloads"));
    }
    Ok(evict)
}

fn remove_files(id: i64) {
    for p in [file_of(id), part_of(id)].into_iter().flatten() {
        match std::fs::remove_file(&p) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => tracing::warn!(path = %p.display(), error = %e, "[downloads] remove failed"),
        }
    }
}

/// Check the file and insert its row (`fetching`); the caller starts the
/// copy with [`spawn_fetch`]. `scope` is the caller's; `source` is
/// [`SOURCE_AGENT`] or [`SOURCE_PERSON`].
pub async fn send(
    store: &Mutex<Store>,
    ssh: &dyn SshExec,
    scope: &ViewScope,
    source: &str,
    args: &SendFileArgs,
) -> Result<DownloadRow, IpcError> {
    check_path(&args.path)?;
    dir()?;
    let session = {
        let s = crate::ipc_error::lock(store)?;
        let row = s.get_session_by_id(args.session_id)?;
        match row {
            // Two arms, because two different callers send files and §4.4
            // separates them:
            //
            // * a PERSON reaches the `own` tier and nothing less. This reads
            //   an unconstrained absolute path off the session's host (see
            //   [`visible`]), which is a subset of a terminal, and spec §4.3
            //   invariant 5 says no grant confers one. A `watch` or `drive`
            //   grantee is refused `E_NOTFOUND`, like any id that is not
            //   theirs;
            // * a per-host token is the session's OWN Claude, which is this
            //   tool's headline use (`whoami` gives it its `session_id`).
            //   `may_own` deliberately excludes it — the pane proof never
            //   reaches that tier — so its arm is §4.4 clauses 1 and 2
            //   instead, which `sees_session_row` already is: its own host,
            //   and either an unclaimed row or the one pane this request
            //   proves. Strictly tighter than main's host-only fence.
            Some(r)
                if (match scope.host.as_deref() {
                    Some(h) => h == r.host_alias && scope.sees_session_row(&r).is_visible(),
                    None => scope.may_own(&r),
                }) =>
            {
                r
            }
            _ => {
                return Err(IpcError::new(
                    codes::E_NOTFOUND,
                    format!("session {} not found", args.session_id),
                ))
            }
        }
    };
    let out = crate::ssh::run_shell(
        ssh,
        &session.host_alias,
        &stat_script(&session.tmux_name, &args.path),
        STAT_TIMEOUT,
    )
    .await?;
    let stat = parse_stat(&out.stdout, &args.path)?;
    let note = clean_note(args.note.as_deref());
    let name = base_name(&stat.path);
    let s = crate::ipc_error::lock(store)?;
    let b = Budget::from_store(&s);
    for id in make_room(&s, &b, stat.size)? {
        s.delete_download(id)?;
        remove_files(id);
        tracing::info!(id, "[downloads] dropped to make room");
    }
    let row = s.insert_download(&NewDownload {
        host_alias: &session.host_alias,
        session_id: Some(session.id),
        session_name: Some(&session.tmux_name),
        org_id: session.org_id,
        path: &stat.path,
        name: &name,
        size: stat.size as i64,
        source,
        note: note.as_deref(),
    })?;
    tracing::info!(
        id = row.id,
        host = %row.host_alias,
        size = row.size,
        "[downloads] copying"
    );
    Ok(present(&b, row))
}

/// Copy `row`'s bytes in the background; the row ends `ready` or `failed`.
pub fn spawn_fetch(store: Arc<Mutex<Store>>, ssh: Arc<dyn SshExec>, row: DownloadRow) {
    tokio::spawn(async move {
        let id = row.id;
        let result = fetch(&*ssh, &row).await;
        let Ok(s) = crate::ipc_error::lock(&store) else {
            return;
        };
        match result {
            Ok(sha) => match s.finish_download(id, &sha) {
                Ok(true) => tracing::info!(id, "[downloads] ready"),
                // Removed while copying: the bytes have no row.
                Ok(false) => remove_files(id),
                Err(e) => tracing::warn!(id, error = %e, "[downloads] finish failed"),
            },
            Err(e) => {
                remove_files(id);
                tracing::warn!(id, error = %e.message, "[downloads] copy failed");
                if let Err(e) = s.fail_download(id, &e.message) {
                    tracing::warn!(id, error = %e, "[downloads] fail failed");
                }
            }
        }
    });
}

/// Pull the bytes into `<id>.part`, then rename it to `<id>`; the SHA-256
/// of what was written.
pub async fn fetch(ssh: &dyn SshExec, row: &DownloadRow) -> Result<String, IpcError> {
    fetch_chunked(ssh, row, CHUNK_BYTES).await
}

async fn fetch_chunked(
    ssh: &dyn SshExec,
    row: &DownloadRow,
    chunk_bytes: u64,
) -> Result<String, IpcError> {
    use sha2::Digest;
    use tokio::io::AsyncWriteExt;
    let part = part_of(row.id)?;
    let mut f = {
        let mut o = tokio::fs::OpenOptions::new();
        o.write(true).create(true).truncate(true);
        #[cfg(unix)]
        o.mode(0o600);
        o.open(&part).await?
    };
    let size = row.size.max(0) as u64;
    let mut hash = sha2::Sha256::new();
    let mut got = 0u64;
    while got < size {
        let want = chunk_bytes.min(size - got);
        let out = crate::ssh::run_shell(
            ssh,
            &row.host_alias,
            &carry::chunk_script(&row.path, got, want),
            CHUNK_TIMEOUT,
        )
        .await?;
        let chunk = carry::payload(&out.stdout).unwrap_or_default();
        if !out.status.success() || chunk.is_empty() {
            return Err(IpcError::new(
                codes::E_SSH,
                format!(
                    "reading {} on {} stopped at {got} of {size} bytes{}",
                    row.path,
                    row.host_alias,
                    if got == 0 {
                        ""
                    } else {
                        " (did the file change?)"
                    }
                ),
            ));
        }
        f.write_all(chunk).await?;
        hash.update(chunk);
        got += chunk.len() as u64;
    }
    f.sync_all().await?;
    drop(f);
    if got != size {
        return Err(IpcError::new(
            codes::E_SSH,
            format!("{} gave {got} bytes, expected {size}", row.path),
        ));
    }
    tokio::fs::rename(&part, file_of(row.id)?).await?;
    Ok(hex::encode(hash.finalize()))
}

// ---- read ------------------------------------------------------------------

/// Arguments of `list_downloads`.
#[derive(Debug, Clone, Default, serde::Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct ListDownloadsArgs {
    /// Only this session's files.
    #[serde(default)]
    pub session_id: Option<i64>,
    /// At most this many, newest first (default and cap 200).
    #[serde(default)]
    pub limit: Option<usize>,
}

/// What `list_downloads` answers.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DownloadList {
    pub downloads: Vec<DownloadRow>,
    /// Bytes every kept (not failed) download takes, the caller's or not:
    /// the budget is the machine's.
    pub total_bytes: u64,
    pub max_total_bytes: u64,
    pub max_file_bytes: u64,
}

pub fn list(
    s: &Store,
    scope: &ViewScope,
    args: &ListDownloadsArgs,
) -> Result<DownloadList, IpcError> {
    let b = Budget::from_store(s);
    let rows = s.downloads()?;
    let total_bytes = rows
        .iter()
        .filter(|r| r.state != "failed")
        .map(|r| r.size.max(0) as u64)
        .sum();
    let limit = args.limit.unwrap_or(LIST_LIMIT).clamp(1, LIST_LIMIT);
    let downloads = rows
        .into_iter()
        .filter(|r| visible(s, scope, r))
        .filter(|r| args.session_id.is_none_or(|id| r.session_id == Some(id)))
        .take(limit)
        .map(|r| present(&b, r))
        .collect();
    Ok(DownloadList {
        downloads,
        total_bytes,
        max_total_bytes: b.max_total,
        max_file_bytes: b.max_file,
    })
}

fn visible_row(s: &Store, scope: &ViewScope, id: i64) -> Result<DownloadRow, IpcError> {
    match s.download(id)? {
        Some(r) if visible(s, scope, &r) => Ok(r),
        _ => Err(IpcError::new(
            codes::E_NOTFOUND,
            format!("download {id} not found"),
        )),
    }
}

/// Drop a download and its bytes. `false` when it was already gone. A copy
/// still in flight finds no row when it ends and deletes what it wrote.
pub fn remove(s: &Store, scope: &ViewScope, id: i64) -> Result<bool, IpcError> {
    let Ok(row) = visible_row(s, scope, id) else {
        return Ok(false);
    };
    let removed = s.delete_download(row.id)?;
    remove_files(row.id);
    Ok(removed)
}

/// A ready download the caller may fetch, and where its bytes are; stamps
/// `downloaded_at`. `E_NOTFOUND` otherwise.
pub fn open_ready(
    s: &Store,
    scope: &ViewScope,
    id: i64,
) -> Result<(DownloadRow, PathBuf), IpcError> {
    let row = visible_row(s, scope, id)?;
    let path = file_of(id)?;
    if row.state != "ready" || !path.is_file() {
        return Err(IpcError::new(
            codes::E_NOTFOUND,
            format!("download {id} is not ready"),
        ));
    }
    s.mark_downloaded(id)?;
    Ok((row, path))
}

/// The `Content-Type` a download is served with: by extension, else
/// `application/octet-stream`.
pub fn content_type(name: &str) -> &'static str {
    let ext = name
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "pdf" => "application/pdf",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "txt" | "log" => "text/plain; charset=utf-8",
        "md" => "text/markdown; charset=utf-8",
        "csv" => "text/csv; charset=utf-8",
        "json" => "application/json",
        "html" | "htm" => "text/html; charset=utf-8",
        "zip" => "application/zip",
        "gz" | "tgz" => "application/gzip",
        "tar" => "application/x-tar",
        "mp4" => "video/mp4",
        "mov" => "video/quicktime",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "apk" => "application/vnd.android.package-archive",
        _ => "application/octet-stream",
    }
}

/// `Content-Disposition` for `name`: an ASCII fallback and the RFC 5987
/// UTF-8 form.
pub fn content_disposition(name: &str) -> String {
    let ascii: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_graphic() && c != '"' && c != '\\' || c == ' ' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let mut enc = String::new();
    for b in name.bytes() {
        if b.is_ascii_alphanumeric() || b"!#$&+-.^_`|~".contains(&b) {
            enc.push(b as char);
        } else {
            enc.push_str(&format!("%{b:02X}"));
        }
    }
    format!("attachment; filename=\"{ascii}\"; filename*=UTF-8''{enc}")
}

// ---- retention -------------------------------------------------------------

/// Drop downloads past `downloads.keep_secs` (ready and failed; a copy in
/// flight is left alone) and bytes no row owns. The count of rows dropped.
pub fn sweep(store: &Mutex<Store>, now: i64) -> usize {
    sweep_with(store, now, ORPHAN_MIN_AGE_SECS)
}

/// Bytes with no row are removed only once they are this old (seconds):
/// the rows are read before the directory, so a download inserted in
/// between has a fresh `.part` and no row in that read — it must survive.
pub const ORPHAN_MIN_AGE_SECS: i64 = 60 * 60;

fn sweep_with(store: &Mutex<Store>, now: i64, orphan_min_age: i64) -> usize {
    let Ok(dir) = dir() else {
        return 0;
    };
    let Ok(s) = crate::ipc_error::lock(store) else {
        return 0;
    };
    let b = Budget::from_store(&s);
    let Ok(rows) = s.downloads() else {
        return 0;
    };
    let mut n = 0;
    for r in &rows {
        if r.state == "fetching" {
            continue;
        }
        if b.expires_at(r).is_some_and(|t| t <= now) && s.delete_download(r.id).unwrap_or(false) {
            remove_files(r.id);
            n += 1;
        }
    }
    let live: std::collections::HashSet<i64> = rows.iter().map(|r| r.id).collect();
    drop(s);
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            let name = e.file_name();
            let name = name.to_string_lossy();
            let id = name.strip_suffix(".part").unwrap_or(&name).parse::<i64>();
            if !id.is_ok_and(|id| !live.contains(&id)) {
                continue;
            }
            let modified = e
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64);
            if modified.is_some_and(|m| m + orphan_min_age <= now) {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    n
}

#[cfg(test)]
#[path = "downloads_tests.rs"]
mod tests;
