//! The page DSL (`fleet.page/1`): what a page spec may say. It is data, never
//! code: every node is a closed, tagged shape, unknown fields are refused, and
//! a spec can only NAME things the registries declare — setting keys
//! (`service::settings::SPECS`), data sources (`pages::sources`), other pages
//! and widgets (`pages::catalog`). It cannot declare a type, a default, a
//! label for a setting or a validation rule; `pages::validate` refuses a spec
//! that names anything the registries do not know.
//!
//! `docs/page-spec.schema.json` is generated from these types (schemars) for
//! editors and for an LLM's structured output.

use rmcp::schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The one spec version this build reads.
pub const SPEC_VERSION: &str = "fleet.page/1";

/// One page: a route (`id`), a layout, and its content as sections or tabs
/// of sections.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(deny_unknown_fields)]
pub struct Page {
    /// Always "fleet.page/1".
    pub spec: String,
    /// Dotted lowercase route, e.g. "settings.automation". Also the deep
    /// link: `settings/<id>#<key>`.
    pub id: String,
    pub title: String,
    /// The page this one sits under in the page tree.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    /// One plain sentence under the title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intro: Option<String>,
    pub layout: Layout,
    /// A `master_detail` page's resource (`pages::resources`): the list
    /// shows its records, and the sections lay out one record's fields —
    /// a `field` item's `key` names a field of the resource there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource: Option<String>,
    /// A `master_detail` page's items about the whole collection, shown
    /// above the list: notices and custom items only.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub list_items: Vec<Item>,
    /// A `master_detail` page's records also drawn as a graph above the
    /// list: each one a node joined to the centre, its line solid while
    /// its state is up and dashed while it is not (Federation's linked
    /// hubs). The list stays the text alternative and the keyboard path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub graph: Option<GraphView>,
    /// A `master_detail` page's records also shown as one table (M15 step
    /// G4.7: People & devices), with filters and a grouping. A row opens
    /// its record; the list stays the keyboard path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub table: Option<TableView>,
    /// An `embed` page's place in a hand-built screen (`Slot`). Only an
    /// embed page names one, and each slot has at most one page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slot: Option<Slot>,
    /// A `review_apply` page's proposals: what the page lists for a person
    /// to apply or reject.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review: Option<ReviewSource>,
    /// A `data_page`'s filter bar: each filter sets the parameter of the
    /// same name on every data item whose source declares it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub filters: Vec<Filter>,
    /// Either `sections` or `tabs`, never both.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sections: Vec<Section>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tabs: Vec<Tab>,
}

/// How a `master_detail` page draws its records as a graph (`Page::graph`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(deny_unknown_fields)]
pub struct GraphView {
    /// The centre node's name, e.g. "This hub".
    pub center: String,
    /// The resource's `choice` field each line follows.
    pub state: String,
    /// The values of `state` drawn as a link that is up (a solid line);
    /// every other value is down (a dashed line).
    pub up: Vec<String>,
    /// Fields shown under a node's name, at most three: plain values
    /// (text, count or time), never a list.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub facts: Vec<String>,
}

/// How a `master_detail` page shows its records as a table (`Page::table`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(deny_unknown_fields)]
pub struct TableView {
    /// The view's name, e.g. "People & devices".
    pub title: String,
    /// The resource's plain fields, one column each, at most six.
    pub columns: Vec<String>,
    /// Columns that get a filter: a select over the values the records hold.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub filters: Vec<String>,
    /// A column the rows can be grouped by, offered as a switch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_by: Option<String>,
    /// A plain field shown in a smaller line under the record's title in its
    /// column (M15 step G7.14: a device's "phone · fleet-mobile 0.5.4").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subtitle: Option<String>,
}

/// One control in a `data_page`'s filter bar, bound by name to a source
/// parameter (`pages::sources`). Its control follows the parameter's type:
/// a `days` parameter is a select over `choices`, a `host` parameter a
/// select over the registered hosts with "All hosts" first. A data item
/// may not also set a filtered parameter literally.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(deny_unknown_fields)]
pub struct Filter {
    /// The source parameter this filter sets, e.g. "days" or "host".
    pub param: String,
    /// The control's label; the parameter's own name when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// A `days` filter's options, ascending. A `host` filter has none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub choices: Vec<u64>,
    /// The option picked when the page opens: one of `choices`; the first
    /// when absent. A `host` filter starts at "All hosts".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<u64>,
}

