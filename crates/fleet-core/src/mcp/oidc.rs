//! Single sign-on: pairing a device by signing in with Keycloak (or any
//! OpenID Connect provider) instead of carrying a code from the hub's
//! terminal.
//!
//! It ends where `fleet-hub pair` ends: in a **pairing code** for
//! [`super::pairing::PendingPairings`], redeemed by the same `POST /pair`.
//! So nothing new can hold a credential — no session cookie, no second token
//! kind, no new door past `authorize`; the provider only stands in for the
//! operator at the terminal, deciding WHOSE device the code is for.
//!
//! The flow (authorization code with PKCE, OIDC Core §3.1):
//!
//! 1. `GET /auth/oidc/start[?name=<device>]` — remembers a fresh `state`,
//!    `nonce` and PKCE verifier in memory, sets a cookie holding the state
//!    (login-CSRF: the callback is honoured only in the browser that began
//!    it), and redirects to the provider's authorization endpoint.
//! 2. The person signs in at the provider, which redirects back to
//!    `GET /auth/oidc/callback?code&state`.
//! 3. The hub exchanges the code at the token endpoint (directly, over TLS,
//!    with the verifier and — for a confidential client — the secret) and
//!    checks the ID token: `iss`, `aud`/`azp`, `exp`, `nonce`. The
//!    signature is not checked, and need not be: OIDC Core §3.1.3.7 (6)
//!    lets a client that received the ID token straight from the token
//!    endpoint over TLS rely on the TLS server validation instead. The
//!    token never passes through the browser.
//! 4. The account (issuer, `sub`) is looked up in `person_identities`
//!    (`store::identities`). Linked → that person, refused if disabled.
//!    Unlinked → a new person named after the username claim when
//!    auto-provisioning is on and the name is free; never an existing
//!    person by name (the operator links those: `fleet-hub person
//!    link-sso`).
//! 5. A pairing code is minted for that person and shown on a page with a
//!    `claudefleet:` link to the app and the pair URL for a desktop.
//!
//! Configuration is the hub's environment ([`OidcConfig::from_lookup`]):
//! the client secret must not sit in `state.db` next to the data it guards.

use super::pairing::{MintRequest, PairState};
use crate::net::https::{DirectTransport, HttpTransport, Method, Request};
use crate::store::{PersonRow, Store};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use base64::Engine as _;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// How long a sign-in may take at the provider before its state expires.
pub const LOGIN_TTL: Duration = Duration::from_secs(10 * 60);
/// How long the pairing code a sign-in ends in stays good.
pub const CODE_TTL: Duration = Duration::from_secs(5 * 60);
/// Outstanding sign-ins held at most; the oldest is dropped past it, so a
/// flood of `/auth/oidc/start` costs bounded memory.
pub const MAX_PENDING_LOGINS: usize = 1024;
/// Spacing between two sign-in starts (and, separately, two callbacks) from
/// one address.
pub const ATTEMPT_INTERVAL: Duration = Duration::from_secs(2);
/// Leeway for the provider's clock on `exp` / `iat`.
const CLOCK_SKEW_SECS: i64 = 120;
/// The cookie that binds a callback to the browser that started it.
const STATE_COOKIE: &str = "fleet_oidc_state";
/// The path the cookie is scoped to, and the callback's prefix.
const COOKIE_PATH: &str = "/auth/oidc";

/// The environment variables [`OidcConfig::from_lookup`] reads.
pub const ENV_ISSUER: &str = "FLEET_HUB_OIDC_ISSUER";
pub const ENV_CLIENT_ID: &str = "FLEET_HUB_OIDC_CLIENT_ID";
pub const ENV_CLIENT_SECRET: &str = "FLEET_HUB_OIDC_CLIENT_SECRET";
pub const ENV_ALLOWED_ROLES: &str = "FLEET_HUB_OIDC_ALLOWED_ROLES";
pub const ENV_AUTO_PROVISION: &str = "FLEET_HUB_OIDC_AUTO_PROVISION";
pub const ENV_USERNAME_CLAIM: &str = "FLEET_HUB_OIDC_USERNAME_CLAIM";
pub const ENV_MODE: &str = "FLEET_HUB_OIDC_MODE";
pub const ENV_SCOPES: &str = "FLEET_HUB_OIDC_SCOPES";
pub const ENV_CA_FILE: &str = "FLEET_HUB_OIDC_CA_FILE";

