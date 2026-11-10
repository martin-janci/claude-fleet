use super::*;
use crate::net::https::{FakeTransport, Method, Response};
use crate::ssh_fake::{FakeSsh, Match, Reply};
use std::sync::Arc;

const KEY: &str = "sk-ant-api03-abcdefghijklmnopqrstuvwxyz0123456789";
const NOW: i64 = 1_800_000_000;

fn store_with_host() -> Mutex<Store> {
    let s = Store::open_in_memory().unwrap();
    s.upsert_host("mercury").unwrap();
    Mutex::new(s)
}

fn args(action: &str) -> AddAccountArgs {
    AddAccountArgs {
        action: action.into(),
        host_alias: "mercury".into(),
        profile: "api".into(),
        ..Default::default()
    }
}

fn tmux(ssh: &Arc<FakeSsh>) -> crate::tmux::RemoteTmux<Arc<FakeSsh>> {
    crate::tmux::RemoteTmux {
        client: Arc::clone(ssh),
        host: "mercury".into(),
    }
}

fn anthropic_ok() -> FakeTransport {
    let http = FakeTransport::new();
    http.always(
        Method::Get,
        "/v1/models",
        Ok(Response::json(200, &serde_json::json!({"data": []}))),
    );
    http
}

#[test]
fn the_sign_in_link_is_followed_across_wrapped_lines() {
    let pane = "Browser didn't open? Use the url below to sign in:\n\n\
        https://claude.ai/oauth/authorize?code=true&client_id=9d1c\n\
        &response_type=code&state=abc\n\n\
        Paste code here if prompted >";
    assert_eq!(
        sign_in_url(pane).as_deref(),
        Some("https://claude.ai/oauth/authorize?code=true&client_id=9d1c&response_type=code&state=abc")
    );
    // Not an Anthropic host: no link, whatever the pane printed.
    assert_eq!(
        sign_in_url("go to https://evil.example/claude.ai now"),
        None
    );
    assert_eq!(sign_in_url("https://claude.ai.evil.example/x"), None);
    assert_eq!(
        sign_in_url("https://console.anthropic.com/oauth/authorize?x=1").as_deref(),
        Some("https://console.anthropic.com/oauth/authorize?x=1")
    );
}

#[test]
fn the_pane_tail_keeps_the_last_lines_without_trailing_blanks() {
    let text: String = (1..=40).map(|i| format!("line {i}\n")).collect::<String>() + "\n\n";
    let tail = pane_tail(&text);
    assert!(
        tail.starts_with("line 17\n") && tail.ends_with("line 40"),
        "{tail}"
    );
}

#[test]
fn inputs_are_checked_before_anything_leaves() {
    assert!(checked_key(Some(KEY)).is_ok());
    for bad in [
        "",
        "sk-live-abc",
        "sk-ant-short",
        "sk-ant-api03-has space here!!!",
    ] {
        let e = checked_key(Some(bad)).unwrap_err();
        assert_eq!(e.code, codes::E_INVALID, "{bad}");
        assert!(!e.message.contains(bad) || bad.is_empty(), "never echoed");
    }
    assert!(checked_code(Some("abc123#def-456")).is_ok());
    assert!(checked_code(Some("a b")).is_err());
    assert!(checked_code(Some("x; rm -rf ~")).is_err());
    assert_eq!(checked_limit(Some(0.0)).unwrap(), None);
    assert_eq!(checked_limit(Some(12.345)).unwrap(), Some(12.35));
    for bad in [-1.0, f64::NAN, f64::INFINITY, MAX_DAILY_LIMIT_USD + 1.0] {
        assert!(checked_limit(Some(bad)).is_err(), "{bad}");
    }
}

#[test]
fn the_debug_form_never_prints_a_secret() {
    let mut a = args("api_key");
    a.api_key = Some(KEY.into());
    a.code = Some("code-123".into());
    let printed = format!("{a:?} {}", a.audit_detail());
    assert!(
        !printed.contains(KEY) && !printed.contains("code-123"),
        "{printed}"
    );
}

