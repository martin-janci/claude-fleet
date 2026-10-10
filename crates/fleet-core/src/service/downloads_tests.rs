use super::*;
use crate::service::orgs::OrgScope;
use crate::store::NewDownload;

#[test]
fn stat_answers_are_read_past_a_banner() {
    let out = b"Welcome!\n\n__CF_OUT__\n48213\n/home/u/p/out/report.pdf\n";
    assert_eq!(
        parse_stat(out, "out/report.pdf").unwrap(),
        Stat {
            size: 48213,
            path: "/home/u/p/out/report.pdf".into()
        }
    );
    let e = parse_stat(b"__CF_DL_ERR__ dir\n", "out").unwrap_err();
    assert_eq!(e.code, codes::E_INVALID);
    let e = parse_stat(b"__CF_DL_ERR__ missing\n", "x").unwrap_err();
    assert_eq!(e.code, codes::E_NOTFOUND);
    assert!(parse_stat(b"__CF_OUT__\nnope\n", "x").is_err());
}

#[test]
fn the_stat_script_quotes_the_path() {
    let s = stat_script("fleet-a", "a b'; rm -rf ~");
    assert!(s.contains(&quote("a b'; rm -rf ~")));
}

#[test]
fn paths_and_notes_are_checked() {
    assert!(check_path("").is_err());
    assert!(check_path("a\nb").is_err());
    assert!(check_path("/tmp/x").is_ok());
    assert_eq!(clean_note(Some("  a\tb  ")).as_deref(), Some("a b"));
    assert_eq!(clean_note(Some("   ")), None);
    assert_eq!(base_name("/a/b/report.pdf"), "report.pdf");
}

fn insert(s: &Store, host: &str, org: Option<i64>, size: i64) -> DownloadRow {
    s.insert_download(&NewDownload {
        host_alias: host,
        session_id: Some(1),
        session_name: Some("fleet-a"),
        org_id: org,
        path: "/tmp/f",
        name: "f",
        size,
        source: SOURCE_AGENT,
        note: None,
    })
    .unwrap()
}

#[test]
fn room_is_made_from_the_oldest_ready_files() {
    let s = Store::open_in_memory().unwrap();
    let b = Budget {
        max_file: 20,
        max_total: 20,
        keep_secs: 0,
    };
    assert_eq!(make_room(&s, &b, 21).unwrap_err().code, codes::E_LIMIT);
    let a = insert(&s, "h", None, 8);
    let c = insert(&s, "h", None, 8);
    s.finish_download(a.id, "x").unwrap();
    s.finish_download(c.id, "x").unwrap();
    assert!(make_room(&s, &b, 4).unwrap().is_empty());
    assert_eq!(make_room(&s, &b, 10).unwrap(), vec![a.id]);
    assert_eq!(make_room(&s, &b, 14).unwrap(), vec![a.id, c.id]);
    // A copy in flight is never dropped.
    let _busy = insert(&s, "h", None, 10);
    assert_eq!(make_room(&s, &b, 10).unwrap(), vec![a.id, c.id]);
    insert(&s, "h", None, 5);
    assert_eq!(make_room(&s, &b, 6).unwrap_err().code, codes::E_LIMIT);
}

#[test]
fn a_host_sees_its_own_files_and_an_org_its_own() {
    let s = Store::open_in_memory().unwrap();
    let mine = insert(&s, "web-1", Some(1), 1);
    let other = insert(&s, "web-2", None, 1);
    let theirs = insert(&s, "web-3", Some(2), 1);
    let host = OrgScope::Host {
        alias: "web-1".into(),
        org: Some(1),
        isolated: Default::default(),
    };
    // No `sessions` row backs any of these, so `visible` takes its
    // session-is-gone arm: the ORG answer alone, which is what this test is
    // about. The person half — `may_own` on a LIVE session row — is the
    // other arm, held by
    // `only_the_owner_reaches_a_private_sessions_download` above.
    let host_view = crate::service::view_scope::ViewScope::internal().with_org(host.clone());
    assert!(visible(&s, &host_view, &mine));
    assert!(!visible(&s, &host_view, &other));
    assert!(visible(
        &s,
        &crate::service::view_scope::ViewScope::internal(),
        &theirs
    ));
    let ids: Vec<_> = list(&s, &host_view, &ListDownloadsArgs::default())
        .unwrap()
        .downloads
        .into_iter()
        .map(|d| d.id)
        .collect();
    assert_eq!(ids, vec![mine.id]);
    // Removing a row it cannot see is a no-op, not a leak.
    assert!(!remove(&s, &host_view, theirs.id).unwrap());
    assert!(s.download(theirs.id).unwrap().is_some());
}

