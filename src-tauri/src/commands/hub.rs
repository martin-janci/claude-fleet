//! Pointing this desktop at a `fleet-hub`, and pointing it back.
//!
//! Three commands, all of them about **this process** rather than about a
//! fleet, which is why all three behave identically in both modes (see
//! `backend::tests_routing::SAME_IN_BOTH_MODES`): `hub_status` reports which
//! fleet this window is onto, `hub_pair` redeems a pairing code, and
//! `hub_disconnect` forgets the pairing.
//!
//! # Disconnect does not revoke
//!
//! Clearing the token here removes it from **this machine**. The client row
//! stays on the hub and the token stays valid there until an operator revokes
//! it (`fleet-hub client revoke`), and a paired client is refused
//! `revoke_client` by design, so this app could not do it even if it wanted
//! to. Settings says so in those words; anything softer would leave someone
//! believing a stolen laptop had been locked out when it had not.
//!
//! # The mode is decided once, at startup
//!
//! [`crate::backend::Backend::resolve`] runs in `lib.rs`'s setup closure and
//! is deliberately never re-read — a mode flip mid-run would leave half the
//! app talking to a hub and half to the local store. So pairing and
//! disconnecting change what the *next* launch will do, and
//! [`HubStatus::restart_required`] is how Settings knows to say so rather
//! than leaving the user wondering why nothing happened.

use crate::backend::pairing::{PairTransport, TcpPairTransport};
use crate::backend::token_store::TokenStore;
use crate::backend::Backend;
use fleet_core::ipc_error::IpcError;
use fleet_core::store::Store;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use tauri::State;

/// Which fleet this window is onto, as the frontend sees it.
///
/// Carries no token and must never carry one: this is a Tauri return value,
/// so it is serialised into the webview, where it would show up in a devtools
/// network panel and in any crash report the webview makes.
#[derive(Serialize, Debug, Clone, PartialEq, Eq, Default)]
pub struct HubStatus {
    /// What this process is doing **right now**.
    pub remote: bool,
    /// The hub it is a window onto; `None` when standalone.
    pub url: Option<String>,
    pub client_name: Option<String>,
    /// `full` or `readonly` as the hub recorded it at pairing time, when this
    /// process has just paired. `None` after a restart — the mode is the
    /// hub's to know, and nothing stores it.
    pub client_mode: Option<String>,
    /// `hub.remote_url` as **stored**, which differs from [`Self::url`]
    /// between a pairing and the restart that applies it.
    pub configured_url: Option<String>,
    pub configured_client_name: Option<String>,
    /// `hub.allow_plaintext`: this operator decided to send the client token
    /// over plain http to a routable host.
    pub allow_plaintext: bool,
    /// Why a configured hub is not in use, or why the one in use is risky.
    /// The same string `Backend::resolve` logs — it belongs in front of a
    /// person, not only in a log file.
    pub warning: Option<String>,
    /// The stored configuration no longer matches the running mode.
    pub restart_required: bool,
}

#[derive(Deserialize, Debug, Clone)]
pub struct HubPairArgs {
    /// The hub's base URL as typed. Normalised here.
    pub url: String,
    /// The 8-character code from `fleet-hub pair --name <this machine>`.
    pub code: String,
    /// The user has seen [`fleet_core::ipc_error::codes::E_HUB_PLAINTEXT`]
    /// and chose to send the token in the clear anyway.
    #[serde(default)]
    pub allow_plaintext: bool,
}

#[tauri::command]
pub fn hub_status(
    backend: State<'_, Backend>,
    store: State<'_, Arc<Mutex<Store>>>,
    tokens: State<'_, Arc<dyn TokenStore>>,
) -> Result<HubStatus, IpcError> {
    logic::status(&backend, &store, tokens.inner().as_ref())
}

#[tauri::command]
pub async fn hub_pair(
    args: HubPairArgs,
    backend: State<'_, Backend>,
    store: State<'_, Arc<Mutex<Store>>>,
    tokens: State<'_, Arc<dyn TokenStore>>,
) -> Result<HubStatus, IpcError> {
    logic::pair(
        &TcpPairTransport,
        &backend,
        &store,
        tokens.inner().as_ref(),
        args,
    )
    .await
}

/// Forget the pairing. **Revokes nothing** — see the module docs.
#[tauri::command]
pub fn hub_disconnect(
    backend: State<'_, Backend>,
    store: State<'_, Arc<Mutex<Store>>>,
    tokens: State<'_, Arc<dyn TokenStore>>,
) -> Result<HubStatus, IpcError> {
    logic::disconnect(&backend, &store, tokens.inner().as_ref())
}

pub(crate) mod logic {
    use super::*;
    use crate::backend::{
        normalise_base_url, pairing, plaintext_risk, ALLOW_PLAINTEXT_KEY, CLIENT_NAME_KEY,
        REMOTE_URL_KEY,
    };
    use fleet_core::ipc_error::{codes, lock};

