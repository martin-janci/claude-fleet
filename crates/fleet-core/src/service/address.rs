//! The fleet address: one string naming any addressable endpoint.
//!
//! ```text
//! <fleet>/session/<host_alias>/<tmux_name>
//! <fleet>/client/<client_name>
//! <fleet>/hub
//! ```
//!
//! An empty `<fleet>` means "this fleet", so every call written before
//! addresses existed keeps working and cycle 3 (hub↔hub) adds federation
//! without another schema change.
//!
//! This is a RESOLUTION KEY, never a stored foreign key: a session move
//! changes both the host alias and the row id, so the durable thing is the
//! participant (see `store/participants.rs`).
//!
//! The parser is the only new way to name a session, so it delegates to the
//! canonical validators rather than inventing looser rules of its own.

use crate::ipc_error::{codes, IpcError};

/// A parsed address. `fleet: None` means the local fleet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Addr {
    Session {
        fleet: Option<String>,
        host: String,
        name: String,
    },
    Client {
        fleet: Option<String>,
        name: String,
    },
    Hub {
        fleet: Option<String>,
    },
}

fn bad(s: &str) -> IpcError {
    IpcError::new(
        codes::E_VALIDATE,
        format!(
            "malformed address {s:?}: expected <fleet>/session/<host>/<name>, \
             <fleet>/client/<name> or <fleet>/hub"
        ),
    )
}

/// Max length of the fleet segment: a UUID v4 without braces is 36 chars.
const FLEET_MAX: usize = 36;

fn fleet_segment(raw: &str, whole: &str) -> Result<Option<String>, IpcError> {
    if raw.is_empty() {
        return Ok(None);
    }
    if raw.len() > FLEET_MAX || !raw.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Err(bad(whole));
    }
    Ok(Some(raw.to_string()))
}

/// Parse an address. Every malformed input is `E_VALIDATE`; nothing panics.
pub fn parse(s: &str) -> Result<Addr, IpcError> {
    let parts: Vec<&str> = s.split('/').collect();
    match parts.as_slice() {
        [fleet, "hub"] => Ok(Addr::Hub {
            fleet: fleet_segment(fleet, s)?,
        }),
        [fleet, "client", name] => {
            let fleet = fleet_segment(fleet, s)?;
            crate::validate::not_blank("client name", name).map_err(|_| bad(s))?;
            crate::validate::friendly_name(name).map_err(|_| bad(s))?;
            Ok(Addr::Client {
                fleet,
                name: (*name).to_string(),
            })
        }
        [fleet, "session", host, name] => {
            let fleet = fleet_segment(fleet, s)?;
            crate::validate::host_alias_syntax(host).map_err(|_| bad(s))?;
            crate::validate::tmux_name_addressable(name).map_err(|_| bad(s))?;
            Ok(Addr::Session {
                fleet,
                host: (*host).to_string(),
                name: (*name).to_string(),
            })
        }
        _ => Err(bad(s)),
    }
}

/// The canonical string for an address. `parse` ∘ `render` is the identity.
pub fn render(a: &Addr) -> String {
    let f = |fleet: &Option<String>| fleet.clone().unwrap_or_default();
    match a {
        Addr::Hub { fleet } => format!("{}/hub", f(fleet)),
        Addr::Client { fleet, name } => format!("{}/client/{name}", f(fleet)),
        Addr::Session { fleet, host, name } => {
            format!("{}/session/{host}/{name}", f(fleet))
        }
    }
}

/// True when the address names a fleet that is explicitly not this one. An
/// implicit fleet is always local, so today's callers are never foreign.
pub fn is_foreign(a: &Addr, local_fleet: &str) -> bool {
    let fleet = match a {
        Addr::Hub { fleet } | Addr::Client { fleet, .. } | Addr::Session { fleet, .. } => fleet,
    };
    fleet.as_deref().is_some_and(|f| f != local_fleet)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_three_shapes_with_an_implicit_local_fleet() {
        assert_eq!(
            parse("/session/mac/dev-foo").unwrap(),
            Addr::Session {
                fleet: None,
                host: "mac".into(),
                name: "dev-foo".into()
            }
        );
        assert_eq!(
            parse("/client/phone").unwrap(),
            Addr::Client {
                fleet: None,
                name: "phone".into()
            }
        );
        assert_eq!(parse("/hub").unwrap(), Addr::Hub { fleet: None });
    }

    #[test]
    fn parses_an_explicit_fleet() {
        assert_eq!(
            parse("f00d/session/mefistos/api").unwrap(),
            Addr::Session {
                fleet: Some("f00d".into()),
                host: "mefistos".into(),
                name: "api".into()
            }
        );
    }

    #[test]
    fn render_round_trips_every_shape() {
        for s in [
            "/session/mac/dev-foo",
            "/client/phone",
            "/hub",
            "f00d/session/m/a",
            "f00d/hub",
        ] {
            assert_eq!(render(&parse(s).unwrap()), s, "round trip failed for {s}");
        }
    }

    #[test]
    fn malformed_addresses_are_e_validate_and_never_panic() {
        for bad in [
            "",
            "/",
            "session/mac/x",
            "/session/mac",
            "/session//x",
            "/session/mac/x/y",
            "/client",
            "/client/a/b",
            "/hub/x",
            "/nope/a",
            "/session/mac/../x",
            "/session/ma c/x",
            "/session/mac/na\nme",
            "a/b/c/d/e",
        ] {
            let err = parse(bad).unwrap_err();
            assert_eq!(err.code, "E_VALIDATE", "expected E_VALIDATE for {bad:?}");
        }
    }

    #[test]
    fn a_host_alias_and_tmux_name_are_validated_by_the_canonical_validators() {
        // Anything host_alias/tmux_name_addressable rejects must not parse:
        // the address is the only new way to name a session, so it may not be
        // a way around those checks.
        assert_eq!(parse("/session/-bad/x").unwrap_err().code, "E_VALIDATE");
        assert_eq!(parse("/session/mac/-x").unwrap_err().code, "E_VALIDATE");
    }

    #[test]
    fn is_foreign_only_when_an_explicit_fleet_differs() {
        let local = parse("/session/mac/x").unwrap();
        let same = parse("abc/session/mac/x").unwrap();
        let other = parse("zzz/session/mac/x").unwrap();
        assert!(
            !is_foreign(&local, "abc"),
            "an implicit fleet is always local"
        );
        assert!(!is_foreign(&same, "abc"));
        assert!(is_foreign(&other, "abc"));
    }
}
