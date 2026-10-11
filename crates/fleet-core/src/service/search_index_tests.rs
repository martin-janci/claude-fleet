//! The transcript pass: what it keeps of a transcript, how it frames and
//! resumes, and one real run over a JSONL file through a local shell.

use super::*;
use crate::ssh::LocalExec;

fn user(text: &str, ts: &str) -> String {
    serde_json::json!({ "type": "user", "timestamp": ts, "message": { "role": "user", "content": text } })
        .to_string()
}

fn assistant(text: &str, ts: &str) -> String {
    serde_json::json!({
        "type": "assistant",
        "timestamp": ts,
        "message": { "role": "assistant", "content": [
            { "type": "thinking", "thinking": "secret plan" },
            { "type": "text", "text": text },
            { "type": "tool_use", "name": "Bash", "input": { "command": "cat .env" } }
        ] }
    })
    .to_string()
}

#[test]
fn only_prompts_and_replies_are_kept_never_tools_or_thinking() {
    let (t, at) = line_text(&assistant(
        "The race is in the token refresh.",
        "2026-10-10T12:00:01.5Z",
    ))
    .unwrap();
    assert_eq!(t, "The race is in the token refresh.");
    assert_eq!(at, Some(1_791_633_601));
    assert_eq!(
        line_text(&user("why does login loop?", "2026-10-10T12:00:00Z"))
            .unwrap()
            .0,
        "why does login loop?"
    );
    let tool_result = serde_json::json!({ "type": "user", "message": { "content": [
        { "type": "tool_result", "content": "API_KEY=abc" }
    ] } })
    .to_string();
    assert_eq!(line_text(&tool_result), None);
    assert_eq!(line_text(r#"{"type":"summary","summary":"x"}"#), None);
    assert_eq!(
        line_text(&user(
            "<command-name>/clear</command-name>",
            "2026-10-10T12:00:00Z"
        )),
        None
    );
    assert_eq!(line_text("not json"), None);
}

#[test]
fn pieces_take_complete_lines_and_skip_old_ones() {
    let old = user("old question", "2020-01-01T00:00:00Z");
    let new = user("new question", "2026-10-10T12:00:00Z");
    let half = "{\"type\":\"user\",\"mess";
    let bytes = format!("{old}\n{new}\n{half}");
    let (found, consumed) = pieces(bytes.as_bytes(), 100, 1_700_000_000, 1_800_000_000);
    assert_eq!(
        consumed as usize,
        old.len() + new.len() + 2,
        "the half line waits"
    );
    assert_eq!(found.len(), 1);
    assert_eq!(
        found[0].0,
        100 + old.len() as i64 + 1,
        "a piece is keyed by its first line's offset"
    );
    assert_eq!(found[0].1, "new question");
}

#[test]
fn the_output_is_framed_per_session_and_decoded() {
    let data = base64::engine::general_purpose::STANDARD.encode("{\"a\":1}\n");
    let out = format!("M\t3\nF\t7\tc-1.jsonl\t0\t8\nB\t7\t{data}\nF\t9\tc-2.jsonl\t40\t40\n");
    let reads = parse_output(&out);
    assert!(!reads.contains_key(&3));
    assert_eq!(reads[&7].bytes, b"{\"a\":1}\n");
    assert_eq!(reads[&9].offset, 40);
    assert!(reads[&9].bytes.is_empty());
}

#[test]
fn every_value_in_the_script_is_quoted() {
    let s = batch_script(
        &[Target {
            session_id: 1,
            transcript_path: Some("/tmp/a b/$(rm -rf ~).jsonl".into()),
            claude_session_id: "c-1".into(),
            source: None,
            offset: 0,
        }],
        10,
        10,
    );
    assert!(s.contains("'/tmp/a b/$(rm -rf ~).jsonl'"), "{s}");
}

#[tokio::test]
async fn a_pass_indexes_new_text_and_resumes_where_it_stopped() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("c-1.jsonl");
    let now = 1_791_640_000;
    std::fs::write(
        &file,
        format!(
            "{}\n{}\n",
            user("why does the login loop?", "2026-10-10T12:00:00Z"),
            assistant("The redirect drops the SSO cookie.", "2026-10-10T12:00:05Z")
        ),
    )
    .unwrap();
    let st = Mutex::new(Store::open_in_memory().unwrap());
    let sid = {
        let s = st.lock().unwrap();
        s.upsert_host("h1").unwrap();
        let sid = s
            .upsert_session("dev", "h1", None, None, 1, 1, "running", None)
            .unwrap();
        s.set_claude_session_id(sid, "c-1").unwrap();
        s.set_transcript_path_for_row(sid, "c-1", file.to_str().unwrap())
            .unwrap();
        sid
    };
    let exec = LocalExec::default();
    assert_eq!(index_host(&st, &exec, "h1", now).await.unwrap(), 1);
    let found = crate::service::search::search(
        &st,
        &crate::service::view_scope::ViewScope::internal(),
        &crate::service::search::SearchArgs {
            query: "sso cookie".into(),
            kinds: vec!["transcript".into()],
            limit: None,
        },
    )
    .unwrap();
    assert_eq!(found.hits.len(), 1);
    assert_eq!(found.hits[0].session_id, Some(sid));
    assert!(
        found.hits[0].snippet.contains("SSO cookie"),
        "{}",
        found.hits[0].snippet
    );

    // Nothing new: nothing written, the cursor stays.
    assert_eq!(index_host(&st, &exec, "h1", now).await.unwrap(), 0);
    // A new reply is read from where the last pass stopped.
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .open(&file)
        .unwrap();
    use std::io::Write as _;
    writeln!(
        f,
        "{}",
        assistant("Fixed in the callback handler.", "2026-10-10T12:01:00Z")
    )
    .unwrap();
    assert_eq!(index_host(&st, &exec, "h1", now).await.unwrap(), 1);
    let s = st.lock().unwrap();
    let counts = s.search_doc_counts().unwrap();
    assert!(
        counts.contains(&("transcript".to_string(), 2)),
        "{counts:?}"
    );
    // Turned off: every chunk and cursor goes.
    assert_eq!(s.clear_transcript_chunks().unwrap(), 2);
    assert!(s.transcript_cursors().unwrap().is_empty());
}
