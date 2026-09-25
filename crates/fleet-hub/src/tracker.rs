//! `fleet-hub tracker …` — the hub operator's side of `work_admin` (work
//! graph M3.1): add a Jira Cloud site, set its credential, test it, list and
//! remove trackers. Each subcommand is one `work_admin` call over loopback
//! with the master token, exactly like `fleet-hub client …`.
//!
//! **The secret never travels in argv**, where `ps`, shell history and
//! process accounting would keep it. `set-credential` reads it from stdin
//! (one line), from an environment variable named by `--from-env`, or stores
//! a reference (`--ref env:NAME` / `--ref file:/run/secrets/jira`) that the
//! hub resolves itself at use — the Docker-secrets path. Nothing this module
//! prints comes from the secret: every line is rendered from the returned
//! tracker row, which carries only a `…abcd` hint.

use crate::config::HubOptions;
use crate::out;
use crate::pair::{call_tool, fmt_time, hub_conn};
use clap::Subcommand;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::process::ExitCode;

#[derive(Subcommand, Debug)]
pub enum TrackerCmd {
    /// Print the trackers and their state, one per line.
    List,
    /// Add a tracker: paste any ticket or issue URL on it, or the site URL.
    /// The provider is inferred from the URL (atlassian.net → Jira,
    /// github.com → GitHub) unless --provider says otherwise.
    Add {
        /// https://<name>.atlassian.net, https://github.com/<owner>, or a
        /// ticket / issue URL on it.
        url: String,
        /// Display name. [default: from the site]
        #[arg(long)]
        name: Option<String>,
        /// jira | github. [default: from the URL]
        #[arg(long)]
        provider: Option<String>,
        /// Run a trusted CLI on this host with ITS login (GitHub: `gh`);
        /// fleet then stores no credential.
        #[arg(long, conflicts_with = "via_host")]
        via_cli: Option<String>,
        /// Send the requests with `curl` from this host (a tracker only it
        /// can reach); the token is piped there on stdin, never in argv.
        #[arg(long)]
        via_host: Option<String>,
        /// GitHub: only these repositories (owner/repo; repeatable).
        #[arg(long = "repo")]
        repos: Vec<String>,
    },
    /// Set a tracker's credential. The API token is read from stdin (one
    /// line) unless --from-env or --ref says otherwise; it is never an
    /// argument.
    SetCredential {
        /// The tracker's id, from `tracker list`.
        id: i64,
        /// The Atlassian account email the token belongs to (Jira Cloud's
        /// basic auth). Without it the token is sent as the whole
        /// credential (a bearer / API key).
        #[arg(long)]
        email: Option<String>,
        /// Read the token from this environment variable of THIS command
        /// (e.g. `read -rs JIRA_TOKEN; export JIRA_TOKEN`), and store it.
        #[arg(long, conflicts_with = "reference")]
        from_env: Option<String>,
        /// Store a reference instead of the token: env:NAME or
        /// file:/run/secrets/jira, read by the hub whenever it syncs.
        #[arg(long = "ref")]
        reference: Option<String>,
    },
    /// Probe the site with the stored credential and record what it found.
    Test {
        /// The tracker's id, from `tracker list`.
        id: i64,
    },
    /// Remove a tracker and its credential; its items stay, marked unavailable.
    Remove {
        /// The tracker's id, from `tracker list`.
        id: i64,
    },
}

/// Where `set-credential` gets the secret from.
#[derive(Debug, PartialEq, Eq)]
enum Source {
    Stdin,
    Env(String),
    Reference(String),
}

fn source(from_env: Option<String>, reference: Option<String>) -> Source {
    match (from_env, reference) {
        (_, Some(r)) => Source::Reference(r),
        (Some(v), None) => Source::Env(v),
        (None, None) => Source::Stdin,
    }
}

/// The `work_admin` arguments for `add`.
fn add_args(
    url: String,
    name: Option<String>,
    provider: Option<String>,
    via_cli: Option<String>,
    via_host: Option<String>,
    repos: Vec<String>,
) -> Value {
    let mut args = json!({ "action": "add", "site_url": url });
    if let Some(n) = name {
        args["name"] = Value::String(n);
    }
    if let Some(p) = provider {
        args["provider"] = Value::String(p);
    }
    if let Some(h) = via_cli {
        args["transport"] = Value::String(format!("via_cli:{h}"));
    } else if let Some(h) = via_host {
        args["transport"] = Value::String(format!("via_host:{h}"));
    }
    if !repos.is_empty() {
        args["settings"] = json!({ "repos": repos });
    }
    args
}

