//! The provider conformance suite (work graph M6.0): one contract every
//! tracker adapter proves over its own recorded fixtures, so adding a
//! provider is one adapter plus fixtures and core, UI and store never learn
//! provider concepts.
//!
//! An adapter's tests implement [`Harness`] (how to script its fake for each
//! scenario, and what the answers must be) and invoke
//! [`conformance_suite!`](crate::conformance_suite) once, which expands to one
//! `#[tokio::test]` per scenario of the plan's table:
//!
//! | # | Scenario | Checked here |
//! |---|---|---|
//! | 1 | probe | instance id, me, key prefixes (or none without human keys) |
//! | 2 | a view lists items | normalised snapshots, stable `external_id`, the golden |
//! | 3 | incremental | a `since` window or a sync mark; every item carries `updated` |
//! | 4 | by-id fetch of 3, one missing | 2 `Found`, 1 `Unavailable{reason}` |
//! | 5 | status mapping | category plus resolution per item |
//! | 6 | hierarchy | `parent` / `hierarchy_level` where `caps.hierarchy` |
//! | 7 | moved / renamed | same `external_id`, the old reference in `aliases` |
//! | 8 | recognise | keys, URLs, repo-relative `#n` per caps |
//! | 9 | errors | 401, 403 on one view, 429 + Retry-After, offline, garbage |
//! | 10 | no secret | in any `Debug`, request line, snapshot or error |
//!
//! **Snapshot golden.** Scenario 2 compares the normalised listing with
//! `testdata/<provider>/golden_list.json`, so a provider API change shows
//! up as a diff in review. `REGEN_TRACKER_GOLDENS=1 cargo test -p
//! fleet-core conformance` rewrites them.
//!
//! No scenario reaches a real tracker: every harness answers from a
//! [`FakeTransport`].

use super::{
    list_all, Fetched, ItemRef, RefCtx, TrackerError, TrackerProvider, ViewDef, WorkItemSnapshot,
    NOT_FOUND_OR_NO_PERMISSION,
};
use crate::net::https::{FakeTransport, Request};
use serde_json::{json, Value};

/// The error scenarios of row 9.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCase {
    /// The credential is refused (HTTP 401, `gh`'s "Bad credentials").
    Unauthorized,
    /// One view's query is forbidden; the tracker stays ok.
    ForbiddenView,
    /// 429 with `Retry-After: 30`.
    RateLimited,
    /// Nothing answers.
    Offline,
    /// A 200 whose body is not the JSON the provider expects.
    Garbage,
}

