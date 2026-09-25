//! Replay-ring pressure under tracker sync (work graph M10.6).
//!
//! The hub's `/events` replay ring ([`crate::events::REPLAY_RING`], 512
//! slots) is shared by every event kind. This drives the real sync tick over
//! a deterministic fake Jira Cloud (two trackers, 200 items each), a
//! baseline of session frames at the rate `events.rs` measured on a busy
//! fleet, and a real [`BroadcastEventBus`], and measures:
//!
//! * frames per kind per sync pass and per minute;
//! * how long the ring reaches back, right after a sync burst (the worst
//!   moment for a phone that resumes) and on average;
//! * whether a pass emits a `work:item` for an item that did not change, or
//!   a `session:updated` whose row did not change;
//! * whether one item change fans out to more than one frame per item and
//!   per session within one pass.
//!
//! It asserts upper bounds so it keeps guarding. The numbers print with
//! `cargo test -p fleet-core ring_pressure -- --nocapture`; they are
//! recorded in `docs/superpowers/reviews/2026-09-25-replay-ring-pressure.md`.

use super::*;
use crate::events::{BroadcastEventBus, EventBus, RowChange, REPLAY_RING};
use crate::net::https::{HttpTransport, Method, Request, Response, TransportError};
use crate::service::trackers::TrackerNet;
use crate::store::{TrackerConfig, WorkTarget};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::atomic::AtomicI64;

const T0: i64 = 1_790_000_000;
/// Items per tracker.
const BOARD: usize = 200;
/// The busy-fleet churn `events.rs` measured (frames a second, every kind).
const BASELINE_FPS: f64 = 0.64;
/// The plan's threshold: a session frame must stay replayable this long.
const MIN_COVER_SECS: i64 = 5 * 60;

// --- the simulated clock ----------------------------------------------------

/// `TrackerSync::with_clock` takes a `fn`, so the clock is a static. Only
/// this module reads it, and its tests run one after another under
/// [`SERIAL`].
static SIM_NOW: AtomicI64 = AtomicI64::new(T0);
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn sim_now() -> i64 {
    SIM_NOW.load(Ordering::SeqCst)
}

/// Unix seconds → `2026-09-21T12:00:00.000Z` (Jira's `updated`).
fn iso(t: i64) -> String {
    let days = t.div_euclid(86_400);
    let secs = t.rem_euclid(86_400);
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.000Z",
        secs / 3600,
        secs % 3600 / 60,
        secs % 60
    )
}

/// A deterministic xorshift, so every run churns the same items.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

// --- the fake board ---------------------------------------------------------

#[derive(Clone)]
struct Issue {
    id: String,
    key: String,
    title: String,
    /// (name, statusCategory key)
    status: (&'static str, &'static str),
    assignee: &'static str,
    description: String,
    updated: i64,
}

const STATUSES: [(&str, &str); 4] = [
    ("To Do", "new"),
    ("In Progress", "indeterminate"),
    ("In Review", "indeterminate"),
    ("Done", "done"),
];
const PEOPLE: [&str; 4] = ["Dev A", "Dev B", "Dev C", "Dev D"];

impl Issue {
    fn json(&self, project: &str) -> Value {
        json!({
            "id": self.id,
            "key": self.key,
            "fields": {
                "summary": self.title,
                "status": {"name": self.status.0, "statusCategory": {"key": self.status.1}},
                "resolution": if self.status.1 == "done" { json!({"name": "Done"}) } else { Value::Null },
                "issuetype": {"name": "Story", "hierarchyLevel": 0},
                "assignee": {"displayName": self.assignee, "accountId": format!("acc-{}", self.assignee)},
                "updated": iso(self.updated),
                "project": {"key": project},
                "description": self.description,
            }
        })
    }
}

/// How an item changes between passes. Jira bumps `updated` on every one,
/// including a comment or a field fleet does not read (`Touch`).
#[derive(Clone, Copy, Debug)]
enum Churn {
    Status,
    Title,
    Description,
    Assignee,
    Touch,
}

impl Churn {
    /// 35 % status, 10 % title, 15 % description, 15 % assignee, 25 % touch.
    fn pick(rng: &mut Rng) -> Churn {
        match rng.below(100) {
            0..=34 => Churn::Status,
            35..=44 => Churn::Title,
            45..=59 => Churn::Description,
            60..=74 => Churn::Assignee,
            _ => Churn::Touch,
        }
    }
}

