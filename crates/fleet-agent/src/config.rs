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
    /// Send this agent's error-level log events to the hub on each
    /// heartbeat.
    #[serde(default = "default_report_errors")]
    pub report_errors: bool,
}

pub fn default_report_errors() -> bool {
    true
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
    let io = |e| ConfigError::Io(path.into(), e);
    let dir = match path.parent() {
        Some(d) if !d.as_os_str().is_empty() => d,
        _ => Path::new("."),
    };
    // Written for oneself (the user scope): private. Written by root for the
    // run-as user (the system scope): root owns what it creates, so the
    // directory must let that user through, or the agent can never open its
    // own config. The file is 0600 and that user's either way; the directory
    // only has to be passable.
    let created = create_dirs(dir, if owner.is_some() { 0o755 } else { 0o700 }).map_err(io)?;
    let result = write_into(dir, path, config, owner);
    if result.is_err() {
        // A refused or failed write leaves no directory of ours behind.
        for d in created.iter().rev() {
            let _ = std::fs::remove_dir(d);
        }
    }
    result
}

/// Create every missing directory on the way to `dir`, each set to exactly
/// `mode`, and return them, outermost first. The mode is applied explicitly
/// after the create: `DirBuilder::mode` is filtered by the umask, and under
/// a CIS-style 027 (which `sudo` keeps) the system directory came out 0750,
/// which the run-as user cannot pass (the re-review's NEW-3). Directories
/// only: this never opens a file.
fn create_dirs(dir: &Path, mode: u32) -> std::io::Result<Vec<std::path::PathBuf>> {
    let missing: Vec<&Path> = dir
        .ancestors()
        .filter(|d| !d.as_os_str().is_empty())
        .take_while(|d| !d.exists())
        .collect();
    let mut created = Vec::new();
    for d in missing.into_iter().rev() {
        std::fs::create_dir(d)?;
        created.push(d.to_path_buf());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(d, std::fs::Permissions::from_mode(mode))?;
        }
    }
    #[cfg(not(unix))]
    let _ = mode;
    Ok(created)
}

