//! Typed registry of the operator-facing settings stored in the `settings`
//! table (key → string). Every key the Settings dialog can edit is declared
//! here with its default and value shape, so the Tauri command that writes a
//! setting can refuse unknown keys and garbage values, and every backend
//! reader (reconcile tick, playbooks, GC) resolves the same default.
//!
//! Keys that other subsystems own (MCP `mcp.*`, `controller.*`) are NOT
//! listed and cannot be written through this path.

use crate::ipc_error::IpcError;
use crate::store::Store;
use std::collections::BTreeMap;

/// Value shape a setting accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// `"true"` / `"false"`.
    Bool,
    /// Non-negative integer seconds (`0` usually means "disabled" / "never").
    Secs,
}

#[derive(Debug, Clone, Copy)]
pub struct Spec {
    pub key: &'static str,
    pub default: &'static str,
    pub kind: Kind,
}

// ── keys ──
pub const RECONCILE_INTERVAL_SECS: &str = "reconcile.interval_secs";
pub const PLAYBOOK_PRESS_ENTER: &str = "playbooks.press_enter";
pub const PLAYBOOK_OOM_RECREATE: &str = "playbooks.oom_recreate";
pub const GC_ENABLED: &str = "gc.enabled";
pub const GC_BG_IDLE_SECS: &str = "gc.bg_idle_secs";
pub const GC_SHELL_IDLE_SECS: &str = "gc.shell_idle_secs";
pub const GC_WORK_IDLE_SECS: &str = "gc.work_idle_secs";
pub const GC_SWEEP_INTERVAL_SECS: &str = "gc.sweep_interval_secs";

/// Every editable setting. Order is the display order.
pub const SPECS: &[Spec] = &[
    Spec {
        key: RECONCILE_INTERVAL_SECS,
        default: "20",
        kind: Kind::Secs,
    },
    Spec {
        key: PLAYBOOK_PRESS_ENTER,
        default: "false",
        kind: Kind::Bool,
    },
    Spec {
        key: PLAYBOOK_OOM_RECREATE,
        default: "false",
        kind: Kind::Bool,
    },
    Spec {
        key: GC_ENABLED,
        default: "false",
        kind: Kind::Bool,
    },
    Spec {
        key: GC_BG_IDLE_SECS,
        default: "86400",
        kind: Kind::Secs,
    },
    Spec {
        key: GC_SHELL_IDLE_SECS,
        default: "604800",
        kind: Kind::Secs,
    },
    Spec {
        key: GC_WORK_IDLE_SECS,
        default: "0",
        kind: Kind::Secs,
    },
    Spec {
        key: GC_SWEEP_INTERVAL_SECS,
        default: "300",
        kind: Kind::Secs,
    },
];

pub fn spec(key: &str) -> Option<&'static Spec> {
    SPECS.iter().find(|s| s.key == key)
}

/// Validate a `(key, value)` pair against the registry. `E_INVALID` on an
/// unknown key or a value of the wrong shape.
pub fn validate(key: &str, value: &str) -> Result<(), IpcError> {
    let spec = spec(key).ok_or_else(|| IpcError::new("E_INVALID", format!("unknown setting {key}")))?;
    let v = value.trim();
    match spec.kind {
        Kind::Bool if v == "true" || v == "false" => Ok(()),
        Kind::Bool => Err(IpcError::new(
            "E_INVALID",
            format!("{key} must be \"true\" or \"false\""),
        )),
        Kind::Secs if v.parse::<u64>().is_ok() => Ok(()),
        Kind::Secs => Err(IpcError::new(
            "E_INVALID",
            format!("{key} must be a non-negative integer (seconds)"),
        )),
    }
}

/// Pure: resolve a raw stored value (or `None`) against its spec, falling
/// back to the default when missing or malformed.
pub fn resolve(key: &str, raw: Option<&str>) -> String {
    let Some(spec) = spec(key) else {
        return raw.unwrap_or_default().to_string();
    };
    match raw.map(str::trim) {
        Some(v) if validate(key, v).is_ok() => v.to_string(),
        _ => spec.default.to_string(),
    }
}

pub fn get_bool(s: &Store, key: &str) -> bool {
    let raw = s.get_setting(key).ok().flatten();
    resolve(key, raw.as_deref()) == "true"
}

pub fn get_secs(s: &Store, key: &str) -> u64 {
    let raw = s.get_setting(key).ok().flatten();
    resolve(key, raw.as_deref()).parse().unwrap_or(0)
}

/// Every registered setting with its effective value (stored or default).
pub fn read_all(s: &Store) -> BTreeMap<String, String> {
    SPECS
        .iter()
        .map(|spec| {
            let raw = s.get_setting(spec.key).ok().flatten();
            (spec.key.to_string(), resolve(spec.key, raw.as_deref()))
        })
        .collect()
}

/// Validate then persist one setting.
pub fn set(s: &Store, key: &str, value: &str) -> Result<(), IpcError> {
    validate(key, value)?;
    s.set_setting(key, value.trim())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_default_validates_against_its_own_spec() {
        for spec in SPECS {
            assert!(validate(spec.key, spec.default).is_ok(), "{}", spec.key);
        }
    }

    #[test]
    fn validate_rejects_unknown_keys_and_bad_shapes() {
        assert_eq!(validate("mcp.token", "x").unwrap_err().code, "E_INVALID");
        assert_eq!(validate(GC_ENABLED, "yes").unwrap_err().code, "E_INVALID");
        assert_eq!(validate(GC_BG_IDLE_SECS, "-1").unwrap_err().code, "E_INVALID");
        assert_eq!(validate(GC_BG_IDLE_SECS, "1.5").unwrap_err().code, "E_INVALID");
        assert!(validate(GC_BG_IDLE_SECS, " 3600 ").is_ok());
        assert!(validate(PLAYBOOK_PRESS_ENTER, "true").is_ok());
    }

    #[test]
    fn resolve_falls_back_to_default_on_missing_or_garbage() {
        assert_eq!(resolve(GC_ENABLED, None), "false");
        assert_eq!(resolve(GC_ENABLED, Some("maybe")), "false");
        assert_eq!(resolve(GC_ENABLED, Some("true")), "true");
        assert_eq!(resolve(GC_WORK_IDLE_SECS, Some("abc")), "0");
        assert_eq!(resolve(GC_SHELL_IDLE_SECS, None), "604800");
    }

    #[test]
    fn store_roundtrip_and_read_all_defaults() {
        let s = Store::open_in_memory().unwrap();
        let all = read_all(&s);
        assert_eq!(all.len(), SPECS.len());
        assert_eq!(all[GC_BG_IDLE_SECS], "86400");
        assert!(!get_bool(&s, GC_ENABLED));
        assert_eq!(get_secs(&s, GC_SWEEP_INTERVAL_SECS), 300);

        set(&s, GC_ENABLED, "true").unwrap();
        set(&s, GC_BG_IDLE_SECS, "60").unwrap();
        assert!(get_bool(&s, GC_ENABLED));
        assert_eq!(get_secs(&s, GC_BG_IDLE_SECS), 60);
        assert_eq!(set(&s, "nope", "1").unwrap_err().code, "E_INVALID");
    }
}
