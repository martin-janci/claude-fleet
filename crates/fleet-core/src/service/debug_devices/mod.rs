//! Debug devices (`docs/debug-devices.md`): Android phones and emulators,
//! iOS simulators and iOS devices attached to the fleet's hosts, set up once
//! on the host they are plugged into and used from any session they are
//! visible to.
//!
//! - **Inventory.** A scan runs [`scripts::scan_script`] on a host and
//!   records what it found (`store::debug_devices`). A device that
//!   disappears stays as `missing`, with its label and sharing, until a
//!   person forgets it. [`list`] is cache-first: it answers the stored rows
//!   and rescans hosts whose last scan is older than [`STALE_SECS`] in the
//!   background.
//! - **Reach.** A person (the master, a paired device) sees the devices of
//!   every host its org scope sees. A host's own Claude (a per-host token)
//!   sees its host's devices, and a device a person marked `shared` on any
//!   host of the same org. Nothing else: the token of one host cannot share
//!   another host's device to itself, since `configure` is a person's.
//! - **Use.** Every operation runs on the device's own host, over the same
//!   SSH (or local spawn) the fleet runs everything else with, so a session
//!   on host B drives a phone plugged into host A without either host
//!   reaching the other. `run` takes a closed set of `adb` / `simctl` /
//!   `devicectl` verbs, each argument quoted; `install` copies an app from
//!   the caller's host to the device's host first.
//! - **Claims.** A claim is an advisory lease ([`DEFAULT_CLAIM_SECS`]): while
//!   it holds, only its holder uses the device, and each use extends it. A
//!   person may release anyone's claim.

pub mod scripts;

use crate::ipc_error::{codes, lock, IpcError};
use crate::service::view_scope::ViewScope;
use crate::ssh::SshExec;
use crate::store::{now_unix, DebugDeviceRow, SeenDevice, Store};
use scripts::{BootTarget, Bootable, RunOutput, Target};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// A host whose last scan is older than this is rescanned by [`list`].
pub const STALE_SECS: i64 = 300;
/// How long a claim lasts unless the claimer says otherwise; each use by
/// its holder extends it to at least this far ahead.
pub const DEFAULT_CLAIM_SECS: i64 = 30 * 60;
/// The longest claim one call may take.
pub const MAX_CLAIM_SECS: i64 = 24 * 3600;
/// The largest app `install` copies between hosts.
pub const MAX_INSTALL_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const SCAN_WALL: Duration = Duration::from_secs(45);
const MAX_LABEL_CHARS: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Android,
    Ios,
}

impl Platform {
    pub fn as_str(self) -> &'static str {
        match self {
            Platform::Android => "android",
            Platform::Ios => "ios",
        }
    }
}

/// One device as callers see it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DebugDevice {
    pub id: i64,
    /// The label if a person set one, else the device's own name.
    pub title: String,
    /// The host it is attached to; every command runs there.
    pub host: String,
    /// `android` | `ios`.
    pub platform: String,
    /// `physical` | `emulator` | `simulator`.
    pub kind: String,
    /// Stable per host: an adb serial, `avd:<name>`, or a UDID.
    pub key: String,
    /// What the host's tools address it by now (absent while a stopped
    /// emulator has none).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub serial: Option<String>,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub os_version: Option<String>,
    /// `online` | `booted` (ready) · `offline` | `unauthorized` |
    /// `shutdown` | `missing` (not).
    pub state: String,
    pub ready: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Sessions on other hosts of the same org may use it too.
    pub shared: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claimed_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claim_note: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claimed_until: Option<i64>,
    pub first_seen_at: i64,
    pub last_seen_at: i64,
}

fn is_ready(state: &str) -> bool {
    matches!(state, "online" | "booted")
}

impl DebugDevice {
    fn from_row(r: DebugDeviceRow, now: i64) -> Self {
        // An expired claim reads as no claim; the row is cleared on the
        // next claim or release.
        let live = r.claimed_until.is_some_and(|u| u > now);
        DebugDevice {
            id: r.id,
            title: r.label.clone().unwrap_or_else(|| r.name.clone()),
            host: r.host_alias,
            platform: r.platform,
            kind: r.kind,
            key: r.dev_key,
            serial: r.serial,
            name: r.name,
            model: r.model,
            os_version: r.os_version,
            ready: is_ready(&r.state),
            state: r.state,
            label: r.label,
            shared: r.shared,
            claimed_by: r.claimed_by.filter(|_| live),
            claim_note: r.claim_note.filter(|_| live),
            claimed_until: r.claimed_until.filter(|_| live),
            first_seen_at: r.first_seen_at,
            last_seen_at: r.last_seen_at,
        }
    }