/// **The `own` tier, and the second route M1 fences** (multi-user M1 ·
/// `mcp::downloads_route`, integration decisions Q2 and D2).
///
/// One private session owned by `ada`, one download taken out of it, and
/// every caller shape the feature has, asked through each of its three
/// paths — `visible` (what `list_downloads` filters on), `open_ready` (the
/// bytes `GET /downloads/<id>` streams) and `remove`. The claim is the TIER,
/// not merely the fence: a `watch` grantee and a `drive` grantee are BOTH
/// refused, because a download is an unconstrained absolute-path read of
/// ada's host and spec §4.3 invariant 5 says no grant confers a terminal.
///
/// The per-host token arm matters on its own: `downloads_route` refuses a
/// `host_alias` caller on its first lines, and this holds that deleting that
/// early return would still not open the route — the GATE refuses it.
///
/// Every scope here is built by `Caller::view_scope`, the one constructor
/// (`view_scope_tests::only_caller_view_scope_constructs_a_view_scope`), so
/// what is tested is the real path a request takes and not a hand-made
/// scope.
#[test]
fn only_the_owner_reaches_a_private_sessions_download() {
    use crate::mcp::auth::{Caller, ClientRef, TokenMode};
    use crate::service::view_scope::ViewScope;
    use crate::store::{GrantRecipient, GRANT_DRIVE, GRANT_WATCH};
    test_dir();
    let _files = files_guard();
    let s = Store::open_in_memory().unwrap();
    let ada = s.create_person("ada", None).unwrap().id;
    let bob = s.create_person("bob", None).unwrap().id;
    let carol = s.create_person("carol", None).unwrap().id;
    let eve = s.create_person("eve", None).unwrap().id;
    s.upsert_host("web-1").unwrap();
    let session = s
        .upsert_session("fleet-a", "web-1", None, None, 1, 1, "running", None)
        .unwrap();
    assert!(s.claim_if_unclaimed(session, Some(ada)).unwrap());
    s.grant_session(session, GrantRecipient::Person(bob), GRANT_WATCH, ada)
        .unwrap();
    s.grant_session(session, GrantRecipient::Person(carol), GRANT_DRIVE, ada)
        .unwrap();

    let row = s
        .insert_download(&NewDownload {
            host_alias: "web-1",
            session_id: Some(session),
            session_name: Some("fleet-a"),
            org_id: None,
            path: "/home/ada/.claude/.credentials.json",
            name: ".credentials.json",
            size: 5,
            source: SOURCE_PERSON,
            note: None,
        })
        .unwrap();
    // `test_dir()` is ONE directory for the whole test binary and
    // `file_of(id)` names the copy by row id, so two tests that both write a
    // file for row 1 of their own in-memory store collide — which is exactly
    // what broke `a_copy_is_written_in_chunks_hashed_and_moved_into_place`
    // when this test was first written. Move this row out of the way instead
    // of relying on the order tests happen to run in. A distinct id is not
    // enough on its own — the other test's sweep deletes any file its store
    // has no row for — hence `files_guard` above.
    let id = 90_001;
    s.conn_ref()
        .execute("UPDATE downloads SET id = ?2 WHERE id = ?1", [row.id, id])
        .unwrap();
    std::fs::write(file_of(id).unwrap(), b"hello").unwrap();
    assert!(s.finish_download(id, "x").unwrap(), "the row is ready");
    let row = s.download(id).unwrap().unwrap();

    // A person's own device, through the one constructor.
    let device = |person: i64, org: Option<i64>| {
        Caller {
            api: None,
            host_alias: None,
            client: Some(ClientRef {
                id: 1,
                name: "phone".into(),
                trusted: false,
                org_id: org,
                person_id: Some(person),
            }),
            mode: TokenMode::Full,
            pane: None,
            is_personal_owner: false,
        }
        .view_scope(&s)
        .unwrap()
    };
    // The session's own Claude: its host, and the pane this request proves.
    // No pane header here, and the row is `private` rather than `unclaimed`,
    // so §4.4 refuses it either way — which is the point.
    let host_token = Caller {
        api: None,
        host_alias: Some("web-1".into()),
        client: None,
        mode: TokenMode::Full,
        pane: None,
        is_personal_owner: false,
    }
    .view_scope(&s)
    .unwrap();

    // The owner, and the hub's own reader.
    assert!(visible(&s, &device(ada, None), &row), "the owner");
    assert!(visible(&s, &ViewScope::internal(), &row), "the hub itself");

    // Everybody else — and the two grantees are the point.
    for (label, scope) in [
        ("a watch grantee", device(bob, None)),
        ("a drive grantee", device(carol, None)),
        ("another person", device(eve, None)),
        ("the session's own host token", host_token),
        ("a client bound to another org", device(eve, Some(7))),
    ] {
        assert!(!visible(&s, &scope, &row), "{label} sees the row");
        // The bytes `GET /downloads/<id>` would stream.
        assert_eq!(
            open_ready(&s, &scope, row.id).unwrap_err().code,
            codes::E_NOTFOUND,
            "{label} fetches the bytes"
        );
        // Removing what it cannot see is a no-op, not a leak and not an
        // answer that tells it the id exists.
        assert!(!remove(&s, &scope, row.id).unwrap(), "{label} removes it");
        assert!(
            s.download(row.id).unwrap().is_some(),
            "{label} destroyed it"
        );
    }

    // The owner really does get the bytes, so the five refusals above are
    // the tier and not a broken fixture.
    let (got, path) = open_ready(&s, &device(ada, None), row.id).unwrap();
    assert_eq!(got.id, row.id);
    assert_eq!(std::fs::read(path).unwrap(), b"hello");
}