struct Site {
    project: &'static str,
    issues: Vec<Issue>,
    /// The newest `updated` a listing has served: the sync's watermark.
    served_max: i64,
    /// The window of the listing in progress (fixed at its first page).
    window: Option<i64>,
}

/// A fake Jira Cloud per site: `/search/jql` (whole, or incremental from the
/// watermark minus the sync's overlap, paged by 100) and
/// `/issue/bulkfetch`. The window is the one the sync asks for; the fake
/// derives it from the watermark it served rather than parsing Jira's
/// relative `-Nm`, which the adapter computes from the wall clock.
struct Board {
    sites: Mutex<HashMap<String, Site>>,
}

impl Board {
    fn new() -> Board {
        let mut sites = HashMap::new();
        for (host, project, base) in [("acme", "ABC", 10_000), ("beta", "XYZ", 20_000)] {
            let issues = (0..BOARD)
                .map(|i| Issue {
                    id: (base + i).to_string(),
                    key: format!("{project}-{}", i + 1),
                    title: format!("{project} story {}", i + 1),
                    status: STATUSES[i % 3],
                    assignee: PEOPLE[i % PEOPLE.len()],
                    description: format!("Acceptance criteria for story {}.", i + 1),
                    updated: T0 - 86_400 - i as i64 * 60,
                })
                .collect();
            sites.insert(
                format!("https://{host}.atlassian.net"),
                Site {
                    project,
                    issues,
                    served_max: 0,
                    window: None,
                },
            );
        }
        Board {
            sites: Mutex::new(sites),
        }
    }

    /// Change `n` distinct items of every site at times in `(from, to]`.
    /// Returns every changed `external_id`.
    fn churn(&self, rng: &mut Rng, n: usize, from: i64, to: i64) -> HashSet<String> {
        let mut changed = HashSet::new();
        let mut sites = self.sites.lock().unwrap();
        let mut hosts: Vec<&String> = sites.keys().collect();
        hosts.sort();
        let hosts: Vec<String> = hosts.into_iter().cloned().collect();
        for host in hosts {
            let site = sites.get_mut(&host).unwrap();
            let mut picked = HashSet::new();
            while picked.len() < n {
                picked.insert(rng.below(BOARD as u64) as usize);
            }
            let mut picked: Vec<usize> = picked.into_iter().collect();
            picked.sort();
            for i in picked {
                let it = &mut site.issues[i];
                match Churn::pick(rng) {
                    Churn::Status => {
                        let at = STATUSES.iter().position(|s| *s == it.status).unwrap();
                        it.status = STATUSES[(at + 1) % STATUSES.len()];
                    }
                    Churn::Title => it.title.push('!'),
                    Churn::Description => it.description.push_str(" More."),
                    Churn::Assignee => {
                        let at = PEOPLE.iter().position(|p| *p == it.assignee).unwrap();
                        it.assignee = PEOPLE[(at + 1) % PEOPLE.len()];
                    }
                    Churn::Touch => {}
                }
                it.updated = from + 1 + rng.below((to - from) as u64) as i64;
                changed.insert(it.id.clone());
            }
        }
        changed
    }

    /// One change of one named item (the fan-out probe).
    fn move_status(&self, key: &str, at: i64) -> String {
        let mut sites = self.sites.lock().unwrap();
        for site in sites.values_mut() {
            if let Some(it) = site.issues.iter_mut().find(|i| i.key == key) {
                it.status = if it.status.1 == "done" {
                    STATUSES[0]
                } else {
                    STATUSES[3]
                };
                it.updated = at;
                return it.id.clone();
            }
        }
        panic!("no {key}");
    }
}

