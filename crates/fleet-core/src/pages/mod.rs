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

pub mod catalog;
pub mod model;
pub mod sources;
pub mod validate;

#[cfg(test)]
mod tests;

pub use model::Page;
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
    ("usage.json", include_str!("../../pages/usage.json")),
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

pub fn get(id: &str) -> Option<&'static Page> {
    all().iter().find(|p| p.id == id)
}
