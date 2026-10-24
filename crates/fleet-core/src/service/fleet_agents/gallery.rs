//! The definition gallery: the fleet agents claude-fleet ships
//! (`catalog-seed/fleet-agents/`), embedded in the binary, which a person
//! adds to their catalog in one step (`agents { action: install, definition }`,
//! Automation › Agents › Gallery) and then grants like any other.
//!
//! An installed copy is the person's own: private, with no source, edited
//! in Assets like an asset they wrote. Installing never overwrites: a name
//! the catalog already has is refused, so an edited copy is never lost.

use super::form::{writes_line, DefinitionView};
use crate::ipc_error::{codes, IpcError};
use crate::service::catalog::author::{self, WriteResult};
use crate::service::catalog::model::{Asset, Kind};
use crate::service::catalog::CatalogTarget;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

use crate::store::Store;

/// One shipped definition: its `asset.yaml` and its `skill.md`.
pub struct Seed {
    pub name: &'static str,
    pub asset: &'static str,
    pub skill: &'static str,
}

/// Every definition in `catalog-seed/fleet-agents/`, in the gallery's order
/// (a test checks the list against the directory). Each path is spelled out:
/// the hub image's build test finds the embeds by their literal paths.
pub const SEEDS: &[Seed] = &[
    Seed {
        name: "support",
        asset: include_str!("../../../../../catalog-seed/fleet-agents/support/asset.yaml"),
        skill: include_str!("../../../../../catalog-seed/fleet-agents/support/skill.md"),
    },
    Seed {
        name: "issue-triage",
        asset: include_str!("../../../../../catalog-seed/fleet-agents/issue-triage/asset.yaml"),
        skill: include_str!("../../../../../catalog-seed/fleet-agents/issue-triage/skill.md"),
    },
    Seed {
        name: "pr-questions",
        asset: include_str!("../../../../../catalog-seed/fleet-agents/pr-questions/asset.yaml"),
        skill: include_str!("../../../../../catalog-seed/fleet-agents/pr-questions/skill.md"),
    },
    Seed {
        name: "daily-digest",
        asset: include_str!("../../../../../catalog-seed/fleet-agents/daily-digest/asset.yaml"),
        skill: include_str!("../../../../../catalog-seed/fleet-agents/daily-digest/skill.md"),
    },
    Seed {
        name: "self-debug",
        asset: include_str!("../../../../../catalog-seed/fleet-agents/self-debug/asset.yaml"),
        skill: include_str!("../../../../../catalog-seed/fleet-agents/self-debug/skill.md"),
    },
    Seed {
        name: "proposer",
        asset: include_str!("../../../../../catalog-seed/fleet-agents/proposer/asset.yaml"),
        skill: include_str!("../../../../../catalog-seed/fleet-agents/proposer/skill.md"),
    },
    Seed {
        name: "simplify",
        asset: include_str!("../../../../../catalog-seed/fleet-agents/simplify/asset.yaml"),
        skill: include_str!("../../../../../catalog-seed/fleet-agents/simplify/skill.md"),
    },
    Seed {
        name: "refactor",
        asset: include_str!("../../../../../catalog-seed/fleet-agents/refactor/asset.yaml"),
        skill: include_str!("../../../../../catalog-seed/fleet-agents/refactor/skill.md"),
    },
];

impl Seed {
    /// The seed as a catalog asset.
    pub fn asset(&self) -> Result<Asset, String> {
        let mut a = Asset::from_yaml(Some(Kind::FleetAgent), self.asset)?;
        a.body = self.skill.to_string();
        Ok(a)
    }
}

/// One gallery definition, as the Agents tab lists it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GalleryEntry {
    #[serde(flatten)]
    pub view: DefinitionView,
    /// The catalog has a fleet agent of this name already.
    pub installed: bool,
}

