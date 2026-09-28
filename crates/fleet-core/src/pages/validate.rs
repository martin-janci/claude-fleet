//! The one validator for page specs: CI runs it over every compiled-in page
//! (`pages::tests`), and anything that loads a spec at runtime runs the same
//! code. It refuses what the registries do not know — a setting key, a data
//! source, a widget a kind cannot take, a page to link to — and names the
//! place in the spec, so an agent can fix a spec from the message alone.

use super::catalog;
use super::model::{Condition, Item, Page, Section, SPEC_VERSION};
use super::sources::{self, Shape};
use crate::service::settings::{self, Kind};
use std::collections::{BTreeMap, BTreeSet};

/// One thing wrong with a spec: the page, where in it, and what.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    pub page: String,
    pub at: String,
    pub message: String,
}

impl std::fmt::Display for Problem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.at.is_empty() {
            write!(f, "{}: {}", self.page, self.message)
        } else {
            write!(f, "{} › {}: {}", self.page, self.at, self.message)
        }
    }
}

const MAX_TITLE: usize = 60;
const MAX_TEXT: usize = 300;
const MAX_HINT: usize = 120;

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.split('.').all(|part| {
            let mut chars = part.chars();
            chars.next().is_some_and(|c| c.is_ascii_lowercase())
                && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        })
}

/// Plain text: bounded, no markup.
fn plain(text: &str, max: usize) -> Result<(), String> {
    if text.trim().is_empty() {
        return Err("must not be empty".into());
    }
    if text.len() > max {
        return Err(format!("is longer than {max} characters"));
    }
    if text.contains('<') || text.contains('>') {
        return Err("is plain text: no markup".into());
    }
    Ok(())
}

struct Ctx<'a> {
    page: &'a str,
    problems: &'a mut Vec<Problem>,
}

impl Ctx<'_> {
    fn bad(&mut self, at: &str, message: impl Into<String>) {
        self.problems.push(Problem {
            page: self.page.to_string(),
            at: at.to_string(),
            message: message.into(),
        });
    }

    fn text(&mut self, at: &str, what: &str, text: &str, max: usize) {
        if let Err(e) = plain(text, max) {
            self.bad(at, format!("{what} {e}"));
        }
    }
}

fn check_condition(cx: &mut Ctx, at: &str, c: &Condition) {
    let forms = [
        c.key.is_some(),
        c.all.is_some(),
        c.any.is_some(),
        c.not.is_some(),
    ]
    .iter()
    .filter(|b| **b)
    .count();
    if forms != 1 {
        cx.bad(
            at,
            "a condition is exactly one of {key, …}, {all}, {any} or {not}",
        );
        return;
    }
    if let Some(key) = &c.key {
        let tests = [c.eq.is_some(), c.one_of.is_some(), c.truthy.is_some()]
            .iter()
            .filter(|b| **b)
            .count();
        if tests != 1 {
            cx.bad(at, format!("`{key}`: say exactly one of eq, in or truthy"));
        }
        let Some(spec) = settings::spec(key) else {
            cx.bad(at, format!("`{key}` is not a registered setting"));
            return;
        };
        for v in c.eq.iter().chain(c.one_of.iter().flatten()) {
            if let Err(e) = settings::validate(key, v) {
                cx.bad(at, format!("`{key}` can never be {v:?}: {}", e.message));
            }
        }
        if c.one_of.as_ref().is_some_and(Vec::is_empty) {
            cx.bad(at, format!("`{key}`: `in` needs at least one value"));
        }
        if c.truthy.is_some() && spec.kind != Kind::Bool {
            cx.bad(
                at,
                format!("`{key}`: truthy is for on/off settings; use eq or in"),
            );
        }
    } else if c.eq.is_some() || c.one_of.is_some() || c.truthy.is_some() {
        cx.bad(at, "eq, in and truthy need a key");
    }
    for (i, sub) in c.all.iter().chain(c.any.iter()).flatten().enumerate() {
        check_condition(cx, &format!("{at} › condition {}", i + 1), sub);
    }
    if c.all.as_ref().is_some_and(Vec::is_empty) || c.any.as_ref().is_some_and(Vec::is_empty) {
        cx.bad(at, "all / any need at least one condition");
    }
    if let Some(sub) = &c.not {
        check_condition(cx, &format!("{at} › not"), sub);
    }
}

fn check_source(cx: &mut Ctx, at: &str, source: &super::model::SourceRef) -> Option<Shape> {
    let Some(spec) = sources::source(&source.id) else {
        cx.bad(at, format!("`{}` is not a data source", source.id));
        return None;
    };
    if let Err(e) = sources::resolve_params(spec, &source.params) {
        cx.bad(at, e.message);
    }
    Some(spec.shape)
}