    /// How a script addresses it, when it can be addressed now.
    fn target(&self) -> Option<Target> {
        let serial = self.serial.clone()?;
        Some(match (self.platform.as_str(), self.kind.as_str()) {
            ("android", _) => Target::Adb { serial },
            ("ios", "simulator") => Target::Simulator { udid: serial },
            ("ios", _) => Target::IosDevice { udid: serial },
            _ => return None,
        })
    }

    /// `host/title`, for messages.
    fn display(&self) -> String {
        format!("{}/{} (#{})", self.host, self.title, self.id)
    }
}

/// When each visible host was last scanned.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostScanState {
    pub host: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scanned_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceList {
    pub devices: Vec<DebugDevice>,
    pub hosts: Vec<HostScanState>,
}

/// What one host's scan found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostScan {
    pub host: String,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Devices attached or running now.
    pub found: usize,
    /// The host's tools fleet found (`adb`, `emulator`, `xcrun`).
    pub tools: Vec<String>,
    /// Emulators and simulators `boot` could start.
    pub bootable: Vec<Bootable>,
}

/// Who is asking, as far as devices go.
#[derive(Debug, Clone)]
pub struct Asker {
    pub scope: ViewScope,
    /// What a claim records as its holder: `host:<alias>` (with `#<session>`
    /// when the request proves its pane), `client:<name>` or `master`.
    pub holder: String,
}

impl Asker {
    /// A host's own Claude rather than a person.
    pub fn is_host(&self) -> bool {
        self.scope.host.is_some()
    }

    /// The desktop on its own store: every device, as a person.
    pub fn desktop() -> Self {
        Asker {
            scope: ViewScope::internal(),
            holder: "you (desktop)".into(),
        }
    }
}

/// The hosts `asker` may see devices on, with each host's org.
fn visible_hosts(s: &Store, asker: &Asker) -> Result<BTreeMap<String, Option<i64>>, IpcError> {
    let mut out = BTreeMap::new();
    for h in s.list_hosts()? {
        if h.hidden {
            continue;
        }
        let shown = match asker.scope.host.as_deref() {
            // A host token: its own host, and hosts of its own org (whose
            // SHARED devices it may use — `may_use` narrows to those).
            Some(own) => h.alias == own || h.org_id == s.host_org(own)?,
            // A person: the hosts its org scope sees.
            None => asker.scope.org.sees_org(h.org_id),
        };
        if shown {
            out.insert(h.alias, h.org_id);
        }
    }
    Ok(out)
}

fn may_use(asker: &Asker, d: &DebugDevice) -> bool {
    match asker.scope.host.as_deref() {
        Some(own) => d.host == own || d.shared,
        None => true,
    }
}

/// Every device `asker` may see, read from the store (no scan).
pub fn visible(s: &Store, asker: &Asker) -> Result<Vec<DebugDevice>, IpcError> {
    let hosts = visible_hosts(s, asker)?;
    let now = now_unix();
    Ok(s.debug_devices()?
        .into_iter()
        .filter(|r| hosts.contains_key(&r.host_alias))
        .map(|r| DebugDevice::from_row(r, now))
        .filter(|d| may_use(asker, d))
        .collect())
}

/// Hosts a scan for `asker` covers: a host token scans its own host only.
fn scannable_hosts(s: &Store, asker: &Asker, only: Option<&str>) -> Result<Vec<String>, IpcError> {
    let hosts: Vec<String> = match asker.scope.host.as_deref() {
        Some(own) => vec![own.to_string()],
        None => visible_hosts(s, asker)?.into_keys().collect(),
    };
    let hosts: Vec<String> = hosts
        .into_iter()
        .filter(|h| crate::service::hub::ensure_local_allowed(h).is_ok())
        .collect();
    match only {
        Some(one) if hosts.iter().any(|h| h == one) => Ok(vec![one.to_string()]),
        Some(one) => Err(IpcError::new(
            codes::E_NOTFOUND,
            format!("host {one} not found"),
        )),
        None => Ok(hosts),
    }
}

