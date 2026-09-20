//! Pointing this desktop at a `fleet-hub`, and pointing it back.
//!
//! Three commands, all of them about **this process** rather than about a
//! fleet, which is why all three behave identically in both modes (their rows
//! in `backend::verdicts::VERDICTS` say so, each with its reason):
//! `hub_status` reports which fleet this window is onto, `hub_pair` redeems a
//! pairing code, and `hub_disconnect` forgets the pairing.
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
    /// `hub.client_plaintext_token`
    /// ([`crate::backend::ALLOW_PLAINTEXT_KEY`]): this operator decided to
    /// send the client token over plain http to a routable host. The wire
    /// name stays `allow_plaintext` — it is a field between this app's
    /// halves, not the stored key, and not the daemon's setting of that name.
    pub allow_plaintext: bool,
    /// Why a configured hub is not in use, or why the one in use is risky —
    /// for the stored configuration, as the next launch would resolve it.
    /// The same string `Backend::resolve` logs — it belongs in front of a
    /// person, not only in a log file.
    pub warning: Option<String>,
    /// The stored configuration no longer matches the running mode.
    pub restart_required: bool,
    /// Set when a hub is configured but this launch could not use it, with
    /// the reason. This process then owns NOTHING — no reconcile tick, no
    /// usage poll, no control API — and refuses every fleet command.
    pub unavailable: Option<String>,
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

/// Whether this window's live link to the hub is up, for a window that
/// mounted after the last `hub:connection` event. See `backend::connection`.
#[tauri::command]
pub fn hub_connection(
    status: State<'_, Arc<crate::backend::connection::HubConnectionStatus>>,
) -> crate::backend::connection::HubConnection {
    status.current()
}

