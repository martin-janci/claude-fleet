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
//! canonical validators rather than inventing looser rules of its own — in
//! particular, `<tmux_name>` and `<client_name>` are parsed with a bounded
//! split (not exact segment counts), because both may legitimately contain
//! `/` themselves and an address grammar that rejected that would make a
//! validly named session unaddressable.

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
///
/// The split is bounded, not exact-arity: `kind` consumes exactly one
/// segment, and a session's `name` — like a client's — greedily absorbs
/// everything after it, `/` included. `host_alias_syntax` never allows a
/// `/` in a host, so the host/name boundary (the first `/` after `kind`) is
/// never ambiguous. Deciding what characters a name may contain is
/// `tmux_name_addressable`'s and `friendly_name`'s job, not this parser's —
/// the address grammar must not be stricter than the value it names, or a
/// legitimately named session (tmux allows `/` in a name) becomes
/// unaddressable.
pub fn parse(s: &str) -> Result<Addr, IpcError> {
    let mut top = s.splitn(3, '/');
    let fleet_raw = top.next().unwrap_or_default();
    let Some(kind) = top.next() else {
        return Err(bad(s));
    };
    let rest = top.next();

    match kind {
        "hub" => {
            // No `rest` at all: `/hub` is valid, `/hub/x` is not.
            if rest.is_some() {
                return Err(bad(s));
            }
            Ok(Addr::Hub {
                fleet: fleet_segment(fleet_raw, s)?,
            })
        }
        "client" => {
            // `rest` is the whole client name, `/` included.
            let Some(name) = rest else {
                return Err(bad(s));
            };
            let fleet = fleet_segment(fleet_raw, s)?;
            crate::validate::not_blank("client name", name).map_err(|_| bad(s))?;
            crate::validate::friendly_name(name).map_err(|_| bad(s))?;
            Ok(Addr::Client {
                fleet,
                name: name.to_string(),
            })
        }
        "session" => {
            let Some(rest) = rest else {
                return Err(bad(s));
            };
            // One more bounded split: `host` takes the first segment, `name`
            // greedily absorbs everything after it (again `/` included).
            let mut inner = rest.splitn(2, '/');
            let host = inner.next().unwrap_or_default();
            let Some(name) = inner.next() else {
                return Err(bad(s));
            };
            let fleet = fleet_segment(fleet_raw, s)?;
            crate::validate::host_alias_syntax(host).map_err(|_| bad(s))?;
            crate::validate::tmux_name_addressable(name).map_err(|_| bad(s))?;
            Ok(Addr::Session {
                fleet,
                host: host.to_string(),
                name: name.to_string(),
            })
        }
        _ => Err(bad(s)),
    }
}

/// The canonical string for an address. `parse` ∘ `render` is the identity:
/// `render` never puts a `/` before a host (fleet and kind are single
/// segments) and never puts one inside a host (`host_alias_syntax`
/// forbids it), so `parse`'s bounded splits always land on the same
/// boundaries `render` used, no matter what a `name` itself contains.
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

/// Settings key holding this fleet's identity.
pub const FLEET_ID_KEY: &str = "fleet.id";

/// This fleet's id if one has been minted, WITHOUT minting one. `None` when
/// the setting is absent or empty.
///
/// The read half of the pair (final review, Minor 7): minting writes to the
/// store, and `whoami` — which merely reports the id — is callable with a
/// readonly token. A readonly caller must never cause a database write, so
/// reporting uses this and says "not minted yet" rather than creating it.
pub fn stored_local_fleet_id(
    store: &std::sync::Mutex<crate::store::Store>,
) -> Result<Option<String>, IpcError> {
    let s = crate::ipc_error::lock(store)?;
    Ok(s.get_setting(FLEET_ID_KEY)?.filter(|v| !v.is_empty()))
}

/// This fleet's id, minted on first call and stable thereafter. Kept in
/// `settings` rather than a column: it is one value per store, and a
/// migration for it would buy nothing.
///
/// Named for the write it may do. Call it only where an address genuinely
/// has to be compared against this fleet's identity (`is_foreign`) — the
/// comparison is meaningless without one, and the caller is on a write path
/// already. To only report the id, use [`stored_local_fleet_id`].
pub fn ensure_local_fleet_id(
    store: &std::sync::Mutex<crate::store::Store>,
) -> Result<String, IpcError> {
    let s = crate::ipc_error::lock(store)?;
    if let Some(existing) = s.get_setting(FLEET_ID_KEY)? {
        if !existing.is_empty() {
            return Ok(existing);
        }
    }
    let id = uuid::Uuid::new_v4().to_string();
    s.set_setting(FLEET_ID_KEY, &id)?;
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;

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
            // A session or client name may itself contain `/` (tmux allows
            // it; `friendly_name` allows it) — the parser's bounded splits
            // must still round-trip these, not just the slash-free shapes.
            "/session/mac/x/y",
            "/client/a/b",
            "/session/mac/feature/foo",
        ] {
            assert_eq!(render(&parse(s).unwrap()), s, "round trip failed for {s}");
        }
    }

    #[test]
    fn a_slash_in_a_session_or_client_name_lands_in_the_name_field_not_the_grammar() {
        // Round-tripping the string isn't enough on its own — pin the parsed
        // `Addr`'s fields too, so a bounded-split regression that happened to
        // still render the same string would still be caught.
        assert_eq!(
            parse("/session/mac/feature/foo").unwrap(),
            Addr::Session {
                fleet: None,
                host: "mac".into(),
                name: "feature/foo".into()
            }
        );
        assert_eq!(
            parse("/client/a/b").unwrap(),
            Addr::Client {
                fleet: None,
                name: "a/b".into()
            }
        );
    }

    #[test]
    fn malformed_addresses_are_e_validate_and_never_panic() {
        for bad in [
            "",
            "/",
            "session/mac/x",
            "/session/mac",
            "/session//x",
            "/client",
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

    #[test]
    fn fleet_id_is_minted_once_and_then_stable() {
        let store = std::sync::Mutex::new(Store::open_in_memory().unwrap());
        let a = ensure_local_fleet_id(&store).unwrap();
        let b = ensure_local_fleet_id(&store).unwrap();
        assert_eq!(a, b, "the fleet id must not be re-minted");
        assert_eq!(a.len(), 36, "a uuid v4 in hyphenated form");
        // It parses as the fleet segment of an address.
        assert!(!is_foreign(&parse(&format!("{a}/hub")).unwrap(), &a));
    }

    /// Final review, Minor 7: `whoami` is callable with a readonly token, and
    /// a readonly caller must never cause a database write. Reading the fleet
    /// id therefore may not mint it.
    #[test]
    fn reading_the_fleet_id_never_mints_it() {
        let store = std::sync::Mutex::new(Store::open_in_memory().unwrap());
        assert_eq!(
            stored_local_fleet_id(&store).unwrap(),
            None,
            "nothing minted yet"
        );
        assert_eq!(
            store.lock().unwrap().get_setting(FLEET_ID_KEY).unwrap(),
            None,
            "and the read wrote nothing"
        );
        let minted = ensure_local_fleet_id(&store).unwrap();
        assert_eq!(stored_local_fleet_id(&store).unwrap(), Some(minted));
    }
}