fn script_error(host: &str, what: &str, out: &std::process::Output) -> IpcError {
    let why = scripts::failure(&out.stderr).unwrap_or_else(|| {
        let e = String::from_utf8_lossy(&out.stderr);
        let e = e.trim();
        if e.is_empty() {
            format!("exit status {:?}", out.status.code())
        } else {
            e.chars().take(300).collect()
        }
    });
    IpcError::new(codes::E_SHELL, format!("{host}: {what}: {why}"))
}

/// Scan one host and record the result. A bootable device fleet already
/// inventoried (an AVD or simulator that ran before) is kept as
/// `shutdown` rather than turning `missing`.
pub async fn scan_host(store: &Mutex<Store>, ssh: &dyn SshExec, host: &str) -> HostScan {
    let res = crate::ssh::run_shell_bounded(
        ssh,
        host,
        &scripts::scan_script(),
        CONNECT_TIMEOUT,
        SCAN_WALL,
    )
    .await
    .and_then(|out| {
        if out.status.success() {
            scripts::parse_scan(&out.stdout)
        } else {
            Err(script_error(host, "device scan", &out))
        }
    });
    let now = now_unix();
    let fail = |e: &IpcError| HostScan {
        host: host.to_string(),
        ok: false,
        error: Some(e.message.clone()),
        found: 0,
        tools: vec![],
        bootable: vec![],
    };
    let scan = match res {
        Ok(scan) => scan,
        Err(e) => {
            if let Ok(s) = lock(store) {
                let _ = s.debug_devices_scan_failed(host, &e.message, now);
            }
            return fail(&e);
        }
    };
    let recorded = (|| -> Result<(), IpcError> {
        let s = lock(store)?;
        let known: BTreeSet<String> = s
            .debug_devices()?
            .into_iter()
            .filter(|r| r.host_alias == host)
            .map(|r| r.dev_key)
            .collect();
        let mut seen: Vec<SeenDevice> = scan.running.clone();
        for b in scan.bootable.iter().filter(|b| known.contains(&b.key)) {
            seen.push(SeenDevice {
                dev_key: b.key.clone(),
                platform: b.platform.clone(),
                kind: b.kind.clone(),
                serial: b.udid.clone(),
                name: b.name.clone(),
                model: None,
                os_version: b.os_version.clone(),
                state: "shutdown".into(),
            });
        }
        s.debug_devices_apply_scan(host, &seen, now)?;
        Ok(())
    })();
    match recorded {
        Ok(()) => HostScan {
            host: host.to_string(),
            ok: true,
            error: None,
            found: scan.running.len(),
            tools: scan.tools,
            bootable: scan.bootable,
        },
        Err(e) => fail(&e),
    }
}

/// Scan `hosts` concurrently.
pub async fn scan_hosts(
    store: &Mutex<Store>,
    ssh: &dyn SshExec,
    hosts: &[String],
) -> Vec<HostScan> {
    futures_util::future::join_all(hosts.iter().map(|h| scan_host(store, ssh, h))).await
}

/// Scan the hosts `asker` may scan (`only` one of them, if given).
pub async fn scan(
    store: &Mutex<Store>,
    ssh: &dyn SshExec,
    asker: &Asker,
    only: Option<&str>,
) -> Result<Vec<HostScan>, IpcError> {
    let hosts = {
        let s = lock(store)?;
        scannable_hosts(&s, asker, only)?
    };
    Ok(scan_hosts(store, ssh, &hosts).await)
}

/// Hosts whose background rescan is running, so a burst of lists starts
/// one per host.
fn in_flight() -> &'static Mutex<BTreeSet<String>> {
    static IN_FLIGHT: std::sync::OnceLock<Mutex<BTreeSet<String>>> = std::sync::OnceLock::new();
    IN_FLIGHT.get_or_init(Default::default)
}

