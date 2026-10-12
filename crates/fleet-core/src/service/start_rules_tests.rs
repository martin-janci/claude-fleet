//! Start rules (redesign 8.11): the pattern grammar, the tally that offers
//! a rule after five identical starts, who may read and change one, and the
//! start path: a rule decides before history and Jev, so a rule match
//! records no Jev call.

use super::*;
use crate::mcp::auth::{Caller, ClientRef, TokenMode};
use crate::service::decide::{
    BackendError, DecideCtx, DecisionBackend, JevRequest, JevResponse, PROVIDER_JEV,
};
use crate::service::settings;
use crate::service::trackers::tickets::{self, StartArgs};
use crate::store::{Decider, DecisionRunFilter, Secret};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

const NOW: i64 = 1_790_510_400;

#[test]
fn a_pattern_matches_keys_with_stars_and_any_case() {
    assert!(glob_match("PD-*", "PD-12"));
    assert!(glob_match("pd-*", "PD-12"));
    assert!(glob_match("PD-*", "pd-1"));
    assert!(glob_match("PD-*", "PD-"));
    assert!(!glob_match("PD-*", "PDX-1"));
    assert!(!glob_match("PD-*", "XPD-1"));
    assert!(glob_match("*-UI-*", "PD-UI-7"));
    assert!(glob_match("PD-1*", "PD-123"));
    assert!(!glob_match("PD-1*", "PD-23"));
    assert!(glob_match("acme/app#*", "acme/app#12"));
    assert!(glob_match("PD-12", "PD-12"));
    assert!(!glob_match("PD-12", "PD-123"));
}

#[test]
fn a_pattern_is_checked() {
    assert_eq!(check_pattern("  PD-* ").unwrap(), "PD-*");
    for bad in ["", "   ", "*", "**", "PD *", "PD-*;rm", &"A".repeat(65)] {
        let e = check_pattern(bad).unwrap_err();
        assert_eq!(e.code, codes::E_INVALID, "{bad:?}");
    }
    assert!(specificity("PD-1*") > specificity("PD-*"));
}

#[test]
fn only_a_prefix_key_is_tallied() {
    assert_eq!(prefix_pattern("PD-12").as_deref(), Some("PD-*"));
    assert_eq!(prefix_pattern("pd-12").as_deref(), Some("PD-*"));
    assert_eq!(
        prefix_pattern("acme/app#12"),
        None,
        "its repository places it"
    );
    assert_eq!(prefix_pattern("asana:1209"), None);
    assert_eq!(prefix_pattern("fix the login"), None);
    assert_eq!(prefix_pattern("PD-"), None);
    assert_eq!(prefix_pattern("12-34"), None, "no letter, no prefix");
    assert_eq!(prefix_pattern("PD-12-b"), None);
}

struct W {
    store: Arc<Mutex<Store>>,
    app: i64,
    pos: i64,
}

fn w() -> W {
    let s = Store::open_in_memory().unwrap();
    s.upsert_host("h").unwrap();
    let app = s.upsert_project("acme", "app", "/p/acme/app").unwrap();
    let pos = s.upsert_project("acme", "pos", "/p/acme/pos").unwrap();
    W {
        store: Arc::new(Mutex::new(s)),
        app,
        pos,
    }
}

impl W {
    fn tally(&self, key: &str, project: i64) -> Option<StartRuleRow> {
        let s = self.store.lock().unwrap();
        tally(&s, None, key, project, NOW).unwrap()
    }
    fn row(&self, project: i64) -> Option<StartRuleRow> {
        let s = self.store.lock().unwrap();
        s.find_start_rule(None, "PD-*", project).unwrap()
    }
}