/// What the hub needs to know about its identity provider.
#[derive(Clone, PartialEq, Eq)]
pub struct OidcConfig {
    /// `https://sso.example.com/realms/acme` for a Keycloak realm; no
    /// trailing slash.
    pub issuer: String,
    pub client_id: String,
    /// A confidential client's secret; `None` for a public client (PKCE
    /// alone).
    pub client_secret: Option<String>,
    /// A sign-in is refused unless the account holds one of these roles or
    /// groups. Empty: any account the provider authenticates.
    pub allowed_roles: Vec<String>,
    /// Create a person on an account's first sign-in. Off: only accounts
    /// the operator linked get in.
    pub auto_provision: bool,
    /// The ID-token claim that names a provisioned person.
    pub username_claim: String,
    /// `full` or `readonly`: what a device paired this way may do.
    pub mode: String,
    /// Space-separated scopes; always includes `openid`.
    pub scopes: String,
    /// Extra CA certificates (PEM) for a provider on a private CA.
    pub extra_ca: Option<String>,
}

impl std::fmt::Debug for OidcConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OidcConfig")
            .field("issuer", &self.issuer)
            .field("client_id", &self.client_id)
            .field(
                "client_secret",
                &self
                    .client_secret
                    .as_ref()
                    .map(|_| crate::logging::REDACTED),
            )
            .field("allowed_roles", &self.allowed_roles)
            .field("auto_provision", &self.auto_provision)
            .field("username_claim", &self.username_claim)
            .field("mode", &self.mode)
            .field("scopes", &self.scopes)
            .field("extra_ca", &self.extra_ca.is_some())
            .finish()
    }
}

fn parse_bool(key: &str, v: &str) -> Result<bool, String> {
    match v.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        other => Err(format!("{key}={other:?} is not a boolean (true/false)")),
    }
}

impl OidcConfig {
    /// Read the configuration through `get` (the process environment in
    /// production). `Ok(None)` when no issuer is set — single sign-on off,
    /// the default. A half-configured provider is an error, so a typo
    /// fails the start instead of silently leaving sign-in off.
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<Option<Self>, String> {
        let get = |k: &str| {
            get(k)
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
        };
        let Some(issuer) = get(ENV_ISSUER) else {
            return Ok(None);
        };
        let issuer = crate::store::normalize_issuer(&issuer);
        if !issuer.starts_with("https://") {
            return Err(format!(
                "{ENV_ISSUER} must be an https:// URL (the hub never sends a client secret or \
                 a code verifier in plaintext); got {issuer:?}"
            ));
        }
        crate::net::https::parse_target(&issuer).map_err(|e| format!("{ENV_ISSUER}: {e}"))?;
        let client_id = get(ENV_CLIENT_ID)
            .ok_or_else(|| format!("{ENV_ISSUER} is set but {ENV_CLIENT_ID} is not"))?;
        let mode = get(ENV_MODE).unwrap_or_else(|| "full".into());
        if mode != "full" && mode != "readonly" {
            return Err(format!("{ENV_MODE} must be full or readonly; got {mode:?}"));
        }
        let mut scopes: Vec<String> = get(ENV_SCOPES)
            .unwrap_or_else(|| "openid profile email".into())
            .split_whitespace()
            .map(str::to_string)
            .collect();
        if !scopes.iter().any(|s| s == "openid") {
            scopes.insert(0, "openid".into());
        }
        let extra_ca = match get(ENV_CA_FILE) {
            Some(path) => Some(
                std::fs::read_to_string(&path).map_err(|e| format!("{ENV_CA_FILE}={path}: {e}"))?,
            ),
            None => None,
        };
        Ok(Some(OidcConfig {
            issuer,
            client_id,
            client_secret: get(ENV_CLIENT_SECRET),
            allowed_roles: get(ENV_ALLOWED_ROLES)
                .map(|v| {
                    v.split(',')
                        .map(str::trim)
                        .filter(|r| !r.is_empty())
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default(),
            auto_provision: match get(ENV_AUTO_PROVISION) {
                Some(v) => parse_bool(ENV_AUTO_PROVISION, &v)?,
                None => true,
            },
            username_claim: get(ENV_USERNAME_CLAIM).unwrap_or_else(|| "preferred_username".into()),
            mode,
            scopes: scopes.join(" "),
            extra_ca,
        }))
    }
}

/// The two endpoints the flow needs, from the provider's discovery
/// document.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Endpoints {
    authorization: String,
    token: String,
}

/// One sign-in in flight, keyed by its `state`.
struct PendingLogin {
    verifier: String,
    nonce: String,
    device_name: Option<String>,
    started: Instant,
}

/// Who signed in, once the ID token checked out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedIn {
    pub subject: String,
    pub username: Option<String>,
    pub display_name: Option<String>,
    /// Realm and client roles and groups, as the provider named them.
    pub roles: Vec<String>,
    pub device_name: Option<String>,
}