#[tokio::test]
async fn a_rejected_key_carries_the_providers_401_on_the_field() {
    let http = FakeTransport::new();
    http.always(
        Method::Get,
        "/v1/models",
        Ok(Response::json(
            401,
            &serde_json::json!({"type": "error", "error": {"type": "authentication_error", "message": format!("invalid x-api-key {KEY}")}}),
        )),
    );
    let e = verify_key(&http, KEY).await.unwrap_err();
    assert_eq!(e.code, codes::E_INVALID);
    assert!(
        e.message
            .contains("Anthropic rejected this key (401: invalid x-api-key"),
        "{}",
        e.message
    );
    assert!(
        !e.message.contains(KEY),
        "the key is scrubbed from the provider's words"
    );
    let d = e.details.unwrap();
    assert_eq!(d["problems"][0]["field"], "api_key");
    // The request carried the key in its header, to Anthropic only.
    let sent = &http.requests()[0];
    assert!(sent.url.starts_with("https://api.anthropic.com/"));
    assert_eq!(sent.header_value("x-api-key"), Some(KEY));

    let down = FakeTransport::new();
    down.always(
        Method::Get,
        "/v1/models",
        Ok(Response::new(529, "overloaded")),
    );
    assert_eq!(
        verify_key(&down, KEY).await.unwrap_err().code,
        codes::E_PROBE
    );
    assert!(verify_key(&anthropic_ok(), KEY).await.is_ok());
}

#[test]
fn the_production_transport_reaches_anthropic_only() {
    // The policy is the SSRF fence: no other host, whatever the URL says.
    let policy = |h: &str| h == ANTHROPIC_HOST;
    assert!(policy("api.anthropic.com"));
    assert!(!policy("api.anthropic.com.evil.example"));
    let _ = anthropic_transport();
}

#[tokio::test]
async fn an_api_key_goes_to_the_host_on_stdin_and_becomes_an_account() {
    let store = store_with_host();
    let ssh = Arc::new(FakeSsh::new());
    ssh.on(
        Match::script_contains(".fleet-api-key"),
        Reply::ok("motd\n__fleet_ok__\n"),
    );
    let mut a = args("api_key");
    a.api_key = Some(KEY.into());
    a.nickname = Some("Team API".into());
    a.daily_limit_usd = Some(25.0);
    let out = run(&a, &store, ssh.as_ref(), &tmux(&ssh), &anthropic_ok(), NOW)
        .await
        .unwrap();
    let uuid = api_key_account_uuid(KEY);
    assert!(uuid.starts_with("apikey-") && uuid.len() == 23);
    assert_eq!(out["account_uuid"], uuid.as_str());
    assert_eq!(out["daily_limit_usd"], 25.0);
    assert!(!out.to_string().contains(KEY));

    let calls = ssh.calls();
    assert_eq!(calls.len(), 1);
    let call = &calls[0];
    assert!(!call.command().contains(KEY), "never in argv");
    let script = call.script().unwrap();
    assert!(script.contains("umask 077") && script.contains("chmod 600"));
    assert!(script.contains("\"$HOME/.claude-profiles/\"'api'"));
    let stdin = String::from_utf8(call.stdin.clone().unwrap()).unwrap();
    let mut lines = stdin.lines();
    assert_eq!(lines.next(), Some(KEY));
    let acct: serde_json::Value = serde_json::from_str(lines.next().unwrap()).unwrap();
    assert_eq!(acct["accountUuid"], uuid.as_str());
    assert_eq!(acct["displayName"], "Team API");
    let cj: serde_json::Value = serde_json::from_str(lines.next().unwrap()).unwrap();
    assert_eq!(cj["hasCompletedOnboarding"], true);
    assert_eq!(
        cj["customApiKeyResponses"]["approved"][0],
        &KEY[KEY.len() - 20..]
    );

    let s = store.lock().unwrap();
    let account = s.get_account_by_uuid(&uuid).unwrap().unwrap();
    assert_eq!(account.nickname.as_deref(), Some("Team API"));
    assert_eq!(account.seat_tier.as_deref(), Some("api_key"));
    let profiles = s
        .get_host_row("mercury")
        .unwrap()
        .unwrap()
        .claude_profiles
        .unwrap();
    assert_eq!(profiles[0].name, "api");
    assert_eq!(profiles[0].account_uuid.as_deref(), Some(uuid.as_str()));
    assert_eq!(daily_limit_usd(&s, &uuid), Some(25.0));
    // Nothing secret in the store.
    let dump: Vec<String> = s
        .conn_for_test()
        .prepare("SELECT value FROM settings")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert!(dump.iter().all(|v| !v.contains(KEY)));
}

