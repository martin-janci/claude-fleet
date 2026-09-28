//! `fleet-hub` — claude-fleet without the desktop app. See `docs/hub.md`.

mod bench;
mod catalog;
mod census;
mod config;
mod decide;
mod demo;
mod host;
mod org;
mod out;
mod pair;
mod peer;
mod provision;
mod ready;
mod reports;
mod serve;
mod tls;
mod tracker;
mod work;

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
        /// What the client may do: full (drive sessions), readonly (observe), or peer (another hub; see fleet-hub peer add). [default: full]
        #[arg(long)]
        mode: Option<String>,
        /// Seconds the pairing code stays valid (30-3600). [default: 600]
        #[arg(long)]
        ttl: Option<u64>,
        /// Pair a device you vouch for: its prompts reach agents WITHOUT the
        /// untrusted-content marker. Only for a keyboard that is yours.
        #[arg(long)]
        trusted: bool,
        /// Bind the client to one org (its id): it reads only that org's work
        /// and sessions, and unassigned ones while the org's
        /// `bound_sees_unassigned` is on (the default).
        #[arg(long)]
        org: Option<i64>,
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
    /// Link this hub to another fleet's hub, list links, or remove one.
    Peer {
        #[command(subcommand)]
        cmd: PeerCmd,
        #[command(flatten)]
        opts: HubOptions,
    },
    /// Add, test and remove issue trackers (Jira Cloud). Needs a running hub.
    ///
    /// The API token is read from stdin, `--from-env` or a `--ref`, never
    /// from an argument.
    Tracker {
        #[command(subcommand)]
        cmd: tracker::TrackerCmd,
        #[command(flatten)]
        opts: HubOptions,
    },
    /// Read-only work graph administration (work graph M13.2): `usage`
    /// counts. Needs a running hub.
    Work {
        #[command(subcommand)]
        cmd: work::WorkCmd,
        #[command(flatten)]
        opts: HubOptions,
    },
    /// Point the hub at its asset catalog (a git checkout on this machine),
    /// show it, or reload it. No running hub needed; a running one picks the
    /// change up at its next catalog call.
    Catalog {
        #[command(subcommand)]
        cmd: catalog::CatalogCmd,
        #[command(flatten)]
        opts: HubOptions,
    },
    /// Local measurements for the Jev evaluation: counts only, read straight
    /// from the database. No running hub needed; nothing is written or sent.
    Census {
        #[command(subcommand)]
        cmd: census::CensusCmd,
        #[command(flatten)]
        opts: HubOptions,
    },
    /// The decision model (Jev evaluation, experimental, off by default):
    /// set or clear its API key, and read what it was asked. `status` and
    /// `runs` read the database only; the key is never printed or an
    /// argument. See docs/decisions.md.
    Decide {
        #[command(subcommand)]
        cmd: decide::DecideCmd,
        #[command(flatten)]
        opts: HubOptions,
    },
    /// Name organisations, their placement rules, and which org each host
    /// and tracker belongs to (work graph M5). Needs a running hub.
    ///
    /// A host's org is its token's boundary: that host's Claude reads only
    /// its org's and unassigned work.
    Org {
        #[command(subcommand)]
        cmd: org::OrgCmd,
        #[command(flatten)]
        opts: HubOptions,
    },
    /// Host administration (merge a renamed alias). Needs a running hub.
    Host {
        #[command(subcommand)]
        cmd: host::HostCmd,
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
    /// Provision every active host (or one) from this hub: skills, the
    /// managed CLAUDE.md block, hooks and the MCP entry. Needs a running hub.
    Provision {
        /// One host; every active host when omitted.
        #[arg(long)]
        host: Option<String>,
        /// Skills, CLAUDE.md block and hooks only (no token, no ~/.claude.json).
        #[arg(long)]
        content_only: bool,
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
        /// Also require readiness (store migrated, listener bound, first reconcile done) from the
        /// running serve's readiness file, and check its build identity. For fleet-updater.
        #[arg(long)]
        ready: bool,
        /// With --ready: print the verdict and the build identity as JSON (always, even when not ready).
        #[arg(long, requires = "ready")]
        json: bool,
        /// Where serve keeps its readiness file [env: FLEET_HUB_DATA_DIR]
        #[arg(long)]
        data_dir: Option<std::path::PathBuf>,
    },
    /// Take a consistent online copy of state.db (read-only; never migrates). Safe while the hub serves.
    Backup {
        /// Write the copy here (refuses to overwrite) [default: <data dir>/backups/<prefix>-<UTC stamp>.db]
        #[arg(long)]
        to: Option<std::path::PathBuf>,
        /// File name prefix for the default path, [A-Za-z0-9._-]+ (fleet-updater uses pre-<version>).
        #[arg(long, default_value = "manual")]
        prefix: String,
        /// Print {path, schema, bytes} as JSON.
        #[arg(long)]
        json: bool,
        #[command(flatten)]
        opts: HubOptions,
    },
}

