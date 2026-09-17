//! `fleet-hub` — claude-fleet without the desktop app. See `docs/hub.md`.

mod config;
mod out;
mod pair;
mod serve;

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
    /// Print this hub's SSH public key (generated on first use; derived when only the private key exists).
    SshKey,
    /// Exit 0 when a hub answers HTTP on 127.0.0.1 (for Docker HEALTHCHECK). Does not open the database.
    Healthcheck {
        /// Port to probe [env: FLEET_HUB_PORT] [default: 4180]
        #[arg(long)]
        port: Option<u16>,
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
}

#[tokio::main]
async fn main() -> ExitCode {
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
        Cmd::Pair {
            name,
            mode,
            ttl,
            opts,
        } => pair::pair(&opts, &env, &name, mode.as_deref(), ttl).await,
        Cmd::Client { cmd, opts } => match cmd {
            ClientCmd::List { include_revoked } => {
                pair::client_list(&opts, &env, include_revoked).await
            }
            ClientCmd::Revoke { name } => pair::client_revoke(&opts, &env, &name).await,
        },
        Cmd::SshKey => serve::ssh_key(),
        Cmd::Healthcheck { port } => serve::healthcheck(port, &env).await,
    };
    match result {
        Ok(code) => code,
        Err(msg) => {
            out::error(&msg);
            ExitCode::from(1)
        }
    }
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
        Cli::try_parse_from(["fleet-hub", "token", "show"]).unwrap();
        Cli::try_parse_from(["fleet-hub", "token", "regenerate"]).unwrap();
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
        // `pair` and `client …`, including HubOptions before or after the
        // subcommand (they are `global = true`).
        let Cmd::Pair {
            name, mode, ttl, ..
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
            (name.as_str(), mode.as_deref(), ttl),
            ("phone", Some("readonly"), Some(120))
        );
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
        Cli::try_parse_from(["fleet-hub", "ssh-key"]).unwrap();
        Cli::try_parse_from(["fleet-hub", "healthcheck"]).unwrap();
        let Cmd::Healthcheck { port } =
            Cli::try_parse_from(["fleet-hub", "healthcheck", "--port", "4190"])
                .unwrap()
                .cmd
        else {
            panic!("healthcheck --port did not parse");
        };
        assert_eq!(port, Some(4190));
        assert!(Cli::try_parse_from(["fleet-hub", "bogus"]).is_err());
    }
}
