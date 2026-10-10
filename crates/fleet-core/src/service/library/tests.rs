use super::*;
use crate::mcp::auth::{Caller, ClientRef, TokenMode};
use crate::store::{GrantRecipient, GRANT_DRIVE};

fn file(path: &str) -> LibraryFile {
    LibraryFile {
        path: path.into(),
        name: None,
        size: Some(12),
    }
}

/// A person's own device, through the one constructor.
fn device(s: &Store, person: i64) -> ViewScope {
    Caller {
        api: None,
        host_alias: None,
        client: Some(ClientRef {
            id: 1,
            name: "phone".into(),
            trusted: false,
            org_id: None,
            person_id: Some(person),
        }),
        mode: TokenMode::Full,
        pane: None,
        is_personal_owner: false,
    }
    .view_scope(s)
    .unwrap()
}

#[test]
fn an_upload_is_listed_with_its_sessions_host_never_the_callers_word() {
    let s = Store::open_in_memory().unwrap();
    s.upsert_host("web-1").unwrap();
    let session = s
        .upsert_session("fleet-a", "web-1", None, None, 1, 1, "running", None)
        .unwrap();
    let rows = add(
        &s,
        &ViewScope::internal(),
        &AddArgs {
            kind: KIND_UPLOAD.into(),
            session_id: session,
            files: vec![file("/w/.claude-fleet-attachments/spec.pdf")],
        },
    )
    .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].host_alias, "web-1");
    assert_eq!(rows[0].session_name.as_deref(), Some("fleet-a"));
    assert_eq!(rows[0].name, "spec.pdf");
    let listed = list(&s, &ViewScope::internal(), &ListArgs::default()).unwrap();
    assert_eq!(listed.items, rows);
    let other_host = ListArgs {
        host_alias: Some("web-2".into()),
        ..ListArgs::default()
    };
    assert!(list(&s, &ViewScope::internal(), &other_host)
        .unwrap()
        .items
        .is_empty());
}

#[test]
fn add_refuses_a_bad_kind_no_files_and_an_unknown_session() {
    let s = Store::open_in_memory().unwrap();
    let scope = ViewScope::internal();
    let args = |kind: &str, files: Vec<LibraryFile>, session_id| AddArgs {
        kind: kind.into(),
        session_id,
        files,
    };
    let e = add(&s, &scope, &args("drive", vec![file("/a")], 1)).unwrap_err();
    assert_eq!(e.code, codes::E_INVALID);
    let e = add(&s, &scope, &args(KIND_UPLOAD, vec![], 1)).unwrap_err();
    assert_eq!(e.code, codes::E_INVALID);
    let e = add(&s, &scope, &args(KIND_UPLOAD, vec![file("  ")], 1)).unwrap_err();
    assert_eq!(e.code, codes::E_INVALID);
    let e = add(&s, &scope, &args(KIND_ATTACHMENT, vec![file("/a")], 99)).unwrap_err();
    assert_eq!(e.code, codes::E_NOTFOUND);
}

/// The Library names paths on a host, so it is the download rule: the
/// session's owner, not a grantee and not another person.
#[test]
fn only_the_owner_sees_or_adds_a_sessions_files() {
    let s = Store::open_in_memory().unwrap();
    let ada = s.create_person("ada", None).unwrap().id;
    let carol = s.create_person("carol", None).unwrap().id;
    let eve = s.create_person("eve", None).unwrap().id;
    s.upsert_host("web-1").unwrap();
    let session = s
        .upsert_session("fleet-a", "web-1", None, None, 1, 1, "running", None)
        .unwrap();
    assert!(s.claim_if_unclaimed(session, Some(ada)).unwrap());
    s.grant_session(session, GrantRecipient::Person(carol), GRANT_DRIVE, ada)
        .unwrap();
    let args = AddArgs {
        kind: KIND_ATTACHMENT.into(),
        session_id: session,
        files: vec![file("/w/notes.md")],
    };
    let row = add(&s, &device(&s, ada), &args).unwrap().remove(0);
    for (label, scope) in [
        ("a drive grantee", device(&s, carol)),
        ("another person", device(&s, eve)),
    ] {
        assert!(!visible(&s, &scope, &row), "{label} sees the row");
        assert!(list(&s, &scope, &ListArgs::default())
            .unwrap()
            .items
            .is_empty());
        assert_eq!(
            add(&s, &scope, &args).unwrap_err().code,
            codes::E_NOTFOUND,
            "{label} adds"
        );
        assert!(!remove(&s, &scope, row.id).unwrap(), "{label} removes it");
    }
    assert!(remove(&s, &device(&s, ada), row.id).unwrap());
    assert!(s.library_item(row.id).unwrap().is_none());
}

/// Review r04 F3: once the session row is reaped, its files stay the
/// owner's; another person on the same hub (and org) sees and removes none.
#[test]
fn a_reaped_sessions_files_stay_the_owners() {
    let s = Store::open_in_memory().unwrap();
    let ada = s.create_person("ada", None).unwrap().id;
    let eve = s.create_person("eve", None).unwrap().id;
    s.upsert_host("web-1").unwrap();
    let session = s
        .upsert_session("fleet-a", "web-1", None, None, 1, 1, "running", None)
        .unwrap();
    assert!(s.claim_if_unclaimed(session, Some(ada)).unwrap());
    let args = AddArgs {
        kind: KIND_UPLOAD.into(),
        session_id: session,
        files: vec![file("/home/ada/contract-SECRET.pdf")],
    };
    let row = add(&s, &device(&s, ada), &args).unwrap().remove(0);
    assert_eq!(row.owner_person_id, Some(ada));
    s.delete_session(session).unwrap();
    let row = s.library_item(row.id).unwrap().unwrap();
    assert!(!visible(&s, &device(&s, eve), &row), "another person");
    assert!(list(&s, &device(&s, eve), &ListArgs::default())
        .unwrap()
        .items
        .is_empty());
    assert!(!remove(&s, &device(&s, eve), row.id).unwrap());
    assert!(visible(&s, &device(&s, ada), &row), "the owner keeps it");
    assert!(remove(&s, &device(&s, ada), row.id).unwrap());
}

#[test]
fn the_index_keeps_its_newest_rows() {
    let s = Store::open_in_memory().unwrap();
    fn new(path: &str) -> NewLibraryItem<'_> {
        NewLibraryItem {
            kind: KIND_ATTACHMENT,
            host_alias: "web-1",
            session_id: None,
            session_name: None,
            org_id: None,
            path,
            name: "x",
            size: None,
        }
    }
    let first = s.insert_library_item(&new("/first")).unwrap();
    s.conn_ref()
        .execute(
            "UPDATE sqlite_sequence SET seq = ?1 WHERE name = 'library_items'",
            [first.id + crate::store::LIBRARY_KEEP],
        )
        .unwrap();
    let last = s.insert_library_item(&new("/last")).unwrap();
    assert!(
        s.library_item(first.id).unwrap().is_none(),
        "the oldest went"
    );
    assert!(s.library_item(last.id).unwrap().is_some());
}