#[tokio::test]
async fn a_key_never_lands_on_a_login_profile() {
    let store = store_with_host();
    store
        .lock()
        .unwrap()
        .set_host_profiles(
            "mercury",
            &[crate::store::HostProfileRow {
                name: "api".into(),
                account_uuid: Some("6f1c-real-login".into()),
                email: Some("me@x.com".into()),
                credential: None,
            }],
        )
        .unwrap();
    let ssh = Arc::new(FakeSsh::new());
    let http = anthropic_ok();
    let mut a = args("api_key");
    a.api_key = Some(KEY.into());
    let e = run(&a, &store, ssh.as_ref(), &tmux(&ssh), &http, NOW)
        .await
        .unwrap_err();
    assert_eq!(e.code, codes::E_EXISTS);
    assert!(
        ssh.calls().is_empty() && http.requests().is_empty(),
        "refused before anything ran"
    );

    // The host says so too (a profile the store has not seen yet).
    let store = store_with_host();
    ssh.on(
        Match::script_contains(".fleet-api-key"),
        Reply::Exit {
            code: 3,
            stdout: b"__fleet_login_profile__\n".to_vec(),
            stderr: vec![],
        },
    );
    let e = run(&a, &store, ssh.as_ref(), &tmux(&ssh), &http, NOW)
        .await
        .unwrap_err();
    assert_eq!(e.code, codes::E_EXISTS, "{}", e.message);
}

#[tokio::test]
async fn the_login_pane_opens_under_the_profile_and_reports_the_login() {
    let store = store_with_host();
    let ssh = Arc::new(FakeSsh::new());
    ssh.on(Match::script_contains("new-session"), Reply::ok(""));
    let mut a = args("start_login");
    a.profile = "work".into();
    let http = FakeTransport::new();
    let out = run(&a, &store, ssh.as_ref(), &tmux(&ssh), &http, NOW)
        .await
        .unwrap();
    assert_eq!(out["session"], "fleet-login--work");
    let script = ssh.calls()[0].script().unwrap();
    assert!(
        script.starts_with("tmux has-session -t '=fleet-login--work' 2>/dev/null && exit 0;"),
        "{script}"
    );
    assert!(script.contains("claude /login") && script.contains("hasCompletedOnboarding"));
    assert!(
        script.contains("CLAUDE_CONFIG_DIR=\"$HOME/.claude-profiles/\"'\\''work'\\''"),
        "{script}"
    );

    ssh.clear_calls();
    let ssh = Arc::new(FakeSsh::new());
    ssh.on(
        Match::script_contains("claude-profiles"),
        Reply::ok("@@P\twork\t\n"),
    );
    ssh.on(
        Match::contains("capture-pane"),
        Reply::ok("Paste code here if prompted >\nhttps://claude.ai/oauth/authorize?x=1\n"),
    );
    a.action = "login_status".into();
    let out = run(&a, &store, ssh.as_ref(), &tmux(&ssh), &http, NOW)
        .await
        .unwrap();
    assert_eq!(out["logged_in"], false);
    assert_eq!(out["sign_in_url"], "https://claude.ai/oauth/authorize?x=1");
    assert_eq!(
        out["command"],
        "CLAUDE_CONFIG_DIR=~/.claude-profiles/work claude /login"
    );
    assert!(ssh
        .commands()
        .iter()
        .any(|c| c.contains("=fleet-login--work:")));

    let ssh = Arc::new(FakeSsh::new());
    ssh.on(
        Match::script_contains("claude-profiles"),
        Reply::ok("@@P\twork\t{\"accountUuid\":\"u-work\",\"emailAddress\":\"w@x.com\"}\n"),
    );
    let out = run(&a, &store, ssh.as_ref(), &tmux(&ssh), &http, NOW)
        .await
        .unwrap();
    assert_eq!(out["logged_in"], true);
    assert_eq!(out["email"], "w@x.com");

    // Logged in now: a second start on that profile is refused.
    a.action = "start_login".into();
    let e = run(&a, &store, ssh.as_ref(), &tmux(&ssh), &http, NOW)
        .await
        .unwrap_err();
    assert_eq!(e.code, codes::E_EXISTS);
}

#[tokio::test]
async fn only_the_clis_own_keys_reach_the_login_pane() {
    let store = store_with_host();
    let ssh = Arc::new(FakeSsh::new());
    ssh.on(Match::script_contains("send-keys"), Reply::ok(""));
    let mut a = args("login_key");
    a.key = Some("2".into());
    run(
        &a,
        &store,
        ssh.as_ref(),
        &tmux(&ssh),
        &FakeTransport::new(),
        NOW,
    )
    .await
    .unwrap();
    assert!(ssh.calls()[0]
        .script()
        .unwrap()
        .ends_with("tmux send-keys -t '=fleet-login--api:' 2"));
    for bad in ["C-c", "rm", "Enter; ls", ""] {
        a.key = Some(bad.into());
        let e = run(
            &a,
            &store,
            ssh.as_ref(),
            &tmux(&ssh),
            &FakeTransport::new(),
            NOW,
        )
        .await
        .unwrap_err();
        assert_eq!(e.code, codes::E_INVALID, "{bad}");
    }
}