/// What an adapter's answers must be (the scenario table's right column).
pub struct Expect {
    pub instance_id: Option<&'static str>,
    /// The API user as the probe records it (`config.account_id`).
    pub me: &'static str,
    /// Key prefixes; empty exactly when `caps.human_keys` is false.
    pub prefixes: Vec<&'static str>,
    /// The view scenario 2 lists (and 3 reads incrementally).
    pub view: ViewDef,
    /// The listing's external ids, in order.
    pub list_ids: Vec<&'static str>,
    /// Three references, the last of which the tracker does not have.
    pub fetch: [ItemRef; 3],
    /// `(external_id, category, resolution)` from the listing.
    pub statuses: Vec<(&'static str, &'static str, Option<&'static str>)>,
    /// `(child external_id, parent external_id, child hierarchy_level)`.
    pub hierarchy: Option<(&'static str, &'static str, Option<i64>)>,
    /// A reference under an old key or repo, the id it resolves to, and the
    /// alias that must be recorded.
    pub moved: (ItemRef, &'static str, &'static str),
    /// `(text, the session's repo, what must be recognised)`.
    pub recognize: Vec<(&'static str, Option<&'static str>, Vec<ItemRef>)>,
    /// The credential's secret literal; `None` for a provider that holds
    /// none (GitHub through `gh`).
    pub secret: Option<&'static str>,
}

/// One adapter's side of the suite.
#[async_trait::async_trait]
pub trait Harness: Send + Sync {
    /// `testdata/<name>/` holds its fixtures and golden.
    fn name(&self) -> &'static str;
    fn expect(&self) -> Expect;
    /// The adapter, with a credential, over `fake`.
    fn provider(&self, fake: &FakeTransport) -> Box<dyn TrackerProvider>;
    fn script_probe(&self, f: &FakeTransport);
    /// Every page of `expect().view`.
    fn script_list(&self, f: &FakeTransport);
    /// Scenario 3: script, read incrementally (a `since` window or a sync
    /// mark), and assert on the requests that the window or mark was sent.
    /// Returns what the read found.
    async fn incremental(
        &self,
        p: &dyn TrackerProvider,
        f: &FakeTransport,
    ) -> Result<Vec<WorkItemSnapshot>, TrackerError>;
    /// Scenario 4's answers for `expect().fetch`.
    fn script_fetch(&self, f: &FakeTransport);
    /// Scenario 7's answer for `expect().moved.0`.
    fn script_moved(&self, f: &FakeTransport);
    /// Scenario 9: script `case` on the call the suite makes for it
    /// (`probe` for Unauthorized and Offline, `list` of `expect().view` for
    /// the rest).
    fn script_error(&self, f: &FakeTransport, case: ErrorCase);
}

fn fixture_dir(name: &str) -> String {
    format!(
        "{}/src/service/trackers/testdata/{name}",
        env!("CARGO_MANIFEST_DIR")
    )
}

/// A fixture of `provider`'s, parsed.
pub fn fixture(provider: &str, file: &str) -> Value {
    let path = format!("{}/{file}", fixture_dir(provider));
    serde_json::from_str(&std::fs::read_to_string(&path).expect(&path)).expect(file)
}

/// A snapshot as the golden records it (every field, stable order).
pub fn snapshot_json(s: &WorkItemSnapshot) -> Value {
    json!({
        "external_id": s.external_id,
        "key": s.key,
        "aliases": s.aliases,
        "title": s.title,
        "url": s.url,
        "kind": s.kind,
        "hierarchy_level": s.hierarchy_level,
        "status": {
            "name": s.status.name,
            "category": s.status.category,
            "resolution": s.status.resolution,
        },
        "parent_external_id": s.parent_external_id,
        "parent_key": s.parent_key,
        "containers": s.containers,
        "assignees": s.assignees,
        "assignee_id": s.assignee_id,
        "iteration": s.iteration,
        "iteration_active": s.iteration_active,
        "updated": s.updated,
        "description": s.description,
    })
}

async fn listing<H: Harness>(h: &H) -> Vec<WorkItemSnapshot> {
    let f = FakeTransport::new();
    h.script_list(&f);
    let p = h.provider(&f);
    list_all(p.as_ref(), &h.expect().view, None)
        .await
        .unwrap_or_else(|e| panic!("{}: listing failed: {e:?}", h.name()))
}

fn found(f: &Fetched) -> Option<&WorkItemSnapshot> {
    match f {
        Fetched::Found(s) => Some(s),
        Fetched::Unavailable { .. } => None,
    }
}

/// Scenario 1: probe.
pub async fn probe<H: Harness>(h: &H) {
    let f = FakeTransport::new();
    h.script_probe(&f);
    let p = h.provider(&f);
    let info = p
        .probe()
        .await
        .unwrap_or_else(|e| panic!("{}: probe failed: {e:?}", h.name()));
    let e = h.expect();
    assert_eq!(info.instance_id.as_deref(), e.instance_id, "{}", h.name());
    assert_eq!(
        info.config.account_id.as_deref(),
        Some(e.me),
        "{}",
        h.name()
    );
    assert_eq!(info.config.key_prefixes, e.prefixes, "{}", h.name());
    assert_eq!(
        p.caps().human_keys,
        !e.prefixes.is_empty(),
        "{}: prefixes exist exactly when the provider has human keys",
        h.name()
    );
    let views = p
        .views(&info.config)
        .await
        .unwrap_or_else(|e| panic!("{}: views failed: {e:?}", h.name()));
    assert!(
        views.iter().any(|v| v.id == "mine"),
        "{}: every provider has a `mine` view",
        h.name()
    );
}

/// Scenario 2: a view lists items: stable ids, and the golden.
pub async fn list<H: Harness>(h: &H) {
    let items = listing(h).await;
    let ids: Vec<&str> = items.iter().map(|i| i.external_id.as_str()).collect();
    assert_eq!(ids, h.expect().list_ids, "{}", h.name());
    let again = listing(h).await;
    assert_eq!(
        items,
        again,
        "{}: the same answer normalises the same",
        h.name()
    );
    for i in &items {
        assert!(!i.external_id.is_empty(), "{}", h.name());
        assert!(
            matches!(i.status.category.as_str(), "todo" | "in_progress" | "done"),
            "{}: {} has category {:?}",
            h.name(),
            i.external_id,
            i.status.category
        );
        assert!(
            i.updated.is_some(),
            "{}: {} has no updated",
            h.name(),
            i.external_id
        );
    }
    let got = Value::Array(items.iter().map(snapshot_json).collect());
    let path = format!("{}/golden_list.json", fixture_dir(h.name()));
    if std::env::var_os("REGEN_TRACKER_GOLDENS").is_some() {
        std::fs::write(&path, serde_json::to_string_pretty(&got).unwrap() + "\n").unwrap();
        return;
    }
    let want: Value = serde_json::from_str(
        &std::fs::read_to_string(&path)
            .unwrap_or_else(|_| panic!("{path} missing; run with REGEN_TRACKER_GOLDENS=1")),
    )
    .unwrap();
    assert_eq!(
        got,
        want,
        "{}: the normalised listing differs from {path} (REGEN_TRACKER_GOLDENS=1 to accept)",
        h.name()
    );
}

/// Scenario 3: incremental: the read finds something, and every item carries
/// `updated` (the sync dedupes on `(id, updated)`).
pub async fn incremental<H: Harness>(h: &H) {
    let f = FakeTransport::new();
    let p = h.provider(&f);
    let items = h
        .incremental(p.as_ref(), &f)
        .await
        .unwrap_or_else(|e| panic!("{}: incremental read failed: {e:?}", h.name()));
    assert!(
        !items.is_empty(),
        "{}: the fixture changes something",
        h.name()
    );
    let mut seen = std::collections::HashSet::new();
    for i in &items {
        assert!(i.updated.is_some(), "{}", h.name());
        seen.insert((i.external_id.clone(), i.updated));
    }
    assert!(!seen.is_empty());
}

/// Scenario 4: by-id fetch of three references, one missing.
pub async fn fetch<H: Harness>(h: &H) {
    let f = FakeTransport::new();
    h.script_fetch(&f);
    let p = h.provider(&f);
    let refs = h.expect().fetch;
    let got = p
        .fetch(&refs)
        .await
        .unwrap_or_else(|e| panic!("{}: fetch failed: {e:?}", h.name()));
    assert_eq!(got.len(), 3, "{}: every reference gets an answer", h.name());
    assert!(found(&got[0]).is_some(), "{}: {:?}", h.name(), got[0]);
    assert!(found(&got[1]).is_some(), "{}: {:?}", h.name(), got[1]);
    assert_eq!(
        got[2],
        Fetched::Unavailable {
            reference: refs[2].reference(),
            reason: NOT_FOUND_OR_NO_PERMISSION.into(),
        },
        "{}: missing is unavailable, never gone",
        h.name()
    );
}

/// Scenario 5: status mapping.
pub async fn status<H: Harness>(h: &H) {
    let items = listing(h).await;
    let want = h.expect().statuses;
    assert!(
        want.iter()
            .any(|w| w.1 == "done" && w.2 == Some("not_planned"))
            && want
                .iter()
                .any(|w| w.1 == "done" && w.2 == Some("completed"))
            && want.iter().any(|w| w.1 == "in_progress"),
        "{}: the fixture covers in_progress, completed and not_planned",
        h.name()
    );
    for (id, category, resolution) in want {
        let i = items
            .iter()
            .find(|i| i.external_id == id)
            .unwrap_or_else(|| panic!("{}: {id} not listed", h.name()));
        assert_eq!(
            (i.status.category.as_str(), i.status.resolution.as_deref()),
            (category, resolution),
            "{}: {id}",
            h.name()
        );
    }
}

/// Scenario 6: hierarchy, where the provider has it.
pub async fn hierarchy<H: Harness>(h: &H) {
    let f = FakeTransport::new();
    let caps = h.provider(&f).caps();
    let want = h.expect().hierarchy;
    assert_eq!(caps.hierarchy, want.is_some(), "{}", h.name());
    let Some((child, parent, level)) = want else {
        return;
    };
    let items = listing(h).await;
    let c = items
        .iter()
        .find(|i| i.external_id == child)
        .unwrap_or_else(|| panic!("{}: {child} not listed", h.name()));
    assert_eq!(
        c.parent_external_id.as_deref(),
        Some(parent),
        "{}",
        h.name()
    );
    assert_eq!(c.hierarchy_level, level, "{}", h.name());
}

/// Scenario 7: the item moved (a key or repo changed): same id, old reference aliased.
pub async fn moved<H: Harness>(h: &H) {
    let f = FakeTransport::new();
    h.script_moved(&f);
    let p = h.provider(&f);
    let (r, id, alias) = h.expect().moved;
    let got = p
        .fetch(std::slice::from_ref(&r))
        .await
        .unwrap_or_else(|e| panic!("{}: fetch failed: {e:?}", h.name()));
    let s = found(&got[0]).unwrap_or_else(|| panic!("{}: {got:?}", h.name()));
    assert_eq!(s.external_id, id, "{}", h.name());
    assert!(
        s.aliases.iter().any(|a| a == alias),
        "{}: {:?} lacks {alias}",
        h.name(),
        s.aliases
    );
    assert_ne!(s.key.as_deref(), Some(alias), "{}", h.name());
}

/// Scenario 8: recognise.
pub async fn recognize<H: Harness>(h: &H) {
    let f = FakeTransport::new();
    let p = h.provider(&f);
    let caps = p.caps();
    for (text, repo, want) in h.expect().recognize {
        let got = p.recognize(text, RefCtx { repo });
        assert_eq!(got, want, "{}: {text:?} in {repo:?}", h.name());
        if !caps.repo_relative {
            assert!(
                !got.iter().any(|r| matches!(r, ItemRef::RepoNumber { .. })),
                "{}: no repo-relative refs without caps.repo_relative",
                h.name()
            );
        }
    }
    if caps.repo_relative {
        let bare = p.recognize("fixes #42", RefCtx { repo: Some("o/r") });
        assert_eq!(
            bare,
            vec![ItemRef::RepoNumber {
                repo: "o/r".into(),
                n: 42
            }],
            "{}",
            h.name()
        );
        assert!(
            p.recognize("fixes #42", RefCtx { repo: None }).is_empty(),
            "{}: a bare #n needs the session's repo",
            h.name()
        );
    }
}

/// Scenario 9: errors map to the right tracker state.
pub async fn errors<H: Harness>(h: &H) {
    let view = h.expect().view;
    for case in [
        ErrorCase::Unauthorized,
        ErrorCase::ForbiddenView,
        ErrorCase::RateLimited,
        ErrorCase::Offline,
        ErrorCase::Garbage,
    ] {
        let f = FakeTransport::new();
        h.script_error(&f, case);
        let p = h.provider(&f);
        let err = match case {
            ErrorCase::Unauthorized | ErrorCase::Offline => p.probe().await.err(),
            _ => p.list(&view, None, None).await.err(),
        }
        .unwrap_or_else(|| panic!("{}: {case:?} did not fail", h.name()));
        let ok = match case {
            ErrorCase::Unauthorized => {
                matches!(err, TrackerError::Auth(_)) && err.state() == Some("auth_failed")
            }
            ErrorCase::ForbiddenView => {
                matches!(err, TrackerError::Forbidden(_)) && err.state().is_none()
            }
            ErrorCase::RateLimited => {
                err == TrackerError::RateLimited {
                    retry_after_secs: Some(30),
                } && err.state() == Some("rate_limited")
            }
            ErrorCase::Offline => {
                matches!(err, TrackerError::Unreachable(_)) && err.state() == Some("unreachable")
            }
            ErrorCase::Garbage => matches!(err, TrackerError::Invalid(_)) && err.state().is_none(),
        };
        assert!(ok, "{}: {case:?} gave {err:?}", h.name());
    }
}

/// Scenario 10: the secret appears in no `Debug`, request line, snapshot or error.
pub async fn no_secret<H: Harness>(h: &H) {
    let Some(secret) = h.expect().secret else {
        // A provider that holds no credential: nothing can leak, but no
        // request may carry an Authorization header it made up either.
        let f = FakeTransport::new();
        h.script_probe(&f);
        let _ = h.provider(&f).probe().await;
        for r in f.requests() {
            assert!(r.header_value("Authorization").is_none(), "{}", h.name());
        }
        return;
    };
    let mut texts: Vec<String> = Vec::new();
    let mut requests: Vec<Request> = Vec::new();
    let f = FakeTransport::new();
    h.script_probe(&f);
    h.script_list(&f);
    let p = h.provider(&f);
    if let Ok(info) = p.probe().await {
        texts.push(format!("{info:?}"));
    }
    for i in listing(h).await {
        texts.push(format!("{i:?}"));
        texts.push(snapshot_json(&i).to_string());
    }
    requests.extend(f.requests());
    for case in [
        ErrorCase::Unauthorized,
        ErrorCase::RateLimited,
        ErrorCase::Garbage,
    ] {
        let f = FakeTransport::new();
        h.script_error(&f, case);
        let p = h.provider(&f);
        let e = match case {
            ErrorCase::Unauthorized => p.probe().await.err(),
            _ => p.list(&h.expect().view, None, None).await.err(),
        };
        if let Some(e) = e {
            texts.push(format!("{e:?}"));
            texts.push(e.explain());
            texts.push(e.to_ipc().message);
        }
        requests.extend(f.requests());
    }
    for r in &requests {
        texts.push(format!("{r:?}"));
        texts.push(r.url.clone());
        if let Some(b) = &r.body {
            texts.push(String::from_utf8_lossy(b).into_owned());
        }
    }
    assert!(!requests.is_empty(), "{}", h.name());
    for t in texts {
        assert!(
            !t.contains(secret),
            "{}: the secret leaked into {t}",
            h.name()
        );
    }
}

/// Expand to one `#[tokio::test]` per scenario for `$harness` (an
/// expression of a type implementing [`Harness`]).
#[macro_export]
macro_rules! conformance_suite {
    ($harness:expr) => {
        mod conformance {
            use super::*;
            use $crate::service::trackers::conformance as c;
            #[tokio::test]
            async fn c01_probe() {
                c::probe(&$harness).await
            }
            #[tokio::test]
            async fn c02_list_and_golden() {
                c::list(&$harness).await
            }
            #[tokio::test]
            async fn c03_incremental() {
                c::incremental(&$harness).await
            }
            #[tokio::test]
            async fn c04_fetch_one_missing() {
                c::fetch(&$harness).await
            }
            #[tokio::test]
            async fn c05_status_mapping() {
                c::status(&$harness).await
            }
            #[tokio::test]
            async fn c06_hierarchy() {
                c::hierarchy(&$harness).await
            }
            #[tokio::test]
            async fn c07_moved_keeps_identity() {
                c::moved(&$harness).await
            }
            #[tokio::test]
            async fn c08_recognize() {
                c::recognize(&$harness).await
            }
            #[tokio::test]
            async fn c09_errors_map_to_states() {
                c::errors(&$harness).await
            }
            #[tokio::test]
            async fn c10_no_secret_anywhere() {
                c::no_secret(&$harness).await
            }
        }
    };
}
