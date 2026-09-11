//! Typed registry of the operator-facing settings stored in the `settings`
//! table (key → string). Every key the Settings dialog can edit is declared
//! here with its default and value shape, so the Tauri command that writes a
//! setting can refuse unknown keys and garbage values, and every backend
//! reader (reconcile tick, playbooks, GC, project discovery) resolves the
//! same default.
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
    /// Integer seconds in `0..=MAX_SECS` (`0` usually means "disabled" /
    /// "never").
    Secs,
    /// One of a fixed set of strings.
    Choice(&'static [&'static str]),
    /// JSON object `{ "<host alias>": "<projects root>" }`. Every alias must
    /// pass `validate::host_alias`, every path `validate_base_path`. `{}`
    /// means "no per-host overrides".
    PathMap,
}

/// Upper bound for `Kind::Secs` (ten years): keeps every `secs as i64`
/// arithmetic in the sweeper far from overflow.
pub const MAX_SECS: u64 = 10 * 365 * 24 * 3600;

/// Upper bound on one projects-root path.
pub const MAX_PATH_LEN: usize = 1024;

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
/// Per-host projects root, one JSON map (host alias → path). A host with no
/// entry falls back to `$CLAUDE_FLEET_PROJECTS_BASE` (local only), then to the
/// layout default. See `service::projects::project_base_for`.
pub const PROJECTS_BASE_PATH: &str = "projects.base_path";
/// `github` (`<root>/<owner>/<repo>`) or `flat` (`<root>/<repo>`): where a
/// repository sits under the projects root. It does NOT choose the worktree
/// subdir inside a repo (`.worktrees/` vs `.claude/worktrees/`).
pub const PROJECTS_LAYOUT: &str = "projects.layout";

/// Derived, read-only entry `read_all` adds next to the registered keys: the
/// JSON map of host alias → resolved projects root, so the Settings dialog
/// can preview env/default fallbacks it cannot compute itself. Not in
/// `SPECS`, so `set` refuses it.
pub const PROJECTS_RESOLVED_BASE: &str = "projects.resolved_base";

/// Derived, read-only: `$CLAUDE_FLEET_PROJECTS_BASE` as the app sees it
/// (trimmed), or `""` when unset. Lets the Settings dialog preview the local
/// fallback for a layout that is not saved yet.
pub const PROJECTS_LOCAL_ENV_BASE: &str = "projects.local_env_base";

const LAYOUTS: &[&str] = &["github", "flat"];

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
    Spec {
        key: PROJECTS_BASE_PATH,
        default: "{}",
        kind: Kind::PathMap,
    },
    Spec {
        key: PROJECTS_LAYOUT,
        default: "github",
        kind: Kind::Choice(LAYOUTS),
    },
];

pub fn spec(key: &str) -> Option<&'static Spec> {
    SPECS.iter().find(|s| s.key == key)
}

/// Validate one projects-root path: absolute or `~` / `~/…` (expanded
/// against the host's `$HOME`), no control characters, no `..` component.
/// The value ends up inside remote shell commands (always quoted) and in a
/// local `read_dir`, so traversal and control bytes are refused outright.
pub fn validate_base_path(label: &str, path: &str) -> Result<(), IpcError> {
    let bad = |msg: &str| Err(IpcError::new("E_INVALID", format!("{label}: {msg}")));
    if path.is_empty() {
        return bad("path must not be empty");
    }
    if path.len() > MAX_PATH_LEN {
        return bad("path is too long");
    }
    if path.chars().any(char::is_control) {
        return bad("path must not contain control characters");
    }
    if !(path.starts_with('/') || path == "~" || path.starts_with("~/")) {
        return bad("path must be absolute or start with ~/");
    }
    if path.split('/').any(|c| c == "..") {
        return bad("path must not contain '..'");
    }
    Ok(())
}

/// Parse + validate a `Kind::PathMap` value into a map with trimmed paths.
pub fn parse_path_map(key: &str, raw: &str) -> Result<BTreeMap<String, String>, IpcError> {
    let map: BTreeMap<String, String> = serde_json::from_str(raw).map_err(|_| {
        IpcError::new(
            "E_INVALID",
            format!("{key} must be a JSON object of host alias to path strings"),
        )
    })?;
    let mut out = BTreeMap::new();
    for (alias, path) in map {
        crate::validate::host_alias(&alias)?;
        let path = path.trim();
        validate_base_path(&format!("{key}[{alias}]"), path)?;
        out.insert(alias, path.to_string());
    }
    Ok(out)
}

