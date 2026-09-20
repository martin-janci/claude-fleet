//! `upload_to_session` — stage dropped files on the session's host so their
//! remote path can be pasted into the prompt. Local sessions copy with
//! `std::fs`; remote sessions stream bytes over the ControlMaster
//! (`SshClient::upload_file`). No cleanup (per the design — files accumulate
//! under ~/.claude-fleet/uploads/<session>/).
//!
//! The webview never gets to name an arbitrary local path: only paths this
//! process itself authorised — a Tauri drag-drop event, or the OS picker in
//! `pick_attachments` — are accepted, recorded Rust-side into
//! [`UploadAllowList`]. Anything else is `E_FORBIDDEN` (SEC-9).
//!
//! The two origins keep separate TTLs ([`UPLOAD_ALLOW_TTL`] /
//! [`PICKED_ALLOW_TTL`]): a drop is followed by the upload within
//! milliseconds, but a picked file sits in the composer's attachment tray
//! for as long as the user takes to write the prompt around it — ordinary
//! minutes, not a slow IPC round-trip.

use crate::backend::FleetBackend;
use fleet_core::ipc_error::{codes, IpcError};
use fleet_core::shell::quote;
use fleet_core::ssh::SshClient;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::State;

/// Generous timeout — covers an image upload over a slow link.
const UPLOAD_TIMEOUT_SECS: u64 = 60;

/// How long a dropped path stays uploadable. A drop is followed by the
/// upload within milliseconds; the window only needs to cover a slow IPC
/// round-trip and a user who drops, then waits for a session to open.
pub const UPLOAD_ALLOW_TTL: Duration = Duration::from_secs(10 * 60);

/// How long a *picked* path stays uploadable. A pick has no immediate
/// upload the way a drop does: the file sits in the composer's attachment
/// tray while the user keeps writing the prompt around it, and may step
/// away — a break, a phone call — before sending. Four hours covers a tray
/// held open through that kind of pause without leaving a picked file
/// authorised past the sitting it was picked for.
pub const PICKED_ALLOW_TTL: Duration = Duration::from_secs(4 * 60 * 60);

/// Where an allow-listed path came from — the two origins are authorised
/// for different lengths of time (see [`UPLOAD_ALLOW_TTL`] /
/// [`PICKED_ALLOW_TTL`]), so each entry remembers which rule it lives by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Origin {
    Dropped,
    Picked,
}

impl Origin {
    fn ttl(self) -> Duration {
        match self {
            Origin::Dropped => UPLOAD_ALLOW_TTL,
            Origin::Picked => PICKED_ALLOW_TTL,
        }
    }
}

/// Rust-side record of the paths this process itself authorised: the OS
/// drag-drop handed the window, or the OS picker in `pick_attachments`
/// returned. Managed in Tauri state as `Arc<UploadAllowList>`; the drop half
/// is populated from `on_window_event` / `on_webview_event` in `lib.rs`.
#[derive(Default)]
pub struct UploadAllowList {
    entries: Mutex<HashMap<PathBuf, (Instant, Origin)>>,
}

impl UploadAllowList {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record freshly dropped paths.
    pub fn allow(&self, paths: &[PathBuf]) {
        self.allow_at(paths, Instant::now());
    }

    pub fn allow_at(&self, paths: &[PathBuf], now: Instant) {
        self.insert_at(paths, now, Origin::Dropped);
    }

    /// Record freshly picked paths — see [`PICKED_ALLOW_TTL`] for why they
    /// get a longer window than a drop.
    pub fn allow_picked(&self, paths: &[PathBuf]) {
        self.allow_picked_at(paths, Instant::now());
    }

    pub fn allow_picked_at(&self, paths: &[PathBuf], now: Instant) {
        self.insert_at(paths, now, Origin::Picked);
    }

    fn insert_at(&self, paths: &[PathBuf], now: Instant, origin: Origin) {
        let mut e = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        e.retain(|_, (at, o)| now.saturating_duration_since(*at) < o.ttl());
        for p in paths {
            e.insert(p.clone(), (now, origin));
        }
    }

