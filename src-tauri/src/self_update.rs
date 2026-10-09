//! This desktop updating itself (update-channel design S7).
//!
//! Who decides: a paired desktop asks its hub (`POST /update/check`, exempt
//! from the contract gate, §6.5); a standalone one reads the published
//! channel itself under its own `update.*` settings (Git mode). Either way
//! the target is proven against the release key by `verify_target` before
//! this module touches it, and only a target with a signed `tauri` artifact
//! for this platform installs in place.
//!
//! How it installs: `tauri-plugin-updater` downloads the bundle, checks its
//! minisign signature against the same release key a second time (the
//! signature and the key are the signed manifest's, not a server's), and
//! swaps the app. The plugin only takes an update from an HTTP endpoint, so
//! the verified decision is handed to it from a one-shot loopback listener
//! in this process — the reason `plugins.updater.dangerousInsecureTransportProtocol`
//! is on: it allows that `http://127.0.0.1` endpoint and nothing else, since
//! no other endpoint is ever configured, and the bundle itself still comes
//! from the signed `https` URL.
//!
//! What is reported (paired only): `downloading` → `installing` before the
//! restart, and `validating` → `success` (or `failed`) from the next launch,
//! which finds the attempt in `pending-update.json`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use fleet_core::ipc_error::{codes, IpcError};
use fleet_core::service::update as hub_update;
use fleet_core::store::Store;
use fleet_update::client_header::ClientHeader;
use fleet_update::manifest::Artifact;
use fleet_update::wire::{Installed, Speaks};
use fleet_update::{
    CheckOutcome, CheckRequest, Component, HubTransport, HubUpdateChannel, Platform, Report,
    SequenceStore, Track, UpdateChannel, UpdateError, UpdatePhase, VerifiedTarget, Version, Window,
    UPDATE_PROTO,
};
use serde::{Deserialize, Serialize};

use crate::backend::contract::{MAX_HUB_CONTRACT, MIN_HUB_CONTRACT};
use crate::backend::FleetBackend;

/// This build's version.
pub fn installed_version() -> Version {
    Version::parse(env!("CARGO_PKG_VERSION")).expect("the crate version is semver")
}

/// What this desktop runs on, in the manifest's terms: macOS installs the
/// `.app.tar.gz`, Windows the NSIS installer, Linux the AppImage when it was
/// started as one (`$APPIMAGE`), else it is a `.deb`, which only a person
/// installs.
pub fn platform() -> Platform {
    let arch = std::env::consts::ARCH;
    match std::env::consts::OS {
        "macos" => Platform::new("macos", arch, "tauri"),
        "windows" => Platform::new("windows", arch, "nsis"),
        os => {
            let variant = if std::env::var_os("APPIMAGE").is_some() {
                "appimage"
            } else {
                "deb"
            };
            Platform::new(os, arch, variant)
        }
    }
}

/// The `X-Fleet-Client` value every request to the hub carries (§6.3).
pub fn client_header() -> String {
    let p = platform();
    ClientHeader {
        component: Component::Desktop,
        version: installed_version(),
        platform: Some(Platform::new(&p.os, &p.arch, "")),
        build: option_env!("FLEET_GIT_SHA")
            .filter(|s| s.len() >= 7 && *s != "unknown")
            .map(|s| s[..7].to_string()),
        contract_accepts: Some(Window::new(MIN_HUB_CONTRACT, MAX_HUB_CONTRACT)),
    }
    .to_header_value()
}

fn check_request() -> CheckRequest {
    CheckRequest {
        update_proto: UPDATE_PROTO,
        component: Component::Desktop,
        platform: platform(),
        installed: Installed::version(installed_version()),
        speaks: Speaks {
            contract_accepts: Some(Window::new(MIN_HUB_CONTRACT, MAX_HUB_CONTRACT)),
            agent_proto: None,
        },
        phase: UpdatePhase::Idle,
        attempt: None,
    }
}

/// `HubTransport` over this window's own hub connection.
struct ViaHub(Arc<FleetBackend>);

