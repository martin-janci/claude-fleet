//! Which transport reaches a host.
//!
//! [`HostRouter`] is the per-host resolver the design calls for: it reads the
//! host's `transport` column once per call and, for an `agent` row, names the
//! alias its agent registered under. It answers **synchronously and by
//! value**, so the store guard is taken and dropped inside the lookup and no
//! caller can hold it across the delegated `.await`.
//!
//! **Where it is installed, and why it is not a wrapper.** The design sketch
//! had the router wrap `SshClient` behind `dyn SshExec` at the few places the
//! transport is built. That does not reach: 96 service functions across 30
//! files take a concrete `&Arc<SshClient>`, not `&dyn SshExec`, and a wrapper
//! could only route the handful that take the trait object — `add_host`'s
//! probe would go to the agent while `new_session`'s tmux calls silently went
//! to SSH. The alternative was rewriting those 96 signatures, which is
//! exactly the service-layer churn the whole sub-project exists to avoid. So
//! the router is installed *inside* [`SshClient`](crate::ssh::SshClient)
//! instead, and every existing `Arc<SshClient>` holder routes for free. The
//! seam is still the same seam: the seven `SshExec` methods, unchanged.
//!
//! A client built by [`SshClient::new`](crate::ssh::SshClient::new) — the
//! desktop's — carries no router at all, so it never consults a store and
//! every host resolves to SSH exactly as before.

use super::AgentTransport;
use crate::store::Store;
use std::sync::{Arc, Mutex};

/// Resolves a host to its transport. See the module docs.
pub struct HostRouter {
    agent: Arc<AgentTransport>,
    store: Arc<Mutex<Store>>,
}

impl HostRouter {
    pub fn new(agent: Arc<AgentTransport>, store: Arc<Mutex<Store>>) -> Arc<Self> {
        Arc::new(Self { agent, store })
    }

    /// The alias to address this host's agent by, or `None` to stay on SSH.
    ///
    /// Synchronous, and returns an owned `String`: the guard is released
    /// before this returns, so there is no way for the caller to carry it
    /// into the delegated call.
    ///
    /// A poisoned store falls back to SSH rather than failing the call. That
    /// is the behaviour every host had before this router existed, and a
    /// process whose store mutex is poisoned has worse problems than one
    /// misrouted command.
    ///
    /// Before it answers, a live connection for that alias whose token is no
    /// longer current is dropped (see [`credential_is_current`]). So the call
    /// that follows either reaches an agent holding the host's CURRENT full
    /// token or fails `E_AGENT_OFFLINE` — nothing is ever sent over a
    /// connection that a rotation, a `readonly` or a removal has revoked, not
    /// even in the heartbeat before the endpoint notices. That is what keeps a
    /// rotation from handing its new token to the connection it revokes.
    pub fn agent_alias(&self, host: &str) -> Option<String> {
        let alias = {
            let store = self.store.lock().ok()?;
            store.agent_host_alias(host).ok().flatten()?
        };
        // The guard above is released: the check takes the lock itself.
        self.agent
            .registry()
            .evict_stale(&alias, |c| credential_is_current(&self.store, &alias, c));
        Some(alias)
    }

    /// The transport agent-routed hosts are delegated to.
    pub fn agent(&self) -> &Arc<AgentTransport> {
        &self.agent
    }
}