/// Rescan, in the background, every host in `hosts` whose last scan is
/// older than [`STALE_SECS`] (or that was never scanned).
pub fn refresh_stale_in_background(
    store: Arc<Mutex<Store>>,
    ssh: Arc<dyn SshExec>,
    hosts: Vec<String>,
) {
    let stale: Vec<String> = {
        let Ok(s) = lock(&store) else { return };
        let Ok(scans) = s.debug_device_scans() else {
            return;
        };
        let now = now_unix();
        let mut busy = in_flight()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        hosts
            .into_iter()
            .filter(|h| {
                scans
                    .get(h)
                    .is_none_or(|sc| now - sc.scanned_at >= STALE_SECS)
            })
            .filter(|h| busy.insert(h.clone()))
            .collect()
    };
    if stale.is_empty() {
        return;
    }
    tokio::spawn(async move {
        let _ = scan_hosts(&store, &*ssh, &stale).await;
        let mut busy = in_flight()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for h in &stale {
            busy.remove(h);
        }
    });
}

/// The devices `asker` sees, with each visible host's last scan. With
/// `refresh`, scan first and wait; otherwise answer the stored rows and
/// rescan stale hosts in the background.
pub async fn list(
    store: &Arc<Mutex<Store>>,
    ssh: Arc<dyn SshExec>,
    asker: &Asker,
    refresh: bool,
) -> Result<DeviceList, IpcError> {
    let hosts = {
        let s = lock(store)?;
        scannable_hosts(&s, asker, None)?
    };
    if refresh {
        scan_hosts(store, &*ssh, &hosts).await;
    } else {
        refresh_stale_in_background(store.clone(), ssh, hosts);
    }
    let s = lock(store)?;
    let devices = visible(&s, asker)?;
    let scans = s.debug_device_scans()?;
    let hosts = visible_hosts(&s, asker)?
        .into_keys()
        .filter(|h| {
            asker.scope.host.is_none()
                || devices.iter().any(|d| &d.host == h)
                || asker.scope.host.as_deref() == Some(h)
        })
        .map(|h| {
            let sc = scans.get(&h);
            HostScanState {
                scanned_at: sc.map(|x| x.scanned_at),
                error: sc.and_then(|x| x.error.clone()),
                host: h,
            }
        })
        .collect();
    Ok(DeviceList { devices, hosts })
}