/// Why a sign-in did not end in a code. The text is shown to the person on
/// the callback page, so it never carries a token, a code or a secret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// The provider could not be reached or answered nonsense.
    Provider(String),
    /// The sign-in itself is bad: an unknown or expired state, a token that
    /// does not check out.
    Invalid(String),
    /// A valid account this hub will not pair.
    Forbidden(String),
}

impl Refusal {
    fn status(&self) -> StatusCode {
        match self {
            Refusal::Provider(_) => StatusCode::BAD_GATEWAY,
            Refusal::Invalid(_) => StatusCode::BAD_REQUEST,
            Refusal::Forbidden(_) => StatusCode::FORBIDDEN,
        }
    }
    fn message(&self) -> &str {
        match self {
            Refusal::Provider(m) | Refusal::Invalid(m) | Refusal::Forbidden(m) => m,
        }
    }
}

/// The hub's side of single sign-on: the configuration, the HTTP seam to
/// the provider, its cached endpoints and the sign-ins in flight.
pub struct OidcProvider {
    config: OidcConfig,
    transport: Arc<dyn HttpTransport>,
    endpoints: tokio::sync::Mutex<Option<Endpoints>>,
    pending: Mutex<HashMap<String, PendingLogin>>,
}

impl std::fmt::Debug for OidcProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OidcProvider")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

/// The host part of an `https://` URL, lower-cased.
fn host_of(url: &str) -> Option<String> {
    crate::net::https::parse_target(url)
        .ok()
        .map(|t| t.endpoint.host().to_ascii_lowercase())
}

impl OidcProvider {
    /// A provider reached over HTTPS from this process, and only at the
    /// issuer's own host: the discovery document cannot send the hub's
    /// secret anywhere else.
    pub fn new(config: OidcConfig) -> Self {
        let host = host_of(&config.issuer).unwrap_or_default();
        let policy: crate::net::https::HostPolicy =
            Arc::new(move |h: &str| h.eq_ignore_ascii_case(&host));
        let transport = DirectTransport::new(policy).with_extra_ca(config.extra_ca.clone());
        Self::with_transport(config, Arc::new(transport))
    }

    /// Over any transport — the tests' [`crate::net::https::FakeTransport`].
    pub fn with_transport(config: OidcConfig, transport: Arc<dyn HttpTransport>) -> Self {
        OidcProvider {
            config,
            transport,
            endpoints: tokio::sync::Mutex::new(None),
            pending: Mutex::new(HashMap::new()),
        }
    }

    pub fn config(&self) -> &OidcConfig {
        &self.config
    }

    fn lock_pending(&self) -> std::sync::MutexGuard<'_, HashMap<String, PendingLogin>> {
        self.pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The provider's endpoints, fetched once and then cached. Fetched on
    /// first use rather than at start, so a hub whose provider is down
    /// still starts; a failure is not cached.
    async fn endpoints(&self) -> Result<Endpoints, Refusal> {
        let mut cached = self.endpoints.lock().await;
        if let Some(e) = cached.as_ref() {
            return Ok(e.clone());
        }
        let url = format!("{}/.well-known/openid-configuration", self.config.issuer);
        let resp = self
            .transport
            .send(Request::get(&url))
            .await
            .map_err(|e| Refusal::Provider(format!("discovery at {url}: {e}")))?;
        if !resp.is_success() {
            return Err(Refusal::Provider(format!(
                "discovery at {url} answered HTTP {}",
                resp.status
            )));
        }
        let doc: serde_json::Value = resp
            .parse_json()
            .map_err(|e| Refusal::Provider(format!("discovery at {url}: {e}")))?;
        let issuer = doc["issuer"].as_str().map(crate::store::normalize_issuer);
        if issuer.as_deref() != Some(self.config.issuer.as_str()) {
            return Err(Refusal::Provider(format!(
                "the provider names its issuer {issuer:?}, not {:?} — check {ENV_ISSUER}",
                self.config.issuer
            )));
        }
        let issuer_host = host_of(&self.config.issuer);
        let endpoint = |key: &str| -> Result<String, Refusal> {
            let v = doc[key]
                .as_str()
                .ok_or_else(|| Refusal::Provider(format!("discovery has no {key}")))?;
            // Both endpoints live on the issuer's host (Keycloak serves them
            // under the realm): the token endpoint gets the secret, and the
            // authorization endpoint is where the person types a password.
            if !v.starts_with("https://") || host_of(v) != issuer_host {
                return Err(Refusal::Provider(format!(
                    "discovery's {key} {v:?} is not https on the issuer's host"
                )));
            }
            Ok(v.to_string())
        };
        let e = Endpoints {
            authorization: endpoint("authorization_endpoint")?,
            token: endpoint("token_endpoint")?,
        };
        *cached = Some(e.clone());
        Ok(e)
    }