#[test]
fn names_are_served_safely() {
    assert_eq!(content_type("A.PDF"), "application/pdf");
    assert_eq!(content_type("noext"), "application/octet-stream");
    assert_eq!(
        content_disposition("správa \"q3\".pdf"),
        "attachment; filename=\"spr_va _q3_.pdf\"; filename*=UTF-8''spr%C3%A1va%20%22q3%22.pdf"
    );
}

/// The process's downloads dir, made once for every test that copies.
/// `downloads::DIR` is a process-wide `OnceLock` — right for a real
/// process, which has one downloads directory — so `init` keeps and returns
/// whichever directory came first, here or in
/// `mcp::tools::tests_sessions_isolation::downloads_dir`.
fn test_dir() -> &'static Path {
    static D: OnceLock<PathBuf> = OnceLock::new();
    D.get_or_init(|| {
        let tmp = tempfile::tempdir().unwrap().keep();
        let s = Store::open_in_memory().unwrap();
        init(&tmp, &s).unwrap()
    })
}

/// The lock every test that puts BYTES in the shared dir holds for as long
/// as its files must live.
///
/// One directory per process plus `file_of(id)` naming a copy by row id
/// means every such test writes into one namespace, and two separate
/// in-memory stores both hand out id 1. Worse, `sweep_with`'s orphan pass
/// walks the whole directory and deletes every file ITS OWN store has no
/// row for — which is every other test's file, whatever id it chose. So
/// the files of two tests may not overlap in time: this serialises them,
/// which makes the suite order- and interleaving-independent without
/// touching the production `OnceLock`.
fn files_guard() -> std::sync::MutexGuard<'static, ()> {
    static L: Mutex<()> = Mutex::new(());
    L.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn a_copy_is_written_in_chunks_hashed_and_moved_into_place() {
    use crate::ssh_fake::{FakeSsh, Match, Reply};
    test_dir();
    let _files = files_guard();
    let s = Store::open_in_memory().unwrap();
    let row = insert(&s, "web-1", None, 10);
    let ssh = FakeSsh::new();
    // Two 6-byte chunks for a 10-byte file: offsets 1 and 7 (tail -c +N).
    ssh.on(
        Match::script_contains("tail -c +1 "),
        Reply::ok("banner\n\n__CF_OUT__\nhello "),
    );
    ssh.on(
        Match::script_contains("tail -c +7 "),
        Reply::ok("\n__CF_OUT__\nworl"),
    );
    let heard = Mutex::new(vec![]);
    let sha = fetch_chunked(&ssh, &row, 6, &|got| heard.lock().unwrap().push(got))
        .await
        .unwrap();
    // The running total after each slice but the last (then it is ready).
    assert_eq!(*heard.lock().unwrap(), [6]);
    let got = std::fs::read(file_of(row.id).unwrap()).unwrap();
    assert_eq!(got, b"hello worl");
    use sha2::Digest;
    assert_eq!(sha, hex::encode(sha2::Sha256::digest(b"hello worl")));
    assert!(!part_of(row.id).unwrap().exists());

    // A host that stops answering leaves no bytes behind once the caller
    // cleans up, and says where it stopped.
    let short = insert(&s, "web-1", None, 10);
    let ssh = FakeSsh::new();
    ssh.on(
        Match::script_contains("tail -c +1 "),
        Reply::ok("\n__CF_OUT__\nhello "),
    );
    ssh.on(
        Match::script_contains("tail -c +7 "),
        Reply::fail(5, "gone"),
    );
    let e = fetch_chunked(&ssh, &short, 6, &|_| {}).await.unwrap_err();
    assert!(e.message.contains("stopped at 6 of 10"), "{}", e.message);
    remove_files(short.id);
    assert!(!part_of(short.id).unwrap().exists());

    // In the same test: the downloads dir is one per process, and row ids
    // of separate in-memory stores collide, so no two tests may write it.
    the_sweep_drops_expired_rows_and_orphaned_bytes();
}

