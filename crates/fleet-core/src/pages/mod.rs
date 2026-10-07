//! Declarative pages (design
//! `docs/superpowers/specs/2026-09-28-declarative-pages-design.md`): pages
//! and forms described as data, rendered by a closed catalog of widgets and
//! layouts, so a new settings or data page is a JSON file rather than new UI
//! code — and one an AI agent can write safely.
//!
//! - [`model`]: the `fleet.page/1` DSL.
//! - [`catalog`]: widgets per setting kind, items per layout.
//! - [`sources`]: named read-only data sources (`fetch`).
//! - [`validate`]: the one validator.
//!
//! The pages themselves are the JSON files in `crates/fleet-core/pages/`,
//! compiled in through [`PAGE_FILES`]. `docs/pages.md` is the authoring
//! guide; `docs/page-spec.schema.json` and `docs/page-catalog.json` are
//! generated for editors and agents (`REGEN_PAGE_DOCS=1`).

pub mod actions;
pub mod catalog;
pub mod flows;
pub mod model;
pub mod resources;
pub mod sources;
pub mod validate;

#[cfg(test)]
mod tests;

pub use model::Page;

/// Every page spec, data source shape, resource and page action: what
/// `list_pages` answers on the desktop and the hub (P6, for a phone).
/// Compiled in, so the same for every caller that may see pages. Embed pages
/// are not in it: they place items in the desktop's own screens, which read
/// them from `src/lib/pages/embeds.generated.json` (generated from the same
/// specs), so a phone never sees them.
#[derive(Debug, serde::Serialize)]
pub struct PagesBundle {
    pub pages: &'static [Page],
    pub sources: &'static [sources::SourceSpec],
    pub resources: &'static [resources::ResourceType],
    pub actions: &'static [actions::PageAction],
}

pub fn bundle() -> PagesBundle {
    PagesBundle {
        pages: navigable(),
        sources: sources::SOURCES,
        resources: resources::RESOURCES,
        actions: actions::PAGE_ACTIONS,
    }
}
use std::sync::OnceLock;

/// Every page spec, by file name. A new file in `pages/` must be listed
/// here (`every_page_file_is_compiled_in`).
pub const PAGE_FILES: &[(&str, &str)] = &[
    ("settings.json", include_str!("../../pages/settings.json")),
    (
        "settings.automation.json",
        include_str!("../../pages/settings.automation.json"),
    ),
    (
        "settings.limits.json",
        include_str!("../../pages/settings.limits.json"),
    ),
    (
        "settings.projects.json",
        include_str!("../../pages/settings.projects.json"),
    ),
    (
        "settings.work.json",
        include_str!("../../pages/settings.work.json"),
    ),
    (
        "settings.decisions.json",
        include_str!("../../pages/settings.decisions.json"),
    ),
    (
        "settings.hub.json",
        include_str!("../../pages/settings.hub.json"),
    ),
    (
        "settings.control_api.json",
        include_str!("../../pages/settings.control_api.json"),
    ),
    (
        "settings.trackers.json",
        include_str!("../../pages/settings.trackers.json"),
    ),
    (
        "settings.orgs.json",
        include_str!("../../pages/settings.orgs.json"),
    ),
    (
        "settings.catalogs.json",
        include_str!("../../pages/settings.catalogs.json"),
    ),
    (
        "settings.devices.json",
        include_str!("../../pages/settings.devices.json"),
    ),
    (
        "settings.people.json",
        include_str!("../../pages/settings.people.json"),
    ),
    (
        "settings.updates.json",
        include_str!("../../pages/settings.updates.json"),
    ),
    (
        "settings.review.json",
        include_str!("../../pages/settings.review.json"),
    ),
    ("guides.json", include_str!("../../pages/guides.json")),
    ("usage.json", include_str!("../../pages/usage.json")),
    (
        "usage.work.json",
        include_str!("../../pages/usage.work.json"),
    ),
    (
        "usage.accounts.json",
        include_str!("../../pages/usage.accounts.json"),
    ),
    (
        "embed.host_detail.json",
        include_str!("../../pages/embed.host_detail.json"),
    ),
    (
        "embed.hosts_group_title.json",
        include_str!("../../pages/embed.hosts_group_title.json"),
    ),
    (
        "embed.hosts_group.json",
        include_str!("../../pages/embed.hosts_group.json"),
    ),
    (
        "embed.new_session_chip.json",
        include_str!("../../pages/embed.new_session_chip.json"),
    ),
    (
        "embed.new_session_host.json",
        include_str!("../../pages/embed.new_session_host.json"),
    ),
    (
        "embed.status_footer.json",
        include_str!("../../pages/embed.status_footer.json"),
    ),
];

/// Registered settings that deliberately have no page, and why. Empty
/// today: every setting has one home.
pub const UNLISTED: &[(&str, &str)] = &[];

/// Parse one spec. The error names the file and serde's position.
pub fn parse(file: &str, text: &str) -> Result<Page, String> {
    serde_json::from_str(text).map_err(|e| format!("{file}: {e}"))
}

/// Every compiled-in page, parsed once. The tests hold every file to
/// parsing and validating, so a page that fails here never ships; one that
/// did would be left out and logged rather than take the app down.
pub fn all() -> &'static [Page] {
    static PAGES: OnceLock<Vec<Page>> = OnceLock::new();
    PAGES.get_or_init(|| {
        PAGE_FILES
            .iter()
            .filter_map(|(file, text)| match parse(file, text) {
                Ok(page) => Some(page),
                Err(e) => {
                    tracing::error!("[pages] {e}");
                    None
                }
            })
            .collect()
    })
}

/// The pages a person navigates to: every page but the embeds.
pub fn navigable() -> &'static [Page] {
    static NAV: OnceLock<Vec<Page>> = OnceLock::new();
    NAV.get_or_init(|| {
        all()
            .iter()
            .filter(|p| p.layout != model::Layout::Embed)
            .cloned()
            .collect()
    })
}

/// The embed pages, one per filled slot.
pub fn embeds() -> impl Iterator<Item = &'static Page> {
    all().iter().filter(|p| p.layout == model::Layout::Embed)
}

pub fn get(id: &str) -> Option<&'static Page> {
    all().iter().find(|p| p.id == id)
}