    /// Begin a sign-in: where to send the browser, and the `state` to bind
    /// to it.
    pub async fn begin(
        &self,
        redirect_uri: &str,
        device_name: Option<String>,
    ) -> Result<(String, String), Refusal> {
        let endpoints = self.endpoints().await?;
        let state = random_b64(24);
        let nonce = random_b64(24);
        let verifier = random_b64(48);
        let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(<sha2::Sha256 as sha2::Digest>::digest(verifier.as_bytes()));
        {
            let mut pending = self.lock_pending();
            let now = Instant::now();
            pending.retain(|_, p| now.duration_since(p.started) < LOGIN_TTL);
            while pending.len() >= MAX_PENDING_LOGINS {
                let oldest = pending
                    .iter()
                    .min_by_key(|(_, p)| p.started)
                    .map(|(k, _)| k.clone());
                match oldest {
                    Some(k) => pending.remove(&k),
                    None => break,
                };
            }
            pending.insert(
                state.clone(),
                PendingLogin {
                    verifier,
                    nonce: nonce.clone(),
                    device_name,
                    started: now,
                },
            );
        }
        let sep = if endpoints.authorization.contains('?') {
            '&'
        } else {
            '?'
        };
        let url = format!(
            "{}{sep}response_type=code&client_id={}&redirect_uri={}&scope={}&state={}\
             &nonce={}&code_challenge={}&code_challenge_method=S256",
            endpoints.authorization,
            pct(&self.config.client_id),
            pct(redirect_uri),
            pct(&self.config.scopes),
            pct(&state),
            pct(&nonce),
            pct(&challenge),
        );
        Ok((url, state))
    }

    /// Finish a sign-in: spend `state`, exchange `code`, check the ID token.
    pub async fn finish(
        &self,
        redirect_uri: &str,
        code: &str,
        state: &str,
    ) -> Result<SignedIn, Refusal> {
        let login = {
            let mut pending = self.lock_pending();
            let mut hit = None;
            for key in pending.keys() {
                if super::auth::constant_time_eq(key.as_bytes(), state.as_bytes()) {
                    hit = Some(key.clone());
                }
            }
            hit.and_then(|k| pending.remove(&k))
        };
        let login = login
            .filter(|l| l.started.elapsed() < LOGIN_TTL)
            .ok_or_else(|| {
                Refusal::Invalid(
                    "this sign-in is unknown or has expired (a hub restart forgets every \
                     sign-in in flight); start again"
                        .into(),
                )
            })?;
        let endpoints = self.endpoints().await?;
        let mut form = vec![
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", redirect_uri),
            ("client_id", self.config.client_id.as_str()),
            ("code_verifier", login.verifier.as_str()),
        ];
        if let Some(secret) = self.config.client_secret.as_deref() {
            form.push(("client_secret", secret));
        }
        let body = form
            .iter()
            .map(|(k, v)| format!("{k}={}", pct(v)))
            .collect::<Vec<_>>()
            .join("&");
        let req = Request {
            method: Method::Post,
            url: endpoints.token.clone(),
            headers: vec![
                ("Accept".into(), "application/json".into()),
                (
                    "Content-Type".into(),
                    "application/x-www-form-urlencoded".into(),
                ),
            ],
            body: Some(body.into_bytes()),
            timeout: crate::net::https::DEFAULT_TIMEOUT,
        };
        let resp = self
            .transport
            .send(req)
            .await
            .map_err(|e| Refusal::Provider(format!("the token endpoint: {e}")))?;
        let tokens: serde_json::Value = resp
            .parse_json()
            .map_err(|e| Refusal::Provider(format!("the token endpoint: {e}")))?;
        if !resp.is_success() {
            // `error` / `error_description` are the provider's words (RFC
            // 6749 §5.2), never a credential.
            let what = tokens["error_description"]
                .as_str()
                .or(tokens["error"].as_str())
                .unwrap_or("no reason given");
            return Err(Refusal::Invalid(format!(
                "the provider refused the sign-in (HTTP {}): {}",
                resp.status,
                one_line(what)
            )));
        }
        let id_token = tokens["id_token"]
            .as_str()
            .ok_or_else(|| Refusal::Provider("the token response carries no id_token".into()))?;
        let claims = jwt_claims(id_token)
            .ok_or_else(|| Refusal::Provider("the id_token is not a readable JWT".into()))?;
        self.check_id_claims(&claims, &login.nonce, unix_now())?;
        // Keycloak puts realm and client roles in the ACCESS token
        // (`realm_access`, `resource_access`), not the ID token. It came
        // over the same TLS exchange, so its payload is read the same way.
        let access = tokens["access_token"].as_str().and_then(jwt_claims);
        let mut roles = roles_in(&claims, &self.config.client_id);
        if let Some(a) = &access {
            roles.extend(roles_in(a, &self.config.client_id));
        }
        roles.sort();
        roles.dedup();
        let text = |c: &serde_json::Value, k: &str| {
            c[k].as_str()
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(str::to_string)
        };
        Ok(SignedIn {
            subject: text(&claims, "sub")
                .ok_or_else(|| Refusal::Invalid("the id_token has no sub".into()))?,
            username: text(&claims, &self.config.username_claim),
            display_name: text(&claims, "name"),
            roles,
            device_name: login.device_name,
        })
    }