/// Find one device among those `asker` may use: by id, or by a label,
/// name, serial or key (case-insensitive), optionally as `host/that`.
pub fn resolve(s: &Store, asker: &Asker, reference: &str) -> Result<DebugDevice, IpcError> {
    let all = visible(s, asker)?;
    let r = reference.trim();
    if let Ok(id) = r.trim_start_matches('#').parse::<i64>() {
        if let Some(d) = all.iter().find(|d| d.id == id) {
            return Ok(d.clone());
        }
    }
    let (host, what) = match r.split_once('/') {
        Some((h, w)) if all.iter().any(|d| d.host == h) => (Some(h), w),
        _ => (None, r),
    };
    let eq = |a: &str| a.eq_ignore_ascii_case(what);
    let hits: Vec<&DebugDevice> = all
        .iter()
        .filter(|d| host.is_none_or(|h| d.host == h))
        .filter(|d| {
            d.label.as_deref().is_some_and(eq)
                || eq(&d.name)
                || d.serial.as_deref().is_some_and(eq)
                || eq(&d.key)
        })
        .collect();
    match hits.as_slice() {
        [one] => Ok((*one).clone()),
        [] => Err(IpcError::new(
            codes::E_NOTFOUND,
            format!("no debug device {r:?}; list shows the ones you can use"),
        )),
        many => Err(IpcError::new(
            codes::E_AMBIGUOUS,
            format!(
                "{r:?} matches {}; name one by id",
                many.iter()
                    .map(|d| d.display())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        )),
    }
}

/// Refuse when someone else holds a live claim; extend the holder's own.
fn check_claim(s: &Store, asker: &Asker, d: &DebugDevice) -> Result<(), IpcError> {
    match d.claimed_by.as_deref() {
        Some(h) if h != asker.holder => Err(IpcError::new(
            codes::E_CONFLICT,
            format!(
                "{} is claimed by {h}{} for {} more min; wait, or ask a person to release it",
                d.display(),
                d.claim_note
                    .as_deref()
                    .map(|n| format!(" ({n})"))
                    .unwrap_or_default(),
                (d.claimed_until.unwrap_or(0) - now_unix()).max(0) / 60 + 1
            ),
        )),
        Some(_) => {
            s.debug_device_extend_claim(d.id, &asker.holder, now_unix() + DEFAULT_CLAIM_SECS)?;
            Ok(())
        }
        None => Ok(()),
    }
}

/// The device, usable by `asker` now: visible, not claimed by another, and
/// ready (unless `any_state`).
fn usable(
    store: &Mutex<Store>,
    asker: &Asker,
    reference: &str,
    any_state: bool,
) -> Result<DebugDevice, IpcError> {
    let s = lock(store)?;
    let d = resolve(&s, asker, reference)?;
    check_claim(&s, asker, &d)?;
    if !any_state && !d.ready {
        return Err(IpcError::new(
            codes::E_INVALID_STATE,
            format!(
                "{} is {}{}",
                d.display(),
                d.state,
                match d.state.as_str() {
                    "unauthorized" => ": accept the USB debugging prompt on the phone, then scan",
                    "shutdown" => ": boot it first",
                    "missing" =>
                        ": it was not seen on the last scan; plug it in or start it, then scan",
                    _ => "",
                }
            ),
        ));
    }
    Ok(d)
}

fn target_of(d: &DebugDevice) -> Result<Target, IpcError> {
    d.target().ok_or_else(|| {
        IpcError::new(
            codes::E_INVALID_STATE,
            format!("{} has no address now; boot it first", d.display()),
        )
    })
}

/// Claim a device for `secs` (default [`DEFAULT_CLAIM_SECS`]).
pub fn claim(
    store: &Mutex<Store>,
    asker: &Asker,
    reference: &str,
    secs: Option<i64>,
    note: Option<&str>,
) -> Result<DebugDevice, IpcError> {
    let s = lock(store)?;
    let d = resolve(&s, asker, reference)?;
    if let Some(h) = d.claimed_by.as_deref().filter(|h| *h != asker.holder) {
        return Err(IpcError::new(
            codes::E_CONFLICT,
            format!("{} is already claimed by {h}", d.display()),
        ));
    }
    let secs = secs.unwrap_or(DEFAULT_CLAIM_SECS).clamp(60, MAX_CLAIM_SECS);
    let note = note
        .map(|n| n.trim().chars().take(200).collect::<String>())
        .filter(|n| !n.is_empty());
    s.debug_device_claim(d.id, &asker.holder, note.as_deref(), now_unix() + secs)?;
    reread(&s, d.id)
}

/// Release a claim: the holder's own, or any one by a person.
pub fn release(
    store: &Mutex<Store>,
    asker: &Asker,
    reference: &str,
) -> Result<DebugDevice, IpcError> {
    let s = lock(store)?;
    let d = resolve(&s, asker, reference)?;
    if let Some(h) = d.claimed_by.as_deref() {
        if h != asker.holder && asker.is_host() {
            return Err(IpcError::new(
                codes::E_FORBIDDEN,
                format!(
                    "{} is claimed by {h}; only its holder or a person releases it",
                    d.display()
                ),
            ));
        }
    }
    s.debug_device_release(d.id)?;
    reread(&s, d.id)
}

fn reread(s: &Store, id: i64) -> Result<DebugDevice, IpcError> {
    s.debug_device(id)?
        .map(|r| DebugDevice::from_row(r, now_unix()))
        .ok_or_else(|| IpcError::new(codes::E_NOTFOUND, format!("debug device #{id} not found")))
}

fn person_only(asker: &Asker, what: &str) -> Result<(), IpcError> {
    if asker.is_host() {
        return Err(IpcError::new(
            codes::E_FORBIDDEN,
            format!("{what} is a person's: set it in Debug devices on the desktop"),
        ));
    }
    Ok(())
}

/// A person's settings for a device: its label (empty clears it) and
/// whether other hosts' sessions may use it.
pub fn configure(
    store: &Mutex<Store>,
    asker: &Asker,
    id: i64,
    label: Option<&str>,
    shared: Option<bool>,
) -> Result<DebugDevice, IpcError> {
    person_only(asker, "a device's label and sharing")?;
    let s = lock(store)?;
    let d = resolve(&s, asker, &id.to_string())?;
    let label = label.map(|l| {
        let l = l.trim();
        if l.is_empty() {
            None
        } else {
            Some(l)
        }
    });
    if let Some(Some(l)) = label {
        if l.chars().count() > MAX_LABEL_CHARS || l.contains('/') {
            return Err(IpcError::new(
                codes::E_INVALID,
                format!("a label is at most {MAX_LABEL_CHARS} characters, with no '/'"),
            ));
        }
    }
    s.debug_device_configure(d.id, label, shared)?;
    reread(&s, d.id)
}

/// Drop a device from the inventory. A device still attached comes back
/// on the next scan, without its label or sharing.
pub fn forget(store: &Mutex<Store>, asker: &Asker, id: i64) -> Result<bool, IpcError> {
    person_only(asker, "forgetting a device")?;
    let s = lock(store)?;
    let d = resolve(&s, asker, &id.to_string())?;
    Ok(s.debug_device_forget(d.id)? > 0)
}

async fn run_on(
    ssh: &dyn SshExec,
    host: &str,
    what: &str,
    script: &str,
    wall: Duration,
) -> Result<std::process::Output, IpcError> {
    let out = crate::ssh::run_shell_bounded(ssh, host, script, CONNECT_TIMEOUT, wall).await?;
    if !out.status.success() {
        return Err(script_error(host, what, &out));
    }
    Ok(out)
}

/// The longest `run` may take, and its default.
pub const MAX_RUN_SECS: u64 = 600;
pub const DEFAULT_RUN_SECS: u64 = 60;

/// Run one `adb` / `simctl` / `devicectl` command against a device.
pub async fn run(
    store: &Mutex<Store>,
    ssh: &dyn SshExec,
    asker: &Asker,
    reference: &str,
    args: &[String],
    timeout_secs: Option<u64>,
) -> Result<RunOutput, IpcError> {
    let d = usable(store, asker, reference, false)?;
    let target = target_of(&d)?;
    scripts::check_run_args(&target, args)?;
    let wall = Duration::from_secs(
        timeout_secs
            .unwrap_or(DEFAULT_RUN_SECS)
            .clamp(1, MAX_RUN_SECS),
    );
    let out = run_on(
        ssh,
        &d.host,
        "run",
        &scripts::run_script(&target, args),
        wall,
    )
    .await?;
    scripts::parse_run(&out.stdout)
}

/// Recent log lines from a device (see [`scripts::logs_script`]).
#[allow(clippy::too_many_arguments)]
pub async fn logs(
    store: &Mutex<Store>,
    ssh: &dyn SshExec,
    asker: &Asker,
    reference: &str,
    lines: Option<u32>,
    since_secs: Option<u32>,
    filter: Option<&str>,
    contains: Option<&str>,
) -> Result<RunOutput, IpcError> {
    let d = usable(store, asker, reference, false)?;
    let target = target_of(&d)?;
    let script = scripts::logs_script(
        &target,
        lines.unwrap_or(200),
        since_secs.unwrap_or(300),
        filter,
        contains,
    )?;
    let out = run_on(ssh, &d.host, "logs", &script, Duration::from_secs(120)).await?;
    scripts::parse_run(&out.stdout)
}

/// A screenshot: the image bytes and their MIME type.
pub async fn screenshot(
    store: &Mutex<Store>,
    ssh: &dyn SshExec,
    asker: &Asker,
    reference: &str,
) -> Result<(DebugDevice, Vec<u8>, &'static str), IpcError> {
    let d = usable(store, asker, reference, false)?;
    let target = target_of(&d)?;
    let out = run_on(
        ssh,
        &d.host,
        "screenshot",
        &scripts::screenshot_script(&target),
        Duration::from_secs(60),
    )
    .await?;
    let (bytes, mime) = scripts::parse_screenshot(&out.stdout)?;
    Ok((d, bytes, mime))
}

/// Start an emulator or simulator: an inventoried device by reference, or
/// a bootable one by `host` and its name, AVD name or UDID (from a scan).
pub async fn boot(
    store: &Mutex<Store>,
    ssh: &dyn SshExec,
    asker: &Asker,
    reference: Option<&str>,
    host: Option<&str>,
    name: Option<&str>,
) -> Result<String, IpcError> {
    let (host, target, id) = match (reference, host, name) {
        (Some(r), _, _) => {
            let d = usable(store, asker, r, true)?;
            let t = if let Some(avd) = d.key.strip_prefix("avd:") {
                BootTarget::Avd {
                    name: avd.to_string(),
                }
            } else if d.kind == "simulator" {
                BootTarget::Simulator {
                    udid: d.key.clone(),
                }
            } else {
                return Err(IpcError::new(
                    codes::E_UNSUPPORTED,
                    format!(
                        "{} is a {} device; only emulators and simulators boot",
                        d.display(),
                        d.platform
                    ),
                ));
            };
            (d.host.clone(), t, Some(d.id))
        }
        (None, Some(h), Some(n)) => {
            if asker.is_host() && asker.scope.host.as_deref() != Some(h) {
                return Err(IpcError::new(
                    codes::E_FORBIDDEN,
                    "a host's session boots a device that is not inventoried yet only on its own host".to_string(),
                ));
            }
            let scans = scan(store, ssh, asker, Some(h)).await?;
            let found = scans.into_iter().next().filter(|s| s.ok).ok_or_else(|| {
                IpcError::new(
                    codes::E_SHELL,
                    format!("{h}: could not scan for bootable devices"),
                )
            })?;
            let b = found
                .bootable
                .iter()
                .find(|b| {
                    b.name.eq_ignore_ascii_case(n)
                        || b.key.eq_ignore_ascii_case(n)
                        || b.key
                            .strip_prefix("avd:")
                            .is_some_and(|k| k.eq_ignore_ascii_case(n))
                })
                .ok_or_else(|| {
                    IpcError::new(
                        codes::E_NOTFOUND,
                        format!(
                            "{h} has no stopped emulator or simulator {n:?}; bootable: {}",
                            found
                                .bootable
                                .iter()
                                .map(|b| b.name.as_str())
                                .collect::<Vec<_>>()
                                .join(", ")
                        ),
                    )
                })?;
            let t = match &b.udid {
                Some(u) => BootTarget::Simulator { udid: u.clone() },
                None => BootTarget::Avd {
                    name: b.key.trim_start_matches("avd:").to_string(),
                },
            };
            (h.to_string(), t, None)
        }
        _ => {
            return Err(IpcError::new(
                codes::E_INVALID,
                "boot needs device, or host and name".to_string(),
            ))
        }
    };
    let out = run_on(
        ssh,
        &host,
        "boot",
        &scripts::boot_script(&target),
        Duration::from_secs(120),
    )
    .await?;
    let said = scripts::first_line(&out.stdout).unwrap_or_default();
    if matches!(target, BootTarget::Simulator { .. }) {
        // The simulator is up: inventory it now, so the next call finds it.
        let _ = scan_host(store, ssh, &host).await;
    } else if let Some(id) = id {
        let s = lock(store)?;
        s.debug_device_set_state(id, "booting")?;
    }
    Ok(said)
}

/// Stop a running emulator or simulator.
pub async fn shutdown(
    store: &Mutex<Store>,
    ssh: &dyn SshExec,
    asker: &Asker,
    reference: &str,
) -> Result<DebugDevice, IpcError> {
    let d = usable(store, asker, reference, false)?;
    let script = scripts::shutdown_script(&target_of(&d)?)?;
    run_on(ssh, &d.host, "shutdown", &script, Duration::from_secs(60)).await?;
    let s = lock(store)?;
    s.debug_device_set_state(d.id, "shutdown")?;
    reread(&s, d.id)
}

/// Where an app to install is: a path on `from_host`.
pub struct InstallFrom<'a> {
    pub host: &'a str,
    pub path: &'a str,
}

