//! The `claude -p haiku` baseline: the prompt (the redacted request only),
//! the command (flags and quoting), the reply's parsing, and one call
//! through a fake SSH transport.

use super::*;
use crate::service::decide::status_map;
use crate::ssh_fake::{FakeSsh, Match, Reply};
use serde_json::json;
use std::collections::BTreeMap;

fn cfg(host: &str) -> HaikuConfig {
    HaikuConfig::new(host, None, None).unwrap()
}

fn board() -> Vec<String> {
    ["nové", "v riešení", "čaká na klienta", "hotovo"]
        .iter()
        .map(|s| s.to_string())
        .collect()
}

/// A choice between two ids, with a state that carries a URL and an email.
fn leaky_request() -> JevRequest {
    let mut criteria = BTreeMap::new();
    criteria.insert("i1".to_string(), Some(json!("Login redirect loops")));
    criteria.insert("none".to_string(), Some(json!("None of these")));
    JevRequest {
        state: json!({ "first_prompt": "see https://acme.io/x and mail bob@acme.io" }),
        question: Question::Choice {
            instructions: json!("Which item?"),
            criteria,
        },
    }
}

/// The envelope `claude -p --output-format json` prints around `text`.
fn envelope(text: &str) -> String {
    json!({
        "type": "result",
        "subtype": "success",
        "is_error": false,
        "duration_ms": 1200,
        "result": text,
        "session_id": "x",
        "total_cost_usd": 0.00123,
        "usage": {
            "input_tokens": 12,
            "cache_creation_input_tokens": 100,
            "cache_read_input_tokens": 1000,
            "output_tokens": 9
        }
    })
    .to_string()
}

fn ran(stdout: &str) -> Reply {
    Reply::ok(&format!("motd line\n{HAIKU_TAG}run\n{stdout}\n"))
}

// --- the configuration ------------------------------------------------------------

#[test]
fn the_flags_are_checked() {
    let c = cfg("h1");
    assert_eq!((c.model.as_str(), c.timeout.as_secs()), ("haiku", 120));
    assert_eq!(c.host_timeout_secs(), 110);
    assert!(HaikuConfig::new("h1", Some("sonnet"), Some(30)).is_ok());
    assert!(HaikuConfig::new("h1", Some("claude-opus-5-5"), None).is_err());
    assert!(HaikuConfig::new("h1", Some("haiku; id"), None).is_err());
    assert!(HaikuConfig::new("h1", None, Some(5)).is_err());
    assert!(HaikuConfig::new("h1", None, Some(601)).is_err());
    assert!(HaikuConfig::new("-oProxyCommand=id", None, None).is_err());
    assert!(HaikuConfig::new("h 1", None, None).is_err());
    assert!(HaikuConfig::new("", None, None).is_err());
    let note = c.consent_note();
    assert!(
        note.contains("host h1") && note.contains("Anthropic"),
        "{note}"
    );
    assert!(note.contains("decision_runs"), "{note}");
}

// --- the prompt -------------------------------------------------------------------

#[test]
fn the_prompt_is_the_redacted_state_and_the_options_only() {
    let req = status_map::question_for("čaká na klienta", &board());
    let (p, ids) = prompt_for(&req).unwrap();
    // The same state the envelope would send, as JSON.
    assert!(p.contains(&canonical_json(&req.redacted().state)), "{p}");
    assert!(p.contains("\"section\":\"čaká na klienta\""), "{p}");
    // The adapter's instruction and every option with its description.
    assert!(p.contains(status_map::INSTRUCTIONS), "{p}");
    for (id, desc) in status_map::OPTIONS {
        assert!(p.contains(&format!("- {id}: {desc}")), "{id}: {p}");
    }
    let mut want: Vec<String> = status_map::OPTIONS
        .iter()
        .map(|(k, _)| k.to_string())
        .collect();
    want.sort();
    assert_eq!(ids, want);
    // And the reply format.
    assert!(p.starts_with(PREAMBLE) && p.ends_with(REPLY_FORMAT), "{p}");
    // Nothing else: the text is exactly the pieces above.
    let rebuilt_len = PREAMBLE.len()
        + "\n\n<state>\n".len()
        + canonical_json(&req.redacted().state).len()
        + "\n</state>\n\nQuestion: ".len()
        + status_map::INSTRUCTIONS.len()
        + "\n\nOptions (answer with the id before the colon):\n".len()
        + status_map::OPTIONS
            .iter()
            .map(|(k, d)| 2 + k.len() + 2 + d.len() + 1)
            .sum::<usize>()
        + 1
        + REPLY_FORMAT.len();
    assert_eq!(p.len(), rebuilt_len);
}