    /// The ID-token checks of OIDC Core §3.1.3.7 that apply to a token taken
    /// straight from the token endpoint.
    fn check_id_claims(&self, c: &serde_json::Value, nonce: &str, now: i64) -> Result<(), Refusal> {
        let bad = |why: &str| Err(Refusal::Invalid(format!("the id_token {why}")));
        let iss = c["iss"].as_str().map(crate::store::normalize_issuer);
        if iss.as_deref() != Some(self.config.issuer.as_str()) {
            return bad("names another issuer");
        }
        let aud: Vec<&str> = match &c["aud"] {
            serde_json::Value::String(s) => vec![s.as_str()],
            serde_json::Value::Array(a) => a.iter().filter_map(|v| v.as_str()).collect(),
            _ => vec![],
        };
        if !aud.contains(&self.config.client_id.as_str()) {
            return bad("is not addressed to this hub's client");
        }
        if let Some(azp) = c["azp"].as_str() {
            if azp != self.config.client_id {
                return bad("was issued to another client");
            }
        }
        match c["exp"].as_i64() {
            Some(exp) if exp + CLOCK_SKEW_SECS > now => {}
            Some(_) => return bad("has expired"),
            None => return bad("has no exp"),
        }
        if let Some(iat) = c["iat"].as_i64() {
            if iat > now + CLOCK_SKEW_SECS {
                return bad("was issued in the future (check the clocks)");
            }
        }
        match c["nonce"].as_str() {
            Some(n) if super::auth::constant_time_eq(n.as_bytes(), nonce.as_bytes()) => Ok(()),
            _ => bad("does not carry this sign-in's nonce"),
        }
    }

