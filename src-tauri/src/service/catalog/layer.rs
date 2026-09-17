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

/// Every layer in the catalog, keyed by name, with `extends` resolution.
/// Invalid layers are dropped at construction and reported; the rest load.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LayerSet {
    layers: BTreeMap<String, Layer>,
}

impl LayerSet {
    /// Build from parsed layers. Returns the set plus one message per layer
    /// that was rejected (invalid key, self-contradiction, duplicate name).
    pub fn from_layers(layers: Vec<Layer>) -> (LayerSet, Vec<String>) {
        let mut set = LayerSet::default();
        let mut errors = Vec::new();
        for l in layers {
            if let Err(e) = l.validate() {
                errors.push(e);
                continue;
            }
            if set.layers.contains_key(&l.name) {
                errors.push(format!("duplicate layer name '{}'", l.name));
                continue;
            }
            set.layers.insert(l.name.clone(), l);
        }
        (set, errors)
    }

    pub fn get(&self, name: &str) -> Option<&Layer> {
        self.layers.get(name)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Layer> {
        self.layers.values()
    }

    pub fn is_empty(&self) -> bool {
        self.layers.is_empty()
    }

    /// Flatten `name`'s `extends` chain, ROOT FIRST. Walking here — rather
    /// than inside `resolve` — is what guarantees `resolve` can never loop:
    /// it only ever receives an already-valid chain.
    pub fn chain_for(&self, name: &str) -> Result<Vec<&Layer>, String> {
        let start = self
            .get(name)
            .ok_or_else(|| format!("unknown layer '{name}'"))?;
        let mut chain: Vec<&Layer> = Vec::new();
        let mut seen: Vec<&str> = Vec::new();
        let mut cur = Some(start);
        while let Some(l) = cur {
            if seen.contains(&l.name.as_str()) {
                return Err(format!(
                    "extends cycle in layer '{name}' (revisits '{}')",
                    l.name
                ));
            }
            seen.push(&l.name);
            chain.push(l);
            cur = match &l.extends {
                None => None,
                Some(parent) => {
                    let p = self.get(parent).ok_or_else(|| {
                        format!("layer '{}' extends unknown layer '{parent}'", l.name)
                    })?;
                    if p.axis != l.axis {
                        return Err(format!(
                            "layer '{}' ({}) extends '{parent}' on a different axis ({})",
                            l.name,
                            l.axis.as_str(),
                            p.axis.as_str()
                        ));
                    }
                    Some(p)
                }
            };
        }
        chain.reverse();
        Ok(chain)
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

    fn layer(name: &str, axis: &str, extends: Option<&str>) -> Layer {
        let ext = extends
            .map(|e| format!("extends: {e}\n"))
            .unwrap_or_default();
        Layer::from_yaml(&format!("kind: layer\nname: {name}\naxis: {axis}\n{ext}")).unwrap()
    }

    #[test]
    fn chain_for_returns_root_to_leaf() {
        let (set, errs) = LayerSet::from_layers(vec![
            layer("leaf", "role", Some("mid")),
            layer("mid", "role", Some("root")),
            layer("root", "role", None),
        ]);
        assert!(errs.is_empty(), "{errs:?}");
        let names: Vec<&str> = set
            .chain_for("leaf")
            .unwrap()
            .iter()
            .map(|l| l.name.as_str())
            .collect();
        assert_eq!(names, vec!["root", "mid", "leaf"]);
    }

    #[test]
    fn chain_for_detects_a_cycle_instead_of_hanging() {
        let (set, _) = LayerSet::from_layers(vec![
            layer("a", "role", Some("b")),
            layer("b", "role", Some("a")),
        ]);
        let err = set.chain_for("a").unwrap_err();
        assert!(err.contains("cycle"), "{err}");
    }

    #[test]
    fn chain_for_rejects_missing_parent_and_crossed_axes() {
        let (set, _) = LayerSet::from_layers(vec![
            layer("orphan", "role", Some("ghost")),
            layer("ctx", "context", None),
            layer("crossed", "role", Some("ctx")),
        ]);
        assert!(set.chain_for("orphan").unwrap_err().contains("ghost"));
        assert!(set.chain_for("crossed").unwrap_err().contains("axis"));
        assert!(set.chain_for("nope").unwrap_err().contains("nope"));
    }

    #[test]
    fn from_layers_reports_invalid_and_duplicate_layers_without_dropping_the_rest() {
        let bad =
            Layer::from_yaml("kind: layer\nname: bad\naxis: role\nmembers:\n  - nope/x\n").unwrap();
        let (set, errs) = LayerSet::from_layers(vec![
            layer("good", "role", None),
            bad,
            layer("good", "role", None),
        ]);
        assert!(set.get("good").is_some());
        assert!(set.get("bad").is_none());
        assert_eq!(errs.len(), 2, "{errs:?}");
        assert!(errs.iter().any(|e| e.contains("nope/x")));
        assert!(errs.iter().any(|e| e.contains("duplicate")));
    }
}