/// What a `review_apply` page reviews. Closed: each is a list of proposed
/// changes with an apply and a reject command behind it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(rename_all = "snake_case")]
pub enum ReviewSource {
    /// Settings an agent proposed over the control API (`set_setting
    /// { propose: true }`): `setting_proposals`, applied or rejected with
    /// `decide_setting_proposals`.
    Settings,
    /// Guides an agent proposed (`guide { propose }`, `service::guides`):
    /// approved or rejected one at a time; an approved one joins the pages
    /// under this one.
    Guides,
}

/// The prepared layouts (design §4). A layout decides spacing, saving
/// (auto-save or draft) and how the page degrades on a phone; a spec never
/// styles anything itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(rename_all = "snake_case")]
pub enum Layout {
    /// L1: scalar settings in sections, auto-saved.
    Category,
    /// L2: a list of resources with a detail pane.
    MasterDetail,
    /// L3: a step-by-step wizard.
    Flow,
    /// L4: one object with a draft and Apply.
    ObjectEditor,
    /// L5: cards of values and links, nothing editable.
    Cards,
    /// L6: a diff of proposed changes to accept or reject.
    ReviewApply,
    /// L7: stats, charts and tables over data sources.
    DataPage,
    /// L8: items placed inside a hand-built screen at a named `slot`
    /// rather than a page of their own. Never in the page tree.
    Embed,
    /// L9: a step-by-step guide. Each section is one step, shown one at a
    /// time with Back / Next; a step's `when` skips it. Its fields are a
    /// path through settings whose home is another page, saved as they
    /// change: a guide never owns a setting. A guide may also be stored at
    /// runtime (`service::guides`), proposed by an agent and approved by a
    /// person.
    Guide,
}

/// The places in hand-built screens an `embed` page fills. Closed: each is
/// a spot in the desktop's own UI with the context it hands its items
/// (`catalog::slot_views` says which items each one takes).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(rename_all = "snake_case")]
pub enum Slot {
    /// Host detail, under the account: context the host and its account.
    HostDetail,
    /// A Hosts-list account group's title line, after its name.
    HostsGroupTitle,
    /// A Hosts-list account group's header, right side.
    HostsGroup,
    /// Each host chip in the New-session dialog, under the alias.
    NewSessionChip,
    /// Under the New-session dialog's host chips: the selected host.
    NewSessionHost,
    /// The window's status footer, right end.
    StatusFooter,
}

/// How an `account_usage` item draws an account's plan headroom. The
/// wording, staleness and severity rules are the same in every view (the
/// hosts-view design's "Showing usage" and "Staleness and failure").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(rename_all = "snake_case")]
pub enum UsageView {
    /// The full block: status lines, the 5-hour and weekly rows with pace,
    /// per-model rows, the source and age, and a floor-respecting refresh.
    Block,
    /// The plan tier and a freshness mark.
    Freshness,
    /// The 5-hour and weekly mini bars with % left and reset.
    Bars,
    /// One short headroom label (a host chip).
    Chip,
    /// One full sentence for the selected host.
    Line,
    /// A warning when the selected host's account is low; nothing otherwise.
    Warning,
    /// The status footer's one segment for the whole fleet.
    Footer,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(deny_unknown_fields)]
pub struct Tab {
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when: Option<Condition>,
    pub sections: Vec<Section>,
}

/// A titled group of items. `collapsible` makes it an accordion; an
/// `advanced` section starts collapsed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(deny_unknown_fields)]
pub struct Section {
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intro: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub collapsible: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub advanced: bool,
    /// A `master_detail` page's overview: its `count` and `money` fields
    /// shown as tiles (label, value, and the field's `sub` line) rather
    /// than rows.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub tiles: bool,
    /// Settings fields that are each a subset of the same options, shown as
    /// one grid: a row per option, a column per field, a tick where the
    /// field holds the option (Orbit Fleet 11.9, the notifications matrix).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub matrix: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when: Option<Condition>,
    pub items: Vec<Item>,
}