/// Whether a client token is sitting on this machine with **no hub
/// configured** — a credential nothing reads and, until this existed, nothing
/// offered to clear.
///
/// # The state this exists for
///
/// Pairing writes two stores that no transaction spans. Since `1e14414` the
/// token is written **last**, so a crash can no longer strand one; before it,
/// the token went first, and a crash between the two writes left a live
/// fleet-wide bearer token beside a blank `hub.remote_url`. An install that
/// took that crash carries the token across the upgrade.
///
/// [`Backend::resolve`] deliberately does not look for it: a blank URL is
/// standalone, and a keychain query on every launch is the call that prompts
/// or blocks on a locked macOS keychain (see
/// [`crate::backend::UnavailableHub`]). The question is asked here instead —
/// once, when a person opens Settings, which is both the moment a keychain
/// prompt is explicable and the moment the answer can be acted on: Settings
/// shows Disconnect, and [`hub_disconnect`] clears the token.
///
/// Answers `false` whenever a hub **is** configured. The token is not stranded
/// then — it belongs to that hub, and Settings already offers Disconnect for
/// the working and the [`crate::backend::Backend::Unavailable`] case alike.
///
/// The token never leaves the token store: this returns a bool.
#[tauri::command]
pub fn hub_stranded_token(
    store: State<'_, Arc<Mutex<Store>>>,
    tokens: State<'_, Arc<dyn TokenStore>>,
) -> Result<bool, IpcError> {
    logic::stranded_token(&store, tokens.inner().as_ref())
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
            // What THIS process is doing, like `remote`: a hub fixed since
            // launch is still unavailable here until the restart, which
            // `restart_required` says.
            unavailable: backend.unavailable().map(|hub| hub.reason.clone()),
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

        // From here the code is spent: it cannot be presented twice.
        //
        // A pairing writes two stores that no transaction spans — the token
        // in the keychain or the 0600 file, the settings in `state.db` — so a
        // crash can land between any two writes, and every state in between
        // is one a launch may start from. The order makes each of them safe:
        //
        // 1. Forget any previous hub's token. From here until step 3 there is
        //    no token at all, and a configured URL with no token resolves
        //    `Unavailable`: it owns nothing, says why, and offers Disconnect.
        // 2. Write the settings, the URL last.
        // 3. Store the new token — beside the URL of the hub that issued it.
        //
        // The old order stored the token first. A crash then left it beside
        // the previous hub's URL, and the next launch presented hub B's
        // bearer token to hub A; or beside no URL at all, where no launch
        // reads it and no screen offers to clear it.
        let previous = read_settings(store);
        tokens.clear().map_err(|e| {
            IpcError::new(
                codes::E_IO,
                format!(
                    "paired with {base_url}, but this machine's secure storage would not \
                     forget the previous token ({e}), so nothing was changed here — the \
                     code is spent, so mint a fresh one on the hub and try again"
                ),
            )
        })?;
        if let Err(e) = write_settings(store, &base_url, &client.name, args.allow_plaintext) {
            // Best effort: the store that just failed may fail again, and
            // with no token stored every state it leaves is safe anyway.
            let _ = restore_settings(store, &previous);
            return Err(IpcError::new(
                codes::E_IO,
                format!(
                    "paired with {base_url}, but the settings could not be saved ({}) — \
                     no token was kept. If this app was paired with a hub before, it now \
                     has no token for that one either, and the next launch will say so. \
                     The code is spent, so mint a fresh one on the hub and try again.",
                    e.message
                ),
            ));
        }
        if let Err(e) = tokens.set(&client.token) {
            // Put the settings back: a first pairing leaves no trace, and a
            // re-pairing leaves the previous hub configured with no token,
            // which the next launch reports rather than acts on.
            let _ = restore_settings(store, &previous);
            return Err(IpcError::new(
                codes::E_IO,
                format!(
                    "paired with {base_url}, but this machine's secure storage refused \
                     the token ({e}), so the settings were put back and nothing was kept \
                     — the code is spent, so mint a fresh one on the hub and try again"
                ),
            ));
        }

        let mut out = status(backend, store, tokens)?;
        // The one moment this app ever knows its access level: the hub said
        // so in the pairing response, and nothing stores it.
        out.client_mode = Some(client.mode);
        Ok(out)
    }

    /// The URL is written LAST: until it is, the next launch still sees
    /// the previous URL (or none), and with no token stored — `pair` has
    /// cleared it — either is safe.
    fn write_settings(
        store: &Mutex<Store>,
        base_url: &str,
        client_name: &str,
        allow_plaintext: bool,
    ) -> Result<(), IpcError> {
        let s = lock(store)?;
        s.set_setting(CLIENT_NAME_KEY, client_name)?;
        s.set_setting(
            ALLOW_PLAINTEXT_KEY,
            if allow_plaintext { "true" } else { "false" },
        )?;
        s.set_setting(REMOTE_URL_KEY, base_url)?;
        Ok(())
    }

    /// The three hub settings as stored before a pairing, so a failed one
    /// can put them back. Absent reads as blank, which is what `resolve`
    /// already treats as "no hub".
    fn read_settings(store: &Mutex<Store>) -> [(&'static str, String); 3] {
        let read = |key| {
            store
                .lock()
                .ok()
                .and_then(|s| s.get_setting(key).ok().flatten())
                .unwrap_or_default()
        };
        [
            (CLIENT_NAME_KEY, read(CLIENT_NAME_KEY)),
            (ALLOW_PLAINTEXT_KEY, read(ALLOW_PLAINTEXT_KEY)),
            (REMOTE_URL_KEY, read(REMOTE_URL_KEY)),
        ]
    }

    fn restore_settings(
        store: &Mutex<Store>,
        previous: &[(&'static str, String); 3],
    ) -> Result<(), IpcError> {
        let s = lock(store)?;
        for (key, value) in previous {
            s.set_setting(key, value)?;
        }
        Ok(())
    }

    /// See [`super::hub_stranded_token`]. The settings are consulted FIRST so
    /// that a configured hub — the case Settings already handles — never
    /// reaches the token store at all; the keychain is asked only in the state
    /// this is about.
    pub fn stranded_token(store: &Mutex<Store>, tokens: &dyn TokenStore) -> Result<bool, IpcError> {
        // A settings store that cannot be read is not evidence of a stranded
        // token: `Backend::resolve` already answers that case loudly
        // (`unreadable_settings`), and answering `true` here would put a
        // "leftover credential" warning in front of someone whose actual
        // problem is a database it also names. Not stranded, as far as this
        // can tell.
        let configured = lock(store)?
            .get_setting(REMOTE_URL_KEY)?
            .is_some_and(|v| !v.trim().is_empty());
        if configured {
            return Ok(false);
        }
        // The message is the token store's own diagnostic text, which never
        // contains the secret (`TokenStore`'s contract). A failure is
        // reported rather than swallowed, but the caller is free to stay
        // quiet: a keychain that will not open is not evidence of a leftover.
        tokens.get().map(|t| t.is_some()).map_err(|e| {
            IpcError::new(
                codes::E_IO,
                format!("could not check this machine's secure storage for a leftover client token: {e}"),
            )
        })
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
