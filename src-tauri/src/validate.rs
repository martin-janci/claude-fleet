//! Input validation for values that flow from the frontend (or the DB) into
//! shell commands, SSH invocations, and filesystem paths.
//!
//! The command layer must not trust the frontend: DevTools and direct IPC
//! calls bypass any UI-side checks. Every alias / name / path component is
//! validated here before it reaches `ssh`, `git`, `tmux`, or a path string.

use crate::ipc_error::IpcError;

/// Validate an SSH host alias (or `~/.ssh/config` alias).
///
/// `ssh` treats an argument that begins with `-` as an option, so an alias
/// like `-oProxyCommand=<cmd>` would be parsed as an SSH option and achieve
/// arbitrary local command execution. We reject a leading `-`, whitespace,
/// control characters, and anything outside `[A-Za-z0-9._-]`, and cap the
/// length. (`run`/`run_cancellable` additionally pass `--` before the host
/// as belt-and-suspenders.)
pub fn host_alias(alias: &str) -> Result<(), IpcError> {
    if alias.is_empty() {
        return Err(IpcError::new("E_INVALID", "host alias must not be empty"));
    }
    if alias.len() > 255 {
        return Err(IpcError::new("E_INVALID", "host alias is too long"));
    }
    if alias.starts_with('-') {
        return Err(IpcError::new(
            "E_INVALID",
            "host alias must not start with '-'",
        ));
    }
    if !alias
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        return Err(IpcError::new(
            "E_INVALID",
            "host alias may contain only letters, digits, '.', '_' and '-'",
        ));
    }
    Ok(())
}

/// Validate a single filesystem path component — a project owner, repo name,
/// or worktree directory name. Rejects empty, `.`/`..`, path separators, a
/// leading `-`, and control characters, so a crafted value cannot traverse
/// out of the intended directory or be mistaken for a command option.
pub fn path_component(label: &str, value: &str) -> Result<(), IpcError> {
    if value.is_empty() {
        return Err(IpcError::new(
            "E_INVALID",
            format!("{label} must not be empty"),
        ));
    }
    if value == "." || value == ".." {
        return Err(IpcError::new(
            "E_INVALID",
            format!("{label} must not be '.' or '..'"),
        ));
    }
    if value.contains('/') || value.contains('\\') {
        return Err(IpcError::new(
            "E_INVALID",
            format!("{label} must not contain a path separator"),
        ));
    }
    if value.starts_with('-') {
        return Err(IpcError::new(
            "E_INVALID",
            format!("{label} must not start with '-'"),
        ));
    }
    if value.chars().any(|c| c.is_control()) {
        return Err(IpcError::new(
            "E_INVALID",
            format!("{label} must not contain control characters"),
        ));
    }
    Ok(())
}

/// Validate a git ref (branch) name. Branches legitimately contain `/`
/// (`feature/x`), so that is allowed — but a leading `-` would be read as a
/// `git` option, and whitespace / control characters / `..` are rejected.
pub fn git_ref(value: &str) -> Result<(), IpcError> {
    if value.is_empty() {
        return Err(IpcError::new("E_INVALID", "branch must not be empty"));
    }
    if value.starts_with('-') {
        return Err(IpcError::new("E_INVALID", "branch must not start with '-'"));
    }
    if value.contains("..") {
        return Err(IpcError::new("E_INVALID", "branch must not contain '..'"));
    }
    if value.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err(IpcError::new(
            "E_INVALID",
            "branch must not contain whitespace or control characters",
        ));
    }
    Ok(())
}

/// Validate a tmux session name. tmux forbids `.`, `:` and whitespace in
/// session names; we additionally reject control characters and a leading
/// `-` (which a `tmux -t <name>` target would parse as an option).
pub fn tmux_name(value: &str) -> Result<(), IpcError> {
    let v = value.trim();
    if v.is_empty() {
        return Err(IpcError::new("E_INVALID", "session name must not be empty"));
    }
    if v.starts_with('-') {
        return Err(IpcError::new(
            "E_INVALID",
            "session name must not start with '-'",
        ));
    }
    if v.chars()
        .any(|c| c.is_whitespace() || c.is_control() || matches!(c, '.' | ':'))
    {
        return Err(IpcError::new(
            "E_INVALID",
            "session name must not contain whitespace, control characters, '.' or ':'",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_alias_accepts_normal_aliases() {
        for ok in ["local", "mefistos", "host-1", "box.lan", "a_b"] {
            assert!(host_alias(ok).is_ok(), "{ok} should be valid");
        }
    }

    #[test]
    fn host_alias_rejects_option_injection() {
        // The headline attack: an alias parsed by ssh as an option.
        assert!(host_alias("-oProxyCommand=touch /tmp/x").is_err());
        assert!(host_alias("-tt").is_err());
        assert!(host_alias("").is_err());
        assert!(host_alias("has space").is_err());
        assert!(host_alias("semi;colon").is_err());
        assert!(host_alias("new\nline").is_err());
    }

    #[test]
    fn path_component_blocks_traversal() {
        assert!(path_component("repo", "claude-fleet").is_ok());
        assert!(path_component("repo", "..").is_err());
        assert!(path_component("repo", ".").is_err());
        assert!(path_component("owner", "../../etc").is_err());
        assert!(path_component("repo", "a/b").is_err());
        assert!(path_component("repo", "-rf").is_err());
        assert!(path_component("repo", "").is_err());
    }

    #[test]
    fn git_ref_allows_slash_but_blocks_options() {
        assert!(git_ref("main").is_ok());
        assert!(git_ref("feature/login").is_ok());
        assert!(git_ref("--upload-pack=evil").is_err());
        assert!(git_ref("a..b").is_err());
        assert!(git_ref("has space").is_err());
        assert!(git_ref("").is_err());
    }

    #[test]
    fn tmux_name_matches_tmux_rules() {
        assert!(tmux_name("dev-foo").is_ok());
        assert!(tmux_name("  dev-foo  ").is_ok()); // trimmed
        assert!(tmux_name("has space").is_err());
        assert!(tmux_name("has.dot").is_err());
        assert!(tmux_name("has:colon").is_err());
        assert!(tmux_name("-leading").is_err());
        assert!(tmux_name("").is_err());
    }
}