/// The `work_admin` arguments for `set-credential`, the secret read from its
/// source. `read_stdin` is injected so the test never touches a terminal.
fn credential_args(
    id: i64,
    email: Option<&str>,
    src: &Source,
    env: &HashMap<String, String>,
    read_stdin: impl FnOnce() -> Result<String, String>,
) -> Result<Value, String> {
    let mut args = json!({
        "action": "set_credential",
        "tracker_id": id,
        "auth_kind": if email.is_some() { "basic" } else { "bearer" },
    });
    if let Some(e) = email {
        args["username"] = Value::String(e.to_string());
    }
    match src {
        Source::Reference(r) => args["credential_ref"] = Value::String(r.clone()),
        Source::Env(var) => {
            let v = env
                .get(var)
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
                .ok_or_else(|| format!("${var} is not set (or empty) in this shell"))?;
            args["secret"] = Value::String(v);
        }
        Source::Stdin => {
            let v = read_stdin()?.trim().to_string();
            if v.is_empty() {
                return Err("no token on stdin; pipe it in, or use --from-env / --ref".into());
            }
            args["secret"] = Value::String(v);
        }
    }
    Ok(args)
}

fn read_one_line() -> Result<String, String> {
    let mut line = String::new();
    std::io::stdin()
        .read_line(&mut line)
        .map_err(|e| format!("read the token from stdin: {e}"))?;
    Ok(line)
}

/// One line per tracker. Only fields a read path carries: no secret exists
/// here to print.
fn tracker_line(t: &Value) -> String {
    let cred = if t["has_credential"].as_bool().unwrap_or(false) {
        format!(
            "{} {}",
            t["username"].as_str().unwrap_or("?"),
            t["credential_hint"].as_str().unwrap_or("")
        )
    } else {
        "no credential".into()
    };
    let err = t["last_error"]
        .as_str()
        .map(|e| format!("  — {e}"))
        .unwrap_or_default();
    format!(
        "{:>4}  {:<12}  {:<34}  {:<14}  synced {}  [{}]{err}",
        t["id"].as_i64().unwrap_or_default(),
        t["name"].as_str().unwrap_or_default(),
        t["site_url"].as_str().unwrap_or_default(),
        t["state"].as_str().unwrap_or_default(),
        fmt_time(t["last_sync_at"].as_i64()),
        cred,
    )
}