#[derive(Subcommand)]
enum TokenCmd {
    Show,
    Regenerate,
}

/// What `fleet-hub client grant` can give a paired client.
#[derive(Clone, Copy, Debug, clap::ValueEnum)]
pub enum Grant {
    /// The asset catalog: the hub's `catalog_admin` tool.
    Assets,
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
    /// Bind a client to one org (its id): it reads only that org's work and sessions.
    Bind { name: String, org: i64 },
    /// Let a paired client do what is otherwise the master's. `assets`: manage
    /// the asset catalog (edit, commit, push, Sync, Secrets, layers) from its
    /// Assets tab. Only a `full` client bound to no org. No running hub
    /// needed.
    Grant { name: String, grant: Grant },
    /// Take a `grant` back.
    Ungrant { name: String, grant: Grant },
    /// Lift a client's org binding: it reads every org again.
    Unbind { name: String },
}

#[derive(Subcommand)]
enum PeerCmd {
    /// Dial another hub with a pairing code its operator made with
    /// `fleet-hub pair --mode peer`. This hub then keeps the link open.
    Add {
        /// The other hub's URL (https://…).
        url: String,
        /// The single-use pairing code.
        code: String,
        /// Allow a plain http:// URL; loopback only (for a local test).
        #[arg(long)]
        insecure: bool,
    },
    /// Print this hub's links, one per line (never a token).
    List,
    /// Remove a link by fleet id or link id; waiting messages fail back to their senders.
    Remove { target: String },
}