    /// True if `path` is still authorised — dropped or picked, each judged
    /// against its own TTL. Paths compare verbatim — the frontend echoes
    /// back exactly what the drop event or the picker delivered.
    pub fn is_allowed(&self, path: &Path) -> bool {
        self.is_allowed_at(path, Instant::now())
    }

    pub fn is_allowed_at(&self, path: &Path, now: Instant) -> bool {
        let e = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        e.get(path)
            .is_some_and(|(at, origin)| now.saturating_duration_since(*at) < origin.ttl())
    }

    /// Remove `paths` from the list once an upload has used them: one
    /// drop or pick authorises one upload, not a window of re-reads.
    pub fn consume(&self, paths: &[String]) {
        let mut e = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for p in paths {
            e.remove(Path::new(p));
        }
    }
}

/// How the composer should draw an attachment before it is uploaded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AttachKind {
    Image,
    Text,
    Binary,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct PickedFile {
    /// Absolute local path. Echoed back verbatim by the frontend, and only
    /// accepted again because THIS process put it on the allow-list.
    pub path: String,
    pub name: String,
    pub size: u64,
    pub kind: AttachKind,
}

fn classify(path: &Path) -> AttachKind {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "svg" => AttachKind::Image,
        "txt" | "md" | "log" | "json" | "yaml" | "yml" | "toml" | "csv" | "diff" | "patch"
        | "rs" | "ts" | "js" | "svelte" | "py" | "sh" => AttachKind::Text,
        _ => AttachKind::Binary,
    }
}

/// Record picked paths on the allow-list and describe them for the composer.
/// Split out of the command so the authorisation is unit-testable without a
/// Tauri app handle.
///
/// Validates before authorising: a path only reaches `allow_picked` once it
/// has passed every check, so a bad file in the batch never leaves a stale
/// authorisation behind for nothing to consume. A bad file is skipped, not
/// fatal — of five picked files, four with ordinary names come back as
/// attachments; the fifth is simply left out rather than losing all five.
pub fn record_picked(
    allow: &UploadAllowList,
    paths: Vec<PathBuf>,
) -> Result<Vec<PickedFile>, IpcError> {
    let mut good_paths = Vec::with_capacity(paths.len());
    let mut out = Vec::with_capacity(paths.len());
    for p in paths {
        // Gone or unreadable between the pick and this call: leave it out
        // rather than failing every other file in the batch.
        let Ok(meta) = std::fs::metadata(&p) else {
            continue;
        };
        let name = p
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("file")
            .to_string();
        // A newline in a basename would travel into the prompt text.
        if name.contains(['\n', '\r']) {
            continue;
        }
        let kind = classify(&p);
        out.push(PickedFile {
            path: p.to_string_lossy().into_owned(),
            name,
            size: meta.len(),
            kind,
        });
        good_paths.push(p);
    }
    allow.allow_picked(&good_paths);
    Ok(out)
}

/// Above this, the strip shows an extension tile instead of the picture: a
/// data URL costs ~1.33x its bytes in the webview, and the tile is 44px.
pub const PREVIEW_MAX_BYTES: u64 = 2 * 1024 * 1024;

/// The mime type for a data URL, for the extensions `classify` already calls
/// [`AttachKind::Image`]. Gated on `classify` first so the two extension
/// lists cannot drift apart; this only adds the mime string each already
/// implies.
///
/// `.svg` -> `image/svg+xml` is safe ONLY because a preview is rendered
/// through `<img src="data:...">`: an `<img>` rasterises SVG and cannot
/// execute the script it may contain. The mime string returned here must
/// never be used to inline SVG markup into the DOM (innerHTML, an inline
/// `<svg>`, or anything else that parses it as a document) — that path lets
/// an attacker-controlled `.svg` run script in the app's origin.
fn mime_for(path: &Path) -> Option<&'static str> {
    if classify(path) != AttachKind::Image {
        return None;
    }
    match path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        "bmp" => Some("image/bmp"),
        "svg" => Some("image/svg+xml"),
        _ => None,
    }
}