/// Is `credential` — the SHA-256 of the token an agent connection
/// authenticated with — still a token that may BE `alias`'s agent?
///
/// Yes only while the host is still on the agent transport and its token row
/// still holds that exact token in `full` mode. A rotation replaces the token, `readonly` narrows it, and
/// removing the host deletes the row; each one makes this false. A poisoned
/// store answers no: when in doubt, a connection that receives every command
/// for a host is cut off.
pub(crate) fn credential_is_current(store: &Mutex<Store>, alias: &str, credential: &str) -> bool {
    let Ok(s) = store.lock() else {
        return false;
    };
    // Moving the host back to SSH revokes its agent too.
    if s.agent_host_alias(alias).ok().flatten().as_deref() != Some(alias) {
        return false;
    }
    match s.get_host_token(alias) {
        Ok(Some(row)) => {
            row.mode == "full" && crate::mcp::auth::sha256_hex(&row.token) == credential
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use crate::agent::fake::{self, FakeAgent};
    use crate::agent::AgentRegistry;
    use crate::ipc_error::codes;
    use crate::ssh::{SshClient, SshExec};
    use crate::store::Store;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    /// Short enough that the one test which really does reach for `ssh`
    /// cannot sit on a loaded box, long enough that a routed call never
    /// races its own deadline.
    const CONNECT: Duration = Duration::from_secs(1);
    const WALL: Duration = Duration::from_secs(5);

    /// A store holding one host row per `(alias, ssh_alias, transport)`.
    fn store_with(rows: &[(&str, Option<&str>, &str)]) -> Arc<Mutex<Store>> {
        let store = Store::open_in_memory().unwrap();
        for (alias, ssh_alias, transport) in rows {
            store.insert_host(alias, *ssh_alias).unwrap();
            store.set_host_transport(alias, transport).unwrap();
        }
        Arc::new(Mutex::new(store))
    }

    /// The client a hub builds: routed, over a registry the test drives.
    fn hub_client(
        rows: &[(&str, Option<&str>, &str)],
    ) -> (SshClient, Arc<AgentRegistry>, Arc<Mutex<Store>>) {
        let registry = AgentRegistry::new();
        let store = store_with(rows);
        let ssh = SshClient::with_agents(Arc::clone(&registry), Arc::clone(&store));
        (ssh, registry, store)
    }

    fn script() -> Vec<String> {
        vec![
            "bash".to_string(),
            "-lc".to_string(),
            crate::shell::quote("echo hi"),
        ]
    }

    fn args(v: &[String]) -> Vec<&str> {
        v.iter().map(String::as_str).collect()
    }

    // ── the decision itself ───────────────────────────────────────────────

    #[test]
    fn a_row_marked_agent_routes_to_its_own_alias() {
        let (ssh, _reg, _store) = hub_client(&[("laptop", Some("laptop.example"), "agent")]);
        assert_eq!(ssh.agent_route("laptop").as_deref(), Some("laptop"));
    }

    #[test]
    fn a_row_marked_ssh_is_not_routed() {
        let (ssh, _reg, _store) = hub_client(&[("mefistos", Some("mefistos.example"), "ssh")]);
        assert_eq!(ssh.agent_route("mefistos"), None);
    }

    #[test]
    fn an_alias_with_no_row_keeps_todays_behaviour() {
        let (ssh, _reg, _store) = hub_client(&[("laptop", None, "agent")]);
        assert_eq!(ssh.agent_route("nobody-has-heard-of-this"), None);
    }

    /// `service::hosts` is the one caller that hands the transport a host's
    /// `ssh_alias` rather than its fleet alias (`probe_host` resolves the
    /// row, then probes `ssh_alias`). Both must reach the agent, and both
    /// must name the host by the alias the agent registered under.
    #[test]
    fn a_probe_by_ssh_alias_routes_under_the_fleet_alias() {
        let (ssh, _reg, _store) = hub_client(&[("laptop", Some("laptop.example"), "agent")]);
        assert_eq!(ssh.agent_route("laptop.example").as_deref(), Some("laptop"));
    }

    /// One host's `ssh_alias` can be another host's fleet alias. An exact
    /// alias match is the stronger claim and must win, or a call for `beta`
    /// would be delivered to `alpha`'s agent.
    #[test]
    fn an_exact_alias_beats_another_rows_ssh_alias() {
        let (ssh, _reg, _store) = hub_client(&[
            ("alpha", Some("beta"), "agent"),
            ("beta", Some("beta.example"), "agent"),
        ]);
        assert_eq!(ssh.agent_route("beta").as_deref(), Some("beta"));
    }

    /// An SSH host whose fleet alias happens to be an agent host's
    /// `ssh_alias` is the SSH host: a call for it must not run on the agent's
    /// machine. The exact alias names a row, whatever its transport.
    #[test]
    fn an_ssh_hosts_alias_is_never_captured_by_an_agent_hosts_ssh_alias() {
        let (ssh, _reg, _store) = hub_client(&[
            ("mefistos", Some("mefistos.example"), "ssh"),
            ("laptop", Some("mefistos"), "agent"),
        ]);
        assert_eq!(ssh.agent_route("mefistos"), None, "mefistos is an SSH host");
        assert_eq!(ssh.agent_route("laptop").as_deref(), Some("laptop"));
    }

    /// Two agent hosts claiming one `ssh_alias` is ambiguous: route neither
    /// rather than pick one.
    #[test]
    fn an_ssh_alias_two_agent_hosts_share_routes_to_neither() {
        let (ssh, _reg, _store) = hub_client(&[
            ("a", Some("shared.example"), "agent"),
            ("b", Some("shared.example"), "agent"),
        ]);
        assert_eq!(ssh.agent_route("shared.example"), None);
    }

    /// The desktop's client. It is built with no registry at all, so every
    /// host resolves to SSH whatever any host row says — there is no `/agent`
    /// endpoint in the desktop for an agent to dial.
    #[test]
    fn a_client_built_without_agents_routes_nothing() {
        let ssh = SshClient::new();
        assert_eq!(ssh.agent_route("laptop"), None);
    }

    // ── a revoked credential never gets another frame ─────────────────────
    //
    // The endpoint re-checks each connection's token on every heartbeat;
    // this is the check with no window at all: a call for a host whose live
    // agent authenticated with a token that is no longer current is refused
    // BEFORE anything is sent, and the connection is dropped.

    async fn a_change_stops_the_next_call(change: impl FnOnce(&Store)) {
        let (ssh, reg, store) = hub_client(&[("laptop", None, "agent")]);
        store
            .lock()
            .unwrap()
            .upsert_host_token("laptop", "t1")
            .unwrap();
        let agent =
            FakeAgent::connect_with_token(&reg, "laptop", "t1", fake::answer_with(0, b"", b""));
        let a = script();
        ssh.run("laptop", &args(&a), CONNECT).await.unwrap();
        assert_eq!(agent.sent().len(), 1, "a current token is served");

        change(&store.lock().unwrap());
        let started = std::time::Instant::now();
        let err = ssh.run("laptop", &args(&a), CONNECT).await.unwrap_err();
        assert_eq!(err.code, codes::E_AGENT_OFFLINE, "{err:?}");
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
        assert_eq!(
            agent.sent().len(),
            1,
            "nothing more was sent: {:?}",
            agent.sent()
        );
        assert!(!reg.connected("laptop"), "and the stale connection is gone");
    }

    #[tokio::test]
    async fn a_rotated_token_gets_no_further_frame() {
        a_change_stops_the_next_call(|s| s.upsert_host_token("laptop", "t2").unwrap()).await;
    }

    #[tokio::test]
    async fn a_readonly_token_gets_no_further_frame() {
        a_change_stops_the_next_call(|s| s.set_host_token_mode("laptop", "readonly").unwrap())
            .await;
    }

    /// Removing and re-adding the alias turns routing back on; the old
    /// connection must not be what it routes to.
    #[tokio::test]
    async fn a_removed_and_readded_host_does_not_reach_the_old_agent() {
        a_change_stops_the_next_call(|s| {
            s.delete_host("laptop").unwrap();
            s.insert_host("laptop", None).unwrap();
            s.set_host_transport("laptop", "agent").unwrap();
        })
        .await;
    }

    // ── the seven methods ─────────────────────────────────────────────────

    #[tokio::test]
    async fn run_reaches_the_agent() {
        let (ssh, reg, _store) = hub_client(&[("laptop", None, "agent")]);
        let agent = FakeAgent::connect(&reg, "laptop", fake::answer_with(0, b"hi\n", b""));
        let a = script();
        let out = ssh.run("laptop", &args(&a), CONNECT).await.unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout), "hi\n");
        assert_eq!(agent.sent().len(), 1, "one exec frame: {:?}", agent.sent());
    }

    #[tokio::test]
    async fn every_transport_method_reaches_the_agent() {
        let (ssh, reg, _store) = hub_client(&[("laptop", None, "agent")]);
        let agent = FakeAgent::connect(&reg, "laptop", fake::answer_with(0, b"/home/u\n", b""));
        let a = script();
        let a = args(&a);
        let token = tokio_util::sync::CancellationToken::new();

        ssh.run("laptop", &a, CONNECT).await.unwrap();
        ssh.run_bounded("laptop", &a, CONNECT, WALL).await.unwrap();
        ssh.run_bounded_capped("laptop", &a, CONNECT, WALL, 1024)
            .await
            .unwrap();
        ssh.run_cancellable("laptop", &a, CONNECT, token.clone())
            .await
            .unwrap();
        ssh.run_bounded_cancellable("laptop", &a, CONNECT, WALL, token)
            .await
            .unwrap();

        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(file.path(), b"payload").unwrap();
        ssh.upload_file("laptop", file.path(), "/tmp/there", WALL)
            .await
            .unwrap();

        let home = ssh.remote_home("laptop").await.unwrap();
        assert_eq!(home, "/home/u");

        assert_eq!(
            agent.sent().len(),
            7,
            "all seven methods route: {:?}",
            agent.sent()
        );
    }

    /// An agent host with nothing connected must report unreachable at once,
    /// the way a down SSH host does — and must *not* quietly fall back to
    /// SSH, which is the one thing that cannot work for it.
    #[tokio::test]
    async fn an_agent_host_with_no_connection_is_offline_not_ssh() {
        let (ssh, _reg, _store) = hub_client(&[("laptop", None, "agent")]);
        let a = script();
        let err = ssh
            .run_bounded("laptop", &args(&a), CONNECT, WALL)
            .await
            .unwrap_err();
        assert_eq!(err.code, crate::ipc_error::codes::E_AGENT_OFFLINE);
    }

    /// The negative case end to end, not just as a decision: a host row that
    /// says `ssh` goes to SSH even when an agent is connected under exactly
    /// that alias. The host row is the authority, not the registry.
    ///
    /// The SSH outcome is deliberately not asserted — this box has no
    /// `ssh-only-host.invalid` and does not need one. What is asserted is
    /// that the agent was never asked.
    #[tokio::test]
    async fn an_ssh_row_goes_to_ssh_even_with_an_agent_connected() {
        let (ssh, reg, _store) = hub_client(&[("ssh-only-host.invalid", None, "ssh")]);
        let agent = FakeAgent::connect(
            &reg,
            "ssh-only-host.invalid",
            fake::answer_with(0, b"hijacked\n", b""),
        );
        let a = script();
        let _ = ssh
            .run_bounded("ssh-only-host.invalid", &args(&a), CONNECT, WALL)
            .await;
        assert!(
            agent.sent().is_empty(),
            "an ssh row must never reach an agent: {:?}",
            agent.sent()
        );
    }

    // ── the store guard ───────────────────────────────────────────────────

    /// The route is read under the store mutex; the delegated call is not.
    /// Holding the guard across the `.await` would wedge every other caller
    /// for the whole wall clock of the slowest agent command, so prove the
    /// lock is free while a routed call is in flight.
    #[tokio::test]
    async fn the_store_lock_is_free_while_a_routed_call_is_in_flight() {
        let (ssh, reg, store) = hub_client(&[("laptop", None, "agent")]);
        let agent = FakeAgent::connect(&reg, "laptop", fake::silent());
        let ssh = Arc::new(ssh);
        let call = tokio::spawn({
            let ssh = Arc::clone(&ssh);
            async move {
                let a = script();
                ssh.run_bounded("laptop", &args(&a), CONNECT, Duration::from_secs(60))
                    .await
            }
        });
        // The frame has left the hub, so the route has been read and the
        // call is parked on the agent's answer.
        agent.wait_until_sent(1).await;
        assert!(
            store.try_lock().is_ok(),
            "the store guard is held across the delegated await"
        );
        call.abort();
    }
}
