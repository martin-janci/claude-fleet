//! Named Control API tokens (Orbit Fleet M15 step G2.8): create, list and
//! revoke the tokens a script or another agent uses to drive the fleet.
//!
//! The token is minted here, returned ONCE in [`Created`], and stored only
//! as its SHA-256 (`store::control_tokens`). Nothing here logs it: the
//! callers audit the name, scope, expiry and hosts, never the value.

use crate::ipc_error::{codes, lock, IpcError};
use crate::store::{ApiScope, ControlTokenRow, NewControlToken, Store};
use rmcp::schemars;
use std::sync::Mutex;

/// Prefix of every named token, so a leaked one is recognisable (and a
/// secret scanner can match it). The master and per-host tokens keep their
/// bare hex.
pub const TOKEN_PREFIX: &str = "flt_live_";

/// The environment variable "Copy as env line" names: the one fleet's own
/// hook and MCP templates read the control API token from.
pub const ENV_VAR: &str = "FLEET_MCP_TOKEN";

/// Longest expiry a token may be given, in days (ten years); `None` is
/// "never".
pub const MAX_EXPIRY_DAYS: u32 = 3650;

/// What [`create`] is asked for.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct CreateArgs {
    /// 1-64 characters, unique among live tokens.
    pub name: String,
    /// read | act | admin.
    pub scope: String,
    /// Days until it stops working, 1-3650; omitted = never.
    #[serde(default)]
    pub expires_in_days: Option<u32>,
    /// Host aliases it may reach; omitted = every host. Not for admin.
    #[serde(default)]
    pub hosts: Option<Vec<String>>,
}

/// A created token: the row, and the token itself, shown once.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Created {
    pub token: String,
    /// `FLEET_MCP_TOKEN=<token>`, for "Copy as env line".
    pub env_line: String,
    #[serde(flatten)]
    pub row: ControlTokenRow,
}

/// `api_tokens`' arguments: the MCP tool's and the desktop command's.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct ApiTokensArgs {
    /// list | create | revoke.
    pub action: String,
    /// create: the new token's name; revoke: the live token to end.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// create: read | act | admin.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    /// create: days until it stops working, 1-3650; omitted = never.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_in_days: Option<u32>,
    /// create: host aliases it may reach; omitted = every host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hosts: Option<Vec<String>>,
}

impl ApiTokensArgs {
    /// `list` changes nothing; the other actions hand out or take away
    /// fleet access.
    pub fn is_read(&self) -> bool {
        self.action == "list"
    }
}

/// Run one `api_tokens` call. `minter` is the caller's right to create and
/// revoke, or the refusal a `list`-only caller gets on those actions.
pub fn run(
    store: &Mutex<Store>,
    minter: Result<Minter, IpcError>,
    args: &ApiTokensArgs,
    now: i64,
) -> Result<serde_json::Value, IpcError> {
    let need = |v: &Option<String>, what: &str| -> Result<String, IpcError> {
        v.as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .ok_or_else(|| IpcError::new(codes::E_INVALID, format!("{} needs {what}", args.action)))
    };
    match args.action.as_str() {
        "list" => json(&list(store)?),
        "create" => {
            let minter = minter?;
            let created = create(
                store,
                minter,
                &CreateArgs {
                    name: need(&args.name, "name")?,
                    scope: need(&args.scope, "scope")?,
                    expires_in_days: args.expires_in_days,
                    hosts: args.hosts.clone(),
                },
                now,
            )?;
            json(&created)
        }
        "revoke" => {
            minter?;
            json(&revoke(store, &need(&args.name, "name")?)?)
        }
        other => Err(IpcError::new(
            codes::E_VALIDATE,
            format!(
                "unknown action {:?}: list | create | revoke",
                other.escape_debug()
            ),
        )),
    }
}

fn json<T: serde::Serialize>(v: &T) -> Result<serde_json::Value, IpcError> {
    serde_json::to_value(v).map_err(|e| IpcError::new(codes::E_INTERNAL, e.to_string()))
}

/// Who is asking, as far as minting goes: the most a caller may hand out is
/// what it can do itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Minter {
    /// The master token (or an admin token): any scope.
    Admin,
    /// The owner's own trusted, full paired device: read or act, never
    /// admin, which would hand a device the fleet-admin tools it is refused.
    OwnerDevice,
}

