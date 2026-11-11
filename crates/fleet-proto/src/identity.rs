//! The agent's local identity file: which hub alias THIS machine is.
//!
//! `fleet-agent` writes it when the hub's `welcome` names the alias the hub
//! knows the host by (`HubFrame::Welcome::host_alias`). A desktop app running
//! on the same machine reads it, so a session on that alias is attached with
//! a local `tmux attach` instead of `ssh <alias>`: an agent host has no SSH
//! route from anywhere, including from itself once it has left the network
//! its SSH alias points into (a laptop carried from home to work).
//!
//! Both sides compute the path from `$HOME` alone, the same on Linux and
//! macOS, so neither needs the other's environment (`XDG_STATE_HOME` is not
//! consulted: a launchd agent and a GUI app do not share one).
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Where the file lives, relative to `$HOME`.
pub const RELATIVE_PATH: &str = ".local/state/fleet-agent/identity.json";

/// The longest alias the file may carry; anything longer is not one a hub
/// hands out.
pub const MAX_ALIAS_LEN: usize = 64;

/// The file's content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalIdentity {
    /// The alias the hub knows this machine by.
    pub host_alias: String,
    /// The hub that said so (its `/agent` URL), for an operator reading the
    /// file; nothing compares it.
    #[serde(default)]
    pub hub: String,
}

/// `$HOME/.local/state/fleet-agent/identity.json`.
pub fn path(home: &Path) -> PathBuf {
    home.join(RELATIVE_PATH)
}

/// An alias as the file may carry it: what a host alias can be, and nothing
/// a shell, a tmux target or a path would read specially.
pub fn valid_alias(alias: &str) -> bool {
    !alias.is_empty()
        && alias.len() <= MAX_ALIAS_LEN
        && !alias.starts_with('-')
        && alias
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
}

/// Read the file under `home`. `None` when it is missing, unreadable, not
/// JSON, or names an alias [`valid_alias`] refuses.
pub fn read(home: &Path) -> Option<LocalIdentity> {
    let text = std::fs::read_to_string(path(home)).ok()?;
    let id: LocalIdentity = serde_json::from_str(&text).ok()?;
    valid_alias(&id.host_alias).then_some(id)
}

/// Write the file under `home`, replacing it atomically (a temp file renamed
/// over it), so a reader never sees half of one.
pub fn write(home: &Path, id: &LocalIdentity) -> std::io::Result<()> {
    if !valid_alias(&id.host_alias) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("{:?} is not a host alias", id.host_alias),
        ));
    }
    let target = path(home);
    if let Some(dir) = target.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let body = serde_json::to_string_pretty(id).map_err(std::io::Error::other)?;
    let tmp = target.with_extension("json.tmp");
    std::fs::write(&tmp, body + "\n")?;
    std::fs::rename(&tmp, &target)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "fleet-proto-identity-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn what_is_written_reads_back() {
        let h = home();
        assert_eq!(read(&h), None);
        let id = LocalIdentity {
            host_alias: "mac".into(),
            hub: "wss://hub.example/agent".into(),
        };
        write(&h, &id).unwrap();
        assert_eq!(read(&h), Some(id));
        assert!(h.join(RELATIVE_PATH).is_file());
        let _ = std::fs::remove_dir_all(&h);
    }

    #[test]
    fn an_alias_that_is_not_one_is_neither_written_nor_read() {
        let h = home();
        for bad in [
            "",
            "-oProxyCommand=x",
            "a b",
            "mac;rm",
            "../x",
            &"a".repeat(65),
        ] {
            let id = LocalIdentity {
                host_alias: bad.to_string(),
                hub: String::new(),
            };
            assert!(write(&h, &id).is_err(), "{bad:?}");
        }
        std::fs::create_dir_all(path(&h).parent().unwrap()).unwrap();
        std::fs::write(path(&h), r#"{"host_alias":"mac;rm -rf ~"}"#).unwrap();
        assert_eq!(read(&h), None);
        std::fs::write(path(&h), "not json").unwrap();
        assert_eq!(read(&h), None);
        let _ = std::fs::remove_dir_all(&h);
    }

    #[test]
    fn plain_aliases_pass() {
        for ok in ["mac", "local", "build-box", "mercury.lan", "laptop_2"] {
            assert!(valid_alias(ok), "{ok}");
        }
    }
}
