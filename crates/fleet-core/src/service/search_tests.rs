//! `search` over the full-text index (search phase 3): what each kind is
//! found by, that the triggers keep the index in step, and that every hit
//! passes its fence.

use super::*;
use crate::service::orgs::OrgScope;
use crate::store::{NativeItem, TranscriptChunk};

fn args(q: &str) -> SearchArgs {
    SearchArgs {
        query: q.into(),
        ..Default::default()
    }
}

fn kinds_of(p: &SearchPage) -> Vec<(&str, &str)> {
    p.hits
        .iter()
        .map(|h| (h.kind.as_str(), h.title.as_str()))
        .collect()
}

/// A person's device on a hub with other people, through the one
/// constructor (`Caller::view_scope`): no unclaimed rows, no grants.
fn stranger(st: &Mutex<Store>) -> ViewScope {
    use crate::mcp::auth::{Caller, ClientRef, TokenMode};
    let caller = Caller {
        host_alias: None,
        client: Some(ClientRef {
            id: 0,
            name: "device-of-7".into(),
            trusted: true,
            org_id: None,
            person_id: Some(7),
        }),
        mode: TokenMode::Full,
        pane: None,
        is_personal_owner: false,
        api: None,
    };
    caller.view_scope(&st.lock().unwrap()).unwrap()
}

fn world() -> (Mutex<Store>, i64) {
    let s = Store::open_in_memory().unwrap();
    s.upsert_host("mefistos").unwrap();
    let sid = s
        .upsert_session(
            "pay-refunds",
            "mefistos",
            None,
            None,
            1,
            100,
            "running",
            None,
        )
        .unwrap();
    s.set_friendly_name("mefistos", "pay-refunds", Some("Oprava prihlásenia"))
        .unwrap();
    s.set_last_prompt(sid, "fix the SSO redirect loop on login")
        .unwrap();
    s.create_native_item(&NativeItem {
        title: "Zrýchliť export faktúr",
        parent_id: None,
        project_id: None,
        notes: Some("CSV export times out for large tenants"),
    })
    .unwrap();
    (Mutex::new(s), sid)
}

#[test]
fn every_word_any_order_accents_and_prefixes() {
    let (st, sid) = world();
    let view = ViewScope::internal();
    let p = search(&st, &view, &args("prihlasenia")).unwrap();
    assert_eq!(kinds_of(&p), [("session", "Oprava prihlásenia")]);
    assert_eq!(p.hits[0].session_id, Some(sid));
    assert_eq!(p.hits[0].host_alias.as_deref(), Some("mefistos"));
    // The match is marked on the shown text, in UTF-16 units.
    assert_eq!(p.hits[0].title_marks, vec![[7, 18]]);

    let p = search(&st, &view, &args("redirect sso")).unwrap();
    assert_eq!(p.hits.len(), 1, "the last prompt is in the session's text");
    assert!(
        p.hits[0].snippet.contains("SSO redirect"),
        "{}",
        p.hits[0].snippet
    );

    let p = search(&st, &view, &args("faktur expo")).unwrap();
    assert_eq!(kinds_of(&p), [("item", "TASK-1 Zrýchliť export faktúr")]);
    assert_eq!(
        p.hits[0].task_id.as_deref().map(|t| t.starts_with("item:")),
        Some(true)
    );

    assert!(search(&st, &view, &args("faktur gpu"))
        .unwrap()
        .hits
        .is_empty());
    assert!(
        search(&st, &view, &args("\" * ( "))
            .unwrap()
            .hits
            .is_empty(),
        "no word, no query"
    );
}

#[test]
fn the_index_follows_its_rows() {
    let (st, sid) = world();
    let view = ViewScope::internal();
    {
        let s = st.lock().unwrap();
        s.set_last_prompt(sid, "rotate the signing keys").unwrap();
    }
    assert!(
        search(&st, &view, &args("redirect"))
            .unwrap()
            .hits
            .is_empty(),
        "the old prompt is gone"
    );
    assert_eq!(search(&st, &view, &args("signing")).unwrap().hits.len(), 1);
    {
        let s = st.lock().unwrap();
        s.delete_session(sid).unwrap();
    }
    assert!(
        search(&st, &view, &args("signing"))
            .unwrap()
            .hits
            .is_empty(),
        "a deleted session leaves"
    );
}

#[test]
fn a_session_hit_passes_the_session_fence() {
    let (st, _) = world();
    // An unclaimed row on a hub with several people: not this person's.
    let p = search(&st, &stranger(&st), &args("prihlasenia")).unwrap();
    assert!(p.hits.is_empty(), "{:?}", kinds_of(&p));
    // The task is org data, not a session: the same person finds it.
    assert_eq!(
        search(&st, &stranger(&st), &args("faktur"))
            .unwrap()
            .hits
            .len(),
        1
    );
}

#[test]
fn an_item_hit_passes_the_org_boundary() {
    let (st, _) = world();
    let other_org = crate::service::view_scope::org_only_view(&OrgScope::Org {
        org: 999,
        sees_unassigned: false,
    });
    assert!(search(&st, &other_org, &args("faktur"))
        .unwrap()
        .hits
        .is_empty());
}

#[test]
fn transcript_chunks_are_found_and_pruned() {
    let (st, sid) = world();
    {
        let s = st.lock().unwrap();
        s.upsert_transcript_chunk(&TranscriptChunk {
            session_id: sid,
            claude_session_id: "c-1",
            offset: 0,
            text: "The flaky test was a race in the token refresh",
            at: 50,
        })
        .unwrap();
    }
    let view = ViewScope::internal();
    let p = search(
        &st,
        &view,
        &SearchArgs {
            query: "token refresh".into(),
            kinds: vec!["transcript".into()],
            limit: None,
        },
    )
    .unwrap();
    assert_eq!(p.hits.len(), 1);
    assert_eq!(p.hits[0].session_id, Some(sid));
    assert!(!p.transcripts_indexed, "off by default");
    {
        let s = st.lock().unwrap();
        assert_eq!(s.prune_transcript_chunks(60).unwrap(), 1);
    }
    assert!(search(&st, &view, &args("refresh"))
        .unwrap()
        .hits
        .is_empty());
}

#[test]
fn an_unknown_kind_is_refused() {
    let (st, _) = world();
    let err = search(
        &st,
        &ViewScope::internal(),
        &SearchArgs {
            query: "x".into(),
            kinds: vec!["mail".into()],
            limit: None,
        },
    )
    .unwrap_err();
    assert!(err.message.contains("kinds"), "{}", err.message);
}

#[test]
fn fts_query_quotes_every_word_as_a_prefix() {
    assert_eq!(
        fts_query("ABC-12 login").as_deref(),
        Some("\"ABC-12\"* AND \"login\"*")
    );
    assert_eq!(
        fts_query("say \"hi\" OR x").as_deref(),
        Some("\"say\"* AND \"hi\"* AND \"OR\"* AND \"x\"*")
    );
    assert_eq!(fts_query("  - * "), None);
}