    /// Re-resolving here rather than reusing the startup decision is the
    /// point: `backend` says what this process is *doing*, and a fresh
    /// resolution says what the stored settings *ask for*. The difference
    /// between them is exactly [`HubStatus::restart_required`], and the
    /// resolution's warning is the reason a configured hub is not in use.
    pub fn status(
        backend: &Backend,
        store: &Mutex<Store>,
        tokens: &dyn TokenStore,
    ) -> Result<HubStatus, IpcError> {
        let stored = Backend::resolve_detail(store, tokens);
        // Read for display only. A failure here is not worth failing the
        // whole status over — the warning above already explains a store
        // that cannot be read.
        let (configured_url, configured_client_name, allow_plaintext) = match store.lock() {
            Ok(s) => (
                setting(&s, REMOTE_URL_KEY),
                setting(&s, CLIENT_NAME_KEY),
                matches!(
                    setting(&s, ALLOW_PLAINTEXT_KEY).as_deref(),
                    Some("true" | "1" | "yes")
                ),
            ),
            Err(_) => (None, None, false),
        };
        Ok(HubStatus {
            remote: backend.is_remote(),
            url: backend.remote().map(|c| c.base_url.clone()),
            client_name: backend.remote().map(|c| c.client_name.clone()),
            // Only a pairing in this session knows the mode; a restart
            // forgets it, and guessing `full` would be a claim about the
            // hub's records that this app cannot make.
            client_mode: None,
            configured_url,
            configured_client_name,
            allow_plaintext,
            warning: stored.warning,
            restart_required: &stored.backend != backend,
        })
    }

    /// A stored setting, treating blank as absent — `hub_disconnect` writes
    /// `""` rather than deleting, because the settings registry has no
    /// delete.
    fn setting(s: &Store, key: &str) -> Option<String> {
        s.get_setting(key)
            .ok()
            .flatten()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
    }

    pub async fn pair(
        transport: &dyn PairTransport,
        backend: &Backend,
        store: &Mutex<Store>,
        tokens: &dyn TokenStore,
        args: HubPairArgs,
    ) -> Result<HubStatus, IpcError> {
        // Everything that can be refused without spending the code is
        // refused first. A pairing code dies on first use and the hub allows
        // about ten attempts a minute from one address, so making someone
        // burn one to be told their URL has a typo is a real cost.
        let base_url = normalise_base_url(args.url.trim()).map_err(|why| {
            IpcError::new(
                codes::E_INVALID,
                format!(
                    "{:?} is not a usable hub address ({why}) — it needs a scheme \
                     and a host, e.g. https://fleet.example.com",
                    args.url.trim()
                ),
            )
        })?;
        let code = args.code.trim().to_string();
        if code.is_empty() {
            return Err(IpcError::new(
                codes::E_INVALID,
                "enter the pairing code from `fleet-hub pair --name <this machine>`",
            ));
        }
        // The client half of the hub's own `--allow-plaintext`, asked here
        // because this is the first moment a URL exists and the LAST moment
        // before a fleet-wide credential is minted for it.
        let risk = plaintext_risk(&base_url);
        if let Some(risk) = &risk {
            if !args.allow_plaintext {
                return Err(IpcError::new(
                    codes::E_HUB_PLAINTEXT,
                    format!(
                        "{risk}. Use https://, or pair again with \"send it in the clear \
                         anyway\" if this hop really is already private (a tunnel, a VPN, \
                         a container network)."
                    ),
                ));
            }
        }

        let client = pairing::redeem(transport, &base_url, &code).await?;

        // From here the code is spent: it cannot be presented twice. So a
        // failure below must leave NOTHING behind — a token no launch will
        // ever read is worse than no token, because the hub's client list
        // says this machine is paired and nobody can tell it is not.
        tokens.set(&client.token).map_err(|e| {
            IpcError::new(
                codes::E_IO,
                format!(
                    "paired with {base_url}, but this machine's secure storage refused \
                     the token ({e}) — the code is spent, so mint a fresh one on the hub \
                     and try again"
                ),
            )
        })?;
        let written = write_settings(store, &base_url, &client.name, args.allow_plaintext);
        if let Err(e) = written {
            // Roll back rather than strand it.
            let _ = tokens.clear();
            return Err(IpcError::new(
                codes::E_IO,
                format!(
                    "paired with {base_url}, but the settings could not be saved ({}) — \
                     nothing was kept. The code is spent, so mint a fresh one on the hub \
                     and try again.",
                    e.message
                ),
            ));
        }

        let mut out = status(backend, store, tokens)?;
        // The one moment this app ever knows its access level: the hub said
        // so in the pairing response, and nothing stores it.
        out.client_mode = Some(client.mode);
        Ok(out)
    }

    fn write_settings(
        store: &Mutex<Store>,
        base_url: &str,
        client_name: &str,
        allow_plaintext: bool,
    ) -> Result<(), IpcError> {
        let s = lock(store)?;
        s.set_setting(REMOTE_URL_KEY, base_url)?;
        s.set_setting(CLIENT_NAME_KEY, client_name)?;
        s.set_setting(
            ALLOW_PLAINTEXT_KEY,
            if allow_plaintext { "true" } else { "false" },
        )?;
        Ok(())
    }

    /// Forget the pairing on **this machine**. Revokes nothing on the hub —
    /// see the module docs, and the wording Settings uses.
    ///
    /// The token goes first: if the settings write then fails, what is left
    /// is a URL with no token, which [`Backend::resolve`] already handles
    /// loudly ("configured but no client token is stored"). The other order
    /// would leave a live token against a hub the app no longer names, which
    /// nothing reports.
    pub fn disconnect(
        backend: &Backend,
        store: &Mutex<Store>,
        tokens: &dyn TokenStore,
    ) -> Result<HubStatus, IpcError> {
        tokens.clear().map_err(|e| {
            IpcError::new(
                codes::E_IO,
                format!("this machine's secure storage refused to forget the token: {e}"),
            )
        })?;
        {
            let s = lock(store)?;
            // The settings registry stores strings and has no delete; blank
            // is what `resolve` already treats as "no hub".
            s.set_setting(REMOTE_URL_KEY, "")?;
            s.set_setting(CLIENT_NAME_KEY, "")?;
            s.set_setting(ALLOW_PLAINTEXT_KEY, "")?;
        }
        status(backend, store, tokens)
    }
}

#[cfg(test)]
#[path = "tests_hub.rs"]
mod tests;