#[async_trait]
impl HubTransport for ViaHub {
    async fn post(&self, path: &str, body: Vec<u8>) -> Result<Vec<u8>, UpdateError> {
        let hub = self
            .0
            .hub()
            .ok_or_else(|| UpdateError::Transport("this desktop is not paired".into()))?;
        let body = String::from_utf8(body).map_err(|e| UpdateError::Decode(e.to_string()))?;
        hub.post_update(path, body)
            .await
            .map(String::into_bytes)
            .map_err(|e| UpdateError::Transport(e.message))
    }
}

/// The replay guard (highest channel sequence seen per track), kept in the
/// app's data dir so a restart does not forget it.
struct FileSequences {
    path: PathBuf,
    seen: Mutex<BTreeMap<Track, u64>>,
}

impl FileSequences {
    fn open(dir: &Path) -> Self {
        let path = dir.join("update-sequences.json");
        let seen = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        FileSequences {
            path,
            seen: Mutex::new(seen),
        }
    }
}

impl SequenceStore for FileSequences {
    fn seen(&self, track: Track) -> u64 {
        let m = self.seen.lock().unwrap_or_else(|e| e.into_inner());
        m.get(&track).copied().unwrap_or(0)
    }

    fn record(&self, track: Track, sequence: u64) {
        let mut m = self.seen.lock().unwrap_or_else(|e| e.into_inner());
        let e = m.entry(track).or_insert(0);
        if sequence > *e {
            *e = sequence;
            if let Ok(b) = serde_json::to_vec(&*m) {
                let _ = std::fs::write(&self.path, b);
            }
        }
    }
}

/// How the frontend sees one check (`update_check`'s answer).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DesktopUpdate {
    /// `up_to_date`, `update_available`, `update_required`, `hold`, … (the
    /// decision's status, verbatim).
    pub status: String,
    /// `manual`, `notify` or `automatic`.
    pub mode: String,
    /// `hub` or `git`.
    pub source: String,
    pub installed: String,
    pub reason: String,
    /// The version offered, when there is one.
    pub version: Option<String>,
    pub mandatory: bool,
    pub deadline: Option<String>,
    /// `in_place` (Restart to update), `download` (a person installs it from
    /// `download_url`), or `none`.
    pub install: String,
    pub download_url: Option<String>,
    pub notes_url: Option<String>,
    pub next_check_secs: u64,
}

fn status_str<T: Serialize>(v: &T) -> String {
    serde_json::to_value(v)
        .ok()
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_default()
}

/// What an outcome means for this desktop.
pub fn describe(o: &CheckOutcome) -> DesktopUpdate {
    let d = &o.decision;
    let (install, download_url) = match o.verified.as_ref().map(|v| (&v.artifact, &v.url)) {
        Some((Artifact::Tauri { .. }, Some(url))) => ("in_place", Some(url.clone())),
        Some((Artifact::Download { .. }, Some(url))) => ("download", Some(url.clone())),
        _ => ("none", None),
    };
    DesktopUpdate {
        status: status_str(&d.status),
        mode: status_str(&d.mode),
        source: status_str(&d.source),
        installed: d.installed.to_string(),
        reason: d.reason.text.clone(),
        version: d.target.as_ref().map(|t| t.version.to_string()),
        mandatory: d.target.as_ref().is_some_and(|t| t.mandatory),
        deadline: d.target.as_ref().and_then(|t| t.deadline.clone()),
        install: install.into(),
        download_url,
        notes_url: o
            .verified
            .as_ref()
            .map(|v| v.manifest.release.notes_url.clone())
            .filter(|u| !u.is_empty()),
        next_check_secs: d.next_check_secs,
    }
}

/// Managed state: the last verified target, the only thing `update_install`
/// ever installs (the frontend names no URL and no signature).
#[derive(Default)]
pub struct SelfUpdate {
    last: Mutex<Option<CheckOutcome>>,
    installing: std::sync::atomic::AtomicBool,
}

/// An attempt that restarts the app, found again by the next launch.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Pending {
    attempt: String,
    from: Version,
    to: Version,
}

const PENDING_FILE: &str = "pending-update.json";

fn attempt_id() -> String {
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    format!(
        "{:012x}{:08x}",
        t.as_millis(),
        t.subsec_nanos() ^ std::process::id()
    )
}

