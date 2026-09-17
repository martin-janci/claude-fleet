//! Layer definitions: composable membership + overrides over the catalog.

use crate::service::catalog::model::Kind;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Which axis a layer sits on. A host has exactly one active `Role` and any
/// number of `Context`s layered over it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Axis {
    Role,
    Context,
}

impl Axis {
    pub fn as_str(&self) -> &'static str {
        match self {
            Axis::Role => "role",
            Axis::Context => "context",
        }
    }
}

/// One `layers/<name>.yaml`. `members` / `exclude` / `overrides` are three
/// separate keys on purpose: "not a member" and "a member that is turned
/// off" must never be expressible as the same thing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Layer {
    pub name: String,
    pub axis: Axis,
    #[serde(default = "default_version")]
    pub version: String,
    #[serde(default)]
    pub description: String,
    /// 0..1 parent on the SAME axis. Cycles are caught at load (Task 2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extends: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub members: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclude: Vec<String>,
    /// `<kind>/<name>` → a partial asset mapping, deep-merged over the asset.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub overrides: BTreeMap<String, serde_yaml::Value>,
}

fn default_version() -> String {
    "1".to_string()
}

/// `kind: layer` in the file is a discriminator for humans and for the
/// authoring UI; it carries no data, so it is dropped on parse.
#[derive(Deserialize)]
struct LayerFile {
    #[allow(dead_code)]
    kind: Option<String>,
    #[serde(flatten)]
    layer: Layer,
}

/// Split a `<kind>/<name>` layer key. Shares its spelling with
/// `Manifest::split_key` — see the plan's Global Constraints.
pub fn split_key(key: &str) -> Option<(Kind, String)> {
    let (k, name) = key.split_once('/')?;
    if name.is_empty() {
        return None;
    }
    let kind = Kind::ALL.iter().copied().find(|kind| kind.as_str() == k)?;
    Some((kind, name.to_string()))
}

impl Layer {
    pub fn from_yaml(yaml: &str) -> Result<Layer, String> {
        let f: LayerFile = serde_yaml::from_str(yaml).map_err(|e| e.to_string())?;
        Ok(f.layer)
    }

    /// Static checks that need no catalog. Membership against real assets is
    /// checked at load (Task 2) and is a warning, not an error.
    pub fn validate(&self) -> Result<(), String> {
        for key in self.members.iter().chain(&self.exclude).chain(self.overrides.keys()) {
            if split_key(key).is_none() {
                return Err(format!("'{key}' is not a valid <kind>/<name> key"));
            }
        }
        for key in &self.members {
            if self.exclude.contains(key) {
                return Err(format!(
                    "'{key}' is in both members and exclude of layer '{}'",
                    self.name
                ));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_full_layer() {
        let l = Layer::from_yaml(
            "kind: layer\nname: server\naxis: role\ndescription: CI boxes\n\
             extends: core\nmembers:\n  - skill/argocd\nexclude:\n  - skill/airbnb\n\
             overrides:\n  skill/argocd:\n    version: \"2\"\n",
        )
        .unwrap();
        assert_eq!(l.name, "server");
        assert_eq!(l.axis, Axis::Role);
        assert_eq!(l.extends.as_deref(), Some("core"));
        assert_eq!(l.members, vec!["skill/argocd".to_string()]);
        assert_eq!(l.exclude, vec!["skill/airbnb".to_string()]);
        assert!(l.overrides.contains_key("skill/argocd"));
    }

    #[test]
    fn defaults_are_empty_and_axis_is_required() {
        let l = Layer::from_yaml("kind: layer\nname: core\naxis: context\n").unwrap();
        assert_eq!(l.axis, Axis::Context);
        assert!(l.members.is_empty() && l.exclude.is_empty() && l.overrides.is_empty());
        assert!(l.extends.is_none());
        assert!(Layer::from_yaml("kind: layer\nname: core\n").is_err());
        assert!(Layer::from_yaml("kind: layer\nname: core\naxis: bogus\n").is_err());
    }

    #[test]
    fn validate_rejects_bad_keys_and_self_contradiction() {
        // A key that names no known kind.
        let bad = Layer::from_yaml("kind: layer\nname: a\naxis: role\nmembers:\n  - nope/x\n")
            .unwrap();
        assert!(bad.validate().unwrap_err().contains("nope/x"));

        // Same key in members AND exclude of ONE layer is always a mistake.
        let clash = Layer::from_yaml(
            "kind: layer\nname: a\naxis: role\nmembers:\n  - skill/x\nexclude:\n  - skill/x\n",
        )
        .unwrap();
        assert!(clash.validate().unwrap_err().contains("both members and exclude"));

        // An overrides key must also be a valid <kind>/<name>.
        let ov = Layer::from_yaml(
            "kind: layer\nname: a\naxis: role\noverrides:\n  bogus:\n    version: \"2\"\n",
        )
        .unwrap();
        assert!(ov.validate().is_err());

        let good = Layer::from_yaml("kind: layer\nname: a\naxis: role\nmembers:\n  - skill/x\n")
            .unwrap();
        assert!(good.validate().is_ok());
    }
}
