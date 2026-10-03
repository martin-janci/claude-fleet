//! The closed catalog a page spec draws from: which widget each setting kind
//! gets by default, which widgets a kind accepts, and which items each
//! layout can hold. A renderer implements exactly this catalog; a new widget
//! or layout capability is a code change here, never a spec feature.

use super::model::{Item, Layout, Slot, UsageView, Widget};
use crate::service::settings::Kind;

/// The widget a setting of `kind` renders as when a spec names none.
pub fn default_widget(kind: Kind) -> Widget {
    match kind {
        Kind::Bool => Widget::Switch,
        Kind::Secs | Kind::SecsMin(_) => Widget::Duration,
        Kind::Int { .. } => Widget::Number,
        Kind::Choice(_) => Widget::Select,
        Kind::ChoiceSet(_) => Widget::Multiselect,
        Kind::PathMap => Widget::KeyValueTable,
        Kind::IdSet => Widget::IdList,
        Kind::PriceMap => Widget::Textarea,
        Kind::Text { .. } => Widget::Text,
    }
}

/// Whether `widget` can edit (or show) a setting of `kind`.
pub fn accepts(widget: Widget, kind: Kind) -> bool {
    match widget {
        Widget::Readonly => true,
        Widget::Switch => matches!(kind, Kind::Bool),
        Widget::Number => matches!(kind, Kind::Int { .. } | Kind::Secs | Kind::SecsMin(_)),
        Widget::Duration => matches!(kind, Kind::Secs | Kind::SecsMin(_)),
        Widget::Select | Widget::Radio => matches!(kind, Kind::Choice(_)),
        Widget::Multiselect => matches!(kind, Kind::ChoiceSet(_)),
        Widget::KeyValueTable => matches!(kind, Kind::PathMap),
        Widget::IdList => matches!(kind, Kind::IdSet),
        Widget::Textarea => matches!(kind, Kind::PathMap | Kind::PriceMap | Kind::IdSet),
        Widget::Text => matches!(kind, Kind::Text { .. }),
    }
}

/// The item's `type` tag, for messages.
pub fn item_type(item: &Item) -> &'static str {
    match item {
        Item::Field { .. } => "field",
        Item::Stat { .. } => "stat",
        Item::Record { .. } => "record",
        Item::Table { .. } => "table",
        Item::Chart { .. } => "chart",
        Item::Notice { .. } => "notice",
        Item::Link { .. } => "link",
        Item::Custom { .. } => "custom",
        Item::Action { .. } => "action",
        Item::AccountUsage { .. } => "account_usage",
    }
}

/// Most `custom` items all pages together may hold: the escape hatch stays
/// small (design §8). Raised from 3 to 4 for `tracker_extras` (P4b); P5 paid
/// it back to 3 by expressing `work_retention` as a data source, a table and
/// a page action.
pub const MAX_CUSTOM: usize = 3;

/// Whether this build can render `layout` at all. The resource layouts wait
/// on the resource and action registries (design P4).
pub fn layout_supported(layout: Layout) -> bool {
    !layout_item_types(layout).is_empty()
}

/// The item types a page of `layout` can hold: settings pages edit, cards
/// only show and link, data pages show data. Empty for a layout this build
/// cannot render yet.
pub fn layout_item_types(layout: Layout) -> &'static [&'static str] {
    match layout {
        Layout::Category => &[
            "field", "stat", "record", "table", "action", "notice", "link", "custom",
        ],
        Layout::Cards => &["stat", "record", "notice", "link"],
        Layout::DataPage => &[
            "stat",
            "record",
            "table",
            "chart",
            "account_usage",
            "action",
            "notice",
            "link",
        ],
        Layout::Embed => &["account_usage"],
        Layout::MasterDetail => &["field", "notice", "custom"],
        Layout::ReviewApply => &["notice", "link"],
        Layout::Guide => &["field", "stat", "record", "action", "notice", "link"],
        Layout::Flow | Layout::ObjectEditor => &[],
    }
}

/// Whether a page of `layout` can hold `item`.
pub fn layout_allows(layout: Layout, item: &Item) -> bool {
    layout_item_types(layout).contains(&item_type(item))
}

/// Every layout, in the design's order.
pub const LAYOUTS: &[Layout] = &[
    Layout::Category,
    Layout::MasterDetail,
    Layout::Flow,
    Layout::ObjectEditor,
    Layout::Cards,
    Layout::ReviewApply,
    Layout::DataPage,
    Layout::Embed,
    Layout::Guide,
];

/// Every slot an `embed` page can fill.
pub const SLOTS: &[Slot] = &[
    Slot::HostDetail,
    Slot::HostsGroupTitle,
    Slot::HostsGroup,
    Slot::NewSessionChip,
    Slot::NewSessionHost,
    Slot::StatusFooter,
];

/// The `account_usage` views a slot can hold: what fits the space and the
/// context the screen hands it (a host, an account, or the whole fleet).
pub fn slot_views(slot: Slot) -> &'static [UsageView] {
    match slot {
        Slot::HostDetail => &[UsageView::Block],
        Slot::HostsGroupTitle => &[UsageView::Freshness],
        Slot::HostsGroup => &[UsageView::Bars],
        Slot::NewSessionChip => &[UsageView::Chip],
        Slot::NewSessionHost => &[UsageView::Line, UsageView::Warning],
        Slot::StatusFooter => &[UsageView::Footer],
    }
}

/// The views an `account_usage` item on a page (not in a slot) can take:
/// a page shows every account, so only the views that name their account.
pub const PAGE_USAGE_VIEWS: &[UsageView] = &[UsageView::Block];

/// Every widget.
pub const WIDGETS: &[Widget] = &[
    Widget::Switch,
    Widget::Number,
    Widget::Duration,
    Widget::Select,
    Widget::Radio,
    Widget::Multiselect,
    Widget::Text,
    Widget::Textarea,
    Widget::KeyValueTable,
    Widget::IdList,
    Widget::Readonly,
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::settings::SPECS;

    #[test]
    fn every_kind_accepts_its_default_widget() {
        for spec in SPECS {
            assert!(
                accepts(default_widget(spec.kind), spec.kind),
                "{}: its default widget does not accept its kind",
                spec.key
            );
        }
    }

    #[test]
    fn a_switch_edits_only_a_bool() {
        assert!(accepts(Widget::Switch, Kind::Bool));
        assert!(!accepts(Widget::Switch, Kind::Secs));
        assert!(!accepts(Widget::Select, Kind::ChoiceSet(&["a"])));
        assert!(accepts(Widget::Readonly, Kind::PriceMap));
    }
}
