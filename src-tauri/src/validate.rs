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

/// Validate a git commit hash supplied by the frontend (the commit the user
/// clicked in the History graph). Git object names are lowercase hex; we
/// accept an abbreviated or full SHA-1 (4–40 chars) and nothing else, so the
/// value cannot be read as an option or inject shell/git syntax.
pub fn commit_hash(value: &str) -> Result<(), IpcError> {
    if value.len() < 4 || value.len() > 40 {
        return Err(IpcError::new(
            "E_INVALID",
            "commit hash must be 4–40 characters",
        ));
    }
    if !value
        .chars()
        .all(|c| c.is_ascii_digit() || matches!(c, 'a'..='f'))
    {
        return Err(IpcError::new(
            "E_INVALID",
            "commit hash must be lowercase hexadecimal",
        ));
    }
    Ok(())
}

/// Validate a Claude Code session id (a canonical lowercase UUID,
/// `8-4-4-4-12` hex). The app generates these as UUIDv4 and interpolates them
/// into the pane launch command, so this guards a tampered DB value from
/// injecting shell. Anything not matching the exact shape is rejected.
pub fn claude_session_id(value: &str) -> Result<(), IpcError> {
    let groups = [8usize, 4, 4, 4, 12];
    let parts: Vec<&str> = value.split('-').collect();
    let shape_ok = parts.len() == groups.len()
        && parts.iter().zip(groups).all(|(p, n)| {
            p.len() == n
                && p.chars()
                    .all(|c| c.is_ascii_digit() || matches!(c, 'a'..='f'))
        });
    if shape_ok {
        Ok(())
    } else {
        Err(IpcError::new(
            "E_INVALID",
            "claude session id must be a lowercase UUID",
        ))
    }
}

/// Validate a repo-relative file path supplied by the frontend (the file the
/// user clicked in the Files viewer). The path is joined onto a trusted
/// worktree cwd before being read, so it must not escape that directory:
/// reject empty, absolute paths, a leading `-`, any `..` path component, and
/// control characters. A plain `/` separator is allowed (paths have subdirs).
pub fn repo_rel_path(value: &str) -> Result<(), IpcError> {
    if value.is_empty() {
        return Err(IpcError::new("E_INVALID", "file path must not be empty"));
    }
    if value.len() > 4096 {
        return Err(IpcError::new("E_INVALID", "file path is too long"));
    }
    if value.starts_with('/') || value.starts_with('\\') {
        return Err(IpcError::new(
            "E_INVALID",
            "file path must be relative to the worktree",
        ));
    }
    if value.starts_with('-') {
        return Err(IpcError::new(
            "E_INVALID",
            "file path must not start with '-'",
        ));
    }
    if value.chars().any(|c| c.is_control()) {
        return Err(IpcError::new(
            "E_INVALID",
            "file path must not contain control characters",
        ));
    }
    // Reject `..` as a whole component on either separator — `a/../b`,
    // `../x`, `x/..` all escape the worktree.
    if value.split(['/', '\\']).any(|component| component == "..") {
        return Err(IpcError::new(
            "E_INVALID",
            "file path must not contain a '..' component",
        ));
    }
    Ok(())
}

/// Validate a tmux session name. tmux forbids `.`, `:` and whitespace in
/// session names; we additionally reject control characters and a leading
/// `-` (which a `tmux -t <name>` target would parse as an option).
///
/// The name is checked *verbatim* — leading/trailing whitespace is rejected,
/// not trimmed away. A trimming validator would accept `" foo "` while the
/// rest of the pipeline (tmux create, DB store, `get_session` lookup) uses
/// the un-trimmed value, so a padded name would create a session later
/// commands could not find. The UI trims before calling in; only DevTools /
/// the MCP API reach here with padding, and they get a clear `E_INVALID`.
pub fn tmux_name(value: &str) -> Result<(), IpcError> {
    if value.is_empty() {
        return Err(IpcError::new("E_INVALID", "session name must not be empty"));
    }
    if value.starts_with('-') {
        return Err(IpcError::new(
            "E_INVALID",
            "session name must not start with '-'",
        ));
    }
    if value
        .chars()
        .any(|c| c.is_whitespace() || c.is_control() || matches!(c, '.' | ':'))
    {
        return Err(IpcError::new(
            "E_INVALID",
            "session name must not contain whitespace, control characters, '.' or ':'",
        ));
    }
    Ok(())
}