pub async fn run(
    cmd: TrackerCmd,
    opts: &HubOptions,
    env: &HashMap<String, String>,
) -> Result<ExitCode, String> {
    let conn = hub_conn(opts, env)?;
    match cmd {
        TrackerCmd::List => {
            let v = call_tool(&conn, "work_admin", json!({ "action": "list" })).await?;
            let rows = v.as_array().cloned().unwrap_or_default();
            if rows.is_empty() {
                out::line("no trackers; add one with `fleet-hub tracker add <ticket-url>`");
            }
            for t in rows {
                out::line(&tracker_line(&t));
            }
        }
        TrackerCmd::Add {
            url,
            name,
            provider,
            via_cli,
            via_host,
            repos,
        } => {
            let args = add_args(url, name, provider, via_cli, via_host, repos);
            let t = call_tool(&conn, "work_admin", args).await?;
            out::line(&tracker_line(&t));
            let id = t["id"].as_i64().unwrap_or_default();
            out::line(&match (t["provider"].as_str(), t["transport"].as_str()) {
                (_, Some(tr)) if tr.starts_with("via_cli:") => {
                    format!("next: fleet-hub tracker test {id} (no credential: the host's own login)")
                }
                (Some("jira"), _) => format!(
                    "next: fleet-hub tracker set-credential {id} --email <you@example.com> < token.txt"
                ),
                _ => format!("next: fleet-hub tracker set-credential {id} < token.txt"),
            });
        }
        TrackerCmd::SetCredential {
            id,
            email,
            from_env,
            reference,
        } => {
            let args = credential_args(
                id,
                email.as_deref(),
                &source(from_env, reference),
                env,
                read_one_line,
            )?;
            let t = call_tool(&conn, "work_admin", args).await?;
            out::line(&tracker_line(&t));
            out::line(&format!(
                "next: fleet-hub tracker test {id} (Atlassian API tokens expire within a year)"
            ));
        }
        TrackerCmd::Test { id } => {
            let r = call_tool(
                &conn,
                "work_admin",
                json!({ "action": "test", "tracker_id": id }),
            )
            .await?;
            out::line(&tracker_line(&r["tracker"]));
            if r["ok"].as_bool().unwrap_or(false) {
                let views: Vec<&str> = r["views"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .collect();
                out::line(&format!("ok — views: {}", views.join(", ")));
            } else {
                return Err(r["error"].as_str().unwrap_or("the test failed").to_string());
            }
        }
        TrackerCmd::Remove { id } => {
            call_tool(
                &conn,
                "work_admin",
                json!({ "action": "remove", "tracker_id": id }),
            )
            .await?;
            out::line(&format!(
                "removed tracker {id}; its items stay, marked unavailable"
            ));
        }
    }
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKEN: &str = "ATATT3xFfGF0-cli-test-token-not-real";

    #[test]
    fn the_secret_comes_from_stdin_env_or_a_reference_never_argv() {
        let env = HashMap::from([("JIRA_TOKEN".to_string(), format!(" {TOKEN}\n"))]);
        let a = credential_args(1, Some("me@x.com"), &Source::Stdin, &env, || {
            Ok(format!("{TOKEN}\n"))
        })
        .unwrap();
        assert_eq!(a["secret"], TOKEN);
        assert_eq!(a["username"], "me@x.com");
        let a = credential_args(
            1,
            Some("me@x.com"),
            &Source::Env("JIRA_TOKEN".into()),
            &env,
            || panic!("stdin must not be read"),
        )
        .unwrap();
        assert_eq!(a["secret"], TOKEN);
        let a = credential_args(
            1,
            Some("me@x.com"),
            &Source::Reference("file:/run/secrets/jira".into()),
            &env,
            || panic!("stdin must not be read"),
        )
        .unwrap();
        assert_eq!(a["credential_ref"], "file:/run/secrets/jira");
        assert!(a.get("secret").is_none());
        assert!(
            credential_args(1, Some("x"), &Source::Env("NOPE".into()), &env, || Ok(
                String::new()
            ))
            .is_err()
        );
        assert!(credential_args(1, Some("x"), &Source::Stdin, &env, || Ok("\n".into())).is_err());
        // No email: the token is the whole credential (Asana, Linear, DC).
        let a =
            credential_args(2, None, &Source::Stdin, &env, || Ok("lin_api_x\n".into())).unwrap();
        assert_eq!(a["auth_kind"], "bearer");
        assert!(a.get("username").is_none());
        assert_eq!(source(Some("A".into()), None), Source::Env("A".into()));
        assert_eq!(source(None, None), Source::Stdin);
    }

    #[test]
    fn add_carries_the_transport_and_repos() {
        let a = add_args(
            "https://github.com/acme".into(),
            None,
            None,
            Some("devbox".into()),
            None,
            vec!["acme/api".into()],
        );
        assert_eq!(a["transport"], "via_cli:devbox");
        assert_eq!(a["settings"]["repos"], json!(["acme/api"]));
        assert!(a.get("provider").is_none(), "inferred by the hub");
    }

    /// No flag or positional takes the token: an extra argument is refused
    /// by clap rather than silently accepted.
    #[test]
    fn set_credential_has_no_argument_that_could_carry_the_token() {
        use clap::Parser;
        #[derive(Parser)]
        struct T {
            #[command(subcommand)]
            cmd: TrackerCmd,
        }
        assert!(T::try_parse_from(["t", "set-credential", "1", "--email", "a@b", TOKEN]).is_err());
        assert!(T::try_parse_from([
            "t",
            "set-credential",
            "1",
            "--email",
            "a@b",
            "--token",
            TOKEN
        ])
        .is_err());
        assert!(T::try_parse_from(["t", "set-credential", "1", "--email", "a@b"]).is_ok());
        assert!(
            T::try_parse_from([
                "t",
                "set-credential",
                "1",
                "--email",
                "a@b",
                "--from-env",
                "X",
                "--ref",
                "env:Y"
            ])
            .is_err(),
            "one source only"
        );
        let T { cmd } =
            T::try_parse_from(["t", "add", "https://acme.atlassian.net/browse/ABC-1"]).unwrap();
        assert!(matches!(cmd, TrackerCmd::Add { url, name: None, .. } if url.ends_with("ABC-1")));
    }

    #[test]
    fn a_tracker_line_shows_the_hint_never_more() {
        let line = tracker_line(&json!({
            "id": 3, "name": "acme", "site_url": "https://acme.atlassian.net",
            "state": "auth_failed", "has_credential": true, "username": "me@x.com",
            "credential_hint": "…WXYZ", "last_error": "the tracker refused the credential"
        }));
        assert!(
            line.contains("…WXYZ") && line.contains("auth_failed"),
            "{line}"
        );
        assert!(line.contains("me@x.com"));
        let none = tracker_line(&json!({"id": 1, "state": "unconfigured"}));
        assert!(none.contains("no credential"));
    }
}