fn the_sweep_drops_expired_rows_and_orphaned_bytes() {
    test_dir();
    let store = Mutex::new(Store::open_in_memory().unwrap());
    let (old, orphan) = {
        let s = store.lock().unwrap();
        settings::set(&s, settings::DOWNLOADS_KEEP_SECS, "60").unwrap();
        let old = insert(&s, "h", None, 1);
        s.finish_download(old.id, "x").unwrap();
        std::fs::write(file_of(old.id).unwrap(), b"x").unwrap();
        // Bytes whose row is gone.
        let orphan = insert(&s, "h", None, 1);
        std::fs::write(file_of(orphan.id).unwrap(), b"y").unwrap();
        s.delete_download(orphan.id).unwrap();
        (old, orphan)
    };
    let ready_at = store
        .lock()
        .unwrap()
        .download(old.id)
        .unwrap()
        .unwrap()
        .ready_at
        .unwrap();
    assert_eq!(sweep_with(&store, ready_at + 59, ORPHAN_MIN_AGE_SECS), 0);
    assert_eq!(sweep_with(&store, ready_at + 60, ORPHAN_MIN_AGE_SECS), 1);
    // A fresh orphan is left for a later pass (a copy may just have begun).
    assert!(file_of(orphan.id).unwrap().exists());
    assert_eq!(sweep_with(&store, ready_at + 60, 0), 0);
    assert!(!file_of(old.id).unwrap().exists());
    assert!(!file_of(orphan.id).unwrap().exists());
}

#[test]
fn a_row_being_copied_carries_its_bytes_so_far() {
    let s = Store::open_in_memory().unwrap();
    let mut row = insert(&s, "web-1", None, 10);
    // The progress map is the process's: an id no other test's store reaches.
    row.id = 9_000_001;
    let b = Budget::from_store(&s);
    assert_eq!(present(&b, row.clone()).fetched_bytes, Some(0));
    progress::set(row.id, 6);
    assert_eq!(present(&b, row.clone()).fetched_bytes, Some(6));
    progress::clear(row.id);
    let failed = DownloadRow {
        state: "failed".into(),
        ..row
    };
    assert_eq!(
        present(&b, failed).fetched_bytes,
        None,
        "only while fetching"
    );
}

/// Review r04 F3: a reaped session's download (its bytes are what
/// `GET /downloads/<id>` serves) stays its owner's; another person on the
/// same hub neither lists nor removes it.
#[test]
fn a_reaped_sessions_download_stays_the_owners() {
    use crate::mcp::auth::{Caller, ClientRef, TokenMode};
    let s = Store::open_in_memory().unwrap();
    let ada = s.create_person("ada", None).unwrap().id;
    let eve = s.create_person("eve", None).unwrap().id;
    s.upsert_host("web-1").unwrap();
    let session = s
        .upsert_session("fleet-a", "web-1", None, None, 1, 1, "running", None)
        .unwrap();
    assert!(s.claim_if_unclaimed(session, Some(ada)).unwrap());
    let row = s
        .insert_download(&NewDownload {
            host_alias: "web-1",
            session_id: Some(session),
            session_name: Some("fleet-a"),
            org_id: None,
            path: "/home/ada/.claude/.credentials.json",
            name: "creds",
            size: 1,
            source: SOURCE_AGENT,
            note: None,
        })
        .unwrap();
    assert_eq!(row.owner_person_id, Some(ada));
    s.delete_session(session).unwrap();
    let device = |person: i64| {
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
        .view_scope(&s)
        .unwrap()
    };
    let row = s.download(row.id).unwrap().unwrap();
    assert!(!visible(&s, &device(eve), &row), "another person");
    assert!(list(&s, &device(eve), &ListDownloadsArgs::default())
        .unwrap()
        .downloads
        .is_empty());
    assert!(!remove(&s, &device(eve), row.id).unwrap());
    assert!(visible(&s, &device(ada), &row), "the owner keeps it");
}