/// One thing on a page. Tagged by `type`, one shape per purpose.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Item {
    /// An editable setting. Its label, help, bounds, danger and default come
    /// from the registry; `hint` adds one line under it on this page only.
    Field {
        key: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        widget: Option<Widget>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        hint: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        when: Option<Condition>,
    },
    /// One number: a `scalar` source, or one `field` of a `record` source.
    Stat {
        source: SourceRef,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        field: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
    },
    /// A read-only key → value list of a `record` source.
    Record { source: SourceRef },
    /// A table of a `rows` source; `columns` picks and orders them (all when
    /// empty).
    Table {
        source: SourceRef,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        columns: Vec<String>,
        /// Offer "Copy as text": the source's label with the filters, then
        /// one line per row, `first: rest, …`.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        copy: bool,
    },
    /// A chart of a `series` source.
    Chart {
        source: SourceRef,
        chart: ChartKind,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
    },
    /// An account's plan headroom from an `account_usage` source, drawn as
    /// `view`. On a page it shows every account; in an embed slot, the
    /// slot's host or account.
    AccountUsage { source: SourceRef, view: UsageView },
    /// A static callout.
    Notice { tone: Tone, text: String },
    /// A hand-written component registered in code: the one escape hatch
    /// (design §8), for what the catalog cannot express yet. Closed set,
    /// capped at [`crate::pages::catalog::MAX_CUSTOM`] uses across all
    /// pages.
    Custom { component: CustomComponent },
    /// A button that runs a page action (`pages::actions`), then re-reads
    /// the page's data items.
    Action { action: String },
    /// A link to another page.
    Link {
        page: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
    },
}

/// A data source by id, with literal parameters.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(deny_unknown_fields)]
pub struct SourceRef {
    pub id: String,
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub params: serde_json::Map<String, serde_json::Value>,
}

/// The widgets a `field` may ask for instead of its kind's default. Each
/// accepts only some setting kinds (`catalog::accepts`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(rename_all = "snake_case")]
pub enum Widget {
    Switch,
    Number,
    Duration,
    Select,
    Radio,
    Multiselect,
    Text,
    Textarea,
    KeyValueTable,
    IdList,
    Readonly,
}

/// The registered hand-written components. Each one is a debt: when the
/// catalog can express it (a data source plus an action), it is replaced
/// and removed from this list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(rename_all = "snake_case")]
pub enum CustomComponent {
    /// "Show what auto-tidy would do": the tidy candidates the ticked
    /// reasons would act on (work graph M7.3; `AutoTidyPreview.svelte`).
    /// Waits on a `work.tidy` data source with a param from the page.
    AutoTidyPreview,
    /// Proposed orgs (from owners of live sessions and tracker sites), one
    /// click each to create with their rule and tracker (work graph M5.4;
    /// `OrgSuggestions.svelte`). Waits on actions that chain.
    OrgSuggestions,
    /// A tracker's last sync pass, Asana's section map and Jev's proposals
    /// for it (work graph M11.4, M6, Jev J3; `TrackerExtras.svelte`). Shown
    /// in a tracker's detail. Waits on the review_apply layout (P5), which
    /// takes the proposals, and a choice-map field for the section map.
    TrackerExtras,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(rename_all = "snake_case")]
pub enum ChartKind {
    Line,
    Bar,
    StackedBar,
    Sparkline,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(rename_all = "snake_case")]
pub enum Tone {
    Info,
    Warn,
    Danger,
    /// Something the page will offer and does not yet, shown greyed with
    /// "Not built yet" (M15 step G7.14: a debug device's live screen).
    Later,
}

/// When an item, section or tab is shown. Exactly one form per condition:
/// `{key, eq}`, `{key, in}`, `{key, truthy}`, `{all}`, `{any}` or `{not}`.
/// No expressions: a need beyond these becomes a data source or a widget.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(deny_unknown_fields)]
pub struct Condition {
    /// A registered setting key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub eq: Option<String>,
    #[serde(default, rename = "in", skip_serializing_if = "Option::is_none")]
    pub one_of: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub truthy: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub all: Option<Vec<Condition>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub any: Option<Vec<Condition>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not: Option<Box<Condition>>,
}