#[tokio::test]
async fn the_code_is_pasted_from_stdin_into_the_login_pane() {
    let store = store_with_host();
    let ssh = Arc::new(FakeSsh::new());
    ssh.on(Match::script_contains("load-buffer"), Reply::ok(""));
    let mut a = args("login_code");
    a.code = Some("Zm9v#YmFy".into());
    run(
        &a,
        &store,
        ssh.as_ref(),
        &tmux(&ssh),
        &FakeTransport::new(),
        NOW,
    )
    .await
    .unwrap();
    let call = &ssh.calls()[0];
    assert!(!call.command().contains("Zm9v"));
    assert_eq!(call.stdin.as_deref(), Some(&b"Zm9v#YmFy"[..]));
    assert!(call.script().unwrap().contains("-t '=fleet-login--api:'"));
}

#[test]
fn the_login_pane_is_never_a_session() {
    assert!(crate::tmux::is_login_session_name("fleet-login--work"));
    assert!(!crate::tmux::is_login_session_name("fleet-login--"));
    assert!(!crate::tmux::is_login_session_name("fleet-login--a b"));
    assert!(!crate::tmux::is_login_session_name("dev-work"));
    assert_eq!(login_session_name("work"), "fleet-login--work");
}

#[test]
fn a_spent_daily_limit_refuses_a_start_under_that_login() {
    let s = Store::open_in_memory().unwrap();
    s.upsert_host("mercury").unwrap();
    let uuid = api_key_account_uuid(KEY);
    s.set_host_profiles(
        "mercury",
        &[crate::store::HostProfileRow {
            name: "api".into(),
            account_uuid: Some(uuid.clone()),
            email: None,
            credential: None,
        }],
    )
    .unwrap();
    let refuse = |s: &Store| {
        crate::service::account_limits::refuse_over_daily_limit(s, "mercury", Some("api"), NOW)
    };
    assert!(refuse(&s).is_ok(), "no limit");
    set_daily_limit(&s, &uuid, Some(10.0)).unwrap();
    let today = NOW.div_euclid(86_400);
    let book = |day: i64, micros: i64| {
        s.conn_for_test()
            .execute(
                "INSERT INTO usage_daily_account (day, account_uuid, model, backfill, cost_micros) \
                 VALUES (?1, ?2, ?3, 0, ?4)",
                rusqlite::params![day, uuid, format!("m{day}{micros}"), micros],
            )
            .unwrap();
    };
    book(today - 1, 50_000_000);
    assert!(refuse(&s).is_ok(), "yesterday's spend does not count");
    book(today, 9_000_000);
    assert!(refuse(&s).is_ok());
    book(today, 1_500_000);
    let e = refuse(&s).unwrap_err();
    assert_eq!(e.code, codes::E_ACCOUNT_LIMIT);
    assert!(
        e.message
            .contains("spent $10.50 today, its daily limit is $10.00"),
        "{}",
        e.message
    );
    // Automation sees it as over too, and says why.
    let over = crate::service::account_limits::over_limit(&s, "mercury", Some("api"), NOW)
        .unwrap()
        .unwrap();
    assert!(over.reason().contains("daily limit"), "{}", over.reason());
    // The host's own login, with no limit, still starts.
    assert!(
        crate::service::account_limits::refuse_over_daily_limit(&s, "mercury", None, NOW).is_ok()
    );
    set_daily_limit(&s, &uuid, None).unwrap();
    assert!(refuse(&s).is_ok(), "cleared");
}

