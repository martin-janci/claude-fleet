//! `fleet-hub` — claude-fleet without the desktop app. See `docs/hub.md`.

mod config;
mod demo;
mod out;
mod pair;
mod reports;
mod serve;
mod tls;

use clap::{Parser, Subcommand};
use config::HubOptions;
use std::process::ExitCode;

#[derive(Parser)]
#[command(name = "fleet-hub", version, about = "Headless claude-fleet hub")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Create the data dir and state.db, mint the master token, print it once.
    Init {
        #[command(flatten)]
        opts: HubOptions,
        /// Mint a fresh master token even if one exists.
        #[arg(long)]
        regenerate_token: bool,
    },
    /// Run the hub until SIGTERM/SIGINT.
    Serve {
        #[command(flatten)]
        opts: HubOptions,
    },
    /// Show or rotate the master token.
    Token {
        #[command(subcommand)]
        cmd: TokenCmd,
        #[command(flatten)]
        opts: HubOptions,
    },
    /// Print an agent host's token, for `fleet-agent install` on that host (minted on first use).
    ///
    /// The only way an agent host's token reaches the host: the hub never
    /// sends a new token over the agent connection it replaces. `--rotate`
    /// mints a new one and saves it, which cuts off any agent still connected
    /// on the old one within a heartbeat.
    AgentToken {
        /// The host's fleet alias.
        host: String,
        /// Mint a fresh token, revoking the current one.
        #[arg(long)]
        rotate: bool,
        #[command(flatten)]
        opts: HubOptions,
    },
    /// Set a host's control-API token mode, without a desktop.
    ///
    /// The way back from a `readonly` token, which `/agent` refuses: a
    /// rotation keeps the existing mode, so `agent-token --rotate` cannot
    /// undo it. Leaves the token itself alone, so nothing has to be
    /// re-installed on the host.
    HostTokenMode {
        /// The host's fleet alias.
        host: String,
        /// full (drive the host) or readonly (observe it).
        mode: String,
        #[command(flatten)]
        opts: HubOptions,
    },
    /// Mint a pairing code for a new client device and show it as a QR. Needs a running hub.
    Pair {
        /// Name for the client, as it will appear in `client list` (1-64 characters).
        #[arg(long)]
        name: String,
        /// What the client may do: full (drive sessions) or readonly (observe). [default: full]
        #[arg(long)]
        mode: Option<String>,
        /// Seconds the pairing code stays valid (30-3600). [default: 600]
        #[arg(long)]
        ttl: Option<u64>,
        /// Pair a device you vouch for: its prompts reach agents WITHOUT the
        /// untrusted-content marker. Only for a keyboard that is yours.
        #[arg(long)]
        trusted: bool,
        #[command(flatten)]
        opts: HubOptions,
    },
    /// List or revoke paired client devices. Needs a running hub.
    Client {
        #[command(subcommand)]
        cmd: ClientCmd,
        #[command(flatten)]
        opts: HubOptions,
    },
    /// Fill the store with obviously-fake hosts, projects and sessions, so a
    /// freshly paired client has something to draw. Development only.
    ///
    /// A client paired to a hub that has never run a session shows an empty
    /// list, which is indistinguishable from a broken pairing. This makes the
    /// difference visible. Every row is named `demo-…`, which is what lets
    /// `--clear` remove exactly these and nothing else.
    ///
    /// Refuses a store that already holds rows it did not write: seeding a live
    /// fleet would mix invented sessions into a list an operator makes
    /// decisions from.
    DemoSeed {
        /// How many fake hosts to create (1-4). [default: 2]
        #[arg(long)]
        hosts: Option<usize>,
        /// Remove the demo rows instead of adding them.
        #[arg(long)]
        clear: bool,
        /// Seed even though the store holds real rows.
        #[arg(long)]
        force: bool,
        #[command(flatten)]
        opts: HubOptions,
    },
    /// Show the error reports the hub has collected from its participants, newest first. Needs a running hub.
    Reports {
        /// Rows to show (1-1000). [default: 100]
        #[arg(long, default_value_t = 100)]
        limit: u32,
        /// Only rows received since: 30m, 2h, 3d or a unix timestamp.
        #[arg(long)]
        since: Option<String>,
        /// Only rows from this origin: client:<name>, host:<alias> or hub.
        #[arg(long)]
        origin: Option<String>,
        /// Print the rows as JSON (includes each report's context).
        #[arg(long)]
        json: bool,
        #[command(flatten)]
        opts: HubOptions,
    },
    /// Print this hub's SSH public key (generated on first use; derived when only the private key exists).
    SshKey,
    /// Exit 0 when a hub answers HTTP on 127.0.0.1 (for Docker HEALTHCHECK). Does not open the database.
    Healthcheck {
        /// Port to probe [env: FLEET_HUB_PORT] [default: 4180]
        #[arg(long)]
        port: Option<u16>,
        /// Whether the hub terminates TLS, so the probe speaks it too: off or cert [env: FLEET_HUB_TLS] [default: off]
        #[arg(long)]
        tls: Option<String>,
    },
}