#[test]
fn the_prompt_is_redacted_like_the_envelopes_request() {
    let (p, ids) = prompt_for(&leaky_request()).unwrap();
    assert!(!p.contains("acme.io"), "{p}");
    assert!(!p.contains("bob@"), "{p}");
    assert!(p.contains("[url]") && p.contains("[email]"), "{p}");
    assert_eq!(ids, vec!["i1".to_string(), "none".to_string()]);
    assert!(p.contains("- i1: Login redirect loops"), "{p}");
}

#[test]
fn only_a_choice_is_asked() {
    let req = JevRequest {
        state: json!("x"),
        question: Question::Noul {
            instructions: json!("is it?"),
            criteria: None,
        },
    };
    assert!(prompt_for(&req).is_err());
}

// --- the command ------------------------------------------------------------------

#[test]
fn the_run_has_no_tools_no_mcp_no_hooks_and_no_transcript() {
    let sc = haiku_script("haiku", 110).unwrap();
    for flag in [
        "claude -p --model 'haiku' --output-format json",
        r#"--settings '{"disableAllHooks":true}'"#,
        "--tools ''",
        "--strict-mcp-config",
        "--no-session-persistence | head -c 65536",
        "timeout 110",
        "mktemp -d",
    ] {
        assert!(sc.contains(flag), "missing {flag:?} in {sc}");
    }
    for never in [
        "--mcp-config ",
        "--resume",
        "--dangerously",
        "--permission-mode",
        r#""hooks":{}"#,
        // stdin carries the prompt: it must not be closed or redirected.
        "</dev/null",
        "<&-",
    ] {
        assert!(!sc.contains(never), "{never:?} in {sc}");
    }
    let at = |t: &str| sc.find(&format!("{HAIKU_TAG}{t}")).unwrap();
    assert!(at("noclaude") < at("run"));
    assert!(haiku_script("claude-opus-5-5", 110).is_err());
    assert!(haiku_script("haiku'; id; '", 110).is_err());
}

const NASTY: &str = "it's \"quoted\" $(touch pwned) `id` $HOME \\ back\nslash ; && | > x '\\'' end";

/// The whole script, run by a real bash through the same two layers ssh
/// applies (`bash -lc '<script>'` re-read by a shell) with the prompt on
/// stdin, against a fake `claude` that records its stdin and its argv.
#[cfg(unix)]
#[test]
fn the_script_hands_claude_the_prompt_verbatim_on_stdin_and_never_in_argv() {
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let got = dir.path().join("got");
    let argv = dir.path().join("argv");
    let fake = dir.path().join("claude");
    std::fs::write(
        &fake,
        format!(
            "#!/bin/sh\ncat > '{}'\nprintf '%s\\n' \"$@\" > '{}'\nprintf '%s\\n' '{}'\n",
            got.display(),
            argv.display(),
            envelope("{\"choice\":\"i1\",\"confidence\":0.7}")
        ),
    )
    .unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    let sc = haiku_script("haiku", 30).unwrap();
    let full = format!(
        "export PATH={}:\"$PATH\"; {sc}",
        crate::shell::quote(&dir.path().display().to_string())
    );
    let outer = ["bash", "-c", &crate::shell::quote(&full)].join(" ");
    let mut child = std::process::Command::new("bash")
        .arg("-c")
        .arg(outer)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(NASTY.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "{out:?}");
    assert_eq!(std::fs::read_to_string(&got).unwrap(), NASTY);
    // claude's argv is the fixed flags alone: no word of the prompt.
    let args: Vec<String> = std::fs::read_to_string(&argv)
        .unwrap()
        .lines()
        .map(String::from)
        .collect();
    assert_eq!(
        args,
        [
            "-p",
            "--model",
            "haiku",
            "--output-format",
            "json",
            "--settings",
            r#"{"disableAllHooks":true}"#,
            "--tools",
            "",
            "--strict-mcp-config",
            "--no-session-persistence"
        ]
    );
    assert!(!dir.path().join("pwned").exists());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let ScriptAnswer::Ran(text) = parse_script_output(&stdout) else {
        panic!("{stdout}");
    };
    let env = parse_envelope(&text).unwrap();
    assert_eq!(
        parse_answer(&env.result, &["i1".into(), "none".into()]),
        Parsed::Valid {
            choice: "i1".into(),
            confidence: Some(0.7)
        }
    );
}

// --- the reply --------------------------------------------------------------------