#[test]
fn five_identical_starts_in_a_row_offer_a_rule() {
    let w = w();
    for n in 1..OFFER_AFTER {
        assert_eq!(w.tally(&format!("PD-{n}"), w.pos), None, "start {n}");
    }
    assert_eq!(w.row(w.pos).unwrap().state, "counting");
    let offer = w.tally("PD-5", w.pos).expect("the fifth start offers it");
    assert_eq!(offer.state, "offered");
    assert_eq!(offer.pattern, "PD-*");
    assert_eq!(offer.project_id, w.pos);
    assert_eq!(offer.confirmations, OFFER_AFTER);
    assert_eq!(offer.owner_person_id, None, "fleet's offer is nobody's yet");
    // A sixth counts on, but offers nothing new.
    assert_eq!(w.tally("PD-6", w.pos), None);
    assert_eq!(w.row(w.pos).unwrap().state, "offered");
    // Fleet's private tally is not listed; the offer is.
    let listed = list(&w.store, &ViewScope::internal()).unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].rule.state, "offered");
    assert_eq!(listed[0].project.as_deref(), Some("acme/pos"));
}

#[test]
fn another_project_breaks_the_streak_and_withdraws_the_offer() {
    let w = w();
    for n in 1..OFFER_AFTER {
        w.tally(&format!("PD-{n}"), w.pos);
    }
    w.tally("PD-9", w.app);
    let pos = w.row(w.pos).unwrap();
    assert_eq!((pos.confirmations, pos.state.as_str()), (0, "counting"));
    assert_eq!(w.row(w.app).unwrap().confirmations, 1);
    for n in 1..=OFFER_AFTER {
        w.tally(&format!("PD-{n}"), w.pos);
    }
    assert_eq!(w.row(w.pos).unwrap().state, "offered");
    w.tally("PD-10", w.app);
    assert_eq!(w.row(w.pos).unwrap().state, "counting", "withdrawn");
    assert!(list(&w.store, &ViewScope::internal()).unwrap().is_empty());
}

#[test]
fn a_dismissed_offer_is_never_offered_again_and_an_accepted_one_decides() {
    let w = w();
    for n in 1..=OFFER_AFTER {
        w.tally(&format!("PD-{n}"), w.pos);
    }
    let id = w.row(w.pos).unwrap().id;
    let scope = ViewScope::internal();
    assert_eq!(
        dismiss(&w.store, &scope, id).unwrap().rule.state,
        "dismissed"
    );
    for n in 1..=OFFER_AFTER * 2 {
        assert_eq!(w.tally(&format!("PD-{n}"), w.pos), None);
    }
    assert_eq!(w.row(w.pos).unwrap().state, "dismissed");
    // Changed their mind from Automation: accept the dismissed one.
    assert_eq!(accept(&w.store, &scope, id).unwrap().rule.state, "active");
    let s = w.store.lock().unwrap();
    assert_eq!(matching(&s, None, "pd-77").unwrap().unwrap().id, id);
    assert_eq!(matching(&s, None, "PDX-1").unwrap(), None);
    assert_eq!(
        matching(&s, Some(1), "PD-1").unwrap(),
        None,
        "another org's tasks"
    );
    assert_eq!(offer_for(&s, &scope, None, "PD-1").unwrap(), None);
}

#[test]
fn the_most_specific_active_rule_decides_and_accept_replaces_the_patterns_other() {
    let w = w();
    let scope = ViewScope::internal();
    let input = |pattern: &str, project| StartRuleInput {
        pattern: pattern.into(),
        project_id: project,
        host_alias: None,
        org_id: None,
        ..Default::default()
    };
    let wide = save(&w.store, &scope, None, &input("PD-*", w.app)).unwrap();
    let narrow = save(&w.store, &scope, None, &input("PD-1*", w.pos)).unwrap();
    {
        let s = w.store.lock().unwrap();
        assert_eq!(
            matching(&s, None, "PD-12").unwrap().unwrap().id,
            narrow.rule.id
        );
        assert_eq!(
            matching(&s, None, "PD-2").unwrap().unwrap().id,
            wide.rule.id
        );
    }
    // The same pattern and project twice is refused.
    let e = save(&w.store, &scope, None, &input("pd-*", w.app)).unwrap_err();
    assert_eq!(e.code, codes::E_EXISTS);
    // An offer of PD-* → pos, accepted, takes PD-* → app's place.
    for n in 1..=OFFER_AFTER {
        w.tally(&format!("PD-{n}"), w.pos);
    }
    let offer = w.row(w.pos).unwrap();
    assert_eq!(offer.state, "offered");
    accept(&w.store, &scope, offer.id).unwrap();
    let s = w.store.lock().unwrap();
    assert!(s.get_start_rule(wide.rule.id).unwrap().is_none());
    assert_eq!(matching(&s, None, "PD-2").unwrap().unwrap().id, offer.id);
}