    /// Whose device the sign-in is for. Writes the person and the link on
    /// a first, auto-provisioned sign-in; refuses everything else this hub
    /// has not agreed to.
    pub fn resolve_person(&self, s: &Store, who: &SignedIn) -> Result<PersonRow, Refusal> {
        let cfg = &self.config;
        if !cfg.allowed_roles.is_empty() {
            let held = |want: &str| {
                who.roles
                    .iter()
                    .any(|r| r == want || r.trim_start_matches('/') == want.trim_start_matches('/'))
            };
            if !cfg.allowed_roles.iter().any(|r| held(r)) {
                return Err(Refusal::Forbidden(format!(
                    "your account holds none of the roles this hub admits ({}); ask your \
                     identity provider's admin",
                    cfg.allowed_roles.join(", ")
                )));
            }
        }
        let internal = |e: crate::ipc_error::IpcError| {
            tracing::error!(error = %e.message, "[oidc] store error during sign-in");
            Refusal::Provider("the hub could not read its store; try again".into())
        };
        if let Some(link) = s
            .get_identity(&cfg.issuer, &who.subject)
            .map_err(internal)?
        {
            let person = s.get_person(link.person_id).map_err(internal)?;
            return match person {
                Some(p) if p.disabled_at.is_none() => {
                    let _ = s.touch_identity_login(&cfg.issuer, &who.subject);
                    Ok(p)
                }
                _ => Err(Refusal::Forbidden(
                    "your account belongs to a person this hub has disabled".into(),
                )),
            };
        }
        let link_hint = format!(
            "fleet-hub person link-sso <person> --subject {}",
            who.subject
        );
        if !cfg.auto_provision {
            return Err(Refusal::Forbidden(format!(
                "this hub does not know your account yet. Ask its operator to run, on the \
                 hub machine: {link_hint}"
            )));
        }
        let Some(username) = who.username.as_deref() else {
            return Err(Refusal::Forbidden(format!(
                "your account has no {} claim to name you by; ask the operator to run: \
                 {link_hint}",
                cfg.username_claim
            )));
        };
        let name = crate::store::validate_person_name(username)
            .map_err(|e| Refusal::Forbidden(e.message))?;
        // An existing person is never taken over by name: anybody who can
        // set their username at the provider could otherwise become them.
        if s.get_person_by_name(&name).map_err(internal)?.is_some() {
            return Err(Refusal::Forbidden(format!(
                "a person named {name:?} already exists on this hub and is not linked to your \
                 account. If that is you, ask the operator to run: {}",
                link_hint.replace("<person>", &name)
            )));
        }
        s.atomically(|s| {
            let p = s.create_person(&name, who.display_name.as_deref())?;
            s.link_identity(&cfg.issuer, &who.subject, p.id)?;
            s.touch_identity_login(&cfg.issuer, &who.subject)?;
            Ok(p)
        })
        .map_err(internal)
    }
}

/// Unpadded base64url of `n` CSPRNG bytes.
fn random_b64(n: usize) -> String {
    use rand::Rng;
    let mut raw = vec![0u8; n];
    rand::rng().fill_bytes(&mut raw);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(raw)
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Percent-encode everything but RFC 3986's unreserved characters.
fn pct(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// A provider's text, as one bounded line.
fn one_line(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(200)
        .collect()
}

/// The payload of a compact JWS, unverified. See the module docs for why
/// that is enough here and only here.
fn jwt_claims(jwt: &str) -> Option<serde_json::Value> {
    let mut parts = jwt.split('.');
    let (_header, payload) = (parts.next()?, parts.next()?);
    parts.next()?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload.trim_end_matches('='))
        .ok()?;
    let v: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    v.is_object().then_some(v)
}

/// Keycloak's role and group claims: `realm_access.roles`,
/// `resource_access.<client>.roles`, and the flat `groups` / `roles` a
/// mapper adds.
fn roles_in(c: &serde_json::Value, client_id: &str) -> Vec<String> {
    let strings = |v: &serde_json::Value| -> Vec<String> {
        v.as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|r| r.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    };
    let mut out = strings(&c["realm_access"]["roles"]);
    out.extend(strings(&c["resource_access"][client_id]["roles"]));
    out.extend(strings(&c["groups"]));
    out.extend(strings(&c["roles"]));
    out
}

// --- the routes --------------------------------------------------------------

/// Where the provider sends the browser back.
fn redirect_uri(state: &PairState) -> String {
    format!(
        "{}/auth/oidc/callback",
        state.base_url.trim_end_matches('/')
    )
}

fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

fn page(status: StatusCode, title: &str, body_html: &str) -> Response {
    let html = format!(
        r#"<!doctype html>
<html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta name="robots" content="noindex, nofollow">
<meta name="referrer" content="no-referrer">
<title>{title}</title>
<style>
 :root {{ color-scheme: light dark }}
 body {{ margin: 0; padding: 2rem 1.25rem; font: 16px/1.55 system-ui, sans-serif; max-width: 34rem }}
 h1 {{ font-size: 1.25rem; margin: 0 0 1rem }}
 p {{ margin: 0 0 1rem }}
 .code {{ font: 600 2rem/1.2 ui-monospace, monospace; letter-spacing: .15em }}
 .button {{ display: inline-block; padding: .6rem 1rem; border-radius: .5rem; border: 1px solid currentColor; text-decoration: none }}
 .muted {{ opacity: .7; font-size: .875rem; word-break: break-all }}
</style></head><body>
<h1>{title}</h1>
{body_html}
</body></html>
"#,
        title = esc(title),
    );
    (
        status,
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8".to_string()),
            (header::CACHE_CONTROL, "no-store".to_string()),
            // The page is static apart from its escaped text: nothing on it
            // runs, and nothing may frame it.
            (
                header::CONTENT_SECURITY_POLICY,
                "default-src 'none'; style-src 'unsafe-inline'; frame-ancestors 'none'".to_string(),
            ),
        ],
        html,
    )
        .into_response()
}