fn opts() -> Vec<String> {
    vec!["todo".into(), "done".into(), "unsure".into()]
}

#[test]
fn a_good_answer_parses_with_words_or_a_fence_around_it() {
    for text in [
        r#"{"choice": "done", "confidence": 0.9}"#,
        "Sure.\n```json\n{\"choice\":\"done\",\"confidence\":0.9}\n```",
        r#"format {"choice": "<option id>"} then {"choice": " done ", "confidence": "0.9"}"#,
        r#"{"choice": "DONE", "confidence": 0.9}"#,
    ] {
        assert_eq!(
            parse_answer(text, &opts()),
            Parsed::Valid {
                choice: "done".into(),
                confidence: Some(0.9)
            },
            "{text}"
        );
    }
}

#[test]
fn a_confidence_is_clamped_and_a_missing_one_is_none() {
    let c = |text: &str| match parse_answer(text, &opts()) {
        Parsed::Valid { confidence, .. } => confidence,
        Parsed::Invalid => panic!("{text}"),
    };
    assert_eq!(c(r#"{"choice":"todo","confidence":1.7}"#), Some(1.0));
    assert_eq!(c(r#"{"choice":"todo","confidence":-3}"#), Some(0.0));
    assert_eq!(c(r#"{"choice":"todo"}"#), None);
    assert_eq!(c(r#"{"choice":"todo","confidence":"high"}"#), None);
    assert_eq!(c(r#"{"choice":"todo","confidence":null}"#), None);
}

#[test]
fn a_bad_or_missing_answer_or_an_unoffered_option_is_invalid() {
    for text in [
        "",
        "done",
        "I think it is done.",
        r#"{"choice": "shipped", "confidence": 0.9}"#,
        r#"{"choice": 3}"#,
        r#"{"answer": "done"}"#,
        r#"{"choice": "done", "confidence": 0.9"#,
    ] {
        assert_eq!(parse_answer(text, &opts()), Parsed::Invalid, "{text:?}");
    }
    // An ambiguous case-insensitive match is not guessed.
    let twins = vec!["a".to_string(), "A".to_string()];
    assert_eq!(
        parse_answer(r#"{"choice":"a"}"#, &twins),
        Parsed::Valid {
            choice: "a".into(),
            confidence: None
        }
    );
    let twins = vec!["Ab".to_string(), "aB".to_string()];
    assert_eq!(parse_answer(r#"{"choice":"ab"}"#, &twins), Parsed::Invalid);
}

#[test]
fn the_envelope_gives_the_text_tokens_and_cost() {
    let e = parse_envelope(&envelope("hello")).unwrap();
    assert_eq!(e.result, "hello");
    assert!(!e.is_error);
    assert_eq!(e.input_tokens, Some(1112));
    assert_eq!(e.output_tokens, Some(9));
    assert_eq!(e.cost_microusd, Some(1230));
    // Without usage: unknown, not zero.
    let bare = parse_envelope(r#"{"type":"result","result":"x"}"#).unwrap();
    assert_eq!(
        (bare.input_tokens, bare.output_tokens, bare.cost_microusd),
        (None, None, None)
    );
    // An error result.
    let err = parse_envelope(
        r#"{"type":"result","subtype":"error_during_execution","is_error":true,"result":""}"#,
    )
    .unwrap();
    assert!(err.is_error);
    // An array of messages (verbose json): the last result.
    let arr = format!(r#"[{{"type":"system"}},{}]"#, envelope("from array"));
    assert_eq!(parse_envelope(&arr).unwrap().result, "from array");
    // After other lines.
    let after = format!("warning: x\n{}", envelope("after"));
    assert_eq!(parse_envelope(&after).unwrap().result, "after");
    assert_eq!(parse_envelope("not json"), None);
}

#[test]
fn the_first_tag_line_decides() {
    assert_eq!(
        parse_script_output(&format!("{HAIKU_TAG}noclaude\n")),
        ScriptAnswer::NoClaude
    );
    assert_eq!(
        parse_script_output(&format!("{HAIKU_TAG}run\nx\n{HAIKU_TAG}noclaude\n")),
        ScriptAnswer::Ran(format!("x\n{HAIKU_TAG}noclaude"))
    );
    assert_eq!(parse_script_output("bash: oops"), ScriptAnswer::Nothing);
}

// --- one call ---------------------------------------------------------------------

fn sm_request() -> JevRequest {
    status_map::question_for("čaká na klienta", &board())
}

#[tokio::test]
async fn one_call_runs_the_script_on_the_named_host_and_reads_its_answer() {
    let fake = FakeSsh::new();
    fake.on_host(
        "hq",
        Match::script_contains(HAIKU_TAG),
        ran(&envelope(r#"{"choice":"in_progress","confidence":0.8}"#)),
    );
    let h = Haiku {
        exec: &fake,
        cfg: cfg("hq"),
        host_org: None,
    };
    let r = h.ask(&sm_request()).await;
    assert_eq!(r.error, None);
    assert!(r.ran && !r.invalid);
    assert_eq!(r.choice.as_deref(), Some("in_progress"));
    assert_eq!(r.confidence, Some(0.8));
    assert_eq!((r.input_tokens, r.cost_microusd), (Some(1112), Some(1230)));
    assert!(r.latency_ms.is_some());
    let calls = fake.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].host, "hq");
    // The prompt went on stdin; the command line holds none of it.
    let (prompt, _) = prompt_for(&sm_request()).unwrap();
    assert_eq!(calls[0].stdin_str().as_deref(), Some(prompt.as_str()));
    let cmd = calls[0].command();
    for piece in [
        PREAMBLE,
        "čaká na klienta",
        status_map::INSTRUCTIONS,
        "<state>",
    ] {
        assert!(!cmd.contains(piece), "{piece:?} on the command line: {cmd}");
    }
    let sc = calls[0].script().unwrap();
    assert_eq!(sc, haiku_script("haiku", 110).unwrap());
}

#[test]
fn a_case_goes_only_to_a_host_of_its_own_org() {
    let fake = FakeSsh::new();
    let h = |host_org| Haiku {
        exec: &fake,
        cfg: cfg("h1"),
        host_org,
    };
    assert!(h(Some(3)).may_ask(Some(3)));
    assert!(!h(Some(3)).may_ask(Some(4)));
    assert!(!h(Some(3)).may_ask(None));
    assert!(!h(None).may_ask(Some(3)));
    assert!(h(None).may_ask(None));
    assert!(h(Some(3)).consent_note().contains("org #3"));
    assert!(h(None).consent_note().contains("no org"));
    assert!(h(None).consent_note().contains("other_org"));
}

#[test]
fn the_hosts_org_is_read_from_the_database_and_an_unknown_host_is_refused() {
    let s = crate::store::Store::open_in_memory().unwrap();
    let acme = s.add_org("Acme", None, false).unwrap().id;
    s.upsert_host("plain").unwrap();
    s.upsert_host("acme-box").unwrap();
    s.set_host_org("acme-box", Some(acme)).unwrap();
    assert_eq!(resolve_host_org(&s, "plain"), Ok(None));
    assert_eq!(resolve_host_org(&s, "acme-box"), Ok(Some(acme)));
    let e = resolve_host_org(&s, "stranger").unwrap_err();
    assert!(e.contains("stranger") && e.contains("org"), "{e}");
}

#[tokio::test]
async fn an_answer_outside_the_options_is_invalid_and_usage_may_be_unknown() {
    let fake = FakeSsh::new();
    fake.on(
        Match::script_contains(HAIKU_TAG),
        ran(r#"{"type":"result","result":"{\"choice\":\"shipped\",\"confidence\":0.9}"}"#),
    );
    let r = Haiku {
        exec: &fake,
        cfg: cfg("h1"),
        host_org: None,
    }
    .ask(&sm_request())
    .await;
    assert!(r.ran && r.invalid && r.error.is_none());
    assert_eq!(r.choice, None);
    assert_eq!((r.input_tokens, r.cost_microusd), (None, None));
}

#[tokio::test]
async fn failures_are_named() {
    let ask = |reply: Reply| async move {
        let fake = FakeSsh::new();
        fake.on(Match::script_contains(HAIKU_TAG), reply);
        fake.set_wall_clock(Duration::from_millis(50));
        Haiku {
            exec: &fake,
            cfg: cfg("h1"),
            host_org: None,
        }
        .ask(&sm_request())
        .await
    };
    let r = ask(Reply::hang()).await;
    assert_eq!((r.ran, r.error), (true, Some(reason::TIMEOUT)));
    let r = ask(Reply::Exit {
        code: 124,
        stdout: format!("{HAIKU_TAG}run\n").into_bytes(),
        stderr: Vec::new(),
    })
    .await;
    assert_eq!(r.error, Some(reason::TIMEOUT));
    let r = ask(Reply::ok(&format!("{HAIKU_TAG}noclaude\n"))).await;
    assert_eq!(r.error, Some(reason::NO_CLAUDE));
    let r = ask(Reply::Unreachable).await;
    assert_eq!(r.error, Some(reason::SSH_ERROR));
    let r = ask(Reply::fail(1, "boom")).await;
    assert_eq!(r.error, Some(reason::CALL_FAILED));
    let r = ask(ran(
        r#"{"type":"result","subtype":"error_during_execution","is_error":true,"result":""}"#,
    ))
    .await;
    assert_eq!(r.error, Some(reason::CALL_FAILED));
    // No envelope but a readable answer: taken, usage unknown.
    let r = ask(ran(r#"{"choice":"done","confidence":0.6}"#)).await;
    assert_eq!(r.choice.as_deref(), Some("done"));
    assert_eq!(r.input_tokens, None);
}

#[tokio::test]
async fn a_prompt_over_the_cap_is_not_sent() {
    let fake = FakeSsh::new();
    let mut criteria = BTreeMap::new();
    criteria.insert("a".to_string(), Some(json!("x")));
    criteria.insert("b".to_string(), Some(json!("y")));
    let req = JevRequest {
        state: json!("x".repeat(MAX_PROMPT_BYTES + 1)),
        question: Question::Choice {
            instructions: json!("which?"),
            criteria,
        },
    };
    let r = Haiku {
        exec: &fake,
        cfg: cfg("h1"),
        host_org: None,
    }
    .ask(&req)
    .await;
    assert_eq!((r.ran, r.error), (false, Some(reason::TOO_LONG)));
    assert!(fake.calls().is_empty());
}

#[tokio::test]
async fn calls_to_one_host_run_one_at_a_time() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    /// Counts calls in flight and remembers the most at once.
    struct Slow {
        now: AtomicUsize,
        most: AtomicUsize,
    }
    #[async_trait::async_trait]
    impl SshExec for Slow {
        async fn run(
            &self,
            _: &str,
            _: &[&str],
            _: Duration,
        ) -> Result<std::process::Output, crate::ipc_error::IpcError> {
            unreachable!()
        }
        async fn run_bounded(
            &self,
            _: &str,
            _: &[&str],
            _: Duration,
            _: Duration,
        ) -> Result<std::process::Output, crate::ipc_error::IpcError> {
            unreachable!("the prompt goes on stdin")
        }
        async fn run_with_stdin(
            &self,
            _host: &str,
            _args: &[&str],
            _stdin: Vec<u8>,
            _c: Duration,
            _w: Duration,
            _max: usize,
        ) -> Result<std::process::Output, crate::ipc_error::IpcError> {
            let n = self.now.fetch_add(1, Ordering::SeqCst) + 1;
            self.most.fetch_max(n, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(20)).await;
            self.now.fetch_sub(1, Ordering::SeqCst);
            use std::os::unix::process::ExitStatusExt;
            Ok(std::process::Output {
                status: std::process::ExitStatus::from_raw(0),
                stdout: format!("{HAIKU_TAG}run\n{}\n", envelope(r#"{"choice":"todo"}"#))
                    .into_bytes(),
                stderr: Vec::new(),
            })
        }
        async fn run_cancellable(
            &self,
            _: &str,
            _: &[&str],
            _: Duration,
            _: tokio_util::sync::CancellationToken,
        ) -> Result<std::process::Output, crate::ipc_error::IpcError> {
            unreachable!()
        }
        async fn run_bounded_cancellable(
            &self,
            _: &str,
            _: &[&str],
            _: Duration,
            _: Duration,
            _: tokio_util::sync::CancellationToken,
        ) -> Result<std::process::Output, crate::ipc_error::IpcError> {
            unreachable!()
        }
        async fn upload_file(
            &self,
            _: &str,
            _: &std::path::Path,
            _: &str,
            _: Duration,
        ) -> Result<(), crate::ipc_error::IpcError> {
            unreachable!()
        }
        async fn remote_home(&self, _: &str) -> Result<String, crate::ipc_error::IpcError> {
            unreachable!()
        }
    }
    let slow = Slow {
        now: AtomicUsize::new(0),
        most: AtomicUsize::new(0),
    };
    let h = Haiku {
        exec: &slow,
        cfg: cfg("one-at-a-time-host"),
        host_org: None,
    };
    let req = sm_request();
    let (a, b, c) = tokio::join!(h.ask(&req), h.ask(&req), h.ask(&req));
    assert!([a, b, c]
        .iter()
        .all(|r| r.choice.as_deref() == Some("todo") && r.confidence.is_none()));
    assert_eq!(slow.most.load(Ordering::SeqCst), 1);
}