fn check_item(
    cx: &mut Ctx,
    at: &str,
    page: &Page,
    item: &Item,
    placed: &mut BTreeMap<String, String>,
) {
    if !catalog::layout_allows(page.layout, item) {
        cx.bad(
            at,
            format!(
                "a {:?} page cannot hold a `{}` item",
                page.layout,
                catalog::item_type(item)
            ),
        );
    }
    match item {
        Item::Field {
            key,
            widget,
            hint,
            when,
        } => {
            match settings::spec(key) {
                None => cx.bad(at, format!("`{key}` is not a registered setting")),
                Some(spec) => {
                    if let Some(w) = widget {
                        if !catalog::accepts(*w, spec.kind) {
                            cx.bad(
                                at,
                                format!("`{key}` ({:?}) cannot use the {w:?} widget", spec.kind),
                            );
                        }
                    }
                }
            }
            if let Some(first) = placed.insert(key.clone(), format!("{} › {at}", page.id)) {
                cx.bad(
                    at,
                    format!("`{key}` is already placed at {first}; a setting has one home"),
                );
            }
            if let Some(h) = hint {
                cx.text(at, "hint", h, MAX_HINT);
            }
            if let Some(c) = when {
                check_condition(cx, &format!("{at} › when"), c);
            }
        }
        Item::Stat {
            source,
            field,
            label,
        } => {
            match (check_source(cx, at, source), field) {
                (Some(Shape::Scalar { .. }), None) | (None, _) => {}
                (Some(Shape::Record { fields }), Some(f)) => {
                    if !fields.iter().any(|c| c.id == f) {
                        cx.bad(at, format!("`{}` has no field `{f}`", source.id));
                    }
                }
                (Some(Shape::Record { .. }), None) => cx.bad(
                    at,
                    format!("a stat of the record `{}` names its `field`", source.id),
                ),
                (Some(Shape::Scalar { .. }), Some(_)) => {
                    cx.bad(at, format!("`{}` is a scalar: it has no fields", source.id))
                }
                (Some(shape), _) => cx.bad(
                    at,
                    format!(
                        "a stat shows a scalar or a record field, not {}",
                        shape.name()
                    ),
                ),
            }
            if let Some(l) = label {
                cx.text(at, "label", l, MAX_TITLE);
            }
        }
        Item::Record { source } => {
            if let Some(shape) = check_source(cx, at, source) {
                if !matches!(shape, Shape::Record { .. }) {
                    cx.bad(at, format!("a record shows a record, not {}", shape.name()));
                }
            }
        }
        Item::Table { source, columns } => match check_source(cx, at, source) {
            Some(Shape::Rows { columns: known }) => {
                for c in columns {
                    if !known.iter().any(|k| k.id == c) {
                        cx.bad(at, format!("`{}` has no column `{c}`", source.id));
                    }
                }
            }
            Some(shape) => cx.bad(at, format!("a table shows rows, not {}", shape.name())),
            None => {}
        },
        Item::Chart { source, title, .. } => {
            if let Some(shape) = check_source(cx, at, source) {
                if !matches!(shape, Shape::Series { .. }) {
                    cx.bad(at, format!("a chart shows a series, not {}", shape.name()));
                }
            }
            if let Some(t) = title {
                cx.text(at, "title", t, MAX_TITLE);
            }
        }
        Item::Notice { text, .. } => cx.text(at, "text", text, MAX_TEXT),
        Item::Custom { .. } => {}
        Item::Link { label, .. } => {
            if let Some(l) = label {
                cx.text(at, "label", l, MAX_TITLE);
            }
        }
    }
}

fn check_section(
    cx: &mut Ctx,
    at: &str,
    page: &Page,
    section: &Section,
    placed: &mut BTreeMap<String, String>,
) {
    cx.text(at, "title", &section.title, MAX_TITLE);
    if let Some(i) = &section.intro {
        cx.text(at, "intro", i, MAX_TEXT);
    }
    if let Some(c) = &section.when {
        check_condition(cx, &format!("{at} › when"), c);
    }
    if section.items.is_empty() {
        cx.bad(at, "a section needs at least one item");
    }
    for (i, item) in section.items.iter().enumerate() {
        check_item(cx, &format!("{at} › item {}", i + 1), page, item, placed);
    }
}