/// Validate a `(key, value)` pair against the registry. `E_INVALID` on an
/// unknown key or a value of the wrong shape.
pub fn validate(key: &str, value: &str) -> Result<(), IpcError> {
    let spec =
        spec(key).ok_or_else(|| IpcError::new("E_INVALID", format!("unknown setting {key}")))?;
    let v = value.trim();
    match spec.kind {
        Kind::Bool if v == "true" || v == "false" => Ok(()),
        Kind::Bool => Err(IpcError::new(
            "E_INVALID",
            format!("{key} must be \"true\" or \"false\""),
        )),
        Kind::Secs if v.parse::<u64>().is_ok_and(|n| n <= MAX_SECS) => Ok(()),
        Kind::Secs => Err(IpcError::new(
            "E_INVALID",
            format!("{key} must be an integer number of seconds between 0 and {MAX_SECS}"),
        )),
        Kind::Choice(options) if options.contains(&v) => Ok(()),
        Kind::Choice(options) => Err(IpcError::new(
            "E_INVALID",
            format!("{key} must be one of: {}", options.join(", ")),
        )),
        Kind::PathMap => parse_path_map(key, v).map(|_| ()),
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
    get_string(s, key) == "true"
}

pub fn get_secs(s: &Store, key: &str) -> u64 {
    get_string(s, key).parse().unwrap_or(0)
}

/// Effective value of a registered setting (stored or default).
pub fn get_string(s: &Store, key: &str) -> String {
    let raw = s.get_setting(key).ok().flatten();
    resolve(key, raw.as_deref())
}

/// The `projects.base_path` map (empty when unset or malformed).
pub fn base_path_map(s: &Store) -> BTreeMap<String, String> {
    parse_path_map(PROJECTS_BASE_PATH, &get_string(s, PROJECTS_BASE_PATH)).unwrap_or_default()
}

/// Every registered setting with its effective value (stored or default),
/// plus the derived `PROJECTS_RESOLVED_BASE` preview.
pub fn read_all(s: &Store) -> BTreeMap<String, String> {
    let mut all: BTreeMap<String, String> = SPECS
        .iter()
        .map(|spec| (spec.key.to_string(), get_string(s, spec.key)))
        .collect();
    all.insert(
        PROJECTS_LOCAL_ENV_BASE.to_string(),
        crate::service::projects::local_env_base().unwrap_or_default(),
    );
    let resolved = crate::service::projects::resolved_bases(s);
    all.insert(
        PROJECTS_RESOLVED_BASE.to_string(),
        serde_json::to_string(&resolved).unwrap_or_else(|_| "{}".into()),
    );
    all
}

/// Validate then persist one setting. A `PathMap` is stored normalised
/// (trimmed paths, sorted keys).
pub fn set(s: &Store, key: &str, value: &str) -> Result<(), IpcError> {
    validate(key, value)?;
    let v = value.trim();
    let stored = match spec(key).map(|sp| sp.kind) {
        Some(Kind::PathMap) => serde_json::to_string(&parse_path_map(key, v)?)
            .map_err(|e| IpcError::new("E_INVALID", e.to_string()))?,
        _ => v.to_string(),
    };
    s.set_setting(key, &stored)?;
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
        assert_eq!(
            validate(GC_BG_IDLE_SECS, "-1").unwrap_err().code,
            "E_INVALID"
        );
        assert_eq!(
            validate(GC_BG_IDLE_SECS, "1.5").unwrap_err().code,
            "E_INVALID"
        );
        assert!(validate(GC_BG_IDLE_SECS, " 3600 ").is_ok());
        assert!(validate(GC_BG_IDLE_SECS, &MAX_SECS.to_string()).is_ok());
        assert_eq!(
            validate(GC_BG_IDLE_SECS, &(MAX_SECS + 1).to_string())
                .unwrap_err()
                .code,
            "E_INVALID"
        );
        assert!(validate(PLAYBOOK_PRESS_ENTER, "true").is_ok());
    }

    #[test]
    fn layout_accepts_only_known_choices() {
        assert!(validate(PROJECTS_LAYOUT, "github").is_ok());
        assert!(validate(PROJECTS_LAYOUT, " flat ").is_ok());
        assert_eq!(
            validate(PROJECTS_LAYOUT, "gitlab").unwrap_err().code,
            "E_INVALID"
        );
        assert_eq!(resolve(PROJECTS_LAYOUT, Some("nope")), "github");
    }

    #[test]
    fn base_path_accepts_absolute_and_home_relative() {
        for ok in [
            "/srv/repos",
            "~",
            "~/code",
            "~/projects/github.com",
            "/a/./b",
        ] {
            assert!(validate_base_path("p", ok).is_ok(), "{ok}");
        }
    }

    #[test]
    fn base_path_rejects_relative_traversal_and_control_chars() {
        for bad in [
            "",
            "code",
            "./code",
            "~user/code",
            "-oProxyCommand=x",
            "/srv/../etc",
            "~/..",
            "/srv/a\nb",
            "/srv/a\u{7}b",
        ] {
            assert_eq!(
                validate_base_path("p", bad).unwrap_err().code,
                "E_INVALID",
                "{bad:?}"
            );
        }
        assert!(validate_base_path("p", &format!("/{}", "a".repeat(MAX_PATH_LEN))).is_err());
    }

    #[test]
    fn path_map_validates_aliases_paths_and_shape() {
        assert!(validate(PROJECTS_BASE_PATH, "{}").is_ok());
        assert!(validate(PROJECTS_BASE_PATH, r#"{"local":"/srv","vps-1":"~/code"}"#).is_ok());
        for bad in [
            "",
            "[]",
            "\"/srv\"",
            r#"{"local":1}"#,
            r#"{"-oProxy":"/srv"}"#,
            r#"{"local":"relative"}"#,
            r#"{"local":"/a/../b"}"#,
        ] {
            assert_eq!(
                validate(PROJECTS_BASE_PATH, bad).unwrap_err().code,
                "E_INVALID",
                "{bad}"
            );
        }
        // garbage stored value falls back to "no overrides"
        assert_eq!(resolve(PROJECTS_BASE_PATH, Some("{oops")), "{}");
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
        // every spec plus the two derived projects preview entries
        assert_eq!(all.len(), SPECS.len() + 2);
        assert!(all.contains_key(PROJECTS_LOCAL_ENV_BASE));
        assert_eq!(
            set(&s, PROJECTS_LOCAL_ENV_BASE, "/x").unwrap_err().code,
            "E_INVALID"
        );
        assert_eq!(all[GC_BG_IDLE_SECS], "86400");
        assert_eq!(all[PROJECTS_BASE_PATH], "{}");
        assert_eq!(all[PROJECTS_LAYOUT], "github");
        assert!(all[PROJECTS_RESOLVED_BASE].contains("\"local\""));
        assert!(!get_bool(&s, GC_ENABLED));
        assert_eq!(get_secs(&s, GC_SWEEP_INTERVAL_SECS), 300);

        set(&s, GC_ENABLED, "true").unwrap();
        set(&s, GC_BG_IDLE_SECS, "60").unwrap();
        assert!(get_bool(&s, GC_ENABLED));
        assert_eq!(get_secs(&s, GC_BG_IDLE_SECS), 60);
        assert_eq!(set(&s, "nope", "1").unwrap_err().code, "E_INVALID");
        // the derived preview key is not writable
        assert_eq!(
            set(&s, PROJECTS_RESOLVED_BASE, "{}").unwrap_err().code,
            "E_INVALID"
        );
    }

    #[test]
    fn path_map_is_stored_normalised() {
        let s = Store::open_in_memory().unwrap();
        set(
            &s,
            PROJECTS_BASE_PATH,
            r#" {"vps":" ~/code ","local":"/srv"} "#,
        )
        .unwrap();
        assert_eq!(
            s.get_setting(PROJECTS_BASE_PATH).unwrap().as_deref(),
            Some(r#"{"local":"/srv","vps":"~/code"}"#)
        );
        assert_eq!(base_path_map(&s)["vps"], "~/code");
        // a rejected write leaves the stored value alone
        assert!(set(&s, PROJECTS_BASE_PATH, r#"{"vps":"../x"}"#).is_err());
        assert_eq!(base_path_map(&s)["vps"], "~/code");
    }
}