#[test]
fn save_checks_the_project_and_the_host() {
    let w = w();
    let scope = ViewScope::internal();
    let bad = |project, host: Option<&str>| {
        save(
            &w.store,
            &scope,
            None,
            &StartRuleInput {
                pattern: "PD-*".into(),
                project_id: project,
                host_alias: host.map(str::to_string),
                org_id: None,
                ..Default::default()
            },
        )
        .unwrap_err()
        .code
    };
    assert_eq!(bad(999, None), codes::E_NOTFOUND);
    assert_eq!(bad(w.pos, Some("nowhere")), codes::E_NOTFOUND);
    let ok = save(
        &w.store,
        &scope,
        None,
        &StartRuleInput {
            pattern: "PD-*".into(),
            project_id: w.pos,
            host_alias: Some("h".into()),
            org_id: None,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(ok.rule.host_alias.as_deref(), Some("h"));
    assert_eq!(ok.rule.state, "active");
    assert!(ok.may_change);
}

#[test]
fn an_org_member_reads_a_rule_only_an_admin_changes_it_and_others_see_nothing() {
    let w = w();
    let (org, other, admin, member, outsider) = {
        let s = w.store.lock().unwrap();
        let org = s.add_org("Acme", None, false).unwrap().id;
        let other = s.add_org("Globex", None, false).unwrap().id;
        let p = |name: &str| s.create_person(name, None).unwrap().id;
        let (admin, member, outsider) = (p("ann"), p("bob"), p("cy"));
        s.set_org_member(org, admin, "admin", None).unwrap();
        s.set_org_member(org, member, "member", None).unwrap();
        s.set_org_member(other, outsider, "admin", None).unwrap();
        (org, other, admin, member, outsider)
    };
    let as_person = |p: i64| person(&w.store, p);
    let input = StartRuleInput {
        pattern: "PD-*".into(),
        project_id: w.pos,
        host_alias: None,
        org_id: Some(org),
        ..Default::default()
    };
    let e = save(&w.store, &as_person(member), None, &input).unwrap_err();
    assert_eq!(
        e.code,
        codes::E_FORBIDDEN,
        "a member adds no rule for everyone"
    );
    let rule = save(&w.store, &as_person(admin), None, &input).unwrap();
    let id = rule.rule.id;
    let mine = list(&w.store, &as_person(member)).unwrap();
    assert_eq!(mine.len(), 1);
    assert!(!mine[0].may_change);
    assert_eq!(
        delete(&w.store, &as_person(member), id).unwrap_err().code,
        codes::E_FORBIDDEN
    );
    assert!(list(&w.store, &as_person(outsider)).unwrap().is_empty());
    assert_eq!(
        delete(&w.store, &as_person(outsider), id).unwrap_err().code,
        codes::E_NOTFOUND,
        "another org's rule is an id that does not exist"
    );
    let s = w.store.lock().unwrap();
    assert!(matching(&s, Some(org), "PD-1").unwrap().is_some());
    assert!(matching(&s, Some(other), "PD-1").unwrap().is_none());
    drop(s);
    assert!(delete(&w.store, &as_person(admin), id).unwrap());
}

/// A person's device, bound to no org, scoped the way a request is.
fn person(store: &Mutex<Store>, id: i64) -> ViewScope {
    Caller {
        api: None,
        host_alias: None,
        client: Some(ClientRef {
            id: 7,
            name: "phone".into(),
            trusted: false,
            org_id: None,
            person_id: Some(id),
        }),
        mode: TokenMode::Full,
        pane: None,
        is_personal_owner: false,
    }
    .view_scope(&lock(store).unwrap())
    .unwrap()
}

// --- the start path -------------------------------------------------------------

#[derive(Default)]
struct Fake {
    calls: AtomicUsize,
    answer: String,
}

#[async_trait::async_trait]
impl DecisionBackend for Fake {
    fn provider(&self) -> &'static str {
        PROVIDER_JEV
    }
    async fn ask(
        &self,
        _key: &Secret,
        _model: &str,
        _req: &JevRequest,
        _timeout: Duration,
    ) -> Result<JevResponse, BackendError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(serde_json::from_value(serde_json::json!({
            "model": "jev-1.13.0",
            "answers": { "q": {
                "type": "choice",
                "choice": self.answer,
                "probabilities": { self.answer.clone(): 0.9, "unsure": 0.1 },
                "confidence": 0.9,
            }},
            "usage": { "input_tokens": 90, "output_tokens": 2 },
        }))
        .unwrap())
    }
}

