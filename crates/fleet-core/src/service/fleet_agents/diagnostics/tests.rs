use super::*;
use crate::service::client_logs::{ClientLogs, LogPost};
use fleet_proto::report::Report;

const NOW: i64 = 1_768_472_200; // 2026-01-15T10:16:40Z, before any file the test writes

fn stamp(t: i64) -> String {
    let days = t.div_euclid(86_400);
    let secs = t.rem_euclid(86_400);
    // Inverse of `days_from_civil`, for the fixtures only.
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
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.123456Z",
        secs / 3600,
        secs % 3600 / 60,
        secs % 60
    )
}

fn data(result: &Value) -> Value {
    assert_eq!(result["isError"], false, "{result}");
    let text = result["content"][0]["text"].as_str().unwrap();
    serde_json::from_str::<Value>(text).unwrap()["data"].clone()
}

#[test]
fn a_log_line_is_dated_by_its_stamp() {
    assert_eq!(line_time(&stamp(NOW)), Some(NOW));
    assert_eq!(line_time("2026-01-15T10:16:40Z  INFO x: y"), Some(NOW));
    assert_eq!(line_time("    at continuation of a message"), None);
    assert_eq!(line_time("2026-13-15T10:16:40Z"), None);
    assert_eq!(
        line_level(&format!("{}  WARN fleet_core::ssh: slow", stamp(NOW))),
        Some(1)
    );
}

#[test]
fn a_window_is_bounded() {
    assert_eq!(window(&json!({}), NOW), Ok((NOW - 3600, NOW + 1)));
    assert_eq!(
        window(&json!({ "minutes": 10 }), NOW),
        Ok((NOW - 600, NOW + 1))
    );
    assert_eq!(
        window(&json!({ "since": NOW - 100, "until": NOW - 50 }), NOW),
        Ok((NOW - 100, NOW - 50))
    );
    assert!(window(&json!({ "since": NOW, "until": NOW - 1 }), NOW).is_err());
    assert!(window(&json!({ "minutes": 0 }), NOW).is_err());
    assert!(window(&json!({ "minutes": 60 * 24 * 8 }), NOW).is_err());
    assert!(window(&json!({ "minutes": 60 * 24 * 7 }), NOW).is_ok());
}