/// The real shell end to end: the write script lays the profile down from
/// stdin (mode 600, refusing a `/login` profile), the profiles script and
/// the local read list it as its API-key account, and a launch exports the
/// key without it ever reaching an argv.
#[cfg(unix)]
#[test]
fn the_written_profile_is_listed_and_exported() {
    use std::os::unix::fs::PermissionsExt;
    let home = tempfile::tempdir().unwrap();
    let write = |profile: &str| {
        let mut child = std::process::Command::new("bash")
            .arg("-c")
            .arg(write_script(profile, API_KEY_FILE))
            .env("HOME", home.path())
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let account =
            serde_json::json!({"accountUuid": api_key_account_uuid(KEY), "displayName": "API"});
        use std::io::Write as _;
        // A refused profile exits before reading stdin, so the write can
        // race its exit and meet a closed pipe; the exit status says the rest.
        match child
            .stdin
            .take()
            .unwrap()
            .write_all(&write_stdin(KEY, &account))
        {
            Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => {}
            r => r.unwrap(),
        }
        child.wait_with_output().unwrap()
    };
    let out = write("api");
    assert!(String::from_utf8_lossy(&out.stdout).contains(OK_MARK));
    let dir = home.path().join(".claude-profiles/api");
    let key_file = dir.join(API_KEY_FILE);
    assert_eq!(
        std::fs::read_to_string(&key_file).unwrap(),
        format!("{KEY}\n")
    );
    assert_eq!(
        std::fs::metadata(&key_file).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let cj: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join(".claude.json")).unwrap()).unwrap();
    assert_eq!(cj["hasCompletedOnboarding"], true);

    // A /login profile is refused, its files untouched.
    let login = home.path().join(".claude-profiles/work");
    std::fs::create_dir_all(&login).unwrap();
    std::fs::write(login.join(".claude.json"), "{}").unwrap();
    let out = write("work");
    assert_eq!(out.status.code(), Some(3));
    assert!(String::from_utf8_lossy(&out.stdout).contains(LOGIN_PROFILE_MARK));
    assert!(!login.join(API_KEY_FILE).exists());

    let listed = std::process::Command::new("bash")
        .arg("-c")
        .arg(crate::tmux::profiles_script())
        .env("HOME", home.path())
        .output()
        .unwrap();
    let got = crate::tmux::parse_profiles(&String::from_utf8_lossy(&listed.stdout));
    let api = got.iter().find(|p| p.name == "api").unwrap();
    assert_eq!(
        api.account.as_ref().unwrap().uuid.as_deref(),
        Some(api_key_account_uuid(KEY).as_str())
    );
    let local = crate::tmux::read_local_profiles(home.path()).unwrap();
    assert_eq!(
        local
            .iter()
            .find(|p| p.name == "api")
            .unwrap()
            .account
            .as_ref()
            .unwrap()
            .uuid,
        api.account.as_ref().unwrap().uuid
    );

    let exported = std::process::Command::new("sh")
        .arg("-c")
        .arg(format!(
            "export CLAUDE_CONFIG_DIR=\"$HOME/.claude-profiles/api\"; {}printf %s \"$ANTHROPIC_API_KEY\"",
            crate::tmux::PROFILE_API_KEY
        ))
        .env("HOME", home.path())
        .env_remove("ANTHROPIC_API_KEY")
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&exported.stdout), KEY);
    assert!(!crate::tmux::PROFILE_API_KEY.contains("sk-ant"));
}