/// Install an app on a device. When the app is on another host than the
/// device, it is packed there, copied through this machine in
/// [`scripts::RUN_OUTPUT_BYTES`]-independent chunks, and unpacked on the
/// device's host first; both staging folders are removed afterwards.
pub async fn install(
    store: &Mutex<Store>,
    ssh: &dyn SshExec,
    asker: &Asker,
    reference: &str,
    from: InstallFrom<'_>,
    downgrade: bool,
) -> Result<RunOutput, IpcError> {
    let d = usable(store, asker, reference, false)?;
    let target = target_of(&d)?;
    if from.path.trim().is_empty() || from.path.contains('\0') {
        return Err(IpcError::new(
            codes::E_INVALID,
            "install needs a path".to_string(),
        ));
    }
    if asker.is_host() && asker.scope.host.as_deref() != Some(from.host) {
        return Err(IpcError::new(
            codes::E_FORBIDDEN,
            "a host's session installs an app from its own host".to_string(),
        ));
    }
    {
        let s = lock(store)?;
        if !visible_hosts(&s, asker)?.contains_key(from.host)
            && asker.scope.host.as_deref() != Some(from.host)
        {
            return Err(IpcError::new(
                codes::E_NOTFOUND,
                format!("host {} not found", from.host),
            ));
        }
    }
    let wall = Duration::from_secs(MAX_RUN_SECS);
    if from.host == d.host {
        let out = run_on(
            ssh,
            &d.host,
            "install",
            &scripts::install_script(&target, from.path, downgrade),
            wall,
        )
        .await?;
        return scripts::parse_run(&out.stdout);
    }
    let id = uuid::Uuid::new_v4().simple().to_string();
    let src = stage(ssh, from.host, &format!("{id}-src")).await?;
    let result = async {
        let dst = stage(ssh, &d.host, &format!("{id}-dst")).await?;
        let copied = async {
            let packed = run_on(
                ssh,
                from.host,
                "pack",
                &scripts::pack_script(from.path, &src),
                wall,
            )
            .await?;
            let body =
                crate::service::move_session::carry::payload(&packed.stdout).unwrap_or_default();
            let text = String::from_utf8_lossy(body);
            let mut lines = text.lines();
            let size: u64 = lines
                .next()
                .and_then(|l| l.trim().parse().ok())
                .ok_or_else(|| {
                    IpcError::new(
                        codes::E_PARSE,
                        format!("{}: pack printed no size", from.host),
                    )
                })?;
            let name = lines.next().unwrap_or("").to_string();
            if size > MAX_INSTALL_BYTES {
                return Err(IpcError::new(
                    codes::E_LIMIT,
                    format!("the app is over {} MiB packed", MAX_INSTALL_BYTES >> 20),
                ));
            }
            let archive = format!("{src}/app.tgz");
            let chunk = crate::service::move_session::carry::CHUNK_BYTES;
            let mut got = 0u64;
            while got < size {
                let want = chunk.min(size - got);
                let out = crate::ssh::run_shell_bounded(
                    ssh,
                    from.host,
                    &crate::service::move_session::carry::chunk_script(&archive, got, want),
                    CONNECT_TIMEOUT,
                    wall,
                )
                .await?;
                let bytes = crate::service::move_session::carry::payload(&out.stdout)
                    .filter(|b| b.len() as u64 == want)
                    .ok_or_else(|| {
                        IpcError::new(codes::E_IO, format!("{}: short read of the app", from.host))
                    })?
                    .to_vec();
                let quoted = crate::shell::quote(&scripts::append_script(&dst));
                let out = ssh
                    .run_with_stdin(
                        &d.host,
                        &["bash", "-lc", &quoted],
                        bytes,
                        CONNECT_TIMEOUT,
                        wall,
                        4096,
                    )
                    .await?;
                if !out.status.success() {
                    return Err(script_error(&d.host, "copy", &out));
                }
                got += want;
            }
            let out = run_on(
                ssh,
                &d.host,
                "unpack",
                &scripts::unpack_script(&dst, &name),
                wall,
            )
            .await?;
            scripts::first_line(&out.stdout).ok_or_else(|| {
                IpcError::new(
                    codes::E_PARSE,
                    format!("{}: unpack printed no path", d.host),
                )
            })
        }
        .await;
        let res = match copied {
            Ok(path) => run_on(
                ssh,
                &d.host,
                "install",
                &scripts::install_script(&target, &path, downgrade),
                wall,
            )
            .await
            .and_then(|out| scripts::parse_run(&out.stdout)),
            Err(e) => Err(e),
        };
        let _ = crate::ssh::run_shell(
            ssh,
            &d.host,
            &scripts::cleanup_script(&dst),
            CONNECT_TIMEOUT,
        )
        .await;
        res
    }
    .await;
    let _ = crate::ssh::run_shell(
        ssh,
        from.host,
        &scripts::cleanup_script(&src),
        CONNECT_TIMEOUT,
    )
    .await;
    result
}

async fn stage(ssh: &dyn SshExec, host: &str, id: &str) -> Result<String, IpcError> {
    let out = run_on(
        ssh,
        host,
        "staging folder",
        &scripts::stage_dir_script(id),
        Duration::from_secs(30),
    )
    .await?;
    scripts::first_line(&out.stdout)
        .ok_or_else(|| IpcError::new(codes::E_PARSE, format!("{host}: no staging folder")))
}

#[cfg(test)]
mod tests;