#[derive(Subcommand)]
enum TokenCmd {
    Show,
    Regenerate,
}

#[derive(Subcommand)]
enum ClientCmd {
    /// Print the paired clients, one per line.
    List {
        /// Also show clients whose token has been revoked.
        #[arg(long)]
        include_revoked: bool,
    },
    /// Revoke a client's token by name; its next request is refused.
    Revoke { name: String },
    /// Vouch for a client by name: its prompts reach agents unmarked.
    Trust { name: String },
    /// Take that back: its prompts are marked as untrusted input again.
    Untrust { name: String },
}

#[tokio::main]
async fn main() -> ExitCode {
    // Mandatory and first: fleet-core keeps no default app version, so
    // `app_version::get()` — /healthz, the agent handshake, the MCP hello —
    // panics until this line has run. This crate's version is the hub's.
    fleet_core::app_version::set(env!("CARGO_PKG_VERSION"));
    let cli = Cli::parse();
    let env: std::collections::HashMap<String, String> = std::env::vars().collect();
    let result = match cli.cmd {
        Cmd::Init {
            opts,
            regenerate_token,
        } => serve::init(&opts, &env, regenerate_token),
        Cmd::Serve { opts } => serve::serve(&opts, &env).await,
        Cmd::Token { cmd, opts } => serve::token(&opts, &env, matches!(cmd, TokenCmd::Regenerate)),
        Cmd::AgentToken { host, rotate, opts } => serve::agent_token(&opts, &env, &host, rotate),
        Cmd::HostTokenMode { host, mode, opts } => {
            serve::host_token_mode(&opts, &env, &host, &mode)
        }
        Cmd::Pair {
            name,
            mode,
            ttl,
            trusted,
            opts,
        } => pair::pair(&opts, &env, &name, mode.as_deref(), ttl, trusted).await,
        Cmd::Client { cmd, opts } => match cmd {
            ClientCmd::List { include_revoked } => {
                pair::client_list(&opts, &env, include_revoked).await
            }
            ClientCmd::Revoke { name } => pair::client_revoke(&opts, &env, &name).await,
            ClientCmd::Trust { name } => pair::client_trust(&opts, &env, &name, true).await,
            ClientCmd::Untrust { name } => pair::client_trust(&opts, &env, &name, false).await,
        },
        Cmd::Reports {
            limit,
            since,
            origin,
            json,
            opts,
        } => reports::run(&opts, &env, limit, since, origin, json).await,
        Cmd::DemoSeed {
            hosts,
            clear,
            force,
            opts,
        } => demo_seed(&opts, &env, hosts, clear, force),
        Cmd::SshKey => serve::ssh_key(),
        Cmd::Healthcheck { port, tls } => serve::healthcheck(port, tls, &env).await,
    };
    match result {
        Ok(code) => code,
        Err(msg) => {
            out::error(&msg);
            ExitCode::from(1)
        }
    }
}

