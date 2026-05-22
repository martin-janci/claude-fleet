//! Supervised reverse SSH tunnels: expose the central localhost MCP server on
//! each remote host's localhost via `ssh -R`.

/// Build the `ssh` argv for a reverse tunnel that makes the central machine's
/// `127.0.0.1:<mcp_port>` reachable at `127.0.0.1:<remote_port>` on `host`.
/// `-N` (no command), fail fast if the forward can't bind, keepalives so a
/// dropped link is detected.
// `TunnelSupervisor` (the caller) lands in a later task; remove this allow then.
#[allow(dead_code)]
pub fn tunnel_argv(host: &str, remote_port: u16, mcp_port: u16) -> Vec<String> {
    vec![
        "-N".into(),
        "-o".into(),
        "ExitOnForwardFailure=yes".into(),
        "-o".into(),
        "ServerAliveInterval=30".into(),
        "-o".into(),
        "ServerAliveCountMax=3".into(),
        "-R".into(),
        format!("127.0.0.1:{remote_port}:127.0.0.1:{mcp_port}"),
        host.into(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tunnel_argv_builds_reverse_forward() {
        let a = tunnel_argv("mefistos", 4180, 4180);
        assert!(a.contains(&"-N".to_string()));
        assert!(a.iter().any(|s| s == "127.0.0.1:4180:127.0.0.1:4180"));
        assert!(a.iter().any(|s| s == "ExitOnForwardFailure=yes"));
        assert_eq!(a.last().unwrap(), "mefistos");
    }
}