fn refused(r: &Refusal) -> Response {
    page(
        r.status(),
        "Sign-in did not pair a device",
        &format!("<p>{}</p>", esc(r.message())),
    )
}

fn not_configured() -> Response {
    page(
        StatusCode::NOT_FOUND,
        "Single sign-on is off",
        "<p>This hub has no identity provider configured. Ask its operator for a pairing code \
         instead (<code>fleet-hub pair</code>).</p>",
    )
}

fn too_many(left: Duration) -> Response {
    (
        StatusCode::TOO_MANY_REQUESTS,
        [(header::RETRY_AFTER, left.as_secs().max(1).to_string())],
        "too many attempts",
    )
        .into_response()
}

fn peer_key(request_headers: &axum::http::HeaderMap, peer: Option<std::net::IpAddr>) -> String {
    super::pairing::limiter_key(peer, request_headers)
}

fn cookie_value<'a>(headers: &'a axum::http::HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|kv| kv.trim().split_once('='))
        .find(|(k, _)| *k == name)
        .map(|(_, v)| v)
}

/// `GET /auth/oidc/start[?name=<device>]` — unauthenticated, like `/pair`:
/// whoever arrives here has no credential yet.
pub async fn handle_start(
    axum::extract::State(state): axum::extract::State<PairState>,
    axum::extract::Query(q): axum::extract::Query<HashMap<String, String>>,
    request: axum::extract::Request,
) -> Response {
    let Some(oidc) = state.oidc.clone() else {
        return not_configured();
    };
    let peer = request
        .extensions()
        .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
        .map(|c| c.0.ip());
    let key = peer_key(request.headers(), peer);
    if let Err(left) = state.rate.check(
        &format!("oidc-start:{key}"),
        state.attempt_interval.min(ATTEMPT_INTERVAL),
    ) {
        return too_many(left);
    }
    let device_name = match q.get("name").map(|n| n.trim()).filter(|n| !n.is_empty()) {
        Some(n) => match crate::store::validate_client_name(n) {
            Ok(n) => Some(n),
            Err(e) => {
                return refused(&Refusal::Invalid(e.message));
            }
        },
        None => None,
    };
    let redirect = redirect_uri(&state);
    match oidc.begin(&redirect, device_name).await {
        Ok((url, login_state)) => {
            let secure = if redirect.starts_with("https://") {
                "; Secure"
            } else {
                ""
            };
            (
                StatusCode::FOUND,
                [
                    (header::LOCATION, url),
                    (header::CACHE_CONTROL, "no-store".to_string()),
                    (
                        header::SET_COOKIE,
                        format!(
                            "{STATE_COOKIE}={login_state}; Path={COOKIE_PATH}; Max-Age={}; \
                             HttpOnly; SameSite=Lax{secure}",
                            LOGIN_TTL.as_secs()
                        ),
                    ),
                ],
            )
                .into_response()
        }
        Err(r) => {
            tracing::warn!(error = %r.message(), "[oidc] could not start a sign-in");
            refused(&r)
        }
    }
}