fn report(
    phase: UpdatePhase,
    attempt: &str,
    from: &Version,
    to: &Version,
    error: Option<String>,
) -> Report {
    Report {
        update_proto: UPDATE_PROTO,
        component: Component::Desktop,
        installed: Installed::version(installed_version()),
        phase,
        attempt: Some(attempt.to_string()),
        from: Some(from.clone()),
        to: Some(to.clone()),
        detail: serde_json::Value::Null,
        error,
    }
}

/// The channel this desktop asks: its hub when paired, else the published
/// channel under its own settings.
fn hub_channel(backend: &Arc<FleetBackend>, data_dir: &Path) -> Option<HubUpdateChannel<ViaHub>> {
    backend.hub()?;
    Some(HubUpdateChannel::new(
        ViaHub(backend.clone()),
        hub_update::trusted_keys(),
        Box::new(FileSequences::open(data_dir)),
    ))
}

impl SelfUpdate {
    /// Ask, verify, and remember what may be installed.
    pub async fn check(
        &self,
        backend: &Arc<FleetBackend>,
        store: &Mutex<Store>,
        data_dir: &Path,
    ) -> Result<DesktopUpdate, IpcError> {
        self.finish_pending(backend, data_dir).await;
        let req = check_request();
        let outcome = match hub_channel(backend, data_dir) {
            Some(ch) => ch.check(&req).await.map_err(update_error)?,
            None => {
                let check = {
                    let s = fleet_core::ipc_error::lock(store)?;
                    hub_update::GitCheck::from_store(Some(&s), Component::Desktop, "local", None)?
                };
                let seen = FileSequences::open(data_dir);
                let mut check = check;
                check.seen = check.seen.max(seen.seen(check.track));
                let fetch = hub_update::HttpsFetch::new(Some(&check.base_url));
                let track = check.track;
                let out = hub_update::git_check(
                    fetch,
                    check,
                    hub_update::trusted_keys(),
                    &req,
                    fleet_core::store::now_unix(),
                )
                .await?;
                if let Some(v) = &out.verified {
                    seen.record(track, v.channel_sequence);
                }
                out
            }
        };
        let described = describe(&outcome);
        *self.last.lock().unwrap_or_else(|e| e.into_inner()) = Some(outcome);
        Ok(described)
    }