#[tokio::main]
async fn main() -> ExitCode {
    // Mandatory and first: fleet-core keeps no default app version, so
    // `app_version::get()` — `fleet_health`, the agent handshake, the MCP
    // hello, the outbound User-Agent, the diagnostics bundle — panics until
    // this line has run. This crate's version is the hub's.
    //
    // NOT /healthz: that route answers a fixed `fleet-hub ok` and names no
    // version on purpose (see `mcp::HEALTHZ_BODY`), which is what lets it sit
    // outside the token and the Host allowlist. Reading the running version
    // back out of a container is `fleet-hub --version` (clap, from this same
    // constant) or an authenticated `fleet_health` — docs/hub.md, "Upgrade
    // and rollback".
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
            org,
            opts,
        } => pair::pair(&opts, &env, &name, mode.as_deref(), ttl, trusted, org).await,
        Cmd::Client { cmd, opts } => match cmd {
            ClientCmd::List { include_revoked } => {
                pair::client_list(&opts, &env, include_revoked).await
            }
            ClientCmd::Revoke { name } => pair::client_revoke(&opts, &env, &name).await,
            ClientCmd::Trust { name } => pair::client_trust(&opts, &env, &name, true).await,
            ClientCmd::Untrust { name } => pair::client_trust(&opts, &env, &name, false).await,
            ClientCmd::Bind { name, org } => pair::client_bind(&opts, &env, &name, Some(org)).await,
            ClientCmd::Unbind { name } => pair::client_bind(&opts, &env, &name, None).await,
            ClientCmd::Grant { name, grant } => pair::client_grant(&opts, &env, &name, grant, true),
            ClientCmd::Ungrant { name, grant } => {
                pair::client_grant(&opts, &env, &name, grant, false)
            }
        },
        Cmd::Peer { cmd, opts } => match cmd {
            PeerCmd::Add {
                url,
                code,
                insecure,
            } => peer::add(&opts, &env, &url, &code, insecure).await,
            PeerCmd::List => peer::list(&opts, &env),
            PeerCmd::Remove { target } => peer::remove(&opts, &env, &target),
        },
        Cmd::Tracker { cmd, opts } => tracker::run(cmd, &opts, &env).await,
        Cmd::Org { cmd, opts } => org::run(cmd, &opts, &env).await,
        Cmd::Host { cmd, opts } => host::run(cmd, &opts, &env).await,
        Cmd::Provision {
            host,
            content_only,
            opts,
        } => provision::run(host, content_only, &opts, &env).await,
        Cmd::Work { cmd, opts } => work::run(cmd, &opts, &env).await,
        Cmd::Catalog { cmd, opts } => catalog::run(cmd, &opts, &env),
        Cmd::Census { cmd, opts } => census::run(cmd, &opts, &env),
        Cmd::Decide { cmd, opts } => decide::run(cmd, &opts, &env).await,
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
        Cmd::Healthcheck {
            port,
            tls,
            ready,
            json,
            data_dir,
        } => {
            if ready {
                serve::healthcheck_ready(port, tls, data_dir, json, &env).await
            } else {
                serve::healthcheck(port, tls, &env).await
            }
        }
        Cmd::Backup {
            to,
            prefix,
            json,
            opts,
        } => serve::backup(&opts, &env, to, &prefix, json),
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
        // Host identity & health, task 5: the alias merge.
        let Cmd::Host { cmd, .. } =
            Cli::try_parse_from(["fleet-hub", "host", "merge", "local", "mac"])
                .unwrap()
                .cmd
        else {
            panic!("host merge parses")
        };
        assert!(
            matches!(cmd, host::HostCmd::Merge { ref from, ref into } if from == "local" && into == "mac")
        );
        // Task 6: the content-only re-provision of one host.
        let Cmd::Provision {
            host, content_only, ..
        } = Cli::try_parse_from([
            "fleet-hub",
            "provision",
            "--host",
            "mefistos",
            "--content-only",
        ])
        .unwrap()
        .cmd
        else {
            panic!("provision parses")
        };
        assert_eq!((host.as_deref(), content_only), (Some("mefistos"), true));
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
        // `decide set-key --data-dir`: how a standalone desktop's key is set
        // (docs/decisions.md, "The key on a standalone desktop").
        for argv in [
            ["fleet-hub", "decide", "set-key", "--data-dir", "/tmp/x"],
            ["fleet-hub", "decide", "--data-dir", "/tmp/x", "set-key"],
        ] {
            let Cmd::Decide { cmd, opts } = Cli::try_parse_from(argv).unwrap().cmd else {
                panic!("{argv:?} did not parse as decide");
            };
            assert!(matches!(cmd, decide::DecideCmd::SetKey { .. }), "{argv:?}");
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
        // `--org` binds the pairing to an org (work graph M14).
        let Cmd::Pair { org, .. } =
            Cli::try_parse_from(["fleet-hub", "pair", "--name", "phone", "--org", "2"])
                .unwrap()
                .cmd
        else {
            panic!("pair --org did not parse");
        };
        assert_eq!(org, Some(2));
        let Cmd::Client { cmd, .. } =
            Cli::try_parse_from(["fleet-hub", "client", "bind", "phone", "2"])
                .unwrap()
                .cmd
        else {
            panic!("client bind did not parse");
        };
        assert!(matches!(cmd, ClientCmd::Bind { name, org: 2 } if name == "phone"));
        let Cmd::Client { cmd, .. } =
            Cli::try_parse_from(["fleet-hub", "client", "unbind", "phone"])
                .unwrap()
                .cmd
        else {
            panic!("client unbind did not parse");
        };
        assert!(matches!(cmd, ClientCmd::Unbind { name } if name == "phone"));
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
        // `peer add|list|remove`, HubOptions in either position like `client`.
        Cli::try_parse_from(["fleet-hub", "peer", "add", "https://b.example", "CODE"]).unwrap();
        Cli::try_parse_from(["fleet-hub", "peer", "list"]).unwrap();
        Cli::try_parse_from(["fleet-hub", "peer", "remove", "fleet-b"]).unwrap();
        for argv in [
            ["fleet-hub", "peer", "list", "--data-dir", "/tmp/x"],
            ["fleet-hub", "peer", "--data-dir", "/tmp/x", "list"],
        ] {
            let Cmd::Peer { cmd, opts } = Cli::try_parse_from(argv).unwrap().cmd else {
                panic!("{argv:?} did not parse as peer");
            };
            assert!(matches!(cmd, PeerCmd::List), "{argv:?}");
            assert_eq!(opts.data_dir, Some("/tmp/x".into()), "{argv:?}");
        }
        let Cmd::Peer { cmd, .. } = Cli::try_parse_from([
            "fleet-hub",
            "peer",
            "add",
            "http://127.0.0.1:7788",
            "CODE",
            "--insecure",
        ])
        .unwrap()
        .cmd
        else {
            panic!("peer add did not parse");
        };
        assert!(matches!(
            cmd,
            PeerCmd::Add {
                url, code, insecure
            } if url == "http://127.0.0.1:7788" && code == "CODE" && insecure
        ));
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
        let Cmd::Healthcheck { port, tls, .. } = Cli::try_parse_from([
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
        let Cmd::Healthcheck {
            ready,
            json,
            data_dir,
            ..
        } = Cli::try_parse_from([
            "fleet-hub",
            "healthcheck",
            "--ready",
            "--json",
            "--data-dir",
            "/var/lib/fleet-hub",
        ])
        .unwrap()
        .cmd
        else {
            panic!("healthcheck --ready did not parse");
        };
        assert!(ready && json);
        assert_eq!(data_dir, Some("/var/lib/fleet-hub".into()));
        assert!(
            Cli::try_parse_from(["fleet-hub", "healthcheck", "--json"]).is_err(),
            "--json needs --ready"
        );
        let Cmd::Backup {
            to, prefix, json, ..
        } = Cli::try_parse_from(["fleet-hub", "backup", "--prefix", "pre-0.3.4", "--json"])
            .unwrap()
            .cmd
        else {
            panic!("backup did not parse");
        };
        assert_eq!((to, prefix.as_str(), json), (None, "pre-0.3.4", true));
        assert!(Cli::try_parse_from(["fleet-hub", "bogus"]).is_err());
    }
}