fn view(seed: &Seed) -> DefinitionView {
    let asset = seed.asset();
    let loaded = asset
        .as_ref()
        .map_err(|e| vec![e.clone()])
        .and_then(|a| a.fleet_agent());
    DefinitionView {
        name: seed.name.to_string(),
        description: asset
            .as_ref()
            .map(|a| a.header.description.clone())
            .unwrap_or_default(),
        trigger: loaded
            .as_ref()
            .ok()
            .map(|l| l.definition.trigger.source.word().to_string()),
        writes: loaded.as_ref().map(writes_line).unwrap_or_default(),
        problems: loaded.err().unwrap_or_default(),
    }
}

/// PURE: the gallery, each entry marked by whether `has` the name.
pub fn entries(has: impl Fn(&str) -> bool) -> Vec<GalleryEntry> {
    SEEDS
        .iter()
        .map(|s| GalleryEntry {
            view: view(s),
            installed: has(s.name),
        })
        .collect()
}

/// The gallery against the personal catalog. Without a catalog nothing is
/// installed (an install then says what is missing).
pub fn gallery() -> Vec<GalleryEntry> {
    let names: Vec<String> = CatalogTarget::Personal
        .with(|c| {
            Ok(c.assets
                .iter()
                .filter(|a| a.kind() == Kind::FleetAgent)
                .map(|a| a.header.name.clone())
                .collect())
        })
        .unwrap_or_default();
    entries(|n| names.iter().any(|m| m == n))
}

/// Add gallery definition `name` to the personal catalog.
pub fn install(store: &Mutex<Store>, name: &str) -> Result<WriteResult, IpcError> {
    let seed = SEEDS.iter().find(|s| s.name == name).ok_or_else(|| {
        IpcError::new(
            codes::E_NOTFOUND,
            format!(
                "the gallery has no {name:?}: {}",
                SEEDS.iter().map(|s| s.name).collect::<Vec<_>>().join(" | ")
            ),
        )
    })?;
    let asset = seed
        .asset()
        .map_err(|e| IpcError::new(codes::E_INTERNAL, format!("{name}: {e}")))?;
    author::install_in(CatalogTarget::Personal, asset, store)
}

/// [`install`] the way every catalog write runs: under the PF7 authoring
/// lock, so it cannot land in the middle of a changeset apply or undo (be
/// committed as the card's, or deleted by its failure reset), and with its
/// sync git on a blocking thread, off the async workers (review r06).
pub async fn install_locked(
    store: &Arc<Mutex<Store>>,
    name: &str,
) -> Result<WriteResult, IpcError> {
    let _busy = crate::service::catalog::changesets::authoring_lock().await;
    let store = Arc::clone(store);
    let name = name.to_string();
    tokio::task::spawn_blocking(move || install(&store, &name))
        .await
        .map_err(|e| IpcError::new(codes::E_INTERNAL, format!("gallery install: {e}")))?
}

#[cfg(test)]
mod lock_tests {
    use super::*;

    /// PF7: an install waits for a changeset apply or undo in flight.
    #[tokio::test]
    async fn an_install_waits_for_the_authoring_lock() {
        let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
        let held = crate::service::catalog::changesets::authoring_lock().await;
        let task = {
            let store = Arc::clone(&store);
            tokio::spawn(async move { install_locked(&store, "no-such-seed").await })
        };
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(!task.is_finished(), "the install ran under an apply");
        drop(held);
        let e = task.await.unwrap().unwrap_err();
        assert_eq!(e.code, codes::E_NOTFOUND, "{e:?}");
    }

    /// The sync dispatcher never runs an install unlocked; `call_async` does.
    #[tokio::test]
    async fn only_call_async_installs() {
        use crate::service::agents::admin::{call, call_async, AgentsArgs};
        let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
        let args = AgentsArgs {
            action: "install".into(),
            definition: Some("no-such-seed".into()),
            ..Default::default()
        };
        assert_eq!(
            call(&store, &args, "x", 0).unwrap_err().code,
            codes::E_INTERNAL
        );
        let e = call_async(&store, &args, "x", 0).await.unwrap_err();
        assert_eq!(e.code, codes::E_NOTFOUND, "{e:?}");
    }
}