    /// Install the last verified target in place, and restart.
    pub async fn install<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        backend: &Arc<FleetBackend>,
        data_dir: &Path,
    ) -> Result<(), IpcError> {
        use std::sync::atomic::Ordering;
        let (target, mirror) = {
            let last = self.last.lock().unwrap_or_else(|e| e.into_inner());
            let o = last.as_ref();
            (
                o.and_then(|o| o.verified.clone())
                    .ok_or_else(|| IpcError::new(codes::E_INVALID, "check for an update first"))?,
                o.and_then(|o| o.decision.target.as_ref())
                    .and_then(|t| t.mirror.clone()),
            )
        };
        let (mut url, signature) = installable(&target)?;
        // The hub's mirror first when it offers one (S9): the same bytes,
        // checked here against the signed manifest's sha256 and size and
        // handed to the plugin from loopback; the plugin still checks the
        // bundle's signature. GitHub when the mirror fails.
        if let (Some(path), Some(hub)) = (mirror, backend.hub()) {
            match mirrored_bundle(hub, &path, &target.artifact, data_dir).await {
                Ok(bytes) => url = serve_bytes_once(bytes).await?.to_string(),
                Err(e) => {
                    tracing::warn!(error = %e.message, "the hub's mirror failed; downloading from the release")
                }
            }
        }
        if self.installing.swap(true, Ordering::SeqCst) {
            return Err(IpcError::new(
                codes::E_CONFLICT,
                "an update is already installing",
            ));
        }
        let result = self
            .install_inner(app, backend, data_dir, &target, url, signature)
            .await;
        self.installing.store(false, Ordering::SeqCst);
        result
    }

    async fn install_inner<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        backend: &Arc<FleetBackend>,
        data_dir: &Path,
        target: &VerifiedTarget,
        url: String,
        signature: String,
    ) -> Result<(), IpcError> {
        use tauri::Emitter;
        use tauri_plugin_updater::UpdaterExt;
        let pubkey =
            fleet_update::keys::tauri_pubkey_for(&signature, fleet_update::keys::RELEASE_KEYS)
                .ok_or_else(|| {
                    IpcError::new(
                        codes::E_UPDATE_UNVERIFIED,
                        "the bundle is not signed by a key this build trusts",
                    )
                })?;
        let from = installed_version();
        let to = target.version.clone();
        let attempt = attempt_id();
        let channel = hub_channel(backend, data_dir);
        let say = |phase: UpdatePhase, error: Option<String>| {
            let r = report(phase, &attempt, &from, &to, error);
            let channel = channel.as_ref();
            async move {
                if let Some(ch) = channel {
                    if let Err(e) = ch.report(&r).await {
                        tracing::warn!(error = %e, phase = r.phase.as_str(), "update report not taken");
                    }
                }
            }
        };

        let endpoint = serve_once(release_json(&to, &url, &signature)).await?;
        let fail = |e: String| IpcError::new(codes::E_HUB_UNREACHABLE, e);
        let updater = app
            .updater_builder()
            .endpoints(vec![endpoint])
            .and_then(|b| b.pubkey(pubkey).version_comparator(|_, _| true).build())
            .map_err(|e| fail(format!("the updater: {e}")))?;
        let update = updater
            .check()
            .await
            .map_err(|e| fail(format!("the updater: {e}")))?
            .ok_or_else(|| fail("the updater found nothing to install".into()))?;

        say(UpdatePhase::Downloading, None).await;
        let progress = app.clone();
        let mut done: u64 = 0;
        let bytes = update
            .download(
                move |chunk, total| {
                    done += chunk as u64;
                    let _ = progress.emit(
                        "update:progress",
                        serde_json::json!({ "downloaded": done, "total": total }),
                    );
                },
                || {},
            )
            .await;
        let bytes = match bytes {
            Ok(b) => b,
            Err(e) => {
                let why = format!("download or signature check failed: {e}");
                say(UpdatePhase::Failed, Some(why.clone())).await;
                return Err(IpcError::new(codes::E_UPDATE_UNVERIFIED, why));
            }
        };
        say(UpdatePhase::Verifying, None).await;
        say(UpdatePhase::Ready, None).await;
        let pending = Pending {
            attempt: attempt.clone(),
            from: from.clone(),
            to: to.clone(),
        };
        let _ = std::fs::write(
            data_dir.join(PENDING_FILE),
            serde_json::to_vec(&pending).unwrap_or_default(),
        );
        say(UpdatePhase::Installing, None).await;
        if let Err(e) = update.install(bytes) {
            let _ = std::fs::remove_file(data_dir.join(PENDING_FILE));
            let why = format!("install failed: {e}");
            say(UpdatePhase::Failed, Some(why.clone())).await;
            return Err(IpcError::new(codes::E_INTERNAL, why));
        }
        tracing::info!(%from, %to, "update installed; restarting");
        app.restart();
    }

    /// A launch after an install: report how it ended. Once per launch.
    async fn finish_pending(&self, backend: &Arc<FleetBackend>, data_dir: &Path) {
        let path = data_dir.join(PENDING_FILE);
        let Some(p) = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice::<Pending>(&b).ok())
        else {
            return;
        };
        let _ = std::fs::remove_file(&path);
        let Some(ch) = hub_channel(backend, data_dir) else {
            return;
        };
        let now = installed_version();
        let _ = ch
            .report(&report(
                UpdatePhase::Validating,
                &p.attempt,
                &p.from,
                &p.to,
                None,
            ))
            .await;
        let (phase, error) = if now == p.to {
            (UpdatePhase::Success, None)
        } else {
            (
                UpdatePhase::Failed,
                Some(format!("installed {}, but {now} started", p.to)),
            )
        };
        let _ = ch
            .report(&report(phase, &p.attempt, &p.from, &p.to, error))
            .await;
    }
}

fn update_error(e: UpdateError) -> IpcError {
    match e {
        UpdateError::Unverified(v) => IpcError::new(codes::E_UPDATE_UNVERIFIED, v.to_string()),
        other => IpcError::new(codes::E_HUB_UNREACHABLE, other.to_string()),
    }
}