#[test]
fn hub_log_keeps_the_newest_matching_lines_oldest_first() {
    let tmp = tempfile::tempdir().unwrap();
    let older = format!(
        "{}  INFO fleet_core::tick: before the window\n{}  ERROR fleet_core::ssh: \u{1b}[31mbroken pipe\u{1b}[0m to htz\n  caused by: reset\n",
        stamp(NOW - 7200),
        stamp(NOW - 1800),
    );
    let newer = format!(
        "{}  WARN fleet_core::health: htz slow\n{}  INFO fleet_core::tick: tick\n{}  ERROR fleet_core::ssh: timeout to htz\n",
        stamp(NOW - 600),
        stamp(NOW - 300),
        stamp(NOW - 60),
    );
    std::fs::write(tmp.path().join("claude-fleet.2026-10-17-09.log"), older).unwrap();
    std::fs::write(tmp.path().join("claude-fleet.2026-10-17-10.log"), newer).unwrap();
    std::fs::write(tmp.path().join("other.txt"), "not a log").unwrap();

    let all = hub_log(tmp.path(), &json!({}), NOW).unwrap();
    let lines: Vec<&str> = all["lines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l.as_str().unwrap())
        .collect();
    assert_eq!(lines.len(), 5, "{lines:?}");
    assert!(lines[0].contains("broken pipe") && !lines[0].contains('\u{1b}'));
    assert_eq!(
        lines[1], "  caused by: reset",
        "a continuation keeps its line's time"
    );
    assert!(lines[4].contains("timeout to htz"));
    assert_eq!(all["more"], false);

    let errors = hub_log(
        tmp.path(),
        &json!({ "level": "warn", "contains": "HTZ" }),
        NOW,
    )
    .unwrap();
    assert_eq!(errors["lines"].as_array().unwrap().len(), 3);

    let two = hub_log(tmp.path(), &json!({ "limit": 2 }), NOW).unwrap();
    let two: Vec<&str> = two["lines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l.as_str().unwrap())
        .collect();
    assert!(
        two[0].contains("tick: tick") && two[1].contains("timeout"),
        "{two:?}"
    );

    let target = hub_log(tmp.path(), &json!({ "target": "fleet_core::health" }), NOW).unwrap();
    assert_eq!(target["lines"].as_array().unwrap().len(), 1);
    assert!(hub_log(tmp.path(), &json!({ "level": "loud" }), NOW).is_err());
}

fn store() -> Mutex<Store> {
    let s = Store::open_in_memory().unwrap();
    let mut a = Report::error("fleet_core::ssh", "connect to htz: timed out");
    a.at = NOW - 120;
    let mut b = a.clone();
    b.at = NOW - 60;
    let mut c = Report::error("frontend", "render failed");
    c.at = NOW - 30;
    s.insert_reports("host:htz", &[a, b], NOW - 50).unwrap();
    s.insert_reports("client:7", &[c], NOW - 20).unwrap();
    Mutex::new(s)
}

#[test]
fn errors_are_grouped_newest_first() {
    let st = store();
    let s = st.lock().unwrap();
    let got = errors(&s, &json!({}), NOW).unwrap();
    assert_eq!(got["reports"], 3);
    let items = got["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    let ssh = items
        .iter()
        .find(|i| i["newest"]["origin"] == "host:htz")
        .unwrap();
    assert_eq!(ssh["count"], 2);
    assert_eq!(
        (ssh["first_at"].as_i64(), ssh["last_at"].as_i64()),
        (Some(NOW - 120), Some(NOW - 60))
    );
    let one = errors(&s, &json!({ "origin": "client:7", "group": false }), NOW).unwrap();
    assert_eq!(one["items"][0]["message"], "render failed");
    let none = errors(&s, &json!({ "contains": "nothing like it" }), NOW).unwrap();
    assert_eq!(none["reports"], 0);
}

#[test]
fn the_timeline_puts_sessions_and_errors_on_one_clock() {
    let st = store();
    let now = crate::store::now_unix();
    {
        let s = st.lock().unwrap();
        s.upsert_host("local").unwrap();
        let id = s
            .upsert_session("fleet-x", "local", None, None, now, now, "running", None)
            .unwrap();
        s.insert_session_event(id, "stuck", Some("press_enter"))
            .unwrap();
        s.insert_session_event(id, "prompt_sent", None).unwrap();
        s.insert_reports("hub", &[Report::error("fleet_core::tick", "boom")], now)
            .unwrap();
    }
    let s = st.lock().unwrap();
    let got = timeline(&s, &json!({ "minutes": 5 }), now).unwrap();
    let items = got["items"].as_array().unwrap();
    let kinds: Vec<(&str, &str)> = items
        .iter()
        .map(|i| {
            (
                i["source"].as_str().unwrap(),
                i["kind"].as_str().or(i["message"].as_str()).unwrap(),
            )
        })
        .collect();
    assert!(kinds.contains(&("session", "stuck")), "{kinds:?}");
    assert!(kinds.contains(&("error", "boom")), "{kinds:?}");
    assert!(
        !kinds.iter().any(|(_, k)| *k == "prompt_sent"),
        "not notable"
    );
    let only = timeline(
        &s,
        &json!({ "minutes": 5, "sources": ["sessions"], "host": "local" }),
        now,
    )
    .unwrap();
    assert_eq!(only["items"].as_array().unwrap().len(), 1);
    let elsewhere = timeline(&s, &json!({ "minutes": 5, "host": "htz" }), now).unwrap();
    assert_eq!(elsewhere["items"].as_array().unwrap().len(), 0);
    assert!(timeline(&s, &json!({ "sources": ["logs"] }), now).is_err());
}

#[test]
fn an_answer_never_carries_a_secret_the_store_knows() {
    let st = store();
    let now = crate::store::now_unix();
    {
        let s = st.lock().unwrap();
        s.upsert_host_token("htz", "host-secret-token-123").unwrap();
        s.insert_reports(
            "hub",
            &[Report::error("x", "auth failed for host-secret-token-123")],
            now,
        )
        .unwrap();
    }
    let clients = Mutex::new(ClientLogs::default());
    let src = Sources {
        log_dir: None,
        version: "0.6.3",
        clients: &clients,
    };
    let got = call(
        &st,
        &src,
        &json!({ "name": "errors", "arguments": { "contains": "auth failed" } }),
        now,
    );
    let text = got["content"][0]["text"].as_str().unwrap();
    assert!(!text.contains("host-secret-token-123"), "{text}");
    assert!(text.contains(logging::REDACTED), "{text}");
    let no_log = call(&st, &src, &json!({ "name": "hub_log" }), now);
    assert_eq!(no_log["isError"], true);
    let fleet = data(&call(&st, &src, &json!({ "name": "fleet" }), now));
    assert_eq!(fleet["hub"]["version"], "0.6.3");
    assert!(fleet["hosts"].is_array());
    assert_eq!(
        call(&st, &src, &json!({ "name": "rm" }), now)["isError"],
        true
    );
}

#[test]
fn the_server_speaks_mcp_with_a_run_token() {
    let t = issue(
        DiagGrant {
            rule_id: 4,
            expires_at: 100,
        },
        0,
    );
    assert_eq!(grant_for(&t, 50).map(|g| g.rule_id), Some(4));
    assert_eq!(grant_for(&t, 100), None, "expired");
    revoke(&t);
    assert_eq!(grant_for(&t, 50), None);
    assert_eq!(
        entry("http://hub:4180/", "abc")["url"],
        "http://hub:4180/agent-diagnostics"
    );
    let st = store();
    let clients = Mutex::new(ClientLogs::default());
    let src = Sources {
        log_dir: None,
        version: "1",
        clients: &clients,
    };
    let init = answer(
        &st,
        &src,
        &json!({ "id": 1, "method": "initialize", "params": { "protocolVersion": "2025-03-26" } }),
        NOW,
    )
    .unwrap();
    assert_eq!(init["result"]["serverInfo"]["name"], SERVER);
    let list = answer(&st, &src, &json!({ "id": 2, "method": "tools/list" }), NOW).unwrap();
    let names: Vec<&str> = list["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, TOOLS);
    assert!(answer(
        &st,
        &src,
        &json!({ "method": "notifications/initialized" }),
        NOW
    )
    .is_none());
}

#[test]
fn a_host_log_script_is_fixed_and_quoted() {
    let script = |args: Value| host_log_script(&args, 100, 200);
    let j = script(
        json!({ "source": "journal", "unit": "ssh.service", "priority": "err", "limit": 50 }),
    )
    .unwrap();
    assert!(
        j.contains("journalctl --no-pager -q -o short-iso-precise --since @100 --until @200 -n 50"),
        "{j}"
    );
    assert!(
        j.contains("-u ssh.service") || j.contains("-u 'ssh.service'"),
        "{j}"
    );
    assert!(
        j.contains("-p err") && j.trim_end().ends_with("exit 0"),
        "{j}"
    );
    let user = script(json!({ "source": "journal", "user": true })).unwrap();
    assert!(user.contains(" --user"), "{user}");
    let oom = script(json!({ "source": "oom" })).unwrap();
    assert!(
        oom.contains("journalctl --no-pager -q -k") && oom.contains("dmesg -T"),
        "{oom}"
    );
    let agent = script(json!({ "source": "agent" })).unwrap();
    assert!(
        agent.contains("--user -u fleet-agent") && agent.contains("fleet-agent/agent.log"),
        "{agent}"
    );
    let docker = script(json!({ "source": "docker", "container": "fleet-hub" })).unwrap();
    assert!(
        docker.contains("docker logs --timestamps --since 100 --until 200 --tail 200"),
        "{docker}"
    );
    let updater = script(json!({ "source": "updater" })).unwrap();
    assert!(
        updater.contains("fleet-updater")
            && updater.contains("systemctl cat fleet-updater.service"),
        "{updater}"
    );
    for bad in [
        json!({ "source": "journal", "unit": "x; rm -rf ~" }),
        json!({ "source": "journal", "unit": "$(id)" }),
        json!({ "source": "journal", "priority": "emerg; id" }),
        json!({ "source": "docker" }),
        json!({ "source": "docker", "container": "a b" }),
        json!({ "source": "shell" }),
        json!({}),
    ] {
        assert!(script(bad.clone()).is_err(), "{bad}");
    }
}

#[test]
fn host_output_keeps_its_newest_lines() {
    let out: String = (0..5000)
        .map(|i| format!("line {i} {}\n", "x".repeat(40)))
        .collect();
    let (lines, more) = tail_text(&out);
    assert!(more);
    assert!(lines.last().unwrap().starts_with("line 4999"));
    assert!(lines.iter().map(|l| l.len() + 1).sum::<usize>() <= ANSWER_MAX_BYTES);
    let (few, more) = tail_text("a\nb\n");
    assert_eq!((few, more), (vec!["a".to_string(), "b".to_string()], false));
}

#[tokio::test]
async fn host_tools_need_a_known_host_and_a_way_to_reach_it() {
    let st = std::sync::Arc::new(store());
    let call = |name: &str, args: Value| json!({ "name": name, "arguments": args });
    assert!(call_host(&st, None, &call("errors", json!({})), NOW)
        .await
        .is_none());
    let no_ssh = call_host(
        &st,
        None,
        &call("host_log", json!({ "host": "local", "source": "oom" })),
        NOW,
    )
    .await
    .unwrap();
    assert_eq!(no_ssh["isError"], true);
    let ssh = std::sync::Arc::new(crate::ssh::SshClient::new());
    let unknown = call_host(
        &st,
        Some(&ssh),
        &call("host_log", json!({ "host": "nowhere", "source": "oom" })),
        NOW,
    )
    .await
    .unwrap();
    assert_eq!(unknown["isError"], true);
    assert!(unknown["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("no host"));
    let no_session = call_host(
        &st,
        Some(&ssh),
        &call("session", json!({ "session": "ghost" })),
        NOW,
    )
    .await
    .unwrap();
    assert!(no_session["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("no session"));
    // A hub-only call asked of `call` for a host tool says where it goes.
    let clients = Mutex::new(ClientLogs::default());
    let src = Sources {
        log_dir: None,
        version: "1",
        clients: &clients,
    };
    assert_eq!(
        super::call(&st, &src, &call("session", json!({})), NOW)["isError"],
        true
    );
}

#[test]
fn client_log_asks_first_and_reads_what_came_in() {
    let logs = Mutex::new(ClientLogs::default());
    let asked = client_log(&logs, &json!({ "minutes": 30 }), NOW).unwrap();
    assert_eq!(asked["asked"], true);
    let id = asked["request"].as_str().unwrap().to_string();
    // The desktop polls, is asked, and sends.
    let want = logs
        .lock()
        .unwrap()
        .post("master", LogPost::default(), NOW + 10)
        .unwrap();
    assert_eq!(want["want"], json!({ "id": id, "minutes": 30 }));
    let lines = vec![
        format!("{}  INFO fleet::app: started", stamp(NOW - 3600)),
        format!("{}  ERROR fleet::pty: write failed", stamp(NOW - 60)),
        format!("{}  WARN fleet::hub: reconnecting", stamp(NOW - 30)),
    ];
    let sent = LogPost {
        id: Some(id),
        lines: Some(lines),
        app: Some("desktop 0.6.3".into()),
    };
    assert!(logs
        .lock()
        .unwrap()
        .post("master", sent, NOW + 20)
        .is_none());
    let got = client_log(&logs, &json!({ "minutes": 30 }), NOW + 30).unwrap();
    assert_eq!(got["asked"], false);
    let c = &got["clients"][0];
    assert_eq!(
        (c["client"].as_str(), c["app"].as_str()),
        (Some("master"), Some("desktop 0.6.3"))
    );
    let kept: Vec<&str> = c["lines"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert_eq!(
        kept.len(),
        2,
        "the line before the window is left out: {kept:?}"
    );
    assert!(kept[0].contains("write failed"));
    let one = client_log(
        &logs,
        &json!({ "minutes": 30, "contains": "RECONNECT" }),
        NOW + 30,
    )
    .unwrap();
    assert_eq!(one["clients"][0]["lines"].as_array().unwrap().len(), 1);
    // A wider window than was sent asks again.
    assert_eq!(
        client_log(&logs, &json!({ "minutes": 120 }), NOW + 30).unwrap()["asked"],
        true
    );
    assert!(client_log(&logs, &json!({ "minutes": 0 }), NOW).is_err());
}