impl W {
    /// Jev K1 at assist, for tasks of no org.
    fn jev_on(&self) {
        let s = self.store.lock().unwrap();
        settings::set(&s, settings::DECIDE_JEV_ENABLED, "true").unwrap();
        settings::set(&s, settings::DECIDE_JEV_START_PROJECT, "assist").unwrap();
        settings::set(&s, settings::DECIDE_JEV_UNASSIGNED, "true").unwrap();
        s.set_decision_credential(
            Some(&Secret::new("tsk_test_0123456789abcdefghijklmnop")),
            None,
        )
        .unwrap();
    }
    fn item(&self, key: &str) -> i64 {
        let s = self.store.lock().unwrap();
        s.create_local_work_item(Some(key), "Refund button")
            .unwrap()
            .id
    }
    async fn preview(&self, fake: &Arc<Fake>, item: i64) -> tickets::StartPreview {
        let net = crate::service::trackers::TrackerNet::fake(Arc::new(
            crate::net::https::FakeTransport::new(),
        ));
        let ctx =
            DecideCtx::new(Arc::clone(&self.store), fake.clone()).with_clock(Arc::new(|| NOW));
        tickets::preview_start_decided(
            &self.store,
            &StartArgs {
                item_id: Some(item),
                host_alias: Some("h".into()),
                ..Default::default()
            },
            &ViewScope::internal(),
            &net,
            Some(&ctx),
        )
        .await
        .unwrap()
    }
    fn runs(&self) -> usize {
        self.store
            .lock()
            .unwrap()
            .list_decision_runs(&DecisionRunFilter::default())
            .unwrap()
            .len()
    }
    /// A person's start of `item` in `project`, through `start_many`.
    async fn start(&self, item: i64, project: i64, decider: Decider) {
        let store = Arc::clone(&self.store);
        let out = tickets::start_many(
            &self.store,
            &StartArgs {
                item_id: Some(item),
                host_alias: Some("h".into()),
                decider,
                ..Default::default()
            },
            &[project],
            &ViewScope::internal(),
            &crate::service::trackers::TrackerNet::fake(Arc::new(
                crate::net::https::FakeTransport::new(),
            )),
            tokio::time::Instant::now() + Duration::from_secs(600),
            move |a| {
                let s = store.lock().unwrap();
                let name = a.new_worktree.clone().unwrap_or_else(|| "x".into());
                let id = s
                    .upsert_session(&name, &a.host_alias, None, None, 1, 1, "running", None)
                    .unwrap();
                s.conn_for_test()
                    .execute(
                        "UPDATE sessions SET project_id = ?1 WHERE id = ?2",
                        [a.project_id, id],
                    )
                    .unwrap();
                std::future::ready(Ok(s.get_session_by_id(id).unwrap().unwrap()))
            },
            |_, _| {},
        )
        .await
        .unwrap();
        assert!(out.failed.is_empty() && out.skipped.is_empty(), "{out:?}");
    }
}

