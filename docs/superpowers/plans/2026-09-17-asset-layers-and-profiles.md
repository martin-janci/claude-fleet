# Composable Asset Layers Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let one catalog serve differently-configured machines by adding composable layers — a per-host *role* plus switchable *contexts* — that resolve to an effective catalog before sync plans anything.

**Architecture:** Layer definitions are catalog content (`layers/*.yaml`, git-reviewable); host→layer assignment is fleet state (SQLite). A pure `resolve()` turns (catalog, role chain, contexts) into a plain `Catalog`, so the entire existing plan/apply/manifest path runs over it unchanged. Removal on layer-disable falls out of the existing `Manifest::orphans` mechanism for free.

**Tech Stack:** Rust (Tauri 2 backend), `serde_yaml`, `rusqlite`, `rmcp` for MCP tools. No frontend work in this plan.

**Spec:** `docs/superpowers/specs/2026-09-17-asset-layers-and-profiles-design.md`

## Global Constraints

- **Key convention:** every layer reference is `<kind>/<name>`, the same spelling `Manifest::key` / `Manifest::split_key` already use. `kind` ∈ `skill` | `agent` | `hook` | `mcp_server` | `plugin_ref` (`Kind::as_str`).
- **`Kind::ALL` must NOT be extended.** A layer is not an asset and must never reach `compute_host_plan` as one. `layers/` is a sixth directory with its own loader pass.
- **Backward compatibility is mandatory:** a host with no `host_layers` row resolves to the whole catalog — today's behaviour exactly. Every task must preserve this.
- **Migration number:** the last on disk is `031_asset_sync.sql`. The host-reboot spec also claims `032`. Use the next free number at implementation time; if `032` is taken, renumber. This plan writes `032` — **verify before creating the file.**
- **Shell quoting:** any value interpolated into an SSH/bash string uses `crate::shell::quote` (alias `shq`). No task here builds shell strings, but the rule stands.
- **Store locking:** `Store` sits behind a `std::sync::Mutex`; never hold the guard across an `.await`.
- **Docs:** new Tauri commands *and* new MCP tools both force a reference regeneration (Task 8). CI fails otherwise.
- **Run the tests with:** `cargo test --manifest-path src-tauri/Cargo.toml <filter>`. Cargo builds need the Tauri system libs (dbus, gtk/atk, pkg-config); on a headless box without them the build script fails — that is an environment gap, not a code error.

---

### Task 1: Layer model and parsing

**Files:**
- Create: `src-tauri/src/service/catalog/layer.rs`
- Modify: `src-tauri/src/service/catalog/mod.rs` (add `pub mod layer;` after `pub mod import;`)

**Interfaces:**
- Consumes: `crate::service::catalog::model::Kind`
- Produces: `Layer { name, axis, version, description, extends, members, exclude, overrides }`, `Axis::{Role, Context}`, `Layer::from_yaml(&str) -> Result<Layer, String>`, `Layer::validate(&self) -> Result<(), String>`

- [ ] **Step 1: Write the failing test**

Create `src-tauri/src/service/catalog/layer.rs` with only a test module:

```rust
//! Layer definitions: composable membership + overrides over the catalog.

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
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --manifest-path src-tauri/Cargo.toml catalog::layer`
Expected: FAIL — `cannot find type Layer in this scope` (the module does not exist yet; also add `pub mod layer;` to `mod.rs` so it compiles at all).

- [ ] **Step 3: Write minimal implementation**

Put this **above** the test module in `layer.rs`:

```rust
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
```

Add to `src-tauri/src/service/catalog/mod.rs`, after `pub mod import;`:

```rust
pub mod layer;
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --manifest-path src-tauri/Cargo.toml catalog::layer`
Expected: PASS — 3 tests.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/service/catalog/layer.rs src-tauri/src/service/catalog/mod.rs
git commit -m "feat(catalog): layer model with members, exclude and overrides"
```

---

### Task 2: Load layers, flatten `extends`, detect cycles

**Files:**
- Modify: `src-tauri/src/service/catalog/layer.rs` (add `LayerSet`)
- Modify: `src-tauri/src/service/catalog/repo.rs:15-20` (add `layers` to `Catalog`), `repo.rs:326` (`load_dir` reads `layers/`)

**Interfaces:**
- Consumes: `Layer`, `Axis`, `split_key` (Task 1); `Catalog`, `Problem` from `repo.rs` / `model.rs`
- Produces: `LayerSet::from_layers(Vec<Layer>) -> (LayerSet, Vec<String>)`, `LayerSet::get(&str) -> Option<&Layer>`, `LayerSet::chain_for(&self, name: &str) -> Result<Vec<&Layer>, String>` (root → leaf), and `Catalog::layers: LayerSet`

- [ ] **Step 1: Write the failing test**

Append to the `tests` module in `layer.rs`:

```rust
    fn layer(name: &str, axis: &str, extends: Option<&str>) -> Layer {
        let ext = extends.map(|e| format!("extends: {e}\n")).unwrap_or_default();
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
        let bad = Layer::from_yaml(
            "kind: layer\nname: bad\naxis: role\nmembers:\n  - nope/x\n",
        )
        .unwrap();
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
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --manifest-path src-tauri/Cargo.toml catalog::layer`
Expected: FAIL — `cannot find type LayerSet in this scope`.

- [ ] **Step 3: Write minimal implementation**

Append to `layer.rs` (above the tests):

```rust
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
```

Now wire it into the catalog. In `repo.rs`, extend `Catalog` (line 15):

```rust
pub struct Catalog {
    pub assets: Vec<Asset>,
    pub problems: Vec<Problem>,
    pub head: String,
    pub loaded_at: i64,
    /// Layer definitions from `layers/*.yaml`. Empty ⇒ no layering, and
    /// every host resolves to the whole catalog (backward compatibility).
    pub layers: crate::service::catalog::layer::LayerSet,
}
```

In `load_dir`, immediately before `Ok(cat)`, add the sixth-directory pass:

```rust
    // Layers are NOT a Kind: they must never reach compute_host_plan as an
    // asset. Own directory, own pass.
    let layer_dir = root.join("layers");
    if layer_dir.is_dir() {
        let mut parsed: Vec<crate::service::catalog::layer::Layer> = Vec::new();
        let mut entries: Vec<PathBuf> = match std::fs::read_dir(&layer_dir) {
            Ok(rd) => rd.filter_map(|e| e.ok().map(|e| e.path())).collect(),
            Err(e) => {
                cat.problems.push(Problem {
                    path: rel(root, &layer_dir),
                    message: e.to_string(),
                });
                Vec::new()
            }
        };
        entries.sort();
        for p in entries {
            if p.extension().and_then(|e| e.to_str()) != Some("yaml") {
                continue;
            }
            let stem = p.file_stem().unwrap_or_default().to_string_lossy().to_string();
            match std::fs::read_to_string(&p)
                .map_err(|e| e.to_string())
                .and_then(|t| crate::service::catalog::layer::Layer::from_yaml(&t))
            {
                Ok(l) if l.name != stem => cat.problems.push(Problem {
                    path: rel(root, &p),
                    message: format!("layer name '{}' does not match file stem '{stem}'", l.name),
                }),
                Ok(l) => parsed.push(l),
                Err(message) => cat.problems.push(Problem {
                    path: rel(root, &p),
                    message,
                }),
            }
        }
        let (set, errors) = crate::service::catalog::layer::LayerSet::from_layers(parsed);
        for message in errors {
            cat.problems.push(Problem {
                path: "layers".to_string(),
                message,
            });
        }
        // A member naming an unknown asset is a WARNING: the layer still
        // resolves, matching load_dir's existing tolerance elsewhere.
        for l in set.iter() {
            for key in l.members.iter().chain(l.overrides.keys()) {
                if let Some((kind, name)) = crate::service::catalog::layer::split_key(key) {
                    if cat.find(kind, &name).is_none() {
                        cat.problems.push(Problem {
                            path: format!("layers/{}.yaml", l.name),
                            message: format!("'{key}' is not in the catalog"),
                        });
                    }
                }
            }
        }
        cat.layers = set;
    }
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --manifest-path src-tauri/Cargo.toml catalog::`
Expected: PASS. `Catalog` gains a field with a `Default`, so existing `Catalog { ..Default::default() }` constructions keep compiling; fix any explicit struct literal the compiler flags by adding `layers: Default::default()`.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/service/catalog/layer.rs src-tauri/src/service/catalog/repo.rs
git commit -m "feat(catalog): load layers/ with extends flattening and cycle detection"
```

---

### Task 3: The pure `resolve()`

**Files:**
- Create: `src-tauri/src/service/catalog/resolve.rs`
- Modify: `src-tauri/src/service/catalog/mod.rs` (add `pub mod resolve;`)

**Interfaces:**
- Consumes: `Layer`, `LayerSet`, `split_key` (Tasks 1–2); `Catalog`, `Asset`, `Kind`
- Produces: `resolve(catalog: &Catalog, role_chain: &[&Layer], contexts: &[&Layer]) -> Resolution`; `Resolution { catalog: Catalog, provenance: BTreeMap<String, Provenance>, excluded: BTreeMap<String, String> }`; `Provenance { introduced_by: String, overridden_by: Vec<String> }`

- [ ] **Step 1: Write the failing test**

Create `src-tauri/src/service/catalog/resolve.rs`:

```rust
//! Resolve (catalog, role chain, contexts) into the effective catalog.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::catalog::layer::{Layer, LayerSet};
    use crate::service::catalog::model::{Asset, Kind};
    use crate::service::catalog::repo::Catalog;

    fn skill(name: &str) -> Asset {
        Asset::from_yaml(None, &format!("kind: skill\nname: {name}\ndescription: d\n")).unwrap()
    }

    fn catalog(names: &[&str]) -> Catalog {
        Catalog {
            assets: names.iter().map(|n| skill(n)).collect(),
            ..Default::default()
        }
    }

    fn lay(yaml: &str) -> Layer {
        Layer::from_yaml(yaml).unwrap()
    }

    #[test]
    fn no_layers_resolves_to_the_whole_catalog() {
        let cat = catalog(&["a", "b"]);
        let r = resolve(&cat, &[], &[]);
        assert_eq!(r.catalog.assets.len(), 2);
    }

    #[test]
    fn members_add_and_exclude_removes_along_the_chain() {
        let cat = catalog(&["a", "b", "c"]);
        let root = lay("kind: layer\nname: root\naxis: role\nmembers:\n  - skill/a\n  - skill/b\n");
        let leaf = lay(
            "kind: layer\nname: leaf\naxis: role\nextends: root\n\
             members:\n  - skill/c\nexclude:\n  - skill/b\n",
        );
        let r = resolve(&cat, &[&root, &leaf], &[]);
        let mut got: Vec<&str> = r.catalog.assets.iter().map(|a| a.header.name.as_str()).collect();
        got.sort();
        assert_eq!(got, vec!["a", "c"]);
        assert_eq!(r.excluded.get("skill/b").map(String::as_str), Some("leaf"));
        assert_eq!(r.provenance["skill/a"].introduced_by, "root");
        assert_eq!(r.provenance["skill/c"].introduced_by, "leaf");
    }

    #[test]
    fn a_later_layer_can_re_add_what_an_earlier_one_excluded() {
        let cat = catalog(&["a"]);
        let role = lay("kind: layer\nname: r\naxis: role\nexclude:\n  - skill/a\n");
        let ctx = lay("kind: layer\nname: c\naxis: context\nmembers:\n  - skill/a\n");
        let r = resolve(&cat, &[&role], &[&ctx]);
        assert_eq!(r.catalog.assets.len(), 1);
        assert!(!r.excluded.contains_key("skill/a"));
        assert_eq!(r.provenance["skill/a"].introduced_by, "c");
    }

    #[test]
    fn contexts_apply_after_the_whole_role_chain_in_order() {
        let cat = catalog(&["a"]);
        let role = lay(
            "kind: layer\nname: r\naxis: role\nmembers:\n  - skill/a\n\
             overrides:\n  skill/a:\n    version: \"role\"\n",
        );
        let c1 = lay("kind: layer\nname: c1\naxis: context\noverrides:\n  skill/a:\n    version: \"c1\"\n");
        let c2 = lay("kind: layer\nname: c2\naxis: context\noverrides:\n  skill/a:\n    version: \"c2\"\n");
        let r = resolve(&cat, &[&role], &[&c1, &c2]);
        assert_eq!(r.catalog.assets[0].header.version, "c2");
        assert_eq!(r.provenance["skill/a"].overridden_by, vec!["role", "c1", "c2"]);
    }

    #[test]
    fn overrides_deep_merge_rather_than_replacing_the_header() {
        let cat = Catalog {
            assets: vec![Asset::from_yaml(
                None,
                "kind: skill\nname: a\ndescription: keep me\ntags: [x]\n\
                 targets:\n  claude:\n    enabled: true\n",
            )
            .unwrap()],
            ..Default::default()
        };
        let role = lay(
            "kind: layer\nname: r\naxis: role\nmembers:\n  - skill/a\n\
             overrides:\n  skill/a:\n    targets:\n      claude:\n        enabled: false\n",
        );
        let r = resolve(&cat, &[&role], &[]);
        let a = &r.catalog.assets[0];
        // The override touched targets.claude.enabled ONLY.
        assert_eq!(a.header.description, "keep me");
        assert_eq!(a.header.tags, vec!["x".to_string()]);
        assert!(!a.header.targets["claude"].enabled);
    }

    #[test]
    fn body_and_resources_survive_an_override() {
        let mut asset = skill("a");
        asset.body = "BODY".to_string();
        let cat = Catalog { assets: vec![asset], ..Default::default() };
        let role = lay(
            "kind: layer\nname: r\naxis: role\nmembers:\n  - skill/a\n\
             overrides:\n  skill/a:\n    version: \"9\"\n",
        );
        let r = resolve(&cat, &[&role], &[]);
        assert_eq!(r.catalog.assets[0].body, "BODY");
        assert_eq!(r.catalog.assets[0].header.version, "9");
    }

    #[test]
    fn an_override_for_a_non_member_is_ignored() {
        let cat = catalog(&["a", "b"]);
        let role = lay(
            "kind: layer\nname: r\naxis: role\nmembers:\n  - skill/a\n\
             overrides:\n  skill/b:\n    version: \"9\"\n",
        );
        let r = resolve(&cat, &[&role], &[]);
        assert_eq!(r.catalog.assets.len(), 1);
        assert_eq!(r.catalog.assets[0].header.name, "a");
    }

    #[test]
    fn a_member_naming_an_unknown_asset_is_skipped_and_the_rest_resolve() {
        let cat = catalog(&["a"]);
        let role = lay("kind: layer\nname: r\naxis: role\nmembers:\n  - skill/a\n  - skill/ghost\n");
        let r = resolve(&cat, &[&role], &[]);
        assert_eq!(r.catalog.assets.len(), 1);
        assert!(!r.provenance.contains_key("skill/ghost"));
    }

    #[test]
    fn the_resolved_catalog_keeps_head_and_loaded_at() {
        let mut cat = catalog(&["a"]);
        cat.head = "deadbeef".to_string();
        cat.loaded_at = 42;
        let role = lay("kind: layer\nname: r\naxis: role\nmembers:\n  - skill/a\n");
        let r = resolve(&cat, &[&role], &[]);
        assert_eq!(r.catalog.head, "deadbeef");
        assert_eq!(r.catalog.loaded_at, 42);
    }

    #[test]
    fn layer_set_is_not_carried_into_the_resolved_catalog() {
        // Guards against a layer ever reaching compute_host_plan.
        let cat = catalog(&["a"]);
        let role = lay("kind: layer\nname: r\naxis: role\nmembers:\n  - skill/a\n");
        let r = resolve(&cat, &[&role], &[]);
        assert!(r.catalog.layers.is_empty());
        let _ = LayerSet::default();
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --manifest-path src-tauri/Cargo.toml catalog::resolve`
Expected: FAIL — `cannot find function resolve in this scope`.

- [ ] **Step 3: Write minimal implementation**

Put this above the tests in `resolve.rs`:

```rust
use crate::service::catalog::layer::{split_key, Layer};
use crate::service::catalog::model::Asset;
use crate::service::catalog::repo::Catalog;
use serde::Serialize;
use std::collections::BTreeMap;

/// Where one effective asset came from.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Provenance {
    /// The layer that last introduced it.
    pub introduced_by: String,
    /// Every layer that changed its fields, in application order.
    pub overridden_by: Vec<String>,
}

/// The effective catalog plus why it looks the way it does.
#[derive(Debug, Clone, Serialize)]
pub struct Resolution {
    pub catalog: Catalog,
    /// `<kind>/<name>` → provenance, for the UI's "where did this come from".
    pub provenance: BTreeMap<String, Provenance>,
    /// `<kind>/<name>` → the layer that excluded it, for "why is this gone".
    pub excluded: BTreeMap<String, String>,
}

/// Deep-merge `over` into `base`: mappings recurse, everything else replaces.
fn merge_yaml(base: &mut serde_yaml::Value, over: &serde_yaml::Value) {
    match (base, over) {
        (serde_yaml::Value::Mapping(b), serde_yaml::Value::Mapping(o)) => {
            for (k, v) in o {
                match b.get_mut(k) {
                    Some(existing) => merge_yaml(existing, v),
                    None => {
                        b.insert(k.clone(), v.clone());
                    }
                }
            }
        }
        (b, o) => *b = o.clone(),
    }
}

/// Apply one override mapping to an asset by round-tripping its header+spec
/// through YAML. `body` and `resources` are not part of `to_yaml`, so they
/// are carried over explicitly.
fn apply_override(asset: &Asset, over: &serde_yaml::Value) -> Result<Asset, String> {
    let mut value: serde_yaml::Value =
        serde_yaml::from_str(&asset.to_yaml()).map_err(|e| e.to_string())?;
    merge_yaml(&mut value, over);
    let text = serde_yaml::to_string(&value).map_err(|e| e.to_string())?;
    let mut patched = Asset::from_yaml(Some(asset.kind()), &text)?;
    patched.body = asset.body.clone();
    patched.resources = asset.resources.clone();
    Ok(patched)
}

/// Resolve the effective catalog for one host.
///
/// `role_chain` is ROOT FIRST and already validated by `LayerSet::chain_for`
/// (cycles, missing parents and crossed axes are caught there), so this
/// function cannot loop. `contexts` apply after the entire role chain, in
/// order. An empty `role_chain` AND empty `contexts` means "no layering":
/// the whole catalog is returned, which is the backward-compatible path.
pub fn resolve(catalog: &Catalog, role_chain: &[&Layer], contexts: &[&Layer]) -> Resolution {
    let ordered: Vec<&Layer> = role_chain.iter().chain(contexts.iter()).copied().collect();

    if ordered.is_empty() {
        return Resolution {
            catalog: Catalog {
                assets: catalog.assets.clone(),
                problems: catalog.problems.clone(),
                head: catalog.head.clone(),
                loaded_at: catalog.loaded_at,
                layers: Default::default(),
            },
            provenance: BTreeMap::new(),
            excluded: BTreeMap::new(),
        };
    }

    let mut members: BTreeMap<String, Provenance> = BTreeMap::new();
    let mut excluded: BTreeMap<String, String> = BTreeMap::new();
    let mut overrides: BTreeMap<String, Vec<(String, serde_yaml::Value)>> = BTreeMap::new();

    for layer in &ordered {
        for key in &layer.members {
            // A member naming an asset the catalog does not have is skipped;
            // load_dir already reported it as a Problem.
            let Some((kind, name)) = split_key(key) else { continue };
            if catalog.find(kind, &name).is_none() {
                continue;
            }
            excluded.remove(key);
            members.insert(
                key.clone(),
                Provenance {
                    introduced_by: layer.name.clone(),
                    overridden_by: Vec::new(),
                },
            );
        }
        for key in &layer.exclude {
            members.remove(key);
            excluded.insert(key.clone(), layer.name.clone());
        }
        for (key, value) in &layer.overrides {
            overrides
                .entry(key.clone())
                .or_default()
                .push((layer.name.clone(), value.clone()));
        }
    }

    let mut problems = catalog.problems.clone();
    let mut assets = Vec::new();
    let mut provenance = BTreeMap::new();

    for (key, mut prov) in members {
        let Some((kind, name)) = split_key(&key) else { continue };
        let Some(base) = catalog.find(kind, &name) else { continue };
        let mut asset = base.clone();
        for (layer_name, value) in overrides.get(&key).into_iter().flatten() {
            match apply_override(&asset, value) {
                Ok(patched) => {
                    asset = patched;
                    prov.overridden_by.push(layer_name.clone());
                }
                Err(message) => problems.push(crate::service::catalog::model::Problem {
                    path: format!("layers/{layer_name}.yaml"),
                    message: format!("override for '{key}' could not be applied: {message}"),
                }),
            }
        }
        assets.push(asset);
        provenance.insert(key, prov);
    }

    Resolution {
        catalog: Catalog {
            assets,
            problems,
            head: catalog.head.clone(),
            loaded_at: catalog.loaded_at,
            // Deliberately empty: a layer must never travel into the planner.
            layers: Default::default(),
        },
        provenance,
        excluded,
    }
}
```

Add to `mod.rs`, after `pub mod repo;`:

```rust
pub mod resolve;
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --manifest-path src-tauri/Cargo.toml catalog::resolve`
Expected: PASS — 10 tests.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/service/catalog/resolve.rs src-tauri/src/service/catalog/mod.rs
git commit -m "feat(catalog): pure resolve() producing the effective catalog with provenance"
```

---

### Task 4: `host_layers` table and store accessors

**Files:**
- Create: `src-tauri/migrations/032_asset_layers.sql` (**verify 032 is free first** — see Global Constraints)
- Create: `src-tauri/src/store/layers.rs`
- Modify: `src-tauri/src/store/schema.rs:116` (`MIGRATIONS` list), `src-tauri/src/store/mod.rs` (add `mod layers;`)

**Interfaces:**
- Consumes: `Store`, `rusqlite`
- Produces: `HostLayerRow { host_alias, layer_name, axis, position, active }`; `Store::get_host_layers(&self, host_alias: &str) -> Result<Vec<HostLayerRow>, rusqlite::Error>`; `Store::set_host_layers(&self, host_alias: &str, role: Option<&str>, contexts: &[&str]) -> Result<(), rusqlite::Error>`; `Store::list_all_host_layers(&self) -> Result<Vec<HostLayerRow>, rusqlite::Error>`

- [ ] **Step 1: Write the failing test**

Create `src-tauri/src/store/layers.rs` with only a test module:

```rust
//! Per-host layer assignment: one active role plus ordered contexts.

#[cfg(test)]
mod tests {
    use crate::store::Store;

    /// `PRAGMA foreign_keys = ON` is enforced (`schema.rs:247`), and
    /// `host_layers.host_alias` references `hosts(alias)` — so the host row
    /// must exist or every insert trips the constraint.
    fn store_with_local() -> Store {
        let s = Store::open_in_memory().expect("open");
        s.upsert_host("local").expect("host");
        s
    }

    #[test]
    fn set_and_get_round_trip_with_context_order() {
        let s = store_with_local();
        s.set_host_layers("local", Some("workstation"), &["papayapos", "writing"])
            .unwrap();
        let rows = s.get_host_layers("local").unwrap();
        let role: Vec<_> = rows.iter().filter(|r| r.axis == "role").collect();
        assert_eq!(role.len(), 1);
        assert_eq!(role[0].layer_name, "workstation");
        let mut ctx: Vec<_> = rows.iter().filter(|r| r.axis == "context").collect();
        ctx.sort_by_key(|r| r.position);
        assert_eq!(
            ctx.iter().map(|r| r.layer_name.as_str()).collect::<Vec<_>>(),
            vec!["papayapos", "writing"]
        );
    }

    #[test]
    fn set_replaces_the_previous_assignment_entirely() {
        let s = store_with_local();
        s.set_host_layers("local", Some("a"), &["x", "y"]).unwrap();
        s.set_host_layers("local", Some("b"), &["z"]).unwrap();
        let rows = s.get_host_layers("local").unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().any(|r| r.layer_name == "b" && r.axis == "role"));
        assert!(rows.iter().any(|r| r.layer_name == "z" && r.axis == "context"));
    }

    #[test]
    fn a_host_with_no_assignment_returns_nothing() {
        let s = store_with_local();
        assert!(s.get_host_layers("local").unwrap().is_empty());
    }

    #[test]
    fn clearing_the_role_is_allowed() {
        let s = store_with_local();
        s.set_host_layers("local", Some("a"), &[]).unwrap();
        s.set_host_layers("local", None, &["x"]).unwrap();
        let rows = s.get_host_layers("local").unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].axis, "context");
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --manifest-path src-tauri/Cargo.toml store::layers`
Expected: FAIL — `no method named set_host_layers found for struct Store`.

- [ ] **Step 3: Write minimal implementation**

Create `src-tauri/migrations/032_asset_layers.sql`:

```sql
-- Per-host layer assignment (asset catalog layers). Layer DEFINITIONS live in
-- the catalog repo under layers/*.yaml; only the assignment is fleet state.
-- A host with no row here resolves to the whole catalog, which is the
-- pre-layers behaviour.
CREATE TABLE host_layers (
  host_alias TEXT    NOT NULL REFERENCES hosts(alias),
  layer_name TEXT    NOT NULL,
  axis       TEXT    NOT NULL,             -- 'role' | 'context'
  position   INTEGER NOT NULL DEFAULT 0,   -- context application order
  active     INTEGER NOT NULL DEFAULT 1,
  PRIMARY KEY (host_alias, layer_name)
);

-- At most one active role per host, enforced by the schema rather than by
-- application code.
CREATE UNIQUE INDEX idx_host_active_role
  ON host_layers(host_alias) WHERE axis = 'role' AND active = 1;

INSERT OR IGNORE INTO schema_version (version) VALUES (32);
```

Append to the `MIGRATIONS` list in `schema.rs`:

```rust
    Migration::plain(32, include_str!("../../migrations/032_asset_layers.sql")),
```

Put this above the tests in `store/layers.rs`:

```rust
use crate::store::Store;
use serde::Serialize;

/// One row of `host_layers`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct HostLayerRow {
    pub host_alias: String,
    pub layer_name: String,
    /// `"role"` | `"context"`.
    pub axis: String,
    pub position: i64,
    pub active: bool,
}

fn row_from(r: &rusqlite::Row<'_>) -> Result<HostLayerRow, rusqlite::Error> {
    Ok(HostLayerRow {
        host_alias: r.get(0)?,
        layer_name: r.get(1)?,
        axis: r.get(2)?,
        position: r.get(3)?,
        active: r.get::<_, i64>(4)? != 0,
    })
}

const COLS: &str = "host_alias, layer_name, axis, position, active";

impl Store {
    pub fn get_host_layers(
        &self,
        host_alias: &str,
    ) -> Result<Vec<HostLayerRow>, rusqlite::Error> {
        self.conn
            .prepare(&format!(
                "SELECT {COLS} FROM host_layers WHERE host_alias=?1 AND active=1 \
                 ORDER BY axis, position, layer_name"
            ))?
            .query_map(rusqlite::params![host_alias], |r| row_from(r))?
            .collect()
    }

    pub fn list_all_host_layers(&self) -> Result<Vec<HostLayerRow>, rusqlite::Error> {
        self.conn
            .prepare(&format!(
                "SELECT {COLS} FROM host_layers ORDER BY host_alias, axis, position, layer_name"
            ))?
            .query_map([], |r| row_from(r))?
            .collect()
    }

    /// Replace a host's assignment wholesale: one optional role plus the
    /// contexts in application order. Runs in one transaction so a host is
    /// never left with a half-written assignment.
    pub fn set_host_layers(
        &self,
        host_alias: &str,
        role: Option<&str>,
        contexts: &[&str],
    ) -> Result<(), rusqlite::Error> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM host_layers WHERE host_alias=?1",
            rusqlite::params![host_alias],
        )?;
        if let Some(r) = role {
            tx.execute(
                "INSERT INTO host_layers (host_alias, layer_name, axis, position, active) \
                 VALUES (?1, ?2, 'role', 0, 1)",
                rusqlite::params![host_alias, r],
            )?;
        }
        for (i, c) in contexts.iter().enumerate() {
            tx.execute(
                "INSERT INTO host_layers (host_alias, layer_name, axis, position, active) \
                 VALUES (?1, ?2, 'context', ?3, 1)",
                rusqlite::params![host_alias, c, i as i64],
            )?;
        }
        tx.commit()
    }
}
```

Add to `src-tauri/src/store/mod.rs`, alongside the other `mod` lines:

```rust
mod layers;
pub use layers::HostLayerRow;
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --manifest-path src-tauri/Cargo.toml store::`
Expected: PASS, including the existing `LATEST_SCHEMA_VERSION` / migration tests now reporting 32.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/migrations/032_asset_layers.sql src-tauri/src/store/layers.rs src-tauri/src/store/schema.rs src-tauri/src/store/mod.rs
git commit -m "feat(store): host_layers table and assignment accessors"
```

---

### Task 5: Resolve before planning

**Files:**
- Create: `src-tauri/src/service/catalog/sync/layers.rs`
- Modify: `src-tauri/src/service/catalog/sync/mod.rs:214-221`, `src-tauri/src/service/catalog/sync/mod.rs` (add `mod layers;`)

**Interfaces:**
- Consumes: `resolve`, `Resolution` (Task 3); `LayerSet::chain_for` (Task 2); `Store::get_host_layers` (Task 4)
- Produces: `resolve_for_host(store: &Mutex<Store>, catalog: &Catalog, host_alias: &str) -> Result<Resolution, IpcError>`

- [ ] **Step 1: Write the failing test**

Create `src-tauri/src/service/catalog/sync/layers.rs`:

```rust
//! Bridge between a host's stored layer assignment and the pure resolver.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::catalog::layer::{Layer, LayerSet};
    use crate::service::catalog::model::Asset;
    use crate::service::catalog::repo::Catalog;
    use crate::store::Store;
    use std::sync::Mutex;

    /// FKs are ON, so `local` must exist before an assignment references it.
    fn store_with_local() -> Store {
        let s = Store::open_in_memory().expect("open");
        s.upsert_host("local").expect("host");
        s
    }

    fn cat() -> Catalog {
        let (layers, errs) = LayerSet::from_layers(vec![
            Layer::from_yaml("kind: layer\nname: core\naxis: role\nmembers:\n  - skill/a\n")
                .unwrap(),
            Layer::from_yaml(
                "kind: layer\nname: workstation\naxis: role\nextends: core\nmembers:\n  - skill/b\n",
            )
            .unwrap(),
            Layer::from_yaml("kind: layer\nname: extra\naxis: context\nmembers:\n  - skill/c\n")
                .unwrap(),
        ]);
        assert!(errs.is_empty(), "{errs:?}");
        Catalog {
            assets: ["a", "b", "c"]
                .iter()
                .map(|n| {
                    Asset::from_yaml(None, &format!("kind: skill\nname: {n}\ndescription: d\n"))
                        .unwrap()
                })
                .collect(),
            layers,
            ..Default::default()
        }
    }

    fn names(r: &crate::service::catalog::resolve::Resolution) -> Vec<String> {
        let mut v: Vec<String> = r.catalog.assets.iter().map(|a| a.header.name.clone()).collect();
        v.sort();
        v
    }

    #[test]
    fn a_host_with_no_assignment_gets_the_whole_catalog() {
        let store = Mutex::new(store_with_local());
        let r = resolve_for_host(&store, &cat(), "local").unwrap();
        assert_eq!(names(&r), vec!["a", "b", "c"]);
    }

    #[test]
    fn a_role_pulls_in_its_extends_chain() {
        let store = Mutex::new(store_with_local());
        store.lock().unwrap().set_host_layers("local", Some("workstation"), &[]).unwrap();
        let r = resolve_for_host(&store, &cat(), "local").unwrap();
        assert_eq!(names(&r), vec!["a", "b"]);
    }

    #[test]
    fn contexts_add_on_top_of_the_role() {
        let store = Mutex::new(store_with_local());
        store
            .lock()
            .unwrap()
            .set_host_layers("local", Some("core"), &["extra"])
            .unwrap();
        let r = resolve_for_host(&store, &cat(), "local").unwrap();
        assert_eq!(names(&r), vec!["a", "c"]);
    }

    #[test]
    fn an_assignment_naming_an_unknown_layer_is_an_error_not_a_silent_full_catalog() {
        let store = Mutex::new(store_with_local());
        store.lock().unwrap().set_host_layers("local", Some("ghost"), &[]).unwrap();
        let err = resolve_for_host(&store, &cat(), "local").unwrap_err();
        assert!(err.message.contains("ghost"), "{}", err.message);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --manifest-path src-tauri/Cargo.toml sync::layers`
Expected: FAIL — `cannot find function resolve_for_host in this scope`.

- [ ] **Step 3: Write minimal implementation**

Put this above the tests in `sync/layers.rs`:

```rust
use crate::ipc_error::codes;
use crate::ipc_error::lock;
use crate::ipc_error::IpcError;
use crate::service::catalog::repo::Catalog;
use crate::service::catalog::resolve::{resolve, Resolution};
use crate::store::Store;
use std::sync::Mutex;

/// Read `host_alias`'s stored assignment and resolve the catalog for it.
///
/// No assignment ⇒ the whole catalog (the pre-layers behaviour). An
/// assignment naming a layer the catalog does not define is an ERROR rather
/// than a silent fall-back to the whole catalog: syncing everything to a host
/// the user meant to restrict is the worse failure.
pub fn resolve_for_host(
    store: &Mutex<Store>,
    catalog: &Catalog,
    host_alias: &str,
) -> Result<Resolution, IpcError> {
    let rows = {
        let s = lock(store)?;
        s.get_host_layers(host_alias)?
    };
    if rows.is_empty() {
        return Ok(resolve(catalog, &[], &[]));
    }

    let role_name = rows.iter().find(|r| r.axis == "role").map(|r| r.layer_name.clone());
    let mut context_rows: Vec<_> = rows.iter().filter(|r| r.axis == "context").collect();
    context_rows.sort_by_key(|r| r.position);

    let role_chain = match &role_name {
        None => Vec::new(),
        Some(name) => catalog
            .layers
            .chain_for(name)
            .map_err(|e| IpcError::new(codes::E_INVALID, e))?,
    };

    let mut contexts = Vec::new();
    for r in context_rows {
        // A context may itself extend another context; flatten it too, and
        // skip a parent already contributed by an earlier context.
        for l in catalog
            .layers
            .chain_for(&r.layer_name)
            .map_err(|e| IpcError::new(codes::E_INVALID, e))?
        {
            if !contexts.iter().any(|c: &&crate::service::catalog::layer::Layer| c.name == l.name) {
                contexts.push(l);
            }
        }
    }

    Ok(resolve(catalog, &role_chain, &contexts))
}
```

Add `mod layers;` to `sync/mod.rs` beside the other `mod` declarations, then change the planning call site (`sync/mod.rs:214-221`) to resolve first:

```rust
        // Resolve the host's layers ONCE per host, before its harnesses are
        // planned. The scan below still uses the FULL catalog: inventory is
        // about the whole catalog's drift, while the PLAN is about what this
        // host is supposed to have.
        let resolved = match layers::resolve_for_host(store, &catalog, &h.alias) {
            Ok(r) => r,
            Err(e) => {
                for harness in &scanning {
                    host_plans.push(skipped_plan(&h.alias, harness.id(), &e.message));
                }
                continue;
            }
        };
        for harness in &scanning {
            let harness = *harness;
            match scan_and_persist(store, ssh, &catalog, harness, &h.alias, &secrets).await {
                Ok((snap, manifest)) => host_plans.push(plan::compute_host_plan(
                    &resolved.catalog, harness, &h.alias, &snap, &manifest, &secrets, &filter,
                )),
                Err(e) => host_plans.push(skipped_plan(&h.alias, harness.id(), &e.message)),
            }
        }
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --manifest-path src-tauri/Cargo.toml catalog::`
Expected: PASS — the 4 new tests plus every existing sync test (no host has an assignment in those fixtures, so they take the whole-catalog path).

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/service/catalog/sync/layers.rs src-tauri/src/service/catalog/sync/mod.rs
git commit -m "feat(sync): resolve each host's layers before computing its plan"
```

---

### Task 6: Dropped assets are removed — except plugins

**Files:**
- Modify: `src-tauri/src/service/catalog/sync/plan.rs:306-330` (the `orphans` loop)
- Test: `src-tauri/src/service/catalog/sync/plan.rs` (its existing `tests` module)

**Interfaces:**
- Consumes: `compute_host_plan`, `ActionOp`, `Manifest::orphans` (existing)
- Produces: no new public API — a behavioural rule on `ActionOp::Remove` for `Kind::PluginRef`

- [ ] **Step 1: Write the failing test**

`plan.rs`'s test module already proves the first half of this task:
`a_manifest_entry_the_catalog_lost_becomes_a_remove` shows that an asset the
catalog no longer has is scheduled as `Remove`. Since `resolve()` hands
`compute_host_plan` a narrower catalog, **removal on layer-disable already
works** — do not add a second test for it.

Only the plugin exception is new. Append to the `tests` module in `plan.rs`,
using its existing helpers (`cat()`, `host_with()`, `manifest_with()`,
`plan_for()`, `act()`):

```rust
    #[test]
    fn a_dropped_plugin_ref_is_reported_but_never_removed() {
        // The manifest remembers a plugin the (resolved) catalog no longer
        // has. Uninstalling is slow and network-bound, so a context switch
        // must not depend on it: report, never remove.
        let manifest = manifest_with(&[("plugin_ref/graphify", "h")]);
        let hp = plan_for(
            &catalog_of(&[]),
            &Claude,
            &host_with(&[]),
            &manifest,
            &BTreeMap::new(),
        );
        let a = act(&hp, "graphify");
        assert_eq!(a.op, ActionOp::Noop);
        assert_eq!(a.kind, "plugin_ref");
        assert!(
            a.reason.as_deref().unwrap_or_default().contains("not removed automatically"),
            "{:?}",
            a.reason
        );
    }
```

> `plan_for` takes `&dyn Harness`, not an enum variant. The test module already
> imports the value at `plan.rs:688`
> (`use crate::service::catalog::harness::claude::Claude;`), so `&Claude` is in
> scope — do not write a fully-qualified path.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --manifest-path src-tauri/Cargo.toml a_dropped_plugin_ref`
Expected: FAIL — the action comes back as `ActionOp::Remove`, not `Noop`.

- [ ] **Step 3: Write minimal implementation**

In the `orphans` loop in `compute_host_plan`, right after `split_key` succeeds
and the `filter.matches` check passes, special-case plugin refs. `Action` does
**not** derive `Default`, so every field is spelled out:

```rust
        if kind == Kind::PluginRef {
            actions.push(Action {
                kind: kind.as_str().to_string(),
                name: name.clone(),
                op: ActionOp::Noop,
                reason: Some(
                    "no longer in this host's layers; plugins are not removed automatically"
                        .to_string(),
                ),
                files: Vec::new(),
                merges: Vec::new(),
                backup: false,
                secrets: Vec::new(),
                missing_secrets: Vec::new(),
                plan: None,
                expected: BTreeMap::new(),
                secret_files: BTreeSet::new(),
                remove_entry: None,
                plugin: None,
            });
            continue;
        }
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --manifest-path src-tauri/Cargo.toml plan::tests`
Expected: PASS — the new test plus every existing plan test, including
`a_manifest_entry_the_catalog_lost_becomes_a_remove`.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/service/catalog/sync/plan.rs
git commit -m "feat(sync): report dropped plugin refs instead of uninstalling them"
```

---

### Task 7: `propose_layers`

**Files:**
- Create: `src-tauri/src/service/catalog/propose.rs`
- Modify: `src-tauri/src/service/catalog/mod.rs` (add `pub mod propose;`)

**Interfaces:**
- Consumes: `Store::list_inventory` → `AssetInventoryRow` (existing, `store/catalog.rs:81`)
- Produces: `propose_layers(store: &Mutex<Store>) -> Result<LayerProposal, IpcError>`; `LayerProposal { layers: Vec<ProposedLayer>, singletons: Vec<ProposedSingleton> }`; `ProposedLayer { name, axis, hosts, members }`

- [ ] **Step 1: Write the failing test**

Create `src-tauri/src/service/catalog/propose.rs`:

```rust
//! Propose an initial layer split from what is already installed.

#[cfg(test)]
mod tests {
    use super::*;

    fn item(host: &str, key: &str) -> (String, String) {
        (host.to_string(), key.to_string())
    }

    #[test]
    fn the_largest_host_group_becomes_core_and_an_outlier_gets_its_own_layer() {
        // Mirrors the measured fleet: four hosts share most assets, one
        // near-empty outlier shares only a little. A strict all-host
        // INTERSECTION would collapse core to just "shared"; grouping by
        // host-set signature must not.
        let mut installed = Vec::new();
        for host in ["local", "oci", "trn", "mefistos"] {
            for k in ["skill/a", "skill/b", "skill/c"] {
                installed.push(item(host, k));
            }
        }
        for host in ["local", "oci", "trn", "mefistos", "htz"] {
            installed.push(item(host, "skill/shared"));
        }
        installed.push(item("htz", "skill/only-htz"));

        let p = propose_from_installed(&installed);

        let core = p.layers.iter().find(|l| l.name == "core").unwrap();
        let mut members = core.members.clone();
        members.sort();
        assert_eq!(members, vec!["skill/a", "skill/b", "skill/c"]);
        assert_eq!(core.hosts.len(), 4);

        // The all-host group still becomes its own layer, not part of core.
        assert!(p
            .layers
            .iter()
            .any(|l| l.members == vec!["skill/shared".to_string()] && l.hosts.len() == 5));

        // Single-host assets are triage, not layers.
        assert!(p.layers.iter().all(|l| !l.members.contains(&"skill/only-htz".to_string())));
        assert_eq!(p.singletons.len(), 1);
        assert_eq!(p.singletons[0].key, "skill/only-htz");
        assert_eq!(p.singletons[0].host, "htz");
    }

    #[test]
    fn every_proposed_layer_is_a_role_and_names_are_unique() {
        let installed = vec![
            item("a", "skill/x"),
            item("b", "skill/x"),
            item("a", "skill/y"),
            item("c", "skill/y"),
        ];
        let p = propose_from_installed(&installed);
        assert!(p.layers.iter().all(|l| l.axis == "role"));
        let mut names: Vec<&str> = p.layers.iter().map(|l| l.name.as_str()).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), p.layers.len());
    }

    #[test]
    fn an_empty_fleet_proposes_nothing() {
        let p = propose_from_installed(&[]);
        assert!(p.layers.is_empty() && p.singletons.is_empty());
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --manifest-path src-tauri/Cargo.toml catalog::propose`
Expected: FAIL — `cannot find function propose_from_installed in this scope`.

- [ ] **Step 3: Write minimal implementation**

Put this above the tests in `propose.rs`:

```rust
use crate::ipc_error::lock;
use crate::ipc_error::IpcError;
use crate::store::Store;
use serde::Serialize;
use std::collections::BTreeMap;
use std::sync::Mutex;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProposedLayer {
    pub name: String,
    /// Always `"role"`: the proposal cannot know what is a context.
    pub axis: String,
    /// The hosts whose installed set this layer was derived from.
    pub hosts: Vec<String>,
    pub members: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProposedSingleton {
    pub key: String,
    pub host: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LayerProposal {
    pub layers: Vec<ProposedLayer>,
    /// Assets on exactly one host: a context, or a mistake. The user decides.
    pub singletons: Vec<ProposedSingleton>,
}

/// Group `(host, key)` pairs by each key's EXACT host-set signature.
///
/// Grouping by signature rather than intersecting across all hosts is what
/// makes this usable: on the measured fleet a strict all-host intersection
/// yields a `core` of only 7 skills, because one near-empty outlier host drags
/// it down. By signature, the big shared group wins and the outlier gets its
/// own layer instead of impoverishing everyone else.
pub fn propose_from_installed(installed: &[(String, String)]) -> LayerProposal {
    let mut hosts_by_key: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (host, key) in installed {
        let entry = hosts_by_key.entry(key.as_str()).or_default();
        if !entry.contains(&host.as_str()) {
            entry.push(host.as_str());
        }
    }
    for hosts in hosts_by_key.values_mut() {
        hosts.sort();
    }

    let mut singletons = Vec::new();
    let mut groups: BTreeMap<Vec<String>, Vec<String>> = BTreeMap::new();
    for (key, hosts) in hosts_by_key {
        if hosts.len() < 2 {
            if let Some(h) = hosts.first() {
                singletons.push(ProposedSingleton {
                    key: key.to_string(),
                    host: h.to_string(),
                });
            }
            continue;
        }
        groups
            .entry(hosts.iter().map(|h| h.to_string()).collect())
            .or_default()
            .push(key.to_string());
    }

    // Largest group first; ties broken by host-set so the output is stable.
    let mut ordered: Vec<(Vec<String>, Vec<String>)> = groups.into_iter().collect();
    ordered.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then(a.0.cmp(&b.0)));

    let layers = ordered
        .into_iter()
        .enumerate()
        .map(|(i, (hosts, mut members))| {
            members.sort();
            ProposedLayer {
                name: if i == 0 { "core".to_string() } else { hosts.join("-") },
                axis: "role".to_string(),
                hosts,
                members,
            }
        })
        .collect();

    LayerProposal { layers, singletons }
}

/// Read the last scan's inventory and propose a split. Read-only: it returns
/// a proposal and writes nothing, neither to the catalog nor to the DB.
pub fn propose_layers(store: &Mutex<Store>) -> Result<LayerProposal, IpcError> {
    let rows = {
        let s = lock(store)?;
        s.list_inventory()?
    };
    let installed: Vec<(String, String)> = rows
        .iter()
        // One inventory row per harness; count each asset once.
        .filter(|r| r.harness == "claude")
        .map(|r| (r.host_alias.clone(), format!("{}/{}", r.kind, r.name)))
        .collect();
    Ok(propose_from_installed(&installed))
}
```

`AssetInventoryRow` (`src-tauri/src/store/rows.rs:533`) carries `host_alias`,
`harness`, `kind`, `name`, `state`, `managed`. It has one row per **harness**,
so without the `harness` filter above every asset would be counted twice and
the host-set signatures would still be right but the member lists doubled.

Add to `mod.rs`:

```rust
pub mod propose;
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --manifest-path src-tauri/Cargo.toml catalog::propose`
Expected: PASS — 3 tests.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/service/catalog/propose.rs src-tauri/src/service/catalog/mod.rs
git commit -m "feat(catalog): propose initial layers by host-set signature"
```

---

### Task 8: Commands, MCP tools, docs

**Files:**
- Modify: `src-tauri/src/commands/assets.rs`, `src-tauri/src/lib.rs` (`generate_handler!` list), `src-tauri/src/mcp/tools/assets.rs`, `src-tauri/src/mcp/tools/params.rs`, `src-tauri/src/mcp/guard.rs:38` (`READONLY_TOOLS`)
- Modify: `docs/control-api-reference.md` (regenerated, never hand-edited)

**Interfaces:**
- Consumes: `resolve_for_host` (Task 5), `propose_layers` (Task 7), `Store::list_all_host_layers` / `set_host_layers` (Task 4)
- Produces: Tauri commands `catalog_list_layers`, `catalog_resolve_preview`, `catalog_propose_layers`, `catalog_set_host_layers`; MCP tools `list_layers`, `resolve_preview`, `propose_layers`, `set_host_layers`

- [ ] **Step 1: Write the failing test**

Add to `src-tauri/src/mcp/tools/tests.rs`:

```rust
    #[test]
    fn layer_read_tools_are_readonly_and_the_setter_is_not() {
        use crate::mcp::guard::is_readonly_tool;
        assert!(is_readonly_tool("list_layers"));
        assert!(is_readonly_tool("resolve_preview"));
        assert!(is_readonly_tool("propose_layers"));
        assert!(!is_readonly_tool("set_host_layers"));
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --manifest-path src-tauri/Cargo.toml mcp::tools::tests::layer_read_tools`
Expected: FAIL — `assert!(is_readonly_tool("list_layers"))` is false.

- [ ] **Step 3: Write minimal implementation**

Add the three read tools to `READONLY_TOOLS` in `guard.rs` (keep the list's existing grouping):

```rust
    "list_layers",
    "resolve_preview",
    "propose_layers",
```

Add the params to `mcp/tools/params.rs`:

```rust
#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct ResolvePreviewParams {
    /// The host whose effective asset set to compute.
    pub host_alias: String,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct SetHostLayersParams {
    /// The host whose layer assignment to replace.
    pub host_alias: String,
    /// The role layer, or null to clear it. A host has at most one.
    #[serde(default)]
    pub role: Option<String>,
    /// Context layers, in application order. Omit for none.
    #[serde(default)]
    pub contexts: Vec<String>,
}
```

Add the four tools to `mcp/tools/assets.rs`, following the file's existing `#[tool(description = …)]` + `audit(…)` + `ok_json(…)` shape:

```rust
    #[tool(description = "List the catalog's layer definitions (layers/*.yaml) \
        and each host's role + active contexts. Read-only. Returns JSON.")]
    pub(super) async fn list_layers(&self) -> Result<CallToolResult, McpError> {
        audit("list_layers", "");
        let out = catalog::list_layers(&self.store).map_err(to_mcp_err)?;
        ok_json(&out)
    }

    #[tool(description = "Compute the effective asset set for one host after \
        its role and contexts are resolved, with provenance: which layer \
        introduced each asset, which layers overrode it, and which layer \
        excluded anything missing. Nothing is written. Returns JSON.")]
    pub(super) async fn resolve_preview(
        &self,
        Parameters(p): Parameters<ResolvePreviewParams>,
    ) -> Result<CallToolResult, McpError> {
        audit("resolve_preview", &format!("host_alias={}", p.host_alias));
        let out = catalog::resolve_preview(&p.host_alias, &self.store).map_err(to_mcp_err)?;
        ok_json(&out)
    }

    #[tool(description = "Propose an initial layer split from the last scan, \
        grouping assets by the exact set of hosts they are installed on. The \
        largest group becomes 'core'; assets on a single host are returned \
        separately for triage. Read-only: writes nothing. Returns JSON.")]
    pub(super) async fn propose_layers(&self) -> Result<CallToolResult, McpError> {
        audit("propose_layers", "");
        let out = catalog::propose::propose_layers(&self.store).map_err(to_mcp_err)?;
        ok_json(&out)
    }

    #[tool(description = "Replace a host's layer assignment: one optional role \
        plus context layers in application order. Edits fleet state only, never \
        catalog files. Returns the host's new assignment as JSON.")]
    pub(super) async fn set_host_layers(
        &self,
        Parameters(p): Parameters<SetHostLayersParams>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "set_host_layers",
            &format!("host_alias={} role={:?} contexts={}", p.host_alias, p.role, p.contexts.len()),
        );
        let out = catalog::set_host_layers(
            &p.host_alias,
            p.role.as_deref(),
            &p.contexts.iter().map(String::as_str).collect::<Vec<_>>(),
            &self.store,
        )
        .map_err(to_mcp_err)?;
        ok_json(&out)
    }
```

Add the four service wrappers to `service/catalog/mod.rs` (`list_layers`, `resolve_preview`, `set_host_layers`), each taking `&Mutex<Store>`, loading the catalog via the existing `require_catalog()` helper used by `list_assets`, and delegating to Tasks 4/5/7. Mirror each as a `#[tauri::command]` in `commands/assets.rs` named `catalog_list_layers`, `catalog_resolve_preview`, `catalog_propose_layers`, `catalog_set_host_layers`, and register all four in `generate_handler!` in `lib.rs`.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --manifest-path src-tauri/Cargo.toml mcp::`
Expected: PASS, except `reference_is_current`, which now fails because the tool and command lists changed. Regenerate:

```bash
REGEN_DOCS=1 cargo test --manifest-path src-tauri/Cargo.toml reference_is_current
```

Then re-run `cargo test --manifest-path src-tauri/Cargo.toml mcp::` and expect PASS.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/commands/assets.rs src-tauri/src/lib.rs src-tauri/src/mcp/ docs/control-api-reference.md
git commit -m "feat(mcp): list_layers, resolve_preview, propose_layers, set_host_layers"
```

---

### Task 9: Layer authoring

**Files:**
- Modify: `src-tauri/src/service/catalog/author.rs` (layer template + the three write paths)
- Modify: `src-tauri/src/service/catalog/layer.rs` (add `Layer::to_yaml`)

**Interfaces:**
- Consumes: `Layer`, `Axis`, `LayerSet` (Tasks 1–2); `author.rs`'s existing commit-and-reload helper
- Produces: `author::layer_template(name: &str, axis: Axis) -> Layer`; `author::write_layer(layer: &Layer, store: &Mutex<Store>) -> Result<String, IpcError>`; `author::delete_layer(name: &str, store: &Mutex<Store>) -> Result<String, IpcError>`

The spec requires layers to go through the same authoring path as assets:
auto-commit with a `catalog: …` message and a reload. `author.rs` already
builds `catalog: create skill/foo` (line 614), `catalog: update …` (652) and
`catalog: delete …` (667) — layers reuse that shape with `layer/<name>`.

- [ ] **Step 1: Write the failing test**

Append to the `tests` module in `author.rs`:

```rust
    #[test]
    fn layer_template_round_trips_through_yaml() {
        let l = layer_template("workstation", crate::service::catalog::layer::Axis::Role);
        assert_eq!(l.name, "workstation");
        assert_eq!(l.axis, crate::service::catalog::layer::Axis::Role);
        assert!(l.members.is_empty());
        let back = crate::service::catalog::layer::Layer::from_yaml(&l.to_yaml()).unwrap();
        assert_eq!(back, l);
    }

    #[test]
    fn layer_commit_messages_use_the_layer_key() {
        assert_eq!(layer_commit_message("create", "workstation"), "catalog: create layer/workstation");
        assert_eq!(layer_commit_message("delete", "minimal"), "catalog: delete layer/minimal");
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --manifest-path src-tauri/Cargo.toml author::tests::layer_`
Expected: FAIL — `cannot find function layer_template in this scope`.

- [ ] **Step 3: Write minimal implementation**

Add to `layer.rs`:

```rust
impl Layer {
    /// Serialise back to `layers/<name>.yaml` form, `kind: layer` first so
    /// the file is self-describing for a human reading the repo.
    pub fn to_yaml(&self) -> String {
        let body = serde_yaml::to_string(self).unwrap_or_default();
        format!("kind: layer
{body}")
    }
}
```

Add to `author.rs`:

```rust
use crate::service::catalog::layer::{Axis, Layer};

/// A blank layer to author from, mirroring `template(kind, name)` for assets.
pub fn layer_template(name: &str, axis: Axis) -> Layer {
    Layer {
        name: name.to_string(),
        axis,
        version: "1".to_string(),
        description: String::new(),
        extends: None,
        members: Vec::new(),
        exclude: Vec::new(),
        overrides: Default::default(),
    }
}

/// The commit message for a layer write, in the same shape the asset paths
/// use (`catalog: create skill/foo`).
pub fn layer_commit_message(verb: &str, name: &str) -> String {
    format!("catalog: {verb} layer/{name}")
}
```

Then add `write_layer` and `delete_layer` next to `create_asset` / `delete_asset`,
writing `layers/<name>.yaml` (or removing it) and going through the **same**
commit-and-reload helper those functions already use, with
`layer_commit_message("create" | "update" | "delete", name)`. Reject a `name`
that fails `model::is_valid_name` before touching the working tree, exactly as
`create_asset` does.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --manifest-path src-tauri/Cargo.toml author::`
Expected: PASS — the 2 new tests plus every existing authoring test.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/service/catalog/author.rs src-tauri/src/service/catalog/layer.rs
git commit -m "feat(catalog): author layers through the existing commit-and-reload path"
```

---

### Task 10: Full verification

**Files:** none modified.

- [ ] **Step 1: Run the whole local CI mirror**

```bash
scripts/ci-local.sh
```

Expected: PASS. It runs `cargo test`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check`, `cargo deny check`, plus the frontend checks in CI order.

- [ ] **Step 2: Confirm backward compatibility by hand**

With no `host_layers` rows anywhere, run `plan_sync { host_alias: "local" }` through the app or the MCP server and confirm the action set is byte-for-byte what it was before this branch. This is the one guarantee no unit test fully covers, because it spans the real catalog and a real host.

- [ ] **Step 3: Commit any formatting fixes**

```bash
git add -A
git commit -m "chore: formatting and clippy fixes for asset layers"
```