/// Split out of the command so the allow-list gate is unit-testable.
pub fn preview_for(allow: &UploadAllowList, path: &str) -> Result<Option<String>, IpcError> {
    let p = Path::new(path);
    if !allow.is_allowed(p) {
        return Err(IpcError::new(
            codes::E_FORBIDDEN,
            format!("{path} was not attached by the user; it cannot be previewed"),
        ));
    }
    let Some(mime) = mime_for(p) else {
        return Ok(None);
    };
    let len = std::fs::metadata(p)
        .map_err(|e| IpcError::new(codes::E_UPLOAD, format!("stat {path}: {e}")))?
        .len();
    if len > PREVIEW_MAX_BYTES {
        return Ok(None);
    }
    let bytes = std::fs::read(p)
        .map_err(|e| IpcError::new(codes::E_UPLOAD, format!("read {path}: {e}")))?;
    use base64::Engine as _;
    let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
    Ok(Some(format!("data:{mime};base64,{b64}")))
}

/// A data URL for an attached image, so the composer can show a thumbnail.
/// Reading bytes stays in Rust: there is no fs plugin, and the allow-list is
/// the only thing that decides which files this process will open.
#[tauri::command]
pub fn attachment_preview(
    path: String,
    backend: State<'_, Arc<FleetBackend>>,
    allow: State<'_, Arc<UploadAllowList>>,
) -> Result<Option<String>, IpcError> {
    // The file is on THIS machine; a hub client has nothing local to read.
    backend.refuse_local_only("attachment_preview")?;
    preview_for(&allow, &path)
}

/// Open the OS file picker and authorise whatever the user chooses. The
/// picker runs HERE, not in the webview, so the webview still never gets to
/// name a path (SEC-9).
#[tauri::command]
pub async fn pick_attachments(
    app: tauri::AppHandle,
    backend: State<'_, Arc<FleetBackend>>,
    allow: State<'_, Arc<UploadAllowList>>,
) -> Result<Vec<PickedFile>, IpcError> {
    // The picker opens on THIS machine's desktop; there is no way to show a
    // hub's file dialog through the frame this app draws.
    backend.refuse_local_only("pick_attachments")?;
    use tauri_plugin_dialog::DialogExt;
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .set_title("Attach files")
        .pick_files(move |paths| {
            let _ = tx.send(paths);
        });
    let picked = rx
        .await
        .map_err(|_| IpcError::new(codes::E_UPLOAD, "the file picker closed unexpectedly"))?;
    let Some(paths) = picked else {
        return Ok(vec![]);
    };
    let paths: Vec<PathBuf> = paths
        .into_iter()
        .filter_map(|p| p.into_path().ok())
        .collect();
    record_picked(&allow, paths)
}

#[derive(Deserialize)]
pub struct UploadArgs {
    pub host_alias: String,
    /// The session's tmux name — used as the per-session staging subdir.
    pub session_name: String,
    /// Absolute local paths of the dropped files.
    pub local_paths: Vec<String>,
}

/// Pure gate: every requested path must be on the allow-list.
///
/// The message must stay true for both allow-list origins — a drop and a
/// pick are each authorised by the user, and each can simply time out
/// (`UPLOAD_ALLOW_TTL` / `PICKED_ALLOW_TTL`) before the upload runs.
pub fn check_paths_allowed(allow: &UploadAllowList, paths: &[String]) -> Result<(), IpcError> {
    for p in paths {
        if !allow.is_allowed(Path::new(p)) {
            return Err(IpcError::new(
                codes::E_FORBIDDEN,
                format!(
                    "{p} was not attached by the user, or its authorisation has expired; \
                     attach it again"
                ),
            ));
        }
    }
    Ok(())
}

/// Basenames of `paths`, in order — not yet collision-free (see
/// `dedupe_names`). Shared by `upload_to_session` and `upload_attachments`.
fn basenames_of(paths: &[String]) -> Vec<String> {
    paths
        .iter()
        .map(|p| {
            Path::new(p)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("file")
                .to_string()
        })
        .collect()
}

/// Run `script` on `host` (local via `bash -lc`, remote over the
/// ControlMaster — `fleet_core::ssh::run_shell` picks the branch) and map a
/// non-zero exit to `E_UPLOAD` with the host's stderr.
async fn run_script(
    ssh: &Arc<SshClient>,
    host: &str,
    script: &str,
    timeout: Duration,
) -> Result<std::process::Output, IpcError> {
    let out = fleet_core::ssh::run_shell(ssh.as_ref(), host, script, timeout).await?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let detail = if stderr.trim().is_empty() {
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        } else {
            stderr.trim().to_string()
        };
        return Err(IpcError::new(
            codes::E_UPLOAD,
            format!("on {host}: {detail}"),
        ));
    }
    Ok(out)
}