/// The plan's acceptance line: a rule match records no Jev call.
#[tokio::test]
async fn a_rule_match_records_no_jev_call() {
    let w = w();
    w.jev_on();
    let fake = Arc::new(Fake {
        answer: format!("p{}", w.app),
        ..Default::default()
    });
    // Without a rule, a key no project ran asks Jev once.
    let first = w.item("PD-1");
    let p = w.preview(&fake, first).await;
    assert_eq!(p.missing.as_deref(), Some("project"));
    assert_eq!(p.suggested_project.map(|s| s.project_id), Some(w.app));
    assert_eq!(fake.calls.load(Ordering::SeqCst), 1);
    let runs = w.runs();
    // With a rule, the same kind of key is planned by it: no call, no run.
    save(
        &w.store,
        &ViewScope::internal(),
        None,
        &StartRuleInput {
            pattern: "PD-*".into(),
            project_id: w.pos,
            host_alias: None,
            org_id: None,
            ..Default::default()
        },
    )
    .unwrap();
    let second = w.item("PD-2");
    let p = w.preview(&fake, second).await;
    assert_eq!(p.missing, None);
    let plan = p.plan.expect("the rule plans the start");
    assert_eq!(plan.project_id, w.pos);
    assert!(plan.rule_id.is_some());
    assert_eq!(p.suggested_project, None);
    assert_eq!(fake.calls.load(Ordering::SeqCst), 1, "no Jev call");
    assert_eq!(w.runs(), runs, "no decision run recorded");
}