/// Create a named token. `E_VALIDATE` for a bad name, scope, expiry or host
/// list; `E_FORBIDDEN` when `minter` may not hand out `scope`; `E_NOTFOUND`
/// for a host the fleet does not know.
pub fn create(
    store: &Mutex<Store>,
    minter: Minter,
    args: &CreateArgs,
    now: i64,
) -> Result<Created, IpcError> {
    let scope = ApiScope::parse_strict(args.scope.trim())?;
    if scope == ApiScope::Admin && minter != Minter::Admin {
        return Err(IpcError::new(
            codes::E_FORBIDDEN,
            "only the master token creates an admin token; this device may create read or act",
        ));
    }
    let expires_at = match args.expires_in_days {
        None => None,
        Some(d) if (1..=MAX_EXPIRY_DAYS).contains(&d) => Some(now + i64::from(d) * 86_400),
        Some(d) => {
            return Err(IpcError::new(
                codes::E_VALIDATE,
                format!("expires_in_days {d} must be 1-{MAX_EXPIRY_DAYS}, or omitted for never"),
            ))
        }
    };
    let hosts = match &args.hosts {
        None => None,
        Some(list) => {
            let mut out: Vec<String> = Vec::new();
            for h in list.iter().map(|h| h.trim()) {
                if !h.is_empty() && !out.iter().any(|x| x == h) {
                    out.push(h.to_string());
                }
            }
            if out.is_empty() {
                return Err(IpcError::new(
                    codes::E_VALIDATE,
                    "hosts must name at least one host, or be omitted for every host",
                ));
            }
            Some(out)
        }
    };
    let token = format!("{TOKEN_PREFIX}{}", crate::mcp::generate_token());
    let sha = crate::mcp::auth::sha256_hex(&token);
    let s = lock(store)?;
    if let Some(list) = &hosts {
        for h in list {
            if s.get_host_row(h)?.is_none() {
                return Err(IpcError::new(
                    codes::E_NOTFOUND,
                    format!("no host named '{}'", h.escape_debug()),
                ));
            }
        }
    }
    let row = s.insert_control_token(&NewControlToken {
        name: &args.name,
        token_sha256: &sha,
        scope,
        hosts: hosts.as_deref(),
        expires_at,
    })?;
    Ok(Created {
        env_line: format!("{ENV_VAR}={token}"),
        token,
        row,
    })
}

/// Every live named token, newest first.
pub fn list(store: &Mutex<Store>) -> Result<Vec<ControlTokenRow>, IpcError> {
    lock(store)?.list_control_tokens()
}

/// Revoke the live token named `name`: its next request is refused.
pub fn revoke(store: &Mutex<Store>, name: &str) -> Result<ControlTokenRow, IpcError> {
    lock(store)?.revoke_control_token(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> Mutex<Store> {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("mercury").unwrap();
        Mutex::new(s)
    }

    fn args(name: &str, scope: &str) -> CreateArgs {
        CreateArgs {
            name: name.into(),
            scope: scope.into(),
            expires_in_days: None,
            hosts: None,
        }
    }

    #[test]
    fn a_created_token_is_shown_once_and_stored_as_its_hash() {
        let st = store();
        let c = create(
            &st,
            Minter::Admin,
            &CreateArgs {
                expires_in_days: Some(90),
                hosts: Some(vec!["mercury".into(), " mercury ".into()]),
                ..args("grafana", "read")
            },
            1_000,
        )
        .unwrap();
        assert!(c.token.starts_with(TOKEN_PREFIX) && c.token.len() == TOKEN_PREFIX.len() + 64);
        assert_eq!(c.env_line, format!("FLEET_MCP_TOKEN={}", c.token));
        assert_eq!(c.row.expires_at, Some(1_000 + 90 * 86_400));
        assert_eq!(c.row.hosts, Some(vec!["mercury".to_string()]));
        let listed = list(&st).unwrap();
        assert_eq!(
            listed[0].token_sha256,
            crate::mcp::auth::sha256_hex(&c.token)
        );
        let json = serde_json::to_string(&listed).unwrap();
        assert!(!json.contains(&c.token), "the list never carries the token");
    }

    #[test]
    fn a_device_cannot_mint_admin_and_bad_input_is_refused() {
        let st = store();
        let e = create(&st, Minter::OwnerDevice, &args("root", "admin"), 0).unwrap_err();
        assert_eq!(e.code, codes::E_FORBIDDEN);
        assert!(create(&st, Minter::OwnerDevice, &args("ci", "act"), 0).is_ok());
        assert_eq!(
            create(&st, Minter::Admin, &args("x", "write"), 0)
                .unwrap_err()
                .code,
            codes::E_VALIDATE
        );
        let never = CreateArgs {
            expires_in_days: Some(0),
            ..args("x", "read")
        };
        assert_eq!(
            create(&st, Minter::Admin, &never, 0).unwrap_err().code,
            codes::E_VALIDATE
        );
        let none = CreateArgs {
            hosts: Some(vec![" ".into()]),
            ..args("x", "read")
        };
        assert_eq!(
            create(&st, Minter::Admin, &none, 0).unwrap_err().code,
            codes::E_VALIDATE
        );
        let ghost = CreateArgs {
            hosts: Some(vec!["pluto".into()]),
            ..args("x", "read")
        };
        assert_eq!(
            create(&st, Minter::Admin, &ghost, 0).unwrap_err().code,
            codes::E_NOTFOUND
        );
        let limited_admin = CreateArgs {
            hosts: Some(vec!["mercury".into()]),
            ..args("x", "admin")
        };
        assert_eq!(
            create(&st, Minter::Admin, &limited_admin, 0)
                .unwrap_err()
                .code,
            codes::E_INVALID
        );
        assert_eq!(revoke(&st, "ci").unwrap().name, "ci");
        assert!(list(&st).unwrap().is_empty());
    }
}