/// The URL and the plugin signature of a target that installs in place.
fn installable(t: &VerifiedTarget) -> Result<(String, String), IpcError> {
    match (&t.artifact, &t.url) {
        (
            Artifact::Tauri {
                tauri_signature, ..
            },
            Some(url),
        ) => Ok((url.clone(), tauri_signature.clone())),
        (Artifact::Download { .. }, Some(url)) => Err(IpcError::new(
            codes::E_INVALID,
            format!("{} is installed by hand on this platform: {url}", t.version),
        )),
        _ => Err(IpcError::new(
            codes::E_INVALID,
            format!("{} has nothing this platform installs in place", t.version),
        )),
    }
}

/// The bundle from the hub's mirror, checked against the signed manifest.
async fn mirrored_bundle(
    hub: &crate::backend::remote::HubBackend,
    path: &str,
    artifact: &Artifact,
    data_dir: &Path,
) -> Result<Vec<u8>, IpcError> {
    let Artifact::Tauri { sha256, size, .. } = artifact else {
        return Err(IpcError::new(codes::E_INVALID, "not an in-place bundle"));
    };
    let part = data_dir.join("update-bundle.part");
    let got = hub.fetch_update_artifact(path, &part, *size).await;
    let bytes = std::fs::read(&part);
    let _ = std::fs::remove_file(&part);
    got?;
    let bytes = bytes.map_err(|e| IpcError::new(codes::E_INTERNAL, e.to_string()))?;
    let digest = fleet_update::verify::sha256_hex(&bytes);
    if &digest != sha256 || bytes.len() as u64 != *size {
        return Err(IpcError::new(
            codes::E_UPDATE_UNVERIFIED,
            format!(
                "the mirror sent {} bytes with sha256 {digest}; the manifest signed {size} with {sha256}",
                bytes.len()
            ),
        ));
    }
    Ok(bytes)
}

/// Serve the bundle once from loopback, for the plugin's download.
async fn serve_bytes_once(body: Vec<u8>) -> Result<url::Url, IpcError> {
    serve_loopback_once(body, "application/octet-stream", "bundle", 60).await
}

/// tauri-plugin-updater's "dynamic" release document for one target.
fn release_json(version: &Version, url: &str, signature: &str) -> String {
    serde_json::json!({ "version": version.to_string(), "url": url, "signature": signature })
        .to_string()
}

/// Serve `body` once from `127.0.0.1:<random port>` and return its URL.
async fn serve_once(body: String) -> Result<url::Url, IpcError> {
    serve_loopback_once(body.into_bytes(), "application/json", "update.json", 30).await
}