#[tokio::test]
async fn a_rule_beats_the_keys_history() {
    let w = w();
    let fake = Arc::new(Fake::default());
    // PD ran in app last.
    let first = w.item("PD-1");
    w.start(first, w.app, Decider::Person).await;
    let second = w.item("PD-2");
    assert_eq!(
        w.preview(&fake, second).await.plan.unwrap().project_id,
        w.app,
        "history places it"
    );
    let rule = save(
        &w.store,
        &ViewScope::internal(),
        None,
        &StartRuleInput {
            pattern: "PD-*".into(),
            project_id: w.pos,
            host_alias: Some("h".into()),
            org_id: None,
            ..Default::default()
        },
    )
    .unwrap();
    let plan = w.preview(&fake, second).await.plan.unwrap();
    assert_eq!(plan.project_id, w.pos, "the rule beats history");
    assert_eq!(plan.rule_id, Some(rule.rule.id));
    assert_eq!(fake.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn five_person_starts_put_the_offer_on_the_next_preview_and_an_agents_count_nothing() {
    let w = w();
    let fake = Arc::new(Fake::default());
    for n in 1..=OFFER_AFTER {
        let item = w.item(&format!("AG-{n}"));
        w.start(item, w.pos, Decider::Agent).await;
    }
    {
        let s = w.store.lock().unwrap();
        assert!(s.list_start_rules().unwrap().is_empty(), "an agent's start");
    }
    for n in 1..=OFFER_AFTER {
        let item = w.item(&format!("PD-{n}"));
        let p = w.preview(&fake, item).await;
        assert_eq!(p.rule_offer, None, "not before the fifth, start {n}");
        w.start(item, w.pos, Decider::Person).await;
    }
    let next = w.item("PD-99");
    let p = w.preview(&fake, next).await;
    let json = serde_json::to_value(&p).unwrap();
    let offer = p.rule_offer.expect("offered after five");
    assert_eq!((offer.pattern.as_str(), offer.project_id), ("PD-*", w.pos));
    assert_eq!(json["rule_offer"]["state"], "offered");
    // Accepted: the next start is the rule's, and the preview offers no more.
    accept(&w.store, &ViewScope::internal(), offer.id).unwrap();
    let p = w.preview(&fake, next).await;
    assert_eq!(p.rule_offer, None);
    assert_eq!(p.plan.unwrap().rule_id, Some(offer.id));
}

// --- how a rule's starts run (gap plan G7.1) ---------------------------------

impl W {
    /// A preview that names no host: the rules pick it.
    async fn preview_unhosted(&self, item: i64) -> tickets::StartPreview {
        let net = crate::service::trackers::TrackerNet::fake(Arc::new(
            crate::net::https::FakeTransport::new(),
        ));
        tickets::preview_start_decided(
            &self.store,
            &StartArgs {
                item_id: Some(item),
                ..Default::default()
            },
            &ViewScope::internal(),
            &net,
            None,
        )
        .await
        .unwrap()
    }
    fn host(&self, alias: &str, reachable: bool) {
        let s = self.store.lock().unwrap();
        s.upsert_host(alias).unwrap();
        s.conn_for_test()
            .execute(
                "UPDATE hosts SET reachable = ?2 WHERE alias = ?1",
                rusqlite::params![alias, reachable as i64],
            )
            .unwrap();
    }
    fn rule(&self, input: StartRuleInput) -> StartRuleView {
        save(&self.store, &ViewScope::internal(), None, &input).unwrap()
    }
}

#[tokio::test]
async fn a_rule_names_the_account_model_effort_and_agent_of_its_starts() {
    let w = w();
    let rule = w.rule(StartRuleInput {
        pattern: "PD-*".into(),
        project_id: w.pos,
        host_alias: Some("h".into()),
        profile: Some("work".into()),
        model: Some("opus".into()),
        effort: Some("high".into()),
        ..Default::default()
    });
    assert_eq!(rule.rule.profile.as_deref(), Some("work"));
    assert_eq!(rule.rule.model.as_deref(), Some("opus"));
    assert_eq!(rule.rule.effort.as_deref(), Some("high"));
    let item = w.item("PD-7");
    let plan = w
        .preview_unhosted(item)
        .await
        .plan
        .expect("the rule plans it");
    assert_eq!(plan.host_alias, "h");
    assert_eq!(plan.profile.as_deref(), Some("work"));
    assert_eq!(plan.model.as_deref(), Some("opus"));
    assert_eq!(plan.effort.as_deref(), Some("high"));
    assert_eq!(plan.agent, None);
    // The plan reaches the spawn: the session runs with them.
    let seen = Arc::new(Mutex::new(None));
    let seen2 = Arc::clone(&seen);
    let store = Arc::clone(&w.store);
    tickets::start_one(&w.store, &plan, None, &ViewScope::internal(), move |a| {
        *seen2.lock().unwrap() = Some((a.profile.clone(), a.model.clone(), a.effort.clone()));
        let s = store.lock().unwrap();
        let id = s
            .upsert_session("pd-7", &a.host_alias, None, None, 1, 1, "running", None)
            .unwrap();
        std::future::ready(Ok(s.get_session_by_id(id).unwrap().unwrap()))
    })
    .await
    .unwrap();
    assert_eq!(
        seen.lock().unwrap().clone(),
        Some((
            Some("work".into()),
            Some("opus".into()),
            Some("high".into())
        ))
    );
    // The answer a client reads carries them; one it hands back names none.
    let wire = serde_json::to_value(&plan).unwrap();
    assert_eq!(wire["profile"], "work");
    assert_eq!(wire["model"], "opus");
    let back: tickets::StartPlan = serde_json::from_value(wire).unwrap();
    assert_eq!((back.profile, back.model, back.effort), (None, None, None));
}

#[tokio::test]
async fn a_start_takes_the_rules_fallback_host_while_its_host_is_offline() {
    let w = w();
    w.host("mac", false);
    w.host("mercury", true);
    w.rule(StartRuleInput {
        pattern: "PD-*".into(),
        project_id: w.pos,
        host_alias: Some("mac".into()),
        fallback_host: Some("mercury".into()),
        ..Default::default()
    });
    let item = w.item("PD-8");
    let plan = w.preview_unhosted(item).await.plan.unwrap();
    assert_eq!(plan.host_alias, "mercury", "mac is offline");
    assert_eq!(plan.fell_back_from.as_deref(), Some("mac"));
    w.host("mac", true);
    let plan = w.preview_unhosted(item).await.plan.unwrap();
    assert_eq!(plan.host_alias, "mac", "mac is back");
    assert_eq!(plan.fell_back_from, None);
}

#[test]
fn save_checks_the_fallback_account_model_effort_and_agent() {
    let w = w();
    w.host("g", true);
    let base = || StartRuleInput {
        pattern: "PD-*".into(),
        project_id: w.pos,
        host_alias: Some("h".into()),
        ..Default::default()
    };
    let bad = |input: StartRuleInput| {
        save(&w.store, &ViewScope::internal(), None, &input)
            .unwrap_err()
            .code
    };
    assert_eq!(
        bad(StartRuleInput {
            fallback_host: Some("h".into()),
            ..base()
        }),
        codes::E_INVALID,
        "the fallback is the rule's own host"
    );
    assert_eq!(
        bad(StartRuleInput {
            fallback_host: Some("nowhere".into()),
            ..base()
        }),
        codes::E_NOTFOUND
    );
    assert_eq!(
        bad(StartRuleInput {
            effort: Some("loud".into()),
            ..base()
        }),
        codes::E_INVALID
    );
    assert_eq!(
        bad(StartRuleInput {
            profile: Some("../x".into()),
            ..base()
        }),
        codes::E_INVALID
    );
    assert_eq!(
        bad(StartRuleInput {
            agent: Some("agy".into()),
            ..base()
        }),
        codes::E_INVALID,
        "agy has no launch yet"
    );
    assert_eq!(
        bad(StartRuleInput {
            agent: Some("codex".into()),
            profile: Some("work".into()),
            ..base()
        }),
        codes::E_INVALID,
        "Codex keeps its own login"
    );
    let ok = w.rule(StartRuleInput {
        agent: Some("codex".into()),
        fallback_host: Some(" g ".into()),
        model: Some("  ".into()),
        ..base()
    });
    assert_eq!(ok.rule.agent.as_deref(), Some("codex"));
    assert_eq!(ok.rule.fallback_host.as_deref(), Some("g"));
    assert_eq!(ok.rule.model, None, "blank is none");
    // An edit that leaves a field out clears it: the save is the whole rule.
    let edited = save(&w.store, &ViewScope::internal(), Some(ok.rule.id), &base()).unwrap();
    assert_eq!((edited.rule.agent, edited.rule.fallback_host), (None, None));
}

#[tokio::test]
async fn a_placement_rule_names_the_host_and_account_when_no_start_rule_does() {
    let w = w();
    w.host("g", true);
    // PD ran on h in pos: history alone puts the next start there.
    let first = w.item("PD-1");
    w.start(first, w.pos, Decider::Person).await;
    let placement = crate::service::work::structure::rule_save(
        &w.store,
        &crate::service::orgs::OrgScope::All,
        &crate::service::work::structure::RuleInput {
            name: "Payments".into(),
            group: "Payments".into(),
            conditions: crate::store::RuleConditions {
                key_prefix: Some("PD".into()),
                ..Default::default()
            },
            host_alias: Some("g".into()),
            profile: Some("work".into()),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(placement.host_alias.as_deref(), Some("g"));
    let second = w.item("PD-2");
    let plan = w.preview_unhosted(second).await.plan.unwrap();
    assert_eq!(plan.project_id, w.pos);
    assert_eq!(plan.host_alias, "g", "its sessions start here");
    assert_eq!(plan.profile.as_deref(), Some("work"));
    assert_eq!(plan.placement_rule_id, Some(placement.id));
    // A start rule that names a host decides instead.
    w.rule(StartRuleInput {
        pattern: "PD-*".into(),
        project_id: w.pos,
        host_alias: Some("h".into()),
        ..Default::default()
    });
    let plan = w.preview_unhosted(second).await.plan.unwrap();
    assert_eq!(plan.host_alias, "h");
    assert_eq!(
        plan.profile.as_deref(),
        Some("work"),
        "the account still applies"
    );
}
