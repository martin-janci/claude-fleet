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
    /// A `review_apply` page's proposals: what the page lists for a person
    /// to apply or reject.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review: Option<ReviewSource>,
    /// Either `sections` or `tabs`, never both.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sections: Vec<Section>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tabs: Vec<Tab>,
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
    },
    /// A chart of a `series` source.
    Chart {
        source: SourceRef,
        chart: ChartKind,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
    },
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