#[async_trait::async_trait]
impl HttpTransport for Board {
    async fn send(&self, req: Request) -> Result<Response, TransportError> {
        let mut sites = self.sites.lock().unwrap();
        let (base, site) = sites
            .iter_mut()
            .find(|(b, _)| req.url.starts_with(b.as_str()))
            .ok_or_else(|| TransportError::Connect(format!("no site for {}", req.url)))?;
        let path = &req.url[base.len()..];
        let body = req.json_body().unwrap_or(Value::Null);
        match (req.method, path) {
            (Method::Post, "/rest/api/3/search/jql") => {
                let jql = body["jql"].as_str().unwrap_or_default();
                let cursor: usize = body["nextPageToken"]
                    .as_str()
                    .and_then(|c| c.parse().ok())
                    .unwrap_or(0);
                if cursor == 0 {
                    site.window = jql
                        .contains("updated >= -")
                        .then(|| site.served_max - OVERLAP_SECS);
                }
                let mut hits: Vec<&Issue> = site
                    .issues
                    .iter()
                    .filter(|i| site.window.is_none_or(|w| i.updated >= w))
                    .collect();
                hits.sort_by_key(|i| (std::cmp::Reverse(i.updated), i.id.clone()));
                if let Some(max) = hits.first().map(|i| i.updated) {
                    site.served_max = site.served_max.max(max);
                }
                let page: Vec<Value> = hits
                    .iter()
                    .skip(cursor)
                    .take(crate::service::trackers::jira::PAGE_SIZE)
                    .map(|i| i.json(site.project))
                    .collect();
                let next = cursor + page.len();
                let last = next >= hits.len();
                Ok(Response::json(
                    200,
                    &json!({"issues": page, "isLast": last, "nextPageToken": next.to_string()}),
                ))
            }
            (Method::Post, "/rest/api/3/issue/bulkfetch") => {
                let asked: HashSet<&str> = body["issueIdsOrKeys"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .collect();
                let found: Vec<Value> = site
                    .issues
                    .iter()
                    .filter(|i| asked.contains(i.id.as_str()) || asked.contains(i.key.as_str()))
                    .map(|i| i.json(site.project))
                    .collect();
                Ok(Response::json(200, &json!({"issues": found})))
            }
            _ => Err(TransportError::Connect(format!("unscripted {path}"))),
        }
    }
}

// --- the meter --------------------------------------------------------------

/// One frame as the meter saw it.
struct Frame {
    name: &'static str,
    at: i64,
    bytes: usize,
    /// During a sync pass (not the baseline).
    sync: bool,
    /// `work:item`: the item's `external_id`; `session:updated`: the id.
    subject: Option<String>,
    /// `session:updated` whose row, `row_version` aside, is the one the
    /// last frame for that session carried.
    same_row: bool,
}

/// Tees every emit into a real [`BroadcastEventBus`] (its ring is what is
/// measured) and a log stamped with the simulated time.
struct Meter {
    ring: BroadcastEventBus,
    frames: Mutex<Vec<Frame>>,
    in_sync: AtomicBool,
    last_session: Mutex<HashMap<i64, Value>>,
}

impl Meter {
    fn new() -> Meter {
        Meter {
            ring: BroadcastEventBus::default(),
            frames: Mutex::new(Vec::new()),
            in_sync: AtomicBool::new(false),
            last_session: Mutex::new(HashMap::new()),
        }
    }