/// Every problem in `pages`, taken together: each page on its own, and
/// what only the set can say (unique ids, parents, links, one home per
/// setting). Empty means valid.
pub fn validate(pages: &[Page]) -> Vec<Problem> {
    let mut problems = Vec::new();
    let ids: BTreeSet<&str> = pages.iter().map(|p| p.id.as_str()).collect();
    let mut seen = BTreeSet::new();
    let mut placed: BTreeMap<String, String> = BTreeMap::new();

    for page in pages {
        let mut cx = Ctx {
            page: &page.id,
            problems: &mut problems,
        };
        if page.spec != SPEC_VERSION {
            cx.bad("", format!("spec must be {SPEC_VERSION:?}"));
        }
        if !valid_id(&page.id) {
            cx.bad("", "id is dotted lowercase words, e.g. settings.automation");
        }
        if !seen.insert(page.id.as_str()) {
            cx.bad("", "another page has this id");
        }
        cx.text("", "title", &page.title, MAX_TITLE);
        if let Some(i) = &page.intro {
            cx.text("", "intro", i, MAX_TEXT);
        }
        if !catalog::layout_supported(page.layout) {
            cx.bad(
                "",
                format!(
                    "the {:?} layout needs the resource registry, which this build does not have yet",
                    page.layout
                ),
            );
        }
        match &page.parent {
            Some(parent) if !ids.contains(parent.as_str()) => {
                cx.bad("", format!("parent `{parent}` is not a page"));
            }
            _ => {}
        }
        match (page.sections.is_empty(), page.tabs.is_empty()) {
            (true, true) => cx.bad("", "a page needs sections or tabs"),
            (false, false) => cx.bad("", "a page has sections or tabs, not both"),
            _ => {}
        }
        for (i, section) in page.sections.iter().enumerate() {
            check_section(
                &mut cx,
                &format!("section {}", i + 1),
                page,
                section,
                &mut placed,
            );
        }
        for (t, tab) in page.tabs.iter().enumerate() {
            let at = format!("tab {}", t + 1);
            cx.text(&at, "title", &tab.title, MAX_TITLE);
            if let Some(c) = &tab.when {
                check_condition(&mut cx, &format!("{at} › when"), c);
            }
            if tab.sections.is_empty() {
                cx.bad(&at, "a tab needs at least one section");
            }
            for (i, section) in tab.sections.iter().enumerate() {
                check_section(
                    &mut cx,
                    &format!("{at} › section {}", i + 1),
                    page,
                    section,
                    &mut placed,
                );
            }
        }
        for_each_item(page, |at, item| {
            if let Item::Link { page: target, .. } = item {
                if !ids.contains(target.as_str()) {
                    cx.bad(&at, format!("links to `{target}`, which is not a page"));
                }
            }
        });
    }

    // Parents form a tree: following them never comes back round.
    let parent_of: BTreeMap<&str, &str> = pages
        .iter()
        .filter_map(|p| p.parent.as_deref().map(|par| (p.id.as_str(), par)))
        .collect();
    for page in pages {
        let mut at = page.id.as_str();
        let mut steps = 0;
        while let Some(next) = parent_of.get(at) {
            at = next;
            steps += 1;
            if at == page.id || steps > pages.len() {
                problems.push(Problem {
                    page: page.id.clone(),
                    at: String::new(),
                    message: "its parents lead back to itself".into(),
                });
                break;
            }
        }
    }
    let mut customs = 0;
    for page in pages {
        for_each_item(page, |_, item| {
            if matches!(item, Item::Custom { .. }) {
                customs += 1;
            }
        });
    }
    if customs > catalog::MAX_CUSTOM {
        problems.push(Problem {
            page: "(all pages)".into(),
            at: String::new(),
            message: format!(
                "{customs} custom items, over the cap of {}: express one with the catalog instead",
                catalog::MAX_CUSTOM
            ),
        });
    }
    problems
}

/// Call `f` with every item of `page` and where it is.
pub fn for_each_item(page: &Page, mut f: impl FnMut(String, &Item)) {
    let mut visit = |prefix: String, sections: &[Section]| {
        for (s, section) in sections.iter().enumerate() {
            for (i, item) in section.items.iter().enumerate() {
                f(format!("{prefix}section {} › item {}", s + 1, i + 1), item);
            }
        }
    };
    visit(String::new(), &page.sections);
    for (t, tab) in page.tabs.iter().enumerate() {
        visit(format!("tab {} › ", t + 1), &tab.sections);
    }
}

/// The registered settings no page places: each needs a home, or an entry
/// in `pages::UNLISTED` saying why it has none.
pub fn unplaced_keys<'a>(pages: &[Page], unlisted: &[(&'a str, &'a str)]) -> Vec<&'static str> {
    let mut placed = BTreeSet::new();
    for page in pages {
        for_each_item(page, |_, item| {
            if let Item::Field { key, .. } = item {
                placed.insert(key.clone());
            }
        });
    }
    settings::SPECS
        .iter()
        .map(|s| s.key)
        .filter(|k| !placed.contains(*k) && !unlisted.iter().any(|(u, _)| u == k))
        .collect()
}
