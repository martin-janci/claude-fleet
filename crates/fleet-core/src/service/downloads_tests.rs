use super::*;
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
    assert!(visible(&host, &mine));
    assert!(!visible(&host, &other));
    assert!(visible(&OrgScope::All, &theirs));
    let ids: Vec<_> = list(&s, &host, &ListDownloadsArgs::default())
        .unwrap()
        .downloads
        .into_iter()
        .map(|d| d.id)
        .collect();
    assert_eq!(ids, vec![mine.id]);
    // Removing a row it cannot see is a no-op, not a leak.
    assert!(!remove(&s, &host, theirs.id).unwrap());
    assert!(s.download(theirs.id).unwrap().is_some());
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
fn test_dir() -> &'static Path {
    static D: OnceLock<PathBuf> = OnceLock::new();
    D.get_or_init(|| {
        let tmp = tempfile::tempdir().unwrap().keep();
        let s = Store::open_in_memory().unwrap();
        init(&tmp, &s).unwrap()
    })
}

#[tokio::test]
async fn a_copy_is_written_in_chunks_hashed_and_moved_into_place() {
    use crate::ssh_fake::{FakeSsh, Match, Reply};
    test_dir();
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
    let sha = fetch_chunked(&ssh, &row, 6).await.unwrap();
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
    let e = fetch_chunked(&ssh, &short, 6).await.unwrap_err();
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
    assert_eq!(sweep(&store, ready_at + 59), 0);
    assert_eq!(sweep(&store, ready_at + 60), 1);
    assert!(!file_of(old.id).unwrap().exists());
    assert!(!file_of(orphan.id).unwrap().exists());
}