/// Resolve the session's worktree root: ask tmux for the pane's cwd, then
/// git for the toplevel — the same live resolution every Files-tab read
/// already does (`fleet_core::service::repo::repo_script`). `SessionRow`
/// only carries `worktree_id`/`worktree_key` (an id and a name, not a path),
/// so there is nothing to resolve this from except the live pane.
async fn resolve_worktree_root(
    ssh: &Arc<SshClient>,
    host: &str,
    session_name: &str,
    timeout: Duration,
) -> Result<String, IpcError> {
    let script = fleet_core::service::attachments::root_script(session_name);
    let out = run_script(ssh, host, &script, timeout).await?;
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Copy `local_paths` (named `names`, in order) into `dir` on `host` and
/// return their absolute destination paths, in order. The one local-vs-remote
/// branch `upload_to_session` and `upload_attachments` share: a local session
/// copies with `std::fs`, a remote one streams over the ControlMaster
/// (`SshClient::upload_file`).
async fn transfer_all(
    ssh: &Arc<SshClient>,
    host: &str,
    local_paths: &[String],
    names: &[String],
    dir: &str,
    timeout: Duration,
) -> Result<Vec<String>, IpcError> {
    let mut remote_paths = Vec::with_capacity(names.len());

    if host == "local" {
        std::fs::create_dir_all(dir)
            .map_err(|e| IpcError::new(codes::E_UPLOAD, format!("mkdir {dir}: {e}")))?;
        for (src, name) in local_paths.iter().zip(names) {
            let dest = format!("{dir}/{name}");
            std::fs::copy(src, &dest)
                .map_err(|e| IpcError::new(codes::E_UPLOAD, format!("copy {src}: {e}")))?;
            remote_paths.push(dest);
        }
    } else {
        let mkdir = ssh
            .run(host, &["mkdir", "-p", &quote(dir)], timeout)
            .await?;
        if !mkdir.status.success() {
            return Err(IpcError::new(
                codes::E_UPLOAD,
                format!(
                    "mkdir on {host} failed: {}",
                    String::from_utf8_lossy(&mkdir.stderr).trim()
                ),
            ));
        }
        for (src, name) in local_paths.iter().zip(names) {
            let dest = format!("{dir}/{name}");
            ssh.upload_file(host, Path::new(src), &dest, timeout)
                .await?;
            remote_paths.push(dest);
        }
    }

    Ok(remote_paths)
}

/// Stage `local_paths` under `~/.claude-fleet/uploads/<session>/` on the
/// session's host and return the resulting absolute remote paths, in order.
#[tauri::command]
pub async fn upload_to_session(
    args: UploadArgs,
    backend: State<'_, Arc<FleetBackend>>,
    ssh: State<'_, Arc<SshClient>>,
    allow: State<'_, Arc<UploadAllowList>>,
) -> Result<Vec<String>, IpcError> {
    // The bytes are on THIS machine and the destination is a host only the
    // hub can reach; there is no tool that would carry a file across.
    backend.refuse_local_only("upload_to_session")?;
    fleet_core::validate::host_alias(&args.host_alias)?;
    fleet_core::validate::tmux_name_addressable(&args.session_name)?;
    if args.local_paths.is_empty() {
        return Ok(vec![]);
    }
    check_paths_allowed(&allow, &args.local_paths)?;
    // Consumed up front (not after the upload) so a failed transfer does
    // not leave a re-usable authorisation behind; the user simply drops
    // the file again.
    allow.consume(&args.local_paths);

    let names = dedupe_names(&basenames_of(&args.local_paths));

    let timeout = Duration::from_secs(UPLOAD_TIMEOUT_SECS);
    let is_local = args.host_alias == "local";

    // Resolve the staging dir (absolute, so the returned paths are pasteable).
    let home = if is_local {
        std::env::var("HOME").map_err(|_| IpcError::new(codes::E_UPLOAD, "HOME not set"))?
    } else {
        ssh.remote_home(&args.host_alias).await?
    };
    let dir = format!("{home}/.claude-fleet/uploads/{}", args.session_name);

    transfer_all(
        &ssh,
        &args.host_alias,
        &args.local_paths,
        &names,
        &dir,
        timeout,
    )
    .await
}

#[derive(Deserialize)]
pub struct AttachArgs {
    pub host_alias: String,
    /// The session's tmux name.
    pub session_name: String,
    /// Absolute local paths of the attached files.
    pub local_paths: Vec<String>,
}

/// Stage attachments under the session's worktree root
/// (`.claude-fleet-attachments/`, excluded untracked — see
/// `fleet_core::service::attachments`) and return their absolute remote
/// paths, in order, so Claude Code can read them without a permission
/// prompt: an absolute path outside the working directory asks the user to
/// approve it, and a prompt sent from the composer has nobody there to
/// answer.
#[tauri::command]
pub async fn upload_attachments(
    args: AttachArgs,
    backend: State<'_, Arc<FleetBackend>>,
    ssh: State<'_, Arc<SshClient>>,
    allow: State<'_, Arc<UploadAllowList>>,
) -> Result<Vec<String>, IpcError> {
    // Same reason as upload_to_session: the bytes are on THIS machine and the
    // host is the hub's to reach.
    backend.refuse_local_only("upload_attachments")?;
    fleet_core::validate::host_alias(&args.host_alias)?;
    fleet_core::validate::tmux_name_addressable(&args.session_name)?;
    if args.local_paths.is_empty() {
        return Ok(vec![]);
    }
    check_paths_allowed(&allow, &args.local_paths)?;
    allow.consume(&args.local_paths);

    let timeout = Duration::from_secs(UPLOAD_TIMEOUT_SECS);
    let root = resolve_worktree_root(&ssh, &args.host_alias, &args.session_name, timeout).await?;
    let dir = format!("{root}/{}", fleet_core::service::attachments::ATTACH_DIR);

    let names = dedupe_names(&basenames_of(&args.local_paths));
    run_script(
        &ssh,
        &args.host_alias,
        &fleet_core::service::attachments::stage_script(&root),
        timeout,
    )
    .await?;
    transfer_all(
        &ssh,
        &args.host_alias,
        &args.local_paths,
        &names,
        &dir,
        timeout,
    )
    .await
}

/// Make a batch of basenames collision-free, preserving order. The first
/// occurrence keeps its name; a later duplicate gets `-1`, `-2`, … inserted
/// before its extension (`a.png` → `a-1.png`; `notes` → `notes-1`).
pub fn dedupe_names(names: &[String]) -> Vec<String> {
    use std::collections::HashSet;
    let mut seen: HashSet<String> = HashSet::new();
    let mut out = Vec::with_capacity(names.len());
    for name in names {
        let mut candidate = name.clone();
        let mut n = 1;
        while seen.contains(&candidate) {
            candidate = suffix_name(name, n);
            n += 1;
        }
        seen.insert(candidate.clone());
        out.push(candidate);
    }
    out
}

/// Insert `-{n}` before the final extension (if any). `file_stem`/`extension`
/// semantics: a leading-dot name like `.bashrc` has no extension, so the
/// suffix goes at the end.
fn suffix_name(name: &str, n: u32) -> String {
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => format!("{stem}-{n}.{ext}"),
        _ => format!("{name}-{n}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_unique_names() {
        let got = dedupe_names(&["a.png".into(), "b.png".into()]);
        assert_eq!(got, vec!["a.png", "b.png"]);
    }

    #[test]
    fn suffixes_collisions_before_extension() {
        let got = dedupe_names(&["a.png".into(), "a.png".into(), "a.png".into()]);
        assert_eq!(got, vec!["a.png", "a-1.png", "a-2.png"]);
    }

    #[test]
    fn handles_names_without_extension() {
        let got = dedupe_names(&["notes".into(), "notes".into()]);
        assert_eq!(got, vec!["notes", "notes-1"]);
    }

    #[test]
    fn leading_dot_name_has_no_extension() {
        let got = dedupe_names(&[".env".into(), ".env".into()]);
        assert_eq!(got, vec![".env", ".env-1"]);
    }

    #[test]
    fn allow_list_admits_only_dropped_paths_within_ttl() {
        let al = UploadAllowList::new();
        let t0 = Instant::now();
        let dropped = PathBuf::from("/tmp/shot.png");
        assert!(!al.is_allowed_at(&dropped, t0), "nothing dropped yet");
        al.allow_at(std::slice::from_ref(&dropped), t0);
        assert!(al.is_allowed_at(&dropped, t0));
        assert!(al.is_allowed_at(&dropped, t0 + Duration::from_secs(60)));
        assert!(
            !al.is_allowed_at(Path::new("/etc/passwd"), t0),
            "a path the webview names on its own is refused"
        );
        assert!(
            !al.is_allowed_at(&dropped, t0 + UPLOAD_ALLOW_TTL),
            "entries expire after the TTL"
        );
    }

    #[test]
    fn allow_list_prunes_stale_entries_on_insert() {
        let al = UploadAllowList::new();
        let t0 = Instant::now();
        al.allow_at(&[PathBuf::from("/tmp/old.png")], t0);
        al.allow_at(&[PathBuf::from("/tmp/new.png")], t0 + UPLOAD_ALLOW_TTL);
        let e = al.entries.lock().unwrap();
        assert!(!e.contains_key(Path::new("/tmp/old.png")));
        assert!(e.contains_key(Path::new("/tmp/new.png")));
    }

    #[test]
    fn a_picked_path_survives_a_composer_tray_but_still_expires() {
        let al = UploadAllowList::new();
        let t0 = Instant::now();
        let picked = PathBuf::from("/tmp/shot.png");
        al.allow_picked_at(std::slice::from_ref(&picked), t0);
        // Ten minutes of typing — long enough to expire a drop, not a pick.
        assert!(
            al.is_allowed_at(&picked, t0 + UPLOAD_ALLOW_TTL),
            "a picked file must outlive the drop TTL: the composer tray is not a slow IPC round-trip"
        );
        assert!(al.is_allowed_at(&picked, t0 + Duration::from_secs(60 * 60)));
        // The boundary itself: just inside the window is still allowed,
        // just outside is refused.
        assert!(
            al.is_allowed_at(&picked, t0 + PICKED_ALLOW_TTL - Duration::from_secs(1)),
            "one second before the TTL, still authorised"
        );
        assert!(
            !al.is_allowed_at(&picked, t0 + PICKED_ALLOW_TTL + Duration::from_secs(1)),
            "one second past the TTL, no longer authorised"
        );
        assert!(
            !al.is_allowed_at(&picked, t0 + PICKED_ALLOW_TTL),
            "a pick still expires eventually"
        );
    }

    #[test]
    fn an_expired_pick_is_refused_by_check_paths_allowed_with_a_message_true_for_it() {
        // Back-date the insertion (`Instant` arithmetic, no sleeping) so
        // real-time `is_allowed`/`check_paths_allowed` — the path
        // `upload_to_session` actually calls — sees a genuinely expired
        // pick, not a synthetic one only reachable via `is_allowed_at`.
        let al = UploadAllowList::new();
        let picked = PathBuf::from("/tmp/shot.png");
        let long_ago = Instant::now() - PICKED_ALLOW_TTL - Duration::from_secs(60);
        al.allow_picked_at(std::slice::from_ref(&picked), long_ago);
        assert!(!al.is_allowed(&picked), "the pick has expired");

        let err = check_paths_allowed(&al, &["/tmp/shot.png".to_string()]).unwrap_err();
        assert_eq!(err.code, codes::E_FORBIDDEN);
        assert!(
            !err.message.contains("dropped onto the window"),
            "a picked file was never dropped onto anything: {}",
            err.message
        );
        assert!(
            err.message.contains("expired") || err.message.contains("not attached"),
            "the message must be true for a picked file: {}",
            err.message
        );
    }

    #[test]
    fn record_picked_skips_a_bad_name_but_keeps_the_rest_of_the_batch() {
        let allow = UploadAllowList::new();
        let dir = tempfile::tempdir().unwrap();
        let good = dir.path().join("notes.txt");
        std::fs::write(&good, b"hello").unwrap();
        // POSIX allows a literal newline in a filename (only `/` and NUL are
        // forbidden), so this both exists and stats cleanly — the rejection
        // must come from the newline check, not a missing file.
        let bad = dir.path().join("bad\nname.txt");
        std::fs::write(&bad, b"data").unwrap();

        let picked = record_picked(&allow, vec![good.clone(), bad.clone()]).unwrap();

        assert_eq!(picked.len(), 1, "the bad entry is left out, not fatal");
        assert_eq!(picked[0].name, "notes.txt");
        assert!(allow.is_allowed(&good), "the good file is still authorised");
        assert!(
            !allow.is_allowed(&bad),
            "the bad file must not be authorised — validation runs before allow-listing"
        );
    }

    #[test]
    fn a_picked_file_is_authorised_and_classified() {
        let allow = UploadAllowList::new();
        let dir = tempfile::tempdir().unwrap();
        let png = dir.path().join("shot.png");
        std::fs::write(&png, b"\x89PNG\r\n\x1a\n").unwrap();

        let picked = record_picked(&allow, vec![png.clone()]).unwrap();

        assert_eq!(picked.len(), 1);
        assert_eq!(picked[0].name, "shot.png");
        assert_eq!(picked[0].kind, AttachKind::Image);
        assert_eq!(picked[0].size, 8);
        // The gate the whole design turns on.
        assert!(allow.is_allowed(&png));
        assert!(check_paths_allowed(&allow, &[png.to_string_lossy().into_owned()]).is_ok());
    }

    #[test]
    fn an_unpicked_path_stays_forbidden() {
        let allow = UploadAllowList::new();
        let err = check_paths_allowed(&allow, &["/etc/passwd".to_string()]).unwrap_err();
        assert_eq!(err.code, codes::E_FORBIDDEN);
    }

    #[test]
    fn check_paths_allowed_rejects_any_unlisted_path_with_e_forbidden() {
        let al = UploadAllowList::new();
        al.allow(&[PathBuf::from("/tmp/a.png")]);
        assert!(check_paths_allowed(&al, &["/tmp/a.png".into()]).is_ok());
        let err =
            check_paths_allowed(&al, &["/tmp/a.png".into(), "/etc/passwd".into()]).unwrap_err();
        assert_eq!(err.code, "E_FORBIDDEN");
        assert!(check_paths_allowed(&al, &[]).is_ok());
        // One drop authorises one upload: consumed entries are gone.
        al.consume(&["/tmp/a.png".into()]);
        assert_eq!(
            check_paths_allowed(&al, &["/tmp/a.png".into()])
                .unwrap_err()
                .code,
            "E_FORBIDDEN"
        );
    }

    #[test]
    fn previews_only_small_allow_listed_images() {
        let allow = UploadAllowList::new();
        let dir = tempfile::tempdir().unwrap();

        let png = dir.path().join("a.png");
        std::fs::write(&png, b"\x89PNG\r\n\x1a\n").unwrap();
        allow.allow(std::slice::from_ref(&png));
        let url = preview_for(&allow, png.to_str().unwrap()).unwrap().unwrap();
        assert!(url.starts_with("data:image/png;base64,"));

        // A text file is not an image: no preview, no error.
        let log = dir.path().join("b.log");
        std::fs::write(&log, b"hello").unwrap();
        allow.allow(std::slice::from_ref(&log));
        assert!(preview_for(&allow, log.to_str().unwrap())
            .unwrap()
            .is_none());

        // Not on the allow-list: refused, not read.
        let err = preview_for(&allow, "/etc/passwd").unwrap_err();
        assert_eq!(err.code, codes::E_FORBIDDEN);
    }

    #[test]
    fn a_large_image_gets_no_inline_preview() {
        let allow = UploadAllowList::new();
        let dir = tempfile::tempdir().unwrap();
        let big = dir.path().join("big.png");
        std::fs::write(&big, vec![0u8; (PREVIEW_MAX_BYTES + 1) as usize]).unwrap();
        allow.allow(std::slice::from_ref(&big));
        assert!(preview_for(&allow, big.to_str().unwrap())
            .unwrap()
            .is_none());
    }
}