/// Validate a session friendly-name (display label set by the in-session
/// agent via MCP). Display-only, but still passes through `serde_json` and
/// the row event bus — reject control chars and cap length so a runaway
/// agent can't ship megabytes through the event stream. Empty / whitespace-
/// only is allowed and means "clear" at the service layer.
pub fn friendly_name(value: &str) -> Result<(), IpcError> {
    if value.chars().count() > 80 {
        return Err(IpcError::new(
            "E_INVALID",
            "friendly name must be 80 characters or fewer",
        ));
    }
    if value.chars().any(|c| c.is_control()) {
        return Err(IpcError::new(
            "E_INVALID",
            "friendly name must not contain control characters",
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
    fn repo_rel_path_accepts_normal_paths() {
        for ok in ["README.md", "src/lib/files.ts", "a/b/c/d.rs", ".gitignore"] {
            assert!(repo_rel_path(ok).is_ok(), "{ok} should be valid");
        }
    }

    #[test]
    fn repo_rel_path_blocks_traversal_and_absolute() {
        assert!(repo_rel_path("").is_err());
        assert!(repo_rel_path("/etc/passwd").is_err());
        assert!(repo_rel_path("../secrets").is_err());
        assert!(repo_rel_path("src/../../etc").is_err());
        assert!(repo_rel_path("a/..").is_err());
        assert!(repo_rel_path("-rf").is_err());
        assert!(repo_rel_path("bad\nname").is_err());
        // A `..` only as a substring of a real name is fine.
        assert!(repo_rel_path("src/my..file.ts").is_ok());
    }

    #[test]
    fn commit_hash_accepts_hex_rejects_junk() {
        assert!(commit_hash("a1b2c3d").is_ok());
        assert!(commit_hash("0123456789abcdef0123456789abcdef01234567").is_ok());
        assert!(commit_hash("ABC").is_err()); // uppercase not produced by git short hashes we use
        assert!(commit_hash("xyz").is_err()); // non-hex
        assert!(commit_hash("").is_err());
        assert!(commit_hash("-rf").is_err());
        assert!(commit_hash("123").is_err()); // too short (<4)
        assert!(commit_hash(&"a".repeat(41)).is_err()); // too long (>40)
    }

    #[test]
    fn claude_session_id_accepts_uuid_rejects_junk() {
        assert!(claude_session_id("550e8400-e29b-41d4-a716-446655440000").is_ok());
        assert!(claude_session_id("550E8400-E29B-41D4-A716-446655440000").is_err()); // uppercase
        assert!(claude_session_id("").is_err());
        assert!(claude_session_id("not-a-uuid").is_err());
        assert!(claude_session_id("550e8400e29b41d4a716446655440000").is_err()); // no hyphens
        assert!(claude_session_id("550e8400-e29b-41d4-a716-44665544000g").is_err()); // non-hex
        assert!(claude_session_id("'; rm -rf / #").is_err());
        assert!(claude_session_id(&"a".repeat(36)).is_err()); // right length, wrong shape
    }

    #[test]
    fn tmux_name_matches_tmux_rules() {
        assert!(tmux_name("dev-foo").is_ok());
        assert!(tmux_name("  dev-foo  ").is_err()); // padding rejected, not trimmed
        assert!(tmux_name("dev-foo ").is_err()); // trailing space
        assert!(tmux_name(" dev-foo").is_err()); // leading space
        assert!(tmux_name("has space").is_err());
        assert!(tmux_name("has.dot").is_err());
        assert!(tmux_name("has:colon").is_err());
        assert!(tmux_name("-leading").is_err());
        assert!(tmux_name("").is_err());
    }
}