/// One response on a fresh loopback port: the first connection from this
/// machine within `wait_secs` gets `body`, and the listener is gone.
async fn serve_loopback_once(
    body: Vec<u8>,
    content_type: &'static str,
    name: &str,
    wait_secs: u64,
) -> Result<url::Url, IpcError> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .map_err(|e| IpcError::new(codes::E_INTERNAL, format!("loopback listener: {e}")))?;
    let port = listener
        .local_addr()
        .map_err(|e| IpcError::new(codes::E_INTERNAL, e.to_string()))?
        .port();
    tokio::spawn(async move {
        let accept =
            tokio::time::timeout(std::time::Duration::from_secs(wait_secs), listener.accept());
        if let Ok(Ok((mut conn, peer))) = accept.await {
            // Only this process asks; anything else on the machine is refused.
            if !peer.ip().is_loopback() {
                return;
            }
            let mut buf = [0u8; 4096];
            let _ = conn.read(&mut buf).await;
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = conn.write_all(head.as_bytes()).await;
            let _ = conn.write_all(&body).await;
            let _ = conn.shutdown().await;
        }
    });
    format!("http://127.0.0.1:{port}/{name}")
        .parse()
        .map_err(|e| IpcError::new(codes::E_INTERNAL, format!("{e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_header_names_this_build_and_its_contract_window() {
        let h = client_header();
        assert!(
            h.starts_with(&format!("desktop/{} (", env!("CARGO_PKG_VERSION"))),
            "{h}"
        );
        assert!(
            h.contains(&format!("contract {MIN_HUB_CONTRACT}-{MAX_HUB_CONTRACT}")),
            "{h}"
        );
        let parsed = ClientHeader::parse(&h).expect("the hub parses what the desktop sends");
        assert_eq!(parsed.component, Component::Desktop);
    }

    /// The plugin's configured key is the release key (each install also
    /// passes the key that signed its bundle, `tauri_pubkey_for`).
    #[test]
    fn the_updater_plugin_trusts_the_release_key() {
        let conf: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("tauri.conf.json"))
                .unwrap(),
        )
        .unwrap();
        let u = &conf["plugins"]["updater"];
        assert_eq!(
            u["pubkey"].as_str(),
            Some(fleet_update::keys::tauri_pubkey(fleet_update::keys::RELEASE_KEYS[0]).as_str())
        );
        // No endpoint of its own: every update comes from a verified decision.
        assert_eq!(u["endpoints"], serde_json::json!([]));
        assert_eq!(u["requireSignedVersion"], serde_json::json!(true));
    }

    #[test]
    fn the_platform_is_one_the_manifest_has_artifacts_for() {
        let p = platform();
        assert!(["macos", "windows", "linux"].contains(&p.os.as_str()));
        assert!(["tauri", "nsis", "appimage", "deb"].contains(&p.variant.as_str()));
    }

    #[tokio::test]
    async fn the_loopback_endpoint_serves_the_release_once() {
        let url = serve_once(release_json(
            &Version::new(0, 5, 5),
            "https://github.com/x/claude-fleet_0.5.5_aarch64.app.tar.gz",
            "c2ln",
        ))
        .await
        .unwrap();
        assert_eq!(url.host_str(), Some("127.0.0.1"));
        let mut s = tokio::net::TcpStream::connect(("127.0.0.1", url.port().unwrap()))
            .await
            .unwrap();
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        s.write_all(b"GET /update.json HTTP/1.1\r\nHost: x\r\n\r\n")
            .await
            .unwrap();
        let mut out = String::new();
        s.read_to_string(&mut out).await.unwrap();
        let body = out.split("\r\n\r\n").nth(1).unwrap();
        let v: serde_json::Value = serde_json::from_str(body).unwrap();
        assert_eq!(v["version"], "0.5.5");
        assert_eq!(v["signature"], "c2ln");
        // Once: the listener is gone.
        assert!(
            tokio::net::TcpStream::connect(("127.0.0.1", url.port().unwrap()))
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn a_mirrored_bundle_is_served_to_the_plugin_byte_for_byte() {
        let bundle: Vec<u8> = (0..=255u8).cycle().take(70_000).collect();
        let url = serve_bytes_once(bundle.clone()).await.unwrap();
        assert_eq!(url.path(), "/bundle");
        let mut s = tokio::net::TcpStream::connect(("127.0.0.1", url.port().unwrap()))
            .await
            .unwrap();
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        s.write_all(b"GET /bundle HTTP/1.1\r\nHost: x\r\n\r\n")
            .await
            .unwrap();
        let mut out = Vec::new();
        s.read_to_end(&mut out).await.unwrap();
        let at = out.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
        assert_eq!(&out[at..], bundle.as_slice());
    }

    #[test]
    fn only_a_signed_tauri_bundle_installs_in_place() {
        let m: fleet_update::ReleaseManifest = serde_json::from_str(
            &fleet_update::testkit::manifest_json("0.5.5", 14, [14, 14], 1, [1, 1]),
        )
        .unwrap();
        let t = |artifact: Artifact, url: Option<&str>| VerifiedTarget {
            component: Component::Desktop,
            version: Version::new(0, 5, 5),
            artifact,
            url: url.map(String::from),
            manifest: m.clone(),
            channel_sequence: 1,
        };
        let tauri = Artifact::Tauri {
            platform: "macos-aarch64".into(),
            variant: None,
            name: "n".into(),
            sha256: "s".into(),
            size: 1,
            tauri_signature: "sig".into(),
        };
        assert_eq!(
            installable(&t(tauri, Some("https://u"))).unwrap(),
            ("https://u".into(), "sig".into())
        );
        let deb = Artifact::Download {
            platform: "linux-x86_64".into(),
            variant: Some("deb".into()),
            name: "n".into(),
            sha256: "s".into(),
            size: 1,
        };
        assert_eq!(
            installable(&t(deb, Some("https://d"))).unwrap_err().code,
            codes::E_INVALID
        );
    }
}