    /// The simulated time of the oldest frame the ring can still replay.
    fn oldest_replayable_at(&self) -> Option<i64> {
        let frames = self.frames.lock().unwrap();
        let total = frames.len() as u64;
        let after = total.saturating_sub(REPLAY_RING as u64);
        let replay = self.ring.replay_after(self.ring.generation(), after)?;
        // The ring and the log agree frame for frame.
        assert_eq!(replay.len() as u64, total - after);
        assert!(
            after == 0
                || self
                    .ring
                    .replay_after(self.ring.generation(), after - 1)
                    .is_none()
        );
        for (m, f) in replay.iter().zip(frames[after as usize..].iter()) {
            assert_eq!(m.name, f.name);
        }
        frames.get(after as usize).map(|f| f.at)
    }
}

impl EventBus for Meter {
    fn emit(&self, e: &RowChange) {
        self.ring.emit(e);
        let mut payload = e.payload();
        crate::json::strip_nulls(&mut payload);
        let bytes = serde_json::to_string(&payload)
            .map(|s| s.len())
            .unwrap_or(0);
        let (subject, same_row) = match e {
            RowChange::WorkItemUpdated(r) => (r.external_id.clone(), false),
            RowChange::SessionUpdated(r) | RowChange::SessionCreated(r) => {
                let mut v = payload.clone();
                if let Value::Object(m) = &mut v {
                    m.remove("row_version");
                }
                let prev = self.last_session.lock().unwrap().insert(r.id, v.clone());
                (Some(r.id.to_string()), prev.as_ref() == Some(&v))
            }
            _ => (None, false),
        };
        self.frames.lock().unwrap().push(Frame {
            name: e.name(),
            at: sim_now(),
            bytes,
            sync: self.in_sync.load(Ordering::SeqCst),
            subject,
            same_row,
        });
    }
}

// --- the scenario -----------------------------------------------------------

struct Scenario {
    label: &'static str,
    interval_secs: i64,
    /// Items changed per tracker between two passes.
    churn_per_pass: usize,
    passes: usize,
}

#[derive(Default, Debug)]
struct Report {
    first_pass_frames: usize,
    first_pass_cover_after: i64,
    per_pass: Vec<BTreeMap<&'static str, usize>>,
    per_pass_bytes: Vec<usize>,
    changed_per_pass: Vec<usize>,
    noop_item_frames: usize,
    noop_session_frames: usize,
    sync_session_frames: usize,
    max_frames_per_item: usize,
    max_frames_per_session: usize,
    cover_after_burst: Vec<i64>,
    cover_before_pass: Vec<i64>,
    per_minute: BTreeMap<&'static str, f64>,
}

struct Fleet {
    store: Mutex<Store>,
    meter: Arc<Meter>,
    board: Arc<Board>,
    sync: TrackerSync,
    /// The session rows the baseline re-emits (reconcile, status changes).
    sessions: Vec<i64>,
    /// Keeps the bus recording (it skips rendering with no subscriber).
    _rx: tokio::sync::broadcast::Receiver<crate::events::EventMessage>,
}

/// Linked sessions: `(tmux name, item key)`. Three share `ABC-7` (a pair
/// and a reviewer), the rest one item each, on both trackers.
const LINKS: [(&str, &str); 10] = [
    ("abc-7-api", "ABC-7"),
    ("abc-7-ui", "ABC-7"),
    ("abc-7-review", "ABC-7"),
    ("abc-12", "ABC-12"),
    ("abc-40", "ABC-40"),
    ("abc-101", "ABC-101"),
    ("xyz-3", "XYZ-3"),
    ("xyz-58", "XYZ-58"),
    ("xyz-77", "XYZ-77"),
    ("xyz-150", "XYZ-150"),
];
/// Sessions with no work (they only feed the baseline).
const UNLINKED: usize = 10;

impl Fleet {
    async fn new() -> Fleet {
        SIM_NOW.store(T0, Ordering::SeqCst);
        let meter = Arc::new(Meter::new());
        let rx = meter.ring.subscribe();
        let s = Store::open_with_bus_in_memory(meter.clone()).unwrap();
        for (host, name, prefix) in [("acme", "Acme", "ABC"), ("beta", "Beta", "XYZ")] {
            let t = s
                .add_tracker("jira", name, &format!("https://{host}.atlassian.net"))
                .unwrap()
                .id;
            s.set_tracker_credential(
                t,
                "basic",
                Some("dev@example.com"),
                Some("tok-0123456789abc"),
                None,
            )
            .unwrap();
            s.set_tracker_probe(
                t,
                Some(host),
                &TrackerConfig {
                    account_id: Some("acc-Dev A".into()),
                    key_prefixes: vec![prefix.into()],
                    ..Default::default()
                },
            )
            .unwrap();
            s.sync_tracker_views(
                t,
                &[(
                    "board".into(),
                    "Board".into(),
                    format!("project = {prefix}"),
                )],
            )
            .unwrap();
            s.set_tracker_state(t, "ok", None).unwrap();
        }
        s.upsert_host("h").unwrap();
        let board = Arc::new(Board::new());
        let sync = TrackerSync::new(TrackerNet::fake(board.clone())).with_clock(sim_now);
        Fleet {
            store: Mutex::new(s),
            meter,
            board,
            sync,
            sessions: Vec::new(),
            _rx: rx,
        }
    }

    async fn pass(&self) {
        self.meter.in_sync.store(true, Ordering::SeqCst);
        let out = self.sync.run_pass(&self.store).await.unwrap();
        self.meter.in_sync.store(false, Ordering::SeqCst);
        for p in &out {
            assert_eq!(p.error, None, "{p:?}");
        }
    }

    /// Sessions, and their links once the items are cached.
    fn seed_sessions(&mut self) {
        let s = self.store.lock().unwrap();
        let names = LINKS
            .iter()
            .map(|(n, _)| n.to_string())
            .chain((0..UNLINKED).map(|i| format!("idle-{i}")));
        for (i, name) in names.enumerate() {
            let sid = s
                .upsert_session(&name, "h", None, None, 1, 1, "running", None)
                .unwrap();
            s.conn_for_test()
                .execute(
                    "UPDATE sessions SET claude_session_id = ?1 WHERE id = ?2",
                    rusqlite::params![format!("conv-{i}"), sid],
                )
                .unwrap();
            self.sessions.push(sid);
        }
        for (i, (_, key)) in LINKS.iter().enumerate() {
            s.link_session_work(self.sessions[i], WorkTarget::Key(key), "manual")
                .unwrap();
        }
    }