/// `fleet-hub demo-seed` — see [`demo`] for what it writes and why.
///
/// Like `token` and `agent-token`, this never *creates* a data dir or a
/// database: demo rows in a fresh one would belong to no hub, and the mistake
/// they would hide is exactly the one this command exists to make visible.
fn demo_seed(
    opts: &HubOptions,
    env: &std::collections::HashMap<String, String>,
    hosts: Option<usize>,
    clear: bool,
    force: bool,
) -> Result<ExitCode, String> {
    serve::existing_db(&config::resolve_data_dir(opts, env))?;
    let store = serve::open_store(opts, env)?;

    if clear {
        let removed = demo::clear(&store)?;
        out::line(&format!("removed {removed} demo session(s)"));
        return Ok(ExitCode::SUCCESS);
    }

    // Checked before anything is written, so a refusal leaves the store
    // exactly as it was rather than half seeded.
    if !force && demo::holds_real_rows(&store)? {
        return Err(
            "this hub already has hosts or sessions of its own; demo rows would be mixed in \
             with them and only their names would tell them apart. Pass --force if that is \
             what you want, or --clear to remove demo rows."
                .to_string(),
        );
    }

    let hosts = hosts.unwrap_or(2);
    if hosts == 0 || hosts > 4 {
        return Err(format!("--hosts must be between 1 and 4, got {hosts}"));
    }

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let sessions = demo::seed(&store, &demo::Plan { hosts }, now)?;

    out::line(&format!(
        "seeded {hosts} demo host(s) and {sessions} demo session(s). \
         Remove them with: fleet-hub demo-seed --clear"
    ));
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_parses_every_subcommand() {
        Cli::try_parse_from(["fleet-hub", "init", "--public-url", "https://x.example.com"])
            .unwrap();
        Cli::try_parse_from([
            "fleet-hub",
            "serve",
            "--bind",
            "0.0.0.0",
            "--allow-plaintext",
        ])
        .unwrap();
        // The TLS flags, global like the rest.
        let Cmd::Serve { opts } = Cli::try_parse_from([
            "fleet-hub",
            "serve",
            "--tls",
            "cert",
            "--tls-cert",
            "/etc/tls.crt",
            "--tls-key",
            "/etc/tls.key",
        ])
        .unwrap()
        .cmd
        else {
            panic!("serve --tls did not parse");
        };
        assert_eq!(opts.tls.as_deref(), Some("cert"));
        assert_eq!(opts.tls_cert, Some("/etc/tls.crt".into()));
        assert_eq!(opts.tls_key, Some("/etc/tls.key".into()));
        // `auto` parses — `config::resolve` is what refuses it, with a reason.
        Cli::try_parse_from(["fleet-hub", "init", "--tls", "auto"]).unwrap();
        Cli::try_parse_from(["fleet-hub", "token", "show"]).unwrap();
        Cli::try_parse_from(["fleet-hub", "token", "regenerate"]).unwrap();
        Cli::try_parse_from(["fleet-hub", "agent-token", "laptop"]).unwrap();
        Cli::try_parse_from([
            "fleet-hub",
            "agent-token",
            "laptop",
            "--rotate",
            "--data-dir",
            "/tmp/x",
        ])
        .unwrap();
        for argv in [
            ["fleet-hub", "token", "show", "--data-dir", "/tmp/x"],
            ["fleet-hub", "token", "--data-dir", "/tmp/x", "show"],
        ] {
            let Cmd::Token { cmd, opts } = Cli::try_parse_from(argv).unwrap().cmd else {
                panic!("{argv:?} did not parse as token");
            };
            assert!(matches!(cmd, TokenCmd::Show), "{argv:?}");
            assert_eq!(opts.data_dir, Some("/tmp/x".into()), "{argv:?}");
        }
        // `host-token-mode`, the headless way back to a `full` token — with
        // HubOptions accepted in either position, `--port` included, exactly
        // as `token` and `client` take them.
        for argv in [
            [
                "fleet-hub",
                "host-token-mode",
                "laptop",
                "full",
                "--data-dir",
                "/tmp/x",
                "--port",
                "4190",
            ],
            [
                "fleet-hub",
                "host-token-mode",
                "--data-dir",
                "/tmp/x",
                "--port",
                "4190",
                "laptop",
                "full",
            ],
        ] {
            let Cmd::HostTokenMode { host, mode, opts } = Cli::try_parse_from(argv).unwrap().cmd
            else {
                panic!("{argv:?} did not parse as host-token-mode");
            };
            assert_eq!(
                (host.as_str(), mode.as_str()),
                ("laptop", "full"),
                "{argv:?}"
            );
            assert_eq!(opts.data_dir, Some("/tmp/x".into()), "{argv:?}");
            assert_eq!(opts.port, Some(4190), "{argv:?}");
        }
        Cli::try_parse_from(["fleet-hub", "host-token-mode", "laptop", "readonly"]).unwrap();
        // Both arguments are required; clap refuses a missing one rather than
        // guessing a mode.
        assert!(Cli::try_parse_from(["fleet-hub", "host-token-mode", "laptop"]).is_err());
        assert!(Cli::try_parse_from(["fleet-hub", "host-token-mode"]).is_err());

        // `pair` and `client …`, including HubOptions before or after the
        // subcommand (they are `global = true`).
        let Cmd::Pair {
            name,
            mode,
            ttl,
            trusted,
            ..
        } = Cli::try_parse_from([
            "fleet-hub",
            "pair",
            "--name",
            "phone",
            "--mode",
            "readonly",
            "--ttl",
            "120",
        ])
        .unwrap()
        .cmd
        else {
            panic!("pair did not parse");
        };
        assert_eq!(
            (name.as_str(), mode.as_deref(), ttl, trusted),
            ("phone", Some("readonly"), Some(120), false)
        );
        let Cmd::Pair { trusted, .. } =
            Cli::try_parse_from(["fleet-hub", "pair", "--name", "desk", "--trusted"])
                .unwrap()
                .cmd
        else {
            panic!("pair --trusted did not parse");
        };
        assert!(trusted);
        // `--name` is required.
        assert!(Cli::try_parse_from(["fleet-hub", "pair"]).is_err());
        for argv in [
            ["fleet-hub", "client", "list", "--data-dir", "/tmp/x"],
            ["fleet-hub", "client", "--data-dir", "/tmp/x", "list"],
        ] {
            let Cmd::Client { cmd, opts } = Cli::try_parse_from(argv).unwrap().cmd else {
                panic!("{argv:?} did not parse as client");
            };
            assert!(
                matches!(
                    cmd,
                    ClientCmd::List {
                        include_revoked: false
                    }
                ),
                "{argv:?}"
            );
            assert_eq!(opts.data_dir, Some("/tmp/x".into()), "{argv:?}");
        }
        let Cmd::Client { cmd, .. } =
            Cli::try_parse_from(["fleet-hub", "client", "list", "--include-revoked"])
                .unwrap()
                .cmd
        else {
            panic!("client list --include-revoked did not parse");
        };
        assert!(matches!(
            cmd,
            ClientCmd::List {
                include_revoked: true
            }
        ));
        let Cmd::Client { cmd, .. } =
            Cli::try_parse_from(["fleet-hub", "client", "revoke", "phone"])
                .unwrap()
                .cmd
        else {
            panic!("client revoke did not parse");
        };
        assert!(matches!(cmd, ClientCmd::Revoke { name } if name == "phone"));
        assert!(Cli::try_parse_from(["fleet-hub", "client", "revoke"]).is_err());
        let Cmd::Client { cmd, .. } = Cli::try_parse_from(["fleet-hub", "client", "trust", "desk"])
            .unwrap()
            .cmd
        else {
            panic!("client trust did not parse");
        };
        assert!(matches!(cmd, ClientCmd::Trust { name } if name == "desk"));
        let Cmd::Client { cmd, .. } =
            Cli::try_parse_from(["fleet-hub", "client", "untrust", "desk"])
                .unwrap()
                .cmd
        else {
            panic!("client untrust did not parse");
        };
        assert!(matches!(cmd, ClientCmd::Untrust { name } if name == "desk"));
        assert!(Cli::try_parse_from(["fleet-hub", "client", "trust"]).is_err());
        let Cmd::Reports {
            limit,
            since,
            origin,
            json,
            ..
        } = Cli::try_parse_from([
            "fleet-hub",
            "reports",
            "--limit",
            "5",
            "--since",
            "2h",
            "--origin",
            "hub",
            "--json",
        ])
        .unwrap()
        .cmd
        else {
            panic!("reports did not parse")
        };
        assert_eq!(
            (limit, since.as_deref(), origin.as_deref(), json),
            (5, Some("2h"), Some("hub"), true)
        );
        Cli::try_parse_from(["fleet-hub", "reports"]).unwrap();
        Cli::try_parse_from(["fleet-hub", "ssh-key"]).unwrap();
        Cli::try_parse_from(["fleet-hub", "healthcheck"]).unwrap();
        let Cmd::Healthcheck { port, tls } = Cli::try_parse_from([
            "fleet-hub",
            "healthcheck",
            "--port",
            "4190",
            "--tls",
            "cert",
        ])
        .unwrap()
        .cmd
        else {
            panic!("healthcheck --port did not parse");
        };
        assert_eq!(port, Some(4190));
        assert_eq!(tls.as_deref(), Some("cert"));
        assert!(Cli::try_parse_from(["fleet-hub", "bogus"]).is_err());
    }
}
