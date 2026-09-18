//! The one file the agent keeps: which hub, and the token to show it.
//!
//! The token is the host's per-host bearer token, so the file is a secret. It
//! is created `0600` — at creation, never widened and then narrowed, because
//! an earlier sub-project found exactly that window in a token file — and
//! `run` refuses one that anyone but its owner can read.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// What `install` writes and `run --config` reads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config {
    /// `https://…` (or `wss://…`); `http://`/`ws://` only with `insecure`.
    pub hub: String,
    pub token: String,
    #[serde(default)]
    pub insecure: bool,
    /// A PEM bundle to trust instead of the host's system roots — for a hub
    /// with a private CA.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ca_file: Option<PathBuf>,
}

/// Why a config was refused.
#[derive(Debug)]
pub enum ConfigError {
    Io(PathBuf, std::io::Error),
    Parse(PathBuf, String),
    /// The file can be read by someone other than its owner.
    Exposed(PathBuf, u32),
    Invalid(String),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(p, e) => write!(f, "{}: {e}", p.display()),
            Self::Parse(p, e) => write!(f, "{}: not a fleet-agent config: {e}", p.display()),
            Self::Exposed(p, mode) => write!(
                f,
                "{} holds the hub token but is mode {mode:04o}; run `chmod 600 {}`",
                p.display(),
                p.display()
            ),
            Self::Invalid(why) => f.write_str(why),
        }
    }
}

impl std::error::Error for ConfigError {}

/// A token has to travel as an HTTP header value, and it must not be empty.
pub fn check_token(token: &str) -> Result<(), ConfigError> {
    if token.is_empty() {
        return Err(ConfigError::Invalid("the token is empty".into()));
    }
    // Visible ASCII: what a bearer token is, and what cannot smuggle a second
    // header or a line break into the upgrade request.
    if !token.bytes().all(|b| b.is_ascii_graphic()) {
        return Err(ConfigError::Invalid(
            "the token has a space, a control character or non-ASCII in it".into(),
        ));
    }
    Ok(())
}

/// Read and check a config.
pub fn load(path: &Path) -> Result<Config, ConfigError> {
    let file = std::fs::File::open(path).map_err(|e| ConfigError::Io(path.into(), e))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = file
            .metadata()
            .map_err(|e| ConfigError::Io(path.into(), e))?
            .permissions()
            .mode()
            & 0o777;
        if mode & 0o077 != 0 {
            return Err(ConfigError::Exposed(path.into(), mode));
        }
    }
    let config: Config = serde_json::from_reader(std::io::BufReader::new(file))
        .map_err(|e| ConfigError::Parse(path.into(), e.to_string()))?;
    check_token(&config.token)?;
    Ok(config)
}

/// Create `path` for writing, `0600` from its first instant. Fails if it
/// already exists: callers write a fresh file and rename it into place.
pub fn create_private(path: &Path) -> std::io::Result<std::fs::File> {
    let mut open = std::fs::OpenOptions::new();
    open.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        open.mode(0o600);
    }
    // The mode is part of the create call itself: no chmod follows, so there
    // is no instant at which the file exists wider. A umask can only narrow
    // it.
    open.open(path)
}

/// Write `config` to `path`, replacing whatever is there, never readable by
/// anyone but `owner` (the current user when `None`) at any instant.
///
/// A sibling temp file is created 0600, handed to `owner`, filled, synced,
/// and renamed over `path` — so an existing file, whatever its mode, never
/// receives the token, and a crash leaves either the old file or the new one.
pub fn write(path: &Path, config: &Config, owner: Option<(u32, u32)>) -> Result<(), ConfigError> {
    use std::io::Write as _;
    let io = |e| ConfigError::Io(path.into(), e);
    let dir = match path.parent() {
        Some(d) if !d.as_os_str().is_empty() => d,
        _ => Path::new("."),
    };
    if !dir.exists() {
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(dir).map_err(io)?;
    }
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "config".into());
    let tmp = dir.join(format!(".{name}.{}.tmp", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    let result = (|| {
        let mut file = create_private(&tmp)?;
        #[cfg(unix)]
        if let Some((uid, gid)) = owner {
            std::os::unix::fs::fchown(&file, Some(uid), Some(gid))?;
        }
        #[cfg(not(unix))]
        let _ = owner;
        let mut body = serde_json::to_vec_pretty(config).map_err(std::io::Error::other)?;
        body.push(b'\n');
        file.write_all(&body)?;
        file.sync_all()?;
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result.map_err(io)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn cfg() -> Config {
        Config {
            hub: "https://hub.example".into(),
            token: "t0k3n".into(),
            insecure: false,
            ca_file: None,
        }
    }

    fn mode_of(path: &Path) -> u32 {
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    /// The file is 0600 the moment it exists — before a byte of the token is
    /// in it — whatever the process umask would have given it.
    #[test]
    fn a_private_file_is_0600_from_creation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("c");
        let file = create_private(&path).unwrap();
        assert_eq!(mode_of(&path), 0o600, "nothing has been written yet");
        drop(file);
    }

    #[test]
    fn a_private_file_is_never_created_over_an_existing_one() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("c");
        std::fs::write(&path, b"x").unwrap();
        assert!(create_private(&path).is_err());
    }

    #[test]
    fn the_config_is_written_0600_and_reads_back() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        write(&path, &cfg(), None).unwrap();
        assert_eq!(mode_of(&path), 0o600);
        assert_eq!(load(&path).unwrap(), cfg());
    }

    /// Re-running install over a config someone widened replaces the file
    /// rather than writing the new token into the wide one.
    #[test]
    fn rewriting_a_wide_config_replaces_it_with_a_private_one() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(&path, b"{}").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        write(&path, &cfg(), None).unwrap();
        assert_eq!(mode_of(&path), 0o600);
        assert_eq!(load(&path).unwrap(), cfg());
        let left: Vec<_> = std::fs::read_dir(dir.path()).unwrap().collect();
        assert_eq!(left.len(), 1, "no temp file left behind: {left:?}");
    }

    #[test]
    fn a_config_the_group_or_others_can_read_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        write(&path, &cfg(), None).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();
        match load(&path) {
            Err(ConfigError::Exposed(_, mode)) => assert_eq!(mode, 0o640),
            other => panic!("expected Exposed, got {other:?}"),
        }
    }

    #[test]
    fn a_token_must_be_a_non_empty_header_value() {
        assert!(check_token("abc-123_XYZ.=").is_ok());
        assert!(check_token("").is_err());
        assert!(check_token("two words").is_err());
        assert!(check_token("line\nbreak").is_err());
        assert!(check_token("naïve").is_err());
    }

    #[test]
    fn a_config_with_a_bad_token_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let mut c = cfg();
        c.token = "has space".into();
        write(&path, &c, None).unwrap();
        assert!(matches!(load(&path), Err(ConfigError::Invalid(_))));
    }
}