/// [`write`], once the directory exists.
fn write_into(
    dir: &Path,
    path: &Path,
    config: &Config,
    owner: Option<(u32, u32)>,
) -> Result<(), ConfigError> {
    use std::io::Write as _;
    let io = |e| ConfigError::Io(path.into(), e);
    #[cfg(unix)]
    if let Some((uid, gid)) = owner {
        reachable_by(dir, uid, gid).map_err(ConfigError::Invalid)?;
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

/// Can `uid`/`gid` search every directory on the way to `dir`? Checked before
/// the token is written, so an install that would leave the agent unable to
/// open its config fails instead of producing a unit that restarts forever.
/// Supplementary groups are not consulted: a false alarm here costs a
/// clearer path, a miss costs a crash-looping service.
#[cfg(unix)]
fn reachable_by(dir: &Path, uid: u32, gid: u32) -> Result<(), String> {
    use std::os::unix::fs::MetadataExt;
    if uid == 0 {
        return Ok(());
    }
    for d in dir.ancestors().filter(|d| !d.as_os_str().is_empty()) {
        let md = std::fs::metadata(d).map_err(|e| format!("{}: {e}", d.display()))?;
        let mode = md.mode();
        let search = if md.uid() == uid {
            mode & 0o100
        } else if md.gid() == gid {
            mode & 0o010
        } else {
            mode & 0o001
        };
        if search == 0 {
            return Err(format!(
                "the agent's user (uid {uid}) cannot reach {}: {} is mode {:04o} and \
                 not theirs. Choose another --config, or make that directory passable",
                dir.display(),
                d.display(),
                mode & 0o7777
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// THE GATE on the config's 0600-at-creation rule. The final mode of a
    /// file chmodded after its token was written is identical to one created
    /// 0600, so no test that inspects the result can see the difference;
    /// this one reads the source instead. Every file this module creates must
    /// come from `create_private`, which passes the mode to the create call
    /// itself, and nothing outside it may open, create, write or chmod a file.
    #[test]
    fn the_secret_file_is_only_ever_created_by_create_private() {
        use crate::test_util::{fn_body, production};
        let src = production(include_str!("config.rs"));
        let helper = fn_body(src, "create_private");
        assert!(
            helper.contains("create_new(true)") && helper.contains(".mode(0o600)"),
            "create_private must create exclusively AND pass 0600 to the create call"
        );
        assert!(
            !helper.contains("set_permissions"),
            "create_private must not chmod: the mode belongs to the create call"
        );
        // The one other place a mode is set: directories, never a file.
        let dirs = fn_body(src, "create_dirs");
        for file_api in ["File", "OpenOptions", "fs::write", "fs::copy"] {
            assert!(
                !dirs.contains(file_api),
                "create_dirs must only make directories, but uses `{file_api}`"
            );
        }
        let rest = src.replacen(helper, "", 1).replacen(dirs, "", 1);
        for forbidden in [
            "File::create",
            "File::options",
            "OpenOptions",
            "fs::write",
            "fs::copy",
            "set_permissions",
            "Permissions::from_mode",
            "libc::",
        ] {
            assert!(
                !rest.contains(forbidden),
                "config.rs uses `{forbidden}` outside create_private: route every \
                 secret-bearing file through create_private"
            );
        }
        assert!(
            fn_body(src, "write_into").contains("create_private(&tmp)"),
            "write() must create its temp file with create_private"
        );
    }

    fn cfg() -> Config {
        Config {
            hub: "https://hub.example".into(),
            token: "t0k3n".into(),
            insecure: false,
            ca_file: None,
            report_errors: true,
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

    fn me() -> (u32, u32) {
        // SAFETY: no preconditions.
        unsafe { (libc::getuid(), libc::getgid()) }
    }

    /// The system scope: root writes the config for ANOTHER user. The file
    /// is that user's and 0600, and the directory root creates for it must
    /// let that user through — a root-owned 0700 directory left the agent
    /// unable to open its own config, restarting every five seconds.
    #[test]
    fn a_directory_created_for_another_owner_lets_that_owner_reach_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("etc-fleet-agent/config.json");
        write(&path, &cfg(), Some(me())).unwrap();
        assert_eq!(mode_of(path.parent().unwrap()), 0o755);
        assert_eq!(mode_of(&path), 0o600, "the file stays private");
    }

    /// A user's own config directory stays private.
    #[test]
    fn a_directory_created_for_oneself_stays_private() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fleet-agent/config.json");
        write(&path, &cfg(), None).unwrap();
        assert_eq!(mode_of(path.parent().unwrap()), 0o700);
    }

    /// An existing directory the owner cannot pass through is refused BEFORE
    /// the token is written, naming the directory — rather than installing
    /// an agent that can never read its config.
    #[test]
    fn a_config_its_owner_could_not_reach_is_refused_before_it_is_written() {
        let dir = tempfile::tempdir().unwrap();
        let locked = dir.path().join("locked");
        std::fs::create_dir(&locked).unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o700)).unwrap();
        let path = locked.join("config.json");
        // Someone who is neither this directory's owner nor in its group.
        let stranger = (me().0.wrapping_add(4242), me().1.wrapping_add(4242));
        let err = write(&path, &cfg(), Some(stranger))
            .unwrap_err()
            .to_string();
        assert!(
            err.contains(locked.to_str().unwrap()) && err.contains("cannot reach"),
            "{err}"
        );
        assert_eq!(
            std::fs::read_dir(&locked).unwrap().count(),
            0,
            "nothing written"
        );
        // Its own owner is fine.
        write(&path, &cfg(), Some(me())).unwrap();
    }

    /// NEW-3 (re-review): the directory modes above must not depend on the
    /// umask. `DirBuilder::mode` is filtered by it, so under a CIS-style 027
    /// (which `sudo` keeps) the system directory came out 0750, the run-as
    /// user could not pass, and every system install was refused.
    ///
    /// The umask is process-wide, so the test re-runs this binary, filtered
    /// to itself, and the inner run sets it.
    #[cfg(target_os = "linux")]
    #[test]
    fn the_directory_modes_do_not_depend_on_the_umask() {
        const PROBE: &str = "FLEET_AGENT_UMASK_PROBE";
        const NAME: &str = "config::tests::the_directory_modes_do_not_depend_on_the_umask";
        if std::env::var_os(PROBE).is_some() {
            for umask in [0o027, 0o077] {
                // SAFETY: no preconditions; this inner run is one thread.
                unsafe { libc::umask(umask) };
                let dir = tempfile::tempdir().unwrap();
                let system = dir.path().join("etc/fleet-agent/config.json");
                write(&system, &cfg(), Some(me())).unwrap();
                assert_eq!(
                    (mode_of(system.parent().unwrap()), mode_of(&system)),
                    (0o755, 0o600),
                    "system scope under umask {umask:04o}"
                );
                let user = dir.path().join("home/fleet-agent/config.json");
                write(&user, &cfg(), None).unwrap();
                assert_eq!(
                    mode_of(user.parent().unwrap()),
                    0o700,
                    "user scope under umask {umask:04o}"
                );
            }
            return;
        }
        let out = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", NAME, "--nocapture", "--test-threads=1"])
            .env(PROBE, "1")
            .output()
            .unwrap();
        let said = String::from_utf8_lossy(&out.stdout);
        assert!(
            out.status.success(),
            "{said}{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            said.contains("1 passed"),
            "the inner run ran the probe: {said}"
        );
    }

    /// NEW-3 (re-review): an install refused because its owner could not
    /// reach the config leaves no directory it created behind.
    #[test]
    fn a_refused_write_removes_the_directories_it_created() {
        let dir = tempfile::tempdir().unwrap();
        let locked = dir.path().join("locked");
        std::fs::create_dir(&locked).unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o700)).unwrap();
        let path = locked.join("new/deeper/config.json");
        let stranger = (me().0.wrapping_add(4242), me().1.wrapping_add(4242));
        let err = write(&path, &cfg(), Some(stranger))
            .unwrap_err()
            .to_string();
        assert!(err.contains("cannot reach"), "{err}");
        assert!(
            !locked.join("new").exists(),
            "left behind: {:?}",
            std::fs::read_dir(&locked).unwrap().collect::<Vec<_>>()
        );
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

    #[test]
    fn report_errors_defaults_on_and_reads_back() {
        let c: Config = serde_json::from_str(r#"{"hub":"https://h","token":"t"}"#).unwrap();
        assert!(c.report_errors);
        let c: Config =
            serde_json::from_str(r#"{"hub":"https://h","token":"t","report_errors":false}"#)
                .unwrap();
        assert!(!c.report_errors);
    }
}
