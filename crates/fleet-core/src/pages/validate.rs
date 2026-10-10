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

pub const MAX_TITLE: usize = 60;
pub const MAX_TEXT: usize = 300;
pub const MAX_HINT: usize = 120;

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
    /// A `master_detail` page's resource: its `field` items and `when`
    /// keys name the resource's fields, not settings.
    resource: Option<&'static super::resources::ResourceType>,
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
        if let Some(res) = cx.resource {
            check_record_condition(cx, at, res, key, c);
            return;
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

/// A `when` on a record field: the field exists, and an on/off field is
/// tested with truthy (or eq "true" / "false").
fn check_record_condition(
    cx: &mut Ctx,
    at: &str,
    res: &super::resources::ResourceType,
    key: &str,
    c: &Condition,
) {
    use super::resources::FieldKind;
    let Some(field) = res.field(key) else {
        cx.bad(at, format!("`{key}` is not a field of {}", res.id));
        return;
    };
    let on_off = matches!(field.kind, FieldKind::Bool { .. });
    if c.truthy.is_some() && !on_off {
        cx.bad(
            at,
            format!("`{key}`: truthy is for on/off fields; use eq or in"),
        );
    }
    if on_off {
        for v in c.eq.iter().chain(c.one_of.iter().flatten()) {
            if v != "true" && v != "false" {
                cx.bad(at, format!("`{key}` is on/off: it can never be {v:?}"));
            }
        }
    }
    if let FieldKind::Choice { options } = field.kind {
        for v in c.eq.iter().chain(c.one_of.iter().flatten()) {
            if !options.iter().any(|(o, _)| o == v) {
                cx.bad(at, format!("`{key}` can never be {v:?}"));
            }
        }
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
            if let Some(res) = cx.resource {
                if res.field(key).is_none() {
                    cx.bad(at, format!("`{key}` is not a field of {}", res.id));
                }
                if widget.is_some() {
                    cx.bad(at, "a record field's control follows its kind: no widget");
                }
                if let Some(first) = placed.insert(format!("{}#{key}", page.id), at.to_string()) {
                    cx.bad(at, format!("`{key}` is already placed at {first}"));
                }
                if let Some(h) = hint {
                    cx.text(at, "hint", h, MAX_HINT);
                }
                if let Some(c) = when {
                    check_condition(cx, &format!("{at} › when"), c);
                }
                return;
            }
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
            // A guide is a path through settings whose home is another
            // page: it never takes the home, only a key once per guide.
            if page.layout == super::model::Layout::Guide {
                if let Some(first) = placed.insert(format!("{}#guide#{key}", page.id), at.into()) {
                    cx.bad(at, format!("`{key}` is already a step's field at {first}"));
                }
            } else if let Some(first) = placed.insert(key.clone(), format!("{} › {at}", page.id))
            {
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
        Item::Table {
            source, columns, ..
        } => match check_source(cx, at, source) {
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
        Item::AccountUsage { source, view } => {
            if let Some(shape) = check_source(cx, at, source) {
                if shape != Shape::AccountUsage {
                    cx.bad(
                        at,
                        format!("account_usage shows account usage, not {}", shape.name()),
                    );
                }
            }
            match (page.layout, page.slot) {
                (super::model::Layout::Embed, Some(slot)) => {
                    if !catalog::slot_views(slot).contains(view) {
                        cx.bad(
                            at,
                            format!(
                                "the {slot:?} slot takes {:?}, not the {view:?} view",
                                catalog::slot_views(slot)
                            ),
                        );
                    }
                }
                (super::model::Layout::Embed, None) => {}
                _ => {
                    if !catalog::PAGE_USAGE_VIEWS.contains(view) {
                        cx.bad(
                            at,
                            format!(
                                "on a page, account_usage is one of {:?}: the {view:?} view \
                                 belongs in a slot",
                                catalog::PAGE_USAGE_VIEWS
                            ),
                        );
                    }
                }
            }
        }
        Item::Notice { text, .. } => cx.text(at, "text", text, MAX_TEXT),
        Item::Custom { .. } => {}
        Item::Action { action } => {
            if super::actions::action(action).is_none() {
                cx.bad(at, format!("`{action}` is not a page action"));
            }
        }
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
    if section.tiles {
        check_tiles(cx, at, section);
    }
    if section.matrix {
        check_matrix(cx, at, section);
    }
    for (i, item) in section.items.iter().enumerate() {
        check_item(cx, &format!("{at} › item {}", i + 1), page, item, placed);
    }
}

/// A `tiles` section shows a record's numbers: only on a `master_detail`
/// page, and only its `count` and `money` fields.
fn check_tiles(cx: &mut Ctx, at: &str, section: &Section) {
    use super::resources::FieldKind;
    let Some(res) = cx.resource else {
        cx.bad(at, "only a master_detail page's section shows tiles");
        return;
    };
    for item in &section.items {
        let tile = match item {
            Item::Field { key, .. } => res
                .field(key)
                .is_none_or(|f| matches!(f.kind, FieldKind::Count | FieldKind::Money)),
            _ => false,
        };
        if !tile {
            cx.bad(at, "a tiles section holds count and money fields only");
        }
    }
}

/// A `matrix` section is two or more settings fields, each a choice set over
/// the very same options, and nothing else: the grid's rows are those
/// options.
fn check_matrix(cx: &mut Ctx, at: &str, section: &Section) {
    if cx.resource.is_some() {
        cx.bad(at, "a matrix holds settings, not a record's fields");
        return;
    }
    let mut options: Option<&'static [&'static str]> = None;
    for item in &section.items {
        let set = match item {
            Item::Field {
                key, widget: None, ..
            } => match settings::spec(key).map(|s| s.kind) {
                Some(settings::Kind::ChoiceSet(o)) => Some(o),
                _ => None,
            },
            _ => None,
        };
        match (set, options) {
            (None, _) => {
                cx.bad(at, "a matrix holds choice-set settings fields only");
                return;
            }
            (Some(o), Some(first)) if o != first => {
                cx.bad(at, "a matrix's fields choose from the same options");
                return;
            }
            (Some(o), _) => options = Some(o),
        }
    }
    if section.items.len() < 2 {
        cx.bad(at, "a matrix needs at least two columns");
    }
}

/// Most steps a guide may have: past this it is a manual, not a guide.
pub const MAX_GUIDE_STEPS: usize = 12;

/// A guide is steps: sections (never tabs), at most [`MAX_GUIDE_STEPS`],
/// each titled differently, since a step is named by its title.
fn check_guide(cx: &mut Ctx, page: &Page) {
    if page.layout != super::model::Layout::Guide {
        return;
    }
    if !page.tabs.is_empty() {
        cx.bad("", "a guide's steps are sections, not tabs");
    }
    if page.sections.len() > MAX_GUIDE_STEPS {
        cx.bad(
            "",
            format!(
                "{} steps, over the {MAX_GUIDE_STEPS} a guide may have",
                page.sections.len()
            ),
        );
    }
    let mut titles = BTreeSet::new();
    for (i, s) in page.sections.iter().enumerate() {
        if !titles.insert(s.title.as_str()) {
            cx.bad(&format!("section {}", i + 1), "another step has this title");
        }
        if s.collapsible || s.advanced {
            cx.bad(
                &format!("section {}", i + 1),
                "a step is shown whole: not collapsible or advanced",
            );
        }
    }
}

/// An `embed` page names a slot and sits in no page tree; nothing else
/// names a slot.
fn check_embed(cx: &mut Ctx, page: &Page) {
    let embed = page.layout == super::model::Layout::Embed;
    match (embed, page.slot) {
        (true, None) => cx.bad("", "an embed page names its slot"),
        (false, Some(_)) => cx.bad("", "only an embed page names a slot"),
        _ => {}
    }
    if embed {
        if page.parent.is_some() {
            cx.bad(
                "",
                "an embed page sits in a screen, not the page tree: no parent",
            );
        }
        if !page.tabs.is_empty() {
            cx.bad("", "an embed page has sections, not tabs");
        }
    }
}

/// The source a data item reads, if it reads one.
fn source_of(item: &Item) -> Option<&super::model::SourceRef> {
    match item {
        Item::Stat { source, .. }
        | Item::AccountUsage { source, .. }
        | Item::Record { source }
        | Item::Table { source, .. }
        | Item::Chart { source, .. } => Some(source),
        _ => None,
    }
}

/// A `data_page`'s filter bar: each filter names a parameter that some
/// source on the page declares, with one type across them; a `days`
/// filter's choices fit every such source's bounds; and no data item sets
/// a filtered parameter itself.
fn check_filters(cx: &mut Ctx, page: &Page) {
    use sources::ParamType;
    if page.filters.is_empty() {
        return;
    }
    if page.layout != super::model::Layout::DataPage {
        cx.bad("", "only a data_page has filters");
        return;
    }
    let mut refs: Vec<(String, super::model::SourceRef)> = Vec::new();
    for_each_item(page, |at, item| {
        if let Some(r) = source_of(item) {
            refs.push((at, r.clone()));
        }
    });
    let mut seen = BTreeSet::new();
    for (i, f) in page.filters.iter().enumerate() {
        let at = format!("filter {}", i + 1);
        if !seen.insert(f.param.as_str()) {
            cx.bad(&at, format!("another filter sets `{}`", f.param));
        }
        if let Some(l) = &f.label {
            cx.text(&at, "label", l, MAX_TITLE);
        }
        let declared: Vec<&sources::ParamSpec> = refs
            .iter()
            .filter_map(|(_, r)| sources::source(&r.id))
            .flat_map(|spec| spec.params.iter())
            .filter(|p| p.name == f.param)
            .collect();
        let Some(first) = declared.first() else {
            cx.bad(
                &at,
                format!("no data source on this page takes `{}`", f.param),
            );
            continue;
        };
        let days = |t: ParamType| matches!(t, ParamType::Days { .. });
        if declared.iter().any(|p| days(p.ty) != days(first.ty)) {
            cx.bad(
                &at,
                format!("the sources here disagree on what `{}` is", f.param),
            );
            continue;
        }
        match first.ty {
            ParamType::Days { .. } => {
                if f.choices.is_empty() {
                    cx.bad(&at, "a days filter lists its choices");
                }
                if !f.choices.windows(2).all(|w| w[0] < w[1]) {
                    cx.bad(&at, "choices go up, each once");
                }
                for p in &declared {
                    if let ParamType::Days { min, max } = p.ty {
                        if let Some(c) = f.choices.iter().find(|c| !(min..=max).contains(*c)) {
                            cx.bad(&at, format!("{c} days is outside {min}–{max}"));
                        }
                    }
                }
                if let Some(d) = f.default {
                    if !f.choices.contains(&d) {
                        cx.bad(&at, format!("the default {d} is not one of the choices"));
                    }
                }
            }
            ParamType::HostAlias => {
                if !f.choices.is_empty() || f.default.is_some() {
                    cx.bad(
                        &at,
                        "a host filter's choices are the registered hosts: no choices or default",
                    );
                }
            }
        }
        for (item_at, r) in &refs {
            if r.params.contains_key(&f.param) {
                cx.bad(
                    item_at,
                    format!("`{}` is set by the page's filter, not here", f.param),
                );
            }
        }
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
        let resource = page
            .resource
            .as_deref()
            .and_then(super::resources::resource);
        let mut cx = Ctx {
            page: &page.id,
            problems: &mut problems,
            resource,
        };
        match (&page.resource, resource, page.layout) {
            (Some(r), None, _) => cx.bad("", format!("`{r}` is not a resource")),
            (Some(_), Some(_), l) if l != super::model::Layout::MasterDetail => {
                cx.bad("", "only a master_detail page names a resource")
            }
            (None, _, super::model::Layout::MasterDetail) => {
                cx.bad("", "a master_detail page names its resource")
            }
            _ => {}
        }
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
                format!("this build cannot render the {:?} layout yet", page.layout),
            );
        }
        match &page.parent {
            Some(parent) if !ids.contains(parent.as_str()) => {
                cx.bad("", format!("parent `{parent}` is not a page"));
            }
            _ => {}
        }
        let reviews = page.layout == super::model::Layout::ReviewApply;
        match (page.review.is_some(), reviews) {
            (true, false) => cx.bad("", "only a review_apply page names a review"),
            (false, true) => cx.bad("", "a review_apply page names what it reviews"),
            _ => {}
        }
        match (page.sections.is_empty(), page.tabs.is_empty()) {
            // The proposals are the page: notes around them are optional.
            (true, true) if reviews => {}
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
        check_filters(&mut cx, page);
        check_embed(&mut cx, page);
        check_guide(&mut cx, page);
        if !page.list_items.is_empty() && resource.is_none() {
            cx.bad("", "list_items belong to a master_detail page");
        }
        for (i, item) in page.list_items.iter().enumerate() {
            let at = format!("list item {}", i + 1);
            if !matches!(
                item,
                Item::Notice { .. } | Item::Custom { .. } | Item::Action { .. }
            ) {
                cx.bad(&at, "a list item is a notice, a custom item or an action");
            }
            match item {
                Item::Notice { text, .. } => cx.text(&at, "text", text, MAX_TEXT),
                Item::Action { action } if super::actions::action(action).is_none() => {
                    cx.bad(&at, format!("`{action}` is not a page action"));
                }
                _ => {}
            }
        }
        check_graph(&mut cx, page);
        check_table(&mut cx, page);
        if let Some(res) = resource {
            for f in res.fields {
                if !placed.contains_key(&format!("{}#{}", page.id, f.id)) {
                    cx.bad(
                        "",
                        format!("{}'s field `{}` is on no section", res.id, f.id),
                    );
                }
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

    let mut slots: BTreeMap<String, &str> = BTreeMap::new();
    let embeds: BTreeSet<&str> = pages
        .iter()
        .filter(|p| p.layout == super::model::Layout::Embed)
        .map(|p| p.id.as_str())
        .collect();
    for page in pages {
        let mut bad = |at: String, message: String| {
            problems.push(Problem {
                page: page.id.clone(),
                at,
                message,
            })
        };
        if let Some(slot) = page.slot {
            if let Some(first) = slots.insert(format!("{slot:?}"), &page.id) {
                bad(
                    String::new(),
                    format!("{first} already fills the {slot:?} slot"),
                );
            }
        }
        if page.parent.as_deref().is_some_and(|p| embeds.contains(p)) {
            bad(String::new(), "an embed page is no page's parent".into());
        }
        for_each_item(page, |at, item| {
            if let Item::Link { page: target, .. } = item {
                if embeds.contains(target.as_str()) {
                    bad(
                        at,
                        format!("`{target}` is an embed page: nothing links to it"),
                    );
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

/// Most columns a `table` (`Page::table`) shows.
pub const MAX_TABLE_COLUMNS: usize = 6;

/// A `table` is a `master_detail` page's: its columns are the resource's
/// plain fields (text, choice, yes/no, count or time), its filters and its
/// grouping name columns of it.
fn check_table(cx: &mut Ctx, page: &Page) {
    use super::resources::FieldKind;
    let Some(t) = &page.table else { return };
    let Some(res) = cx.resource else {
        cx.bad("table", "a table belongs to a master_detail page");
        return;
    };
    cx.text("table", "title", &t.title, MAX_TITLE);
    if t.columns.is_empty() || t.columns.len() > MAX_TABLE_COLUMNS {
        cx.bad("table", format!("1 to {MAX_TABLE_COLUMNS} columns"));
    }
    for c in t.columns.iter().chain(t.subtitle.iter()) {
        match res.field(c).map(|f| f.kind) {
            Some(
                FieldKind::Text { .. }
                | FieldKind::Choice { .. }
                | FieldKind::Pick { .. }
                | FieldKind::Bool { .. }
                | FieldKind::Count
                | FieldKind::Time,
            ) => {}
            Some(_) => cx.bad("table", format!("`{c}` is not a plain field of {}", res.id)),
            None => cx.bad("table", format!("{} has no field `{c}`", res.id)),
        }
    }
    if t.subtitle.is_some() && !t.columns.iter().any(|c| *c == res.title_field) {
        cx.bad(
            "table",
            "a subtitle goes under the title's column, which is not shown",
        );
    }
    for f in t.filters.iter().chain(t.group_by.iter()) {
        if !t.columns.contains(f) {
            cx.bad(
                "table",
                format!("`{f}` filters or groups but is not a column"),
            );
        }
    }
}

/// Most facts a graph node shows under its name.
pub const MAX_GRAPH_FACTS: usize = 3;

/// A `graph` (`Page::graph`) is a `master_detail` page's: its `state` is a
/// `choice` field of the resource, `up` names at least one of its options
/// (and not all of them: a graph that can never show a link down says
/// nothing), and its facts are the resource's plain fields.
fn check_graph(cx: &mut Ctx, page: &Page) {
    use super::resources::FieldKind;
    let Some(g) = &page.graph else { return };
    let Some(res) = cx.resource else {
        cx.bad("graph", "a graph belongs to a master_detail page");
        return;
    };
    cx.text("graph", "center", &g.center, MAX_TITLE);
    match res.field(&g.state).map(|f| f.kind) {
        Some(FieldKind::Choice { options }) => {
            for v in &g.up {
                if !options.iter().any(|(o, _)| o == v) {
                    cx.bad(
                        "graph",
                        format!("`{v}` is not one of {}'s `{}` values", res.id, g.state),
                    );
                }
            }
            if g.up.is_empty() || g.up.len() >= options.len() {
                cx.bad(
                    "graph",
                    "`up` names some of the state's values, not none and not all",
                );
            }
        }
        _ => cx.bad(
            "graph",
            format!("`{}` is not a choice field of {}", g.state, res.id),
        ),
    }
    if g.facts.len() > MAX_GRAPH_FACTS {
        cx.bad(
            "graph",
            format!("at most {MAX_GRAPH_FACTS} facts under a node"),
        );
    }
    for f in &g.facts {
        match res.field(f).map(|f| f.kind) {
            Some(FieldKind::Text { .. } | FieldKind::Count | FieldKind::Time) => {}
            Some(_) => cx.bad(
                "graph",
                format!("`{f}` is not a plain field: a fact is text, a count or a time"),
            ),
            None => cx.bad("graph", format!("`{f}` is not a field of {}", res.id)),
        }
    }
}

/// Call `f` with every item of `page` and where it is.
pub fn for_each_item(page: &Page, mut f: impl FnMut(String, &Item)) {
    for (i, item) in page.list_items.iter().enumerate() {
        f(format!("list item {}", i + 1), item);
    }
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
    for page in pages
        .iter()
        .filter(|p| p.resource.is_none() && p.layout != super::model::Layout::Guide)
    {
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