    /// `n` session frames spread evenly over `(from, to]`, round-robin over
    /// the sessions: the reconcile tick and status changes.
    fn baseline(&self, from: i64, to: i64, n: usize) {
        let s = self.store.lock().unwrap();
        for k in 0..n {
            let at = from + ((to - from) * (k as i64 + 1)) / n as i64;
            SIM_NOW.store(at, Ordering::SeqCst);
            let sid = self.sessions[k % self.sessions.len()];
            // A real change (activity moved), so the row is not "the same".
            s.conn_for_test()
                .execute(
                    "UPDATE sessions SET last_activity_at = ?1 WHERE id = ?2",
                    rusqlite::params![at, sid],
                )
                .unwrap();
            let row = s.get_session_by_id(sid).unwrap().unwrap();
            self.meter.session_updated(&row);
        }
    }

    fn frames_since(&self, from: usize) -> Vec<(&'static str, usize, Option<String>, bool)> {
        self.meter.frames.lock().unwrap()[from..]
            .iter()
            .filter(|f| f.sync)
            .map(|f| (f.name, f.bytes, f.subject.clone(), f.same_row))
            .collect()
    }

    fn frame_count(&self) -> usize {
        self.meter.frames.lock().unwrap().len()
    }
}

async fn run(sc: &Scenario) -> Report {
    let mut fleet = Fleet::new().await;
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let mut rep = Report::default();
    let baseline_per_pass = (BASELINE_FPS * sc.interval_secs as f64).round() as usize;

    // Pass 0: a new hub's first sync (every item is new).
    let start = fleet.frame_count();
    fleet.pass().await;
    rep.first_pass_frames = fleet.frames_since(start).len();
    fleet.seed_sessions();
    // The first sync and the links happened together; let a full interval
    // of baseline wash them through before steady state is measured.
    let mut t = T0;
    rep.first_pass_cover_after = {
        // What the ring holds right after the first pass, had the fleet
        // been running for a while before it.
        let mut fresh = Fleet::new().await;
        // Sessions whose keys were typed before the trackers were added.
        fresh.seed_sessions();
        fresh.baseline(T0 - 3600, T0, (BASELINE_FPS * 3600.0) as usize);
        SIM_NOW.store(T0, Ordering::SeqCst);
        fresh.pass().await;
        T0 - fresh.meter.oldest_replayable_at().unwrap()
    };
    SIM_NOW.store(T0, Ordering::SeqCst);
    fleet.baseline(t, t + sc.interval_secs, baseline_per_pass);
    t += sc.interval_secs;
    SIM_NOW.store(t, Ordering::SeqCst);
    fleet.pass().await; // the links' first refresh; not counted

    let mut sync_minutes = 0.0;
    let mut kinds_total: BTreeMap<&'static str, usize> = BTreeMap::new();
    // Twenty minutes of churn first, unmeasured, so the first sync has
    // left the ring before steady state is read.
    let warmup = (1200 / sc.interval_secs) as usize;
    for pass in 0..warmup + sc.passes {
        let next = t + sc.interval_secs;
        let changed = fleet.board.churn(&mut rng, sc.churn_per_pass, t, next);
        fleet.baseline(t, next, baseline_per_pass);
        t = next;
        SIM_NOW.store(t, Ordering::SeqCst);
        if pass < warmup {
            fleet.pass().await;
            continue;
        }
        if let Some(oldest) = fleet.meter.oldest_replayable_at() {
            rep.cover_before_pass.push(t - oldest);
        }
        let start = fleet.frame_count();
        fleet.pass().await;
        rep.cover_after_burst
            .push(t - fleet.meter.oldest_replayable_at().unwrap());

        let frames = fleet.frames_since(start);
        let mut kinds: BTreeMap<&'static str, usize> = BTreeMap::new();
        let mut per_item: HashMap<String, usize> = HashMap::new();
        let mut per_session: HashMap<String, usize> = HashMap::new();
        let mut bytes = 0;
        for (name, b, subject, same_row) in &frames {
            *kinds.entry(name).or_default() += 1;
            bytes += b;
            match *name {
                "work:item" => {
                    let ext = subject.clone().unwrap_or_default();
                    if !changed.contains(&ext) {
                        rep.noop_item_frames += 1;
                    }
                    *per_item.entry(ext).or_default() += 1;
                }
                "session:updated" => {
                    rep.sync_session_frames += 1;
                    if *same_row {
                        rep.noop_session_frames += 1;
                    }
                    *per_session
                        .entry(subject.clone().unwrap_or_default())
                        .or_default() += 1;
                }
                _ => {}
            }
        }
        rep.max_frames_per_item = rep
            .max_frames_per_item
            .max(per_item.values().copied().max().unwrap_or(0));
        rep.max_frames_per_session = rep
            .max_frames_per_session
            .max(per_session.values().copied().max().unwrap_or(0));
        for (k, n) in &kinds {
            *kinds_total.entry(k).or_default() += n;
        }
        rep.changed_per_pass.push(changed.len());
        rep.per_pass.push(kinds);
        rep.per_pass_bytes.push(bytes);
        sync_minutes += sc.interval_secs as f64 / 60.0;
    }
    *kinds_total.entry("session:updated (baseline)").or_default() += baseline_per_pass * sc.passes;
    rep.per_minute = kinds_total
        .into_iter()
        .map(|(k, n)| (k, n as f64 / sync_minutes))
        .collect();
    rep
}

fn print(sc: &Scenario, r: &Report) {
    let n = r.per_pass.len().max(1);
    let sync_frames: Vec<usize> = r.per_pass.iter().map(|m| m.values().sum()).collect();
    let mean = |v: &[usize]| v.iter().sum::<usize>() as f64 / v.len().max(1) as f64;
    let meani = |v: &[i64]| v.iter().sum::<i64>() as f64 / v.len().max(1) as f64;
    println!("\n=== {} ===", sc.label);
    println!(
        "interval {} s, {} changed items per tracker per pass ({:.1} %), {} passes",
        sc.interval_secs,
        sc.churn_per_pass,
        100.0 * sc.churn_per_pass as f64 / BOARD as f64,
        n
    );
    println!(
        "first pass (400 new items): {} frames; ring after it reaches back {} s",
        r.first_pass_frames, r.first_pass_cover_after
    );
    println!(
        "sync frames per pass: mean {:.1}, max {}; bytes per pass: mean {:.0}, max {}",
        mean(&sync_frames),
        sync_frames.iter().max().unwrap_or(&0),
        mean(&r.per_pass_bytes),
        r.per_pass_bytes.iter().max().unwrap_or(&0)
    );
    let mut by_kind: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for m in &r.per_pass {
        for (k, v) in m {
            by_kind.entry(k).or_default().push(*v);
        }
    }
    for (k, v) in &by_kind {
        println!(
            "  {k}: mean {:.2}/pass, max {}",
            v.iter().sum::<usize>() as f64 / n as f64,
            v.iter().max().unwrap()
        );
    }
    println!("frames per minute (sync + baseline):");
    for (k, v) in &r.per_minute {
        println!("  {k}: {v:.2}");
    }
    let total: f64 = r.per_minute.values().sum();
    println!(
        "  total: {total:.2}/min → 512 slots last {:.1} min on average",
        REPLAY_RING as f64 / total
    );
    println!(
        "ring reach: right after a sync burst min {} s / mean {:.0} s; before a pass mean {:.0} s",
        r.cover_after_burst.iter().min().unwrap_or(&0),
        meani(&r.cover_after_burst),
        meani(&r.cover_before_pass)
    );
    println!(
        "no-op work:item frames {}; sync session:updated frames {} of which unchanged rows {}",
        r.noop_item_frames, r.sync_session_frames, r.noop_session_frames
    );
    println!(
        "most frames for one item in one pass {}; for one session {}",
        r.max_frames_per_item, r.max_frames_per_session
    );
}

/// The guards every realistic scenario must hold.
fn assert_bounds(sc: &Scenario, r: &Report) {
    assert_eq!(
        r.noop_item_frames, 0,
        "{}: a pass emitted work:item for an item that did not change",
        sc.label
    );
    assert_eq!(
        r.noop_session_frames, 0,
        "{}: a pass emitted session:updated for a row that did not change",
        sc.label
    );
    assert!(
        r.max_frames_per_item <= 1,
        "{}: an item emitted {} work:item frames in one pass",
        sc.label,
        r.max_frames_per_item
    );
    assert!(
        r.max_frames_per_session <= 1,
        "{}: a session emitted {} frames in one pass",
        sc.label,
        r.max_frames_per_session
    );
    // Every changed item: at most its own frame plus one per linked session.
    let linked_sessions = LINKS.len();
    for (m, changed) in r.per_pass.iter().zip(&r.changed_per_pass) {
        let total: usize = m.values().sum();
        assert!(
            total <= changed + linked_sessions,
            "{}: {total} frames for {changed} changed items: {m:?}",
            sc.label
        );
    }
    let min_cover = *r.cover_after_burst.iter().min().unwrap();
    assert!(
        min_cover >= MIN_COVER_SECS,
        "{}: right after a sync pass the ring reaches back only {min_cover} s",
        sc.label
    );
}

#[tokio::test]
async fn ring_pressure_default_interval_5_pct_churn() {
    let _g = SERIAL.lock().await;
    let sc = Scenario {
        label: "default interval (300 s), 5 % churn per pass",
        interval_secs: 300,
        churn_per_pass: BOARD / 20,
        passes: 36,
    };
    let r = run(&sc).await;
    print(&sc, &r);
    assert_bounds(&sc, &r);
}

#[tokio::test]
async fn ring_pressure_default_interval_10_pct_churn() {
    let _g = SERIAL.lock().await;
    let sc = Scenario {
        label: "default interval (300 s), 10 % churn per pass",
        interval_secs: 300,
        churn_per_pass: BOARD / 10,
        passes: 36,
    };
    let r = run(&sc).await;
    print(&sc, &r);
    assert_bounds(&sc, &r);
}

/// The shortest interval the setting allows, with the same 10 % churn per
/// PASS: five times the change rate of the scenario above. A stress case,
/// not a realistic one; it must still keep five minutes.
#[tokio::test]
async fn ring_pressure_min_interval_10_pct_churn_stress() {
    let _g = SERIAL.lock().await;
    let sc = Scenario {
        label: "min interval (60 s), 10 % churn per pass (stress)",
        interval_secs: 60,
        churn_per_pass: BOARD / 10,
        passes: 60,
    };
    let r = run(&sc).await;
    print(&sc, &r);
    assert_bounds(&sc, &r);
}

/// One item change fans out to exactly its own frame plus one
/// `session:updated` per live session whose primary work it is — no link
/// frame, no second frame for the item — and an unchanged pass (whole
/// listing included) is silent.
#[tokio::test]
async fn ring_pressure_one_change_fans_out_once_and_quiet_passes_are_silent() {
    let _g = SERIAL.lock().await;
    let mut fleet = Fleet::new().await;
    fleet.pass().await;
    fleet.seed_sessions();
    let mut t = T0 + 300;
    SIM_NOW.store(t, Ordering::SeqCst);
    fleet.pass().await;

    // Nothing changed: incremental (the overlap re-lists the newest) …
    t += 300;
    SIM_NOW.store(t, Ordering::SeqCst);
    let start = fleet.frame_count();
    fleet.pass().await;
    assert!(fleet.frames_since(start).is_empty());
    // … and whole (an hour on, every view is listed whole).
    t += FULL_EVERY_SECS;
    SIM_NOW.store(t, Ordering::SeqCst);
    let start = fleet.frame_count();
    fleet.pass().await;
    assert!(
        fleet.frames_since(start).is_empty(),
        "{:?}",
        fleet.frames_since(start)
    );

    // ABC-7 is the primary work of three sessions.
    let ext = fleet.board.move_status("ABC-7", t + 10);
    t += 300;
    SIM_NOW.store(t, Ordering::SeqCst);
    let start = fleet.frame_count();
    fleet.pass().await;
    let frames = fleet.frames_since(start);
    let names: Vec<&str> = frames.iter().map(|f| f.0).collect();
    assert_eq!(
        names,
        vec![
            "work:item",
            "session:updated",
            "session:updated",
            "session:updated"
        ]
    );
    assert_eq!(frames[0].2.as_deref(), Some(ext.as_str()));

    // The next pass re-lists ABC-7 (the overlap), unchanged: silent.
    t += 300;
    SIM_NOW.store(t, Ordering::SeqCst);
    let start = fleet.frame_count();
    fleet.pass().await;
    assert!(fleet.frames_since(start).is_empty());
}
