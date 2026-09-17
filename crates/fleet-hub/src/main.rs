//! `fleet-hub` — claude-fleet without the desktop app. See `docs/hub.md`.

mod config;
mod out;
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
    /// Print this hub's SSH public key (generated on first use).
    SshKey,
}

#[derive(Subcommand)]
enum TokenCmd {
    Show,
    Regenerate,
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
        Cmd::SshKey => serve::ssh_key(),
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
        Cli::try_parse_from(["fleet-hub", "ssh-key"]).unwrap();
        assert!(Cli::try_parse_from(["fleet-hub", "bogus"]).is_err());
    }
}