/// `GET /auth/oidc/callback?code&state` — where the provider sends the
/// browser back. Ends in a pairing code, or in a page saying why not.
pub async fn handle_callback(
    axum::extract::State(state): axum::extract::State<PairState>,
    axum::extract::Query(q): axum::extract::Query<HashMap<String, String>>,
    request: axum::extract::Request,
) -> Response {
    let Some(oidc) = state.oidc.clone() else {
        return not_configured();
    };
    let peer = request
        .extensions()
        .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
        .map(|c| c.0.ip());
    let key = peer_key(request.headers(), peer);
    if let Err(left) = state.rate.check(
        &format!("oidc-callback:{key}"),
        state.attempt_interval.min(ATTEMPT_INTERVAL),
    ) {
        return too_many(left);
    }
    if let Some(err) = q.get("error") {
        let why = q
            .get("error_description")
            .map(String::as_str)
            .unwrap_or(err.as_str());
        return refused(&Refusal::Invalid(format!(
            "the identity provider ended the sign-in: {}",
            one_line(why)
        )));
    }
    let (Some(code), Some(login_state)) = (q.get("code"), q.get("state")) else {
        return refused(&Refusal::Invalid(
            "the provider sent no code or state back".into(),
        ));
    };
    // Login CSRF: the state must also be the one this browser was given.
    let bound = cookie_value(request.headers(), STATE_COOKIE)
        .is_some_and(|c| super::auth::constant_time_eq(c.as_bytes(), login_state.as_bytes()));
    if !bound {
        return refused(&Refusal::Invalid(
            "this sign-in was started in another browser (or its cookie was blocked); start \
             again from the same browser"
                .into(),
        ));
    }
    let who = match oidc.finish(&redirect_uri(&state), code, login_state).await {
        Ok(w) => w,
        Err(r) => {
            tracing::warn!(error = %r.message(), "[oidc] a sign-in failed");
            return refused(&r);
        }
    };
    let mode = oidc.config().mode.clone();
    let minted = {
        let Ok(s) = state.store.lock() else {
            tracing::error!("[oidc] store lock poisoned during sign-in");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        };
        let person = match oidc.resolve_person(&s, &who) {
            Ok(p) => p,
            Err(r) => {
                tracing::warn!(error = %r.message(), "[oidc] refused a sign-in");
                return refused(&r);
            }
        };
        let taken: Vec<String> = match s.active_client_tokens() {
            Ok(rows) => rows.into_iter().map(|c| c.name).collect(),
            Err(e) => {
                tracing::error!(error = %e.message, "[oidc] could not list clients");
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
        };
        let name = match who.device_name.as_deref() {
            Some(n) if taken.iter().any(|t| t == n) => {
                return refused(&Refusal::Invalid(format!(
                    "a device named {n:?} is already paired; pick another name"
                )));
            }
            Some(n) => n.to_string(),
            None => device_name_for(&person.name, &taken),
        };
        tracing::info!(
            person = person.id,
            client = %name,
            mode = %mode,
            "[oidc] signed in; minted a pairing code"
        );
        state.pairings.mint(MintRequest {
            name: &name,
            mode: &mode,
            trusted: false,
            org_id: None,
            person: Some(&person.name),
            ttl: CODE_TTL,
        })
    };
    let pair_url = super::pairing::pair_url(&state.base_url, &minted.code);
    let app_link = format!("claudefleet:{pair_url}");
    let body = format!(
        "<p>Signed in as <strong>{person}</strong>. This code pairs one device as \
         <strong>{name}</strong> ({mode}) and works once, for {mins} minutes:</p>\n\
         <p class=\"code\">{code}</p>\n\
         <p><a class=\"button\" href=\"{app}\">Open in the Orbit Fleet app</a></p>\n\
         <p>On a desktop, paste this pair address into <em>Settings → Hub &amp; sync</em>:</p>\n\
         <p class=\"muted\">{url}</p>",
        person = esc(minted.person.as_deref().unwrap_or("")),
        name = esc(&minted.name),
        mode = esc(&minted.mode),
        mins = CODE_TTL.as_secs() / 60,
        code = esc(&minted.code),
        app = esc(&app_link),
        url = esc(&pair_url),
    );
    let mut resp = page(StatusCode::OK, "Orbit Fleet sign-in", &body);
    // The state is spent: clear its cookie.
    if let Ok(v) =
        format!("{STATE_COOKIE}=; Path={COOKIE_PATH}; Max-Age=0; HttpOnly; SameSite=Lax").parse()
    {
        resp.headers_mut().insert(header::SET_COOKIE, v);
    }
    resp
}

/// A device name for a sign-in that named none: `<person>-sso`, then
/// `<person>-sso-2`, … — the first no live client holds. Bounded to the
/// client-name length by trimming the person part.
fn device_name_for(person: &str, taken: &[String]) -> String {
    let max = crate::store::MAX_CLIENT_NAME_LEN;
    let base: String = person
        .chars()
        .map(|c| if c.is_whitespace() { '-' } else { c })
        .take(max.saturating_sub(8))
        .collect();
    let mut n = 1;
    loop {
        let candidate = if n == 1 {
            format!("{base}-sso")
        } else {
            format!("{base}-sso-{n}")
        };
        if !taken.iter().any(|t| t == &candidate) {
            return candidate;
        }
        n += 1;
    }
}

#[cfg(test)]
mod tests;