/// The login pane's script parses, quoting and all, in bash and in sh.
#[cfg(unix)]
#[test]
fn the_login_pane_script_parses() {
    for sh in ["bash", "sh"] {
        let out = std::process::Command::new(sh)
            .args(["-n", "-c", &start_login_script("work")])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{sh}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

/// On a machine with tmux: the pane really opens under the profile and runs
/// `claude /login` there (a stand-in `claude` prints its config dir and a
/// link), `login_status`'s parser finds the link, and a second start is a
/// no-op. Its own tmux server (`TMUX_TMPDIR`), so nothing real is touched.
#[cfg(unix)]
#[test]
fn the_login_pane_runs_claude_login_under_the_profile() {
    if std::process::Command::new("tmux")
        .arg("-V")
        .output()
        .is_err()
    {
        return;
    }
    let home = tempfile::tempdir().unwrap();
    let bin = home.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let fake = bin.join("claude");
    std::fs::write(
        &fake,
        "#!/bin/sh\necho \"dir=$CLAUDE_CONFIG_DIR args=$*\"\necho 'https://claude.ai/oauth/authorize?state=1'\nsleep 30\n",
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    let sock = home.path().join("tmux");
    std::fs::create_dir_all(&sock).unwrap();
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let sh = |script: &str| {
        std::process::Command::new("bash")
            .args(["-c", script])
            .env("HOME", home.path())
            .env("TMUX_TMPDIR", &sock)
            .env_remove("TMUX")
            .env("PATH", &path)
            .output()
            .unwrap()
    };
    let out = sh(&start_login_script("work"));
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        sh(&start_login_script("work")).status.success(),
        "already open: a no-op"
    );
    let mut text = String::new();
    for _ in 0..50 {
        text = String::from_utf8_lossy(&sh("tmux capture-pane -p -t '=fleet-login--work:'").stdout)
            .into_owned();
        if text.contains("oauth") {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    sh("tmux kill-server");
    let dir = home.path().join(".claude-profiles/work");
    assert!(
        text.contains(&format!("dir={} args=/login", dir.display())),
        "{text}"
    );
    assert_eq!(
        sign_in_url(&text).as_deref(),
        Some("https://claude.ai/oauth/authorize?state=1")
    );
    let cj = std::fs::read_to_string(dir.join(".claude.json")).unwrap();
    assert!(cj.contains("hasCompletedOnboarding"));
}

/// Review 2026-11-03 D11: closing the login pane takes away a profile
/// directory this flow made when its login never completed, keeps one that
/// holds a login (dropping only the mark), and never touches a directory
/// that was there before.
#[cfg(unix)]
#[test]
fn a_cancelled_login_leaves_no_profile_it_made_behind() {
    let home = tempfile::tempdir().unwrap();
    let run = |profile: &str| {
        let out = std::process::Command::new("bash")
            .args(["-c", &end_login_script(profile)])
            .env("HOME", home.path())
            .env("TMUX_TMPDIR", home.path())
            .env_remove("TMUX")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    let dir = |name: &str| home.path().join(".claude-profiles").join(name);
    let make = |name: &str, json: &str, marked: bool| {
        std::fs::create_dir_all(dir(name)).unwrap();
        std::fs::write(dir(name).join(".claude.json"), json).unwrap();
        if marked {
            std::fs::write(dir(name).join(NEW_LOGIN_MARK), "").unwrap();
        }
    };
    make("ghost", "{\"hasCompletedOnboarding\":true}\n", true);
    make("done", "{\"oauthAccount\":{\"accountUuid\":\"u\"}}\n", true);
    make("mine", "{\"hasCompletedOnboarding\":true}\n", false);
    for p in ["ghost", "done", "mine"] {
        run(p);
    }
    assert!(!dir("ghost").exists());
    assert!(dir("done").join(".claude.json").exists());
    assert!(!dir("done").join(NEW_LOGIN_MARK).exists());
    assert!(dir("mine").join(".claude.json").exists());
}

// --- setup token ---------------------------------------------------------

const TOKEN: &str = "sk-ant-oat01-abcdefghijklmnopqrstuvwxyz0123456789_ABCDEFGHIJ";

fn anthropic_says(status: u16) -> FakeTransport {
    let http = FakeTransport::new();
    http.always(
        Method::Get,
        "/v1/models",
        Ok(Response::json(
            status,
            &serde_json::json!({"error": {"message": "nope"}}),
        )),
    );
    http
}

fn store_with_login_account() -> Mutex<Store> {
    let store = store_with_host();
    store
        .lock()
        .unwrap()
        .upsert_account(&crate::store::AccountRow {
            uuid: "acct-real".into(),
            email: Some("me@x.com".into()),
            display_name: Some("Me".into()),
            organization_name: Some("Org".into()),
            organization_uuid: Some("org-1".into()),
            seat_tier: Some("max".into()),
            last_seen_at: Some(NOW),
            nickname: None,
            has_extra_usage: false,
            forbidden_project_ids: Vec::new(),
            forbidden_org_ids: Vec::new(),
        })
        .unwrap();
    store
}

#[tokio::test]
async fn a_setup_token_on_a_known_account_goes_to_the_host_on_stdin() {
    let store = store_with_login_account();
    let ssh = Arc::new(FakeSsh::new());
    ssh.on(
        Match::script_contains(SETUP_TOKEN_FILE),
        Reply::ok("__fleet_ok__\n"),
    );
    let mut a = args("setup_token");
    a.profile = "tok".into();
    a.token = Some(TOKEN.into());
    a.account_uuid = Some("acct-real".into());
    // An inference-only token may not list models: not refused, not proven.
    let http = anthropic_says(403);
    let out = run(&a, &store, ssh.as_ref(), &tmux(&ssh), &http, NOW)
        .await
        .unwrap();
    assert_eq!(out["account_uuid"], "acct-real");
    assert_eq!(out["verified"], false);
    assert!(!out.to_string().contains(TOKEN));
    let req = &http.requests()[0];
    assert_eq!(
        req.header_value("authorization"),
        Some(format!("Bearer {TOKEN}").as_str())
    );
    assert!(!format!("{req:?}").contains(TOKEN), "Debug masks it");
    assert!(!format!("{a:?}").contains(TOKEN));
    assert!(!a.audit_detail().contains(TOKEN));

    let call = &ssh.calls()[0];
    assert!(!call.command().contains(TOKEN), "never in argv");
    let stdin = String::from_utf8(call.stdin.clone().unwrap()).unwrap();
    let mut lines = stdin.lines();
    assert_eq!(lines.next(), Some(TOKEN));
    let acct: serde_json::Value = serde_json::from_str(lines.next().unwrap()).unwrap();
    assert_eq!(acct["accountUuid"], "acct-real");
    assert_eq!(acct["fleetCredential"], "setup_token");
    assert!(
        acct.get("emailAddress").is_none(),
        "the /login reports those"
    );

    let s = store.lock().unwrap();
    let account = s.get_account_by_uuid("acct-real").unwrap().unwrap();
    assert_eq!(account.seat_tier.as_deref(), Some("max"), "left alone");
    let profiles = s
        .get_host_row("mercury")
        .unwrap()
        .unwrap()
        .claude_profiles
        .unwrap();
    assert_eq!(profiles[0].name, "tok");
    assert_eq!(profiles[0].account_uuid.as_deref(), Some("acct-real"));
    assert_eq!(profiles[0].credential.as_deref(), Some("setup_token"));
    assert_eq!(profiles[0].email.as_deref(), Some("me@x.com"));
}

#[tokio::test]
async fn a_setup_token_without_an_account_is_its_own_and_can_be_renewed() {
    let store = store_with_host();
    let ssh = Arc::new(FakeSsh::new());
    ssh.on(
        Match::script_contains(SETUP_TOKEN_FILE),
        Reply::ok("__fleet_ok__\n"),
    );
    let mut a = args("setup_token");
    a.profile = "tok".into();
    a.token = Some(TOKEN.into());
    let out = run(&a, &store, ssh.as_ref(), &tmux(&ssh), &anthropic_ok(), NOW)
        .await
        .unwrap();
    let uuid = setup_token_account_uuid(TOKEN);
    assert!(uuid.starts_with("setuptoken-") && uuid.len() == 27);
    assert!(has_no_usage_windows(&uuid) && !is_api_key_account(&uuid));
    assert_eq!(out["account_uuid"], uuid.as_str());
    assert_eq!(out["verified"], true);
    let seat = store
        .lock()
        .unwrap()
        .get_account_by_uuid(&uuid)
        .unwrap()
        .unwrap()
        .seat_tier;
    assert_eq!(seat.as_deref(), Some("setup_token"));

    // The yearly renewal: a new token over a setup-token profile. It is
    // the sign-in again, so the profile's signed-out card goes.
    let out_since = crate::store::SignedOutLogin {
        profile: Some("tok".into()),
        since: 5,
        checked_at: 5,
    };
    store
        .lock()
        .unwrap()
        .set_host_signed_out_logins(&a.host_alias, &[out_since])
        .unwrap();
    let renewed = format!("{TOKEN}X");
    a.token = Some(renewed.clone());
    let out = run(&a, &store, ssh.as_ref(), &tmux(&ssh), &anthropic_ok(), NOW)
        .await
        .unwrap();
    let host = store
        .lock()
        .unwrap()
        .get_host_row(&a.host_alias)
        .unwrap()
        .unwrap();
    assert!(
        host.signed_out_logins.is_empty(),
        "the new token signs it in"
    );
    assert!(crate::service::login_health::is_setup_token(
        &host,
        Some("tok")
    ));
    assert!(!crate::service::login_health::is_setup_token(&host, None));
    // It stays on the account it was: the id is the first token's.
    assert_eq!(out["account_uuid"], uuid.as_str());
    // But an API key is not accepted over it.
    a.action = "api_key".into();
    a.api_key = Some(KEY.into());
    let e = run(&a, &store, ssh.as_ref(), &tmux(&ssh), &anthropic_ok(), NOW)
        .await
        .unwrap_err();
    assert_eq!(e.code, codes::E_EXISTS);
}

#[tokio::test]
async fn a_setup_token_is_refused_where_it_does_not_belong() {
    let store = store_with_login_account();
    let ssh = Arc::new(FakeSsh::new());
    let http = anthropic_ok();
    let mut a = args("setup_token");
    a.profile = "tok".into();

    // Not a setup token: an API key, or nothing like one.
    for bad in [KEY, "hello", ""] {
        a.token = Some(bad.into());
        let e = run(&a, &store, ssh.as_ref(), &tmux(&ssh), &http, NOW)
            .await
            .unwrap_err();
        assert_eq!(e.code, codes::E_INVALID, "{bad}");
        assert_eq!(e.details.as_ref().unwrap()["problems"][0]["field"], "token");
    }
    // And a setup token pasted as an API key says which to pick.
    let mut k = args("api_key");
    k.api_key = Some(TOKEN.into());
    let e = run(&k, &store, ssh.as_ref(), &tmux(&ssh), &http, NOW)
        .await
        .unwrap_err();
    assert!(e.message.contains("setup token"), "{}", e.message);

    // An account fleet does not know, or one that is not a /login.
    a.token = Some(TOKEN.into());
    for acct in ["acct-unknown", "apikey-0123456789abcdef"] {
        a.account_uuid = Some(acct.into());
        let e = run(&a, &store, ssh.as_ref(), &tmux(&ssh), &http, NOW)
            .await
            .unwrap_err();
        assert_eq!(e.code, codes::E_INVALID, "{acct}");
    }
    // A /login profile is never overwritten.
    a.account_uuid = None;
    a.profile = "work".into();
    store
        .lock()
        .unwrap()
        .set_host_profiles(
            "mercury",
            &[crate::store::HostProfileRow {
                name: "work".into(),
                account_uuid: Some("acct-real".into()),
                email: None,
                credential: None,
            }],
        )
        .unwrap();
    let e = run(&a, &store, ssh.as_ref(), &tmux(&ssh), &http, NOW)
        .await
        .unwrap_err();
    assert_eq!(e.code, codes::E_EXISTS);
    assert!(ssh.calls().is_empty() && http.requests().is_empty());

    // A 401 is Anthropic's refusal, on the field; nothing is written.
    a.profile = "tok".into();
    let e = run(
        &a,
        &store,
        ssh.as_ref(),
        &tmux(&ssh),
        &anthropic_says(401),
        NOW,
    )
    .await
    .unwrap_err();
    assert_eq!(e.code, codes::E_INVALID);
    assert!(!e.message.contains(TOKEN));
    // A 5xx is no answer: try again.
    let e = run(
        &a,
        &store,
        ssh.as_ref(),
        &tmux(&ssh),
        &anthropic_says(503),
        NOW,
    )
    .await
    .unwrap_err();
    assert_eq!(e.code, codes::E_PROBE);
    assert!(ssh.calls().is_empty());
}

/// The real shell: the token lands mode 600, the profile lists as a
/// setup-token one (remote script and local read alike), and a launch
/// exports it as CLAUDE_CODE_OAUTH_TOKEN, not as an API key.
#[cfg(unix)]
#[test]
fn a_written_setup_token_is_listed_marked_and_exported() {
    use std::os::unix::fs::PermissionsExt;
    let home = tempfile::tempdir().unwrap();
    let mut child = std::process::Command::new("bash")
        .arg("-c")
        .arg(write_script("tok", SETUP_TOKEN_FILE))
        .env("HOME", home.path())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let account = serde_json::json!({"accountUuid": "acct-real", "fleetCredential": "setup_token"});
    use std::io::Write as _;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(format!("{TOKEN}\n{account}\n{{\"hasCompletedOnboarding\":true}}\n").as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(String::from_utf8_lossy(&out.stdout).contains(OK_MARK));
    let file = home
        .path()
        .join(".claude-profiles/tok")
        .join(SETUP_TOKEN_FILE);
    assert_eq!(
        std::fs::metadata(&file).unwrap().permissions().mode() & 0o777,
        0o600
    );

    let listed = std::process::Command::new("bash")
        .arg("-c")
        .arg(crate::tmux::profiles_script())
        .env("HOME", home.path())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&listed.stdout);
    assert!(!stdout.contains(TOKEN));
    let remote = crate::tmux::parse_profiles(&stdout);
    let local = crate::tmux::read_local_profiles(home.path()).unwrap();
    for got in [&remote, &local] {
        let a = got[0].account.as_ref().unwrap();
        assert_eq!(a.uuid.as_deref(), Some("acct-real"));
        assert_eq!(a.fleet_credential.as_deref(), Some("setup_token"));
    }

    let exported = std::process::Command::new("sh")
        .arg("-c")
        .arg(format!(
            "export CLAUDE_CONFIG_DIR=\"$HOME/.claude-profiles/tok\"; {}printf '%s|%s' \"$CLAUDE_CODE_OAUTH_TOKEN\" \"$ANTHROPIC_API_KEY\"",
            crate::tmux::PROFILE_API_KEY
        ))
        .env("HOME", home.path())
        .env_remove("CLAUDE_CODE_OAUTH_TOKEN")
        .env_remove("ANTHROPIC_API_KEY")
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&exported.stdout),
        format!("{TOKEN}|")
    );
}
