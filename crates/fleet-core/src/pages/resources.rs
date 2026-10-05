//! Resources (declarative pages P4): collections of things — orgs today,
//! trackers next — that a `master_detail` page lists and edits. A resource
//! type says, as data, what a record's fields are, which of them a person
//! may change and how, and which ACTIONS exist. An action names an existing
//! desktop command and how to build its arguments from the record, a
//! sub-item or a form; it never names code. So hub routing and each
//! command's hub verdict stay exactly what they are (`backend/verdicts.rs`),
//! and a paired desktop shows the same page read-only with that command's
//! reason.
//!
//! `pages::validate` holds a `master_detail` page to its resource: every
//! field placed once, every `when` naming a field. `resource_commands_exist`
//! (src-tauri) holds every command named here to the handler list.

use serde::Serialize;

/// How an action's arguments reach the command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Envelope {
    /// `invoke(command, { args: {…} })`, the shape the org and tracker
    /// commands take.
    Args,
}

/// Where one argument's value comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "from", content = "name", rename_all = "snake_case")]
pub enum Bind {
    /// A field of the record the action runs on.
    Record(&'static str),
    /// The sub-item itself (a host alias in an org's `hosts`).
    Item,
    /// A field of the sub-item (a rule's `id`).
    ItemField(&'static str),
    /// A value the person typed or picked in the action's form.
    Param(&'static str),
    /// JSON `null`.
    Null,
}

/// Where a select's options come from: a frontend list the renderer already
/// holds, minus the values the record's own list field already has.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OptionSource {
    /// Registered hosts, by alias.
    Hosts,
    /// Trackers, by id, labelled by name.
    Trackers,
    /// Orgs, by name (what `list_orgs` answers).
    Orgs,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ParamKind {
    Text {
        max: usize,
        placeholder: &'static str,
    },
    Color,
    /// Write-only: a password input, sent once, never shown again.
    Secret,
    Options {
        source: OptionSource,
    },
}

/// One input of an action's form.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ParamSpec {
    pub name: &'static str,
    pub label: &'static str,
    #[serde(flatten)]
    pub kind: ParamKind,
    pub required: bool,
}

/// Something a person can do to a resource: add one, change one, remove
/// one, or add / remove a sub-item. `confirm` is asked first when set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ActionSpec {
    pub id: &'static str,
    pub label: &'static str,
    /// The desktop command it runs; its hub verdict applies unchanged.
    pub command: &'static str,
    pub envelope: Envelope,
    /// Argument name → where its value comes from.
    pub bind: &'static [(&'static str, Bind)],
    pub params: &'static [ParamSpec],
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confirm: Option<&'static str>,
    /// Only for records whose `variant_by` field is one of these (all when
    /// empty).
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    pub variants: &'static [&'static str],
    /// The command answers `{ ok, error? }`: say which, rather than just
    /// "done" (a tracker's Test).
    pub report: bool,
}

impl ActionSpec {
    pub const fn new(
        id: &'static str,
        label: &'static str,
        command: &'static str,
        bind: &'static [(&'static str, Bind)],
    ) -> Self {
        ActionSpec {
            id,
            label,
            command,
            envelope: Envelope::Args,
            bind,
            params: &[],
            confirm: None,
            variants: &[],
            report: false,
        }
    }
    pub const fn params(self, params: &'static [ParamSpec]) -> Self {
        ActionSpec { params, ..self }
    }
    pub const fn confirm(self, message: &'static str) -> Self {
        ActionSpec {
            confirm: Some(message),
            ..self
        }
    }
    pub const fn variants(self, variants: &'static [&'static str]) -> Self {
        ActionSpec { variants, ..self }
    }
    pub const fn report(self) -> Self {
        ActionSpec {
            report: true,
            ..self
        }
    }
}

const fn param(
    name: &'static str,
    label: &'static str,
    kind: ParamKind,
    required: bool,
) -> ParamSpec {
    ParamSpec {
        name,
        label,
        kind,
        required,
    }
}

/// How a record field is stored and, if it can be edited, written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FieldKind {
    Text {
        max: usize,
    },
    Color,
    /// `true` / `false` in the record. `on_off` writes `"on"` / `"off"`
    /// instead of a boolean. `default` is what an absent value means.
    Bool {
        on_off: bool,
        default: bool,
    },
    /// `true` / `false` / `null` (inherit) in the record, written as
    /// `"on"` / `"off"` / `"inherit"`.
    Inherit,
    /// One of a fixed set, shown by its label (`(value, label)`).
    Choice {
        options: &'static [(&'static str, &'static str)],
    },
    /// Unix seconds, shown as how long ago ("5 min ago", "never").
    Time,
    /// A list of sub-items, each shown with `label` and changed through
    /// `remove` / `add` actions rather than the record's Apply.
    Items {
        item_label: ItemLabel,
        #[serde(skip_serializing_if = "Option::is_none")]
        remove: Option<ActionSpec>,
        add: &'static [ActionSpec],
    },
}

/// How a sub-item is shown: a closed set of formatters the renderer
/// implements, never a template.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "type", content = "field", rename_all = "snake_case")]
pub enum ItemLabel {
    /// The item is a string; show it.
    Plain,
    /// Show this field of the item.
    Field(&'static str),
    /// An org rule: `owner/repo` (or `owner/*`), `path: …`, `host: …`.
    OrgRule,
}

/// A badge in the list and the detail header while a field has a value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "when", rename_all = "snake_case")]
pub enum Badge {
    /// While the field is `true`.
    True { text: &'static str },
    /// While the field is `false` (an absent value takes the default).
    False { text: &'static str },
    /// While an `inherit` field is set: `text` then `on` / `off`.
    Set { text: &'static str },
    /// Always: a `choice` field's option label.
    Label,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct FieldSpec {
    pub id: &'static str,
    pub label: &'static str,
    pub help: &'static str,
    #[serde(flatten)]
    pub kind: FieldKind,
    /// The update action's argument this field is written through;
    /// `None`: shown, never edited.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub edit: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub badge: Option<Badge>,
    /// Asked before Apply writes a change to this field.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confirm: Option<&'static str>,
    /// Non-empty: the value lives at this path inside the record's `edit`
    /// object (a tracker's `settings`), is read from there, and Apply sends
    /// the whole object with it set — the rest kept as it is.
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    pub merge_path: &'static [&'static str],
}

impl FieldSpec {
    pub const fn new(
        id: &'static str,
        label: &'static str,
        help: &'static str,
        kind: FieldKind,
    ) -> Self {
        FieldSpec {
            id,
            label,
            help,
            kind,
            edit: None,
            badge: None,
            confirm: None,
            merge_path: &[],
        }
    }
    pub const fn edit(self, arg: &'static str) -> Self {
        FieldSpec {
            edit: Some(arg),
            ..self
        }
    }
    /// Edited at `path` inside the record's `arg` object.
    pub const fn merge(self, arg: &'static str, path: &'static [&'static str]) -> Self {
        FieldSpec {
            edit: Some(arg),
            merge_path: path,
            ..self
        }
    }
    pub const fn badge(self, badge: Badge) -> Self {
        FieldSpec {
            badge: Some(badge),
            ..self
        }
    }
    pub const fn confirm(self, message: &'static str) -> Self {
        FieldSpec {
            confirm: Some(message),
            ..self
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct ResourceType {
    pub id: &'static str,
    pub label: &'static str,
    pub plural: &'static str,
    pub help: &'static str,
    /// The desktop command listing the records (it routes to the hub when
    /// paired, so the page reads the hub's records there).
    pub list: &'static str,
    pub id_field: &'static str,
    pub title_field: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color_field: Option<&'static str>,
    /// Said when there is none.
    pub empty: &'static str,
    pub fields: &'static [FieldSpec],
    #[serde(skip_serializing_if = "Option::is_none")]
    pub create: Option<ActionSpec>,
    /// Apply's action: the record's id plus every changed editable field.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub update: Option<ActionSpec>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delete: Option<ActionSpec>,
    /// Actions on one record besides update and delete (a tracker's Test).
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    pub actions: &'static [ActionSpec],
    /// A record is added through this flow (`pages::flows`) rather than a
    /// one-step `create` form.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub create_flow: Option<&'static str>,
    /// The field an action's `variants` are matched against.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub variant_by: Option<&'static str>,
}

impl ResourceType {
    pub fn field(&self, id: &str) -> Option<&'static FieldSpec> {
        self.fields.iter().find(|f| f.id == id)
    }

    /// Every action this type declares, sub-item actions included.
    pub fn actions(&self) -> Vec<&ActionSpec> {
        let mut out: Vec<&ActionSpec> = [&self.create, &self.update, &self.delete]
            .into_iter()
            .flatten()
            .collect();
        out.extend(self.actions.iter());
        for f in self.fields {
            if let FieldKind::Items { remove, add, .. } = &f.kind {
                out.extend(remove.iter());
                out.extend(add.iter());
            }
        }
        out
    }
}

const ORG_ID: (&str, Bind) = ("org_id", Bind::Record("id"));
const TRACKER_ID: (&str, Bind) = ("tracker_id", Bind::Record("id"));

const fn text(max: usize, placeholder: &'static str) -> ParamKind {
    ParamKind::Text { max, placeholder }
}

/// An org's rule, added one kind at a time: each form asks for one thing.
const ORG_RULE_ADDS: &[ActionSpec] = &[
    ActionSpec::new(
        "org.add_owner_rule",
        "Add owner rule",
        "add_org_rule",
        &[
            ORG_ID,
            ("owner", Bind::Param("owner")),
            ("repo", Bind::Param("repo")),
        ],
    )
    .params(&[
        param("owner", "GitHub owner", text(100, "acme"), true),
        param("repo", "Repository (optional)", text(100, "api"), false),
    ]),
    ActionSpec::new(
        "org.add_path_rule",
        "Add path rule",
        "add_org_rule",
        &[ORG_ID, ("path_prefix", Bind::Param("path_prefix"))],
    )
    .params(&[param(
        "path_prefix",
        "Path prefix",
        text(1024, "/home/me/work/acme"),
        true,
    )]),
    ActionSpec::new(
        "org.add_host_rule",
        "Add host rule",
        "add_org_rule",
        &[ORG_ID, ("host_alias", Bind::Param("host_alias"))],
    )
    .params(&[param("host_alias", "Host", text(100, "hetzner-a"), true)]),
];

const ORG: ResourceType = ResourceType {
    id: "org",
    label: "Organisation",
    plural: "Organisations",
    help: "Optional. Name an org to merge or split GitHub owners, to attach a tracker, or to make it a boundary: a host in an org lets its Claude read only that org's (and unassigned) work.",
    list: "list_orgs",
    id_field: "id",
    title_field: "name",
    color_field: Some("color"),
    empty: "No organisations. Without one, scopes come from GitHub owners and nothing is fenced.",
    fields: &[
        FieldSpec::new("name", "Name", "What the scope selector and the Work view call it.", FieldKind::Text { max: 80 }).edit("name"),
        FieldSpec::new("color", "Colour", "Marks its sessions in the sidebar.", FieldKind::Color).edit("color"),
        FieldSpec::new(
            "rules",
            "Rules",
            "Which sessions belong to it: a GitHub owner (or one repository), a path prefix, or a host.",
            FieldKind::Items {
                item_label: ItemLabel::OrgRule,
                remove: Some(ActionSpec::new("org.remove_rule", "Remove rule", "remove_org_rule", &[("rule_id", Bind::ItemField("id"))])),
                add: ORG_RULE_ADDS,
            },
        ),
        FieldSpec::new(
            "hosts",
            "Hosts",
            "A host in an org: its per-host token reads only this org's (and unassigned) work.",
            FieldKind::Items {
                item_label: ItemLabel::Plain,
                remove: Some(ActionSpec::new(
                    "org.unassign_host",
                    "Take the host out",
                    "assign_host_org",
                    &[("host_alias", Bind::Item), ("org_id", Bind::Null)],
                )),
                add: &[ActionSpec::new("org.assign_host", "Add host", "assign_host_org", &[("host_alias", Bind::Param("host")), ORG_ID])
                    .params(&[param("host", "Host", ParamKind::Options { source: OptionSource::Hosts }, true)])],
            },
        ),
        FieldSpec::new(
            "trackers",
            "Trackers",
            "Trackers whose tickets belong to this org.",
            FieldKind::Items {
                item_label: ItemLabel::Field("name"),
                remove: Some(ActionSpec::new(
                    "org.unassign_tracker",
                    "Take the tracker out",
                    "assign_tracker_org",
                    &[("tracker_id", Bind::ItemField("id")), ("org_id", Bind::Null)],
                )),
                add: &[ActionSpec::new("org.assign_tracker", "Add tracker", "assign_tracker_org", &[("tracker_id", Bind::Param("tracker")), ORG_ID])
                    .params(&[param("tracker", "Tracker", ParamKind::Options { source: OptionSource::Trackers }, true)])],
            },
        ),
        FieldSpec::new(
            "isolate_sessions",
            "Isolate sessions",
            "Also hide this org's sessions from other orgs' hosts, and theirs from its hosts: list, peer status, messages. Work data is fenced either way.",
            FieldKind::Bool { on_off: false, default: false },
        )
        .edit("isolate_sessions")
        .badge(Badge::True { text: "isolates sessions" })
        .confirm("Hosts outside this org will no longer list or message its sessions, and its hosts will not see other orgs' sessions. It can break a controller that dispatches across companies."),
        FieldSpec::new(
            "auto_tidy",
            "Auto-tidy",
            "Auto-tidy for this org's sessions: on or off whatever the fleet-wide setting says, or inherit it. Safe kill only, never a session in use.",
            FieldKind::Inherit,
        )
        .edit("auto_tidy")
        .badge(Badge::Set { text: "auto-tidy" }),
        FieldSpec::new(
            "jev_allowed",
            "Send to Jev",
            "Let this org's redacted prompts and ticket titles go to TypeSafe's decision model when Decisions (Jev) is on. Only ids and numbers are recorded; an answer is at most a suggestion.",
            FieldKind::Bool { on_off: true, default: false },
        )
        .edit("jev")
        .badge(Badge::True { text: "sends to Jev" })
        .confirm("This org's redacted prompts and ticket titles will be sent to TypeSafe when Decisions (Jev) is on."),
        FieldSpec::new(
            "bound_sees_unassigned",
            "Bound devices see unassigned",
            "Devices paired to this org (fleet-hub pair --org) also see work and sessions that belong to no org, as a host does. Off: only this org's own.",
            FieldKind::Bool { on_off: false, default: true },
        )
        .edit("bound_sees_unassigned")
        .badge(Badge::False { text: "bound devices: own only" }),
    ],
    create: Some(
        ActionSpec::new("org.add", "Add organisation", "add_org", &[("name", Bind::Param("name")), ("color", Bind::Param("color"))])
            .params(&[param("name", "Name", text(80, "Company A"), true), param("color", "Colour", ParamKind::Color, false)]),
    ),
    update: Some(ActionSpec::new("org.update", "Apply", "update_org", &[ORG_ID])),
    delete: Some(
        ActionSpec::new("org.remove", "Remove organisation", "remove_org", &[ORG_ID])
            .confirm("Its rules go with it, and its hosts and trackers become unassigned. Sessions are not touched."),
    ),
    actions: &[],
    create_flow: None,
    variant_by: None,
};

/// `(id, label)` of every provider; the same as `PROVIDERS` in
/// `src/lib/trackers.ts`.
pub const TRACKER_PROVIDERS: &[(&str, &str)] = &[
    ("jira", "Jira Cloud"),
    ("jira_dc", "Jira Data Center"),
    ("github", "GitHub"),
    ("asana", "Asana"),
    ("linear", "Linear"),
];

/// A tracker's states as `trackerStateBadge` (`src/lib/trackers.ts`) says them.
pub const TRACKER_STATES: &[(&str, &str)] = &[
    ("ok", "ok"),
    ("auth_failed", "token expired or wrong"),
    ("captcha", "log in via the browser"),
    ("rate_limited", "rate-limited"),
    ("unreachable", "unreachable"),
    ("unconfigured", "not tested yet"),
];

const TRACKER: ResourceType = ResourceType {
    id: "tracker",
    label: "Tracker",
    plural: "Trackers",
    help: "Trackers add a ticket's title and status to the sessions working on it, and list your tickets in ⌘K. Nothing needs one: keys in branch names group sessions without any tracker.",
    list: "list_trackers",
    id_field: "id",
    title_field: "name",
    color_field: None,
    empty: "No trackers. Keys in branch names group sessions without one; connect a tracker for titles and statuses.",
    fields: &[
        FieldSpec::new("name", "Name", "What chips and ⌘K call it.", FieldKind::Text { max: 80 }).edit("name"),
        FieldSpec::new("provider", "Tracker", "Which tracker it is.", FieldKind::Choice { options: TRACKER_PROVIDERS }).badge(Badge::Label),
        FieldSpec::new("site_url", "Site", "The site or workspace it reads.", FieldKind::Text { max: 2048 }),
        FieldSpec::new("state", "State", "How the last test or sync went.", FieldKind::Choice { options: TRACKER_STATES }).badge(Badge::Label),
        FieldSpec::new("last_sync_at", "Last sync", "When a sync last finished for it.", FieldKind::Time),
        FieldSpec::new("last_error", "Last error", "What the tracker said the last time it refused.", FieldKind::Text { max: 4096 }),
        FieldSpec::new("username", "Account", "The account its credential belongs to (Jira Cloud).", FieldKind::Text { max: 320 }),
        FieldSpec::new("credential_hint", "Credential", "The end of the stored credential; the rest is never shown.", FieldKind::Text { max: 64 }),
        FieldSpec::new("transport", "Reached through", "direct, or gh / requests on a named host.", FieldKind::Text { max: 200 }),
        FieldSpec::new(
            "pr_remote_link",
            "Link pull requests",
            "Add a session's pull request to its ticket as a link, only for work you linked or started. The token needs permission to edit issues.",
            FieldKind::Bool { on_off: false, default: false },
        )
        .merge("settings", &["write_back", "pr_remote_link"])
        .confirm("Fleet will write to this tracker: a link on each ticket whose work you linked or started gets a pull request."),
    ],
    create: None,
    update: Some(ActionSpec::new("tracker.update", "Apply", "update_tracker", &[TRACKER_ID])),
    delete: Some(
        ActionSpec::new("tracker.remove", "Remove tracker", "remove_tracker", &[TRACKER_ID])
            .confirm("Its cached tickets go with it; sessions keep their keys and links."),
    ),
    actions: &[
        ActionSpec::new("tracker.test", "Test", "test_tracker", &[TRACKER_ID]).report(),
        ActionSpec::new(
            "tracker.replace_login",
            "Replace credential",
            "set_tracker_credential",
            &[TRACKER_ID, ("username", Bind::Param("email")), ("secret", Bind::Param("token"))],
        )
        .params(&[
            param("email", "Atlassian account email", text(320, "you@example.com"), true),
            param("token", "API token", ParamKind::Secret, true),
        ])
        .variants(&["jira"]),
        ActionSpec::new("tracker.replace_token", "Replace credential", "set_tracker_credential", &[TRACKER_ID, ("secret", Bind::Param("token"))])
            .params(&[param("token", "Token", ParamKind::Secret, true)])
            .variants(&["jira_dc", "asana", "linear"]),
    ],
    create_flow: Some("tracker.connect"),
    variant_by: Some("provider"),
};

/// A catalog's load state as `CatalogStatus.state` says it.
const CATALOG_STATES: &[(&str, &str)] = &[
    ("loaded", "Loaded"),
    ("problem", "Could not load"),
    ("not_loaded", "Not loaded"),
];

const CATALOG: ResourceType = ResourceType {
    id: "catalog",
    label: "Catalog",
    plural: "Catalogs",
    help: "Git repos of assets fleet syncs to hosts. `personal` is yours; an org's catalog reaches that org's hosts and the org-less hosts that admit it. A GitHub org with SSO must allow the catalog's deploy key — an org admin does that once in the org's settings. Grants are given on the hub: `fleet-hub client grant <client> assets --catalog <name>`.",
    list: "catalog_list_catalogs",
    id_field: "name",
    title_field: "name",
    color_field: None,
    empty: "No catalogs yet. Add an org's catalog by its checkout path.",
    fields: &[
        FieldSpec::new("name", "Name", "What hosts, grants and cards call it; fixed once added.", FieldKind::Text { max: 64 }),
        FieldSpec::new("state", "State", "Whether fleet could read the catalog's repo the last time it looked.", FieldKind::Choice { options: CATALOG_STATES }).badge(Badge::Label),
        FieldSpec::new("repo_path", "Checkout", "Where the catalog's git repo is on this machine.", FieldKind::Text { max: 512 }),
        FieldSpec::new("remote_url", "Remote", "Its git remote, if any.", FieldKind::Text { max: 512 }),
        FieldSpec::new("org", "Org", "The org whose hosts receive it; none for personal.", FieldKind::Text { max: 128 }),
        FieldSpec::new(
            "admitted",
            "Admitted by",
            "Hosts with no org that receive this catalog. Hosts of its org always do.",
            FieldKind::Items {
                item_label: ItemLabel::Plain,
                remove: Some(ActionSpec::new(
                    "catalog.unadmit",
                    "Remove",
                    "catalog_unadmit_catalog",
                    &[("host_alias", Bind::Item), ("catalog", Bind::Record("name"))],
                )),
                add: &[ActionSpec::new(
                    "catalog.admit",
                    "Admit a host",
                    "catalog_admit_catalog",
                    &[("host_alias", Bind::Param("host")), ("catalog", Bind::Record("name"))],
                )
                .params(&[param("host", "Host", ParamKind::Options { source: OptionSource::Hosts }, true)])],
            },
        ),
        FieldSpec::new(
            "granted",
            "Granted to",
            "Paired desktops that may change this catalog. Granted on the hub (see above).",
            FieldKind::Items { item_label: ItemLabel::Plain, remove: None, add: &[] },
        ),
    ],
    create: Some(
        ActionSpec::new(
            "catalog.add",
            "Add catalog",
            "catalog_add_catalog",
            &[
                ("name", Bind::Param("name")),
                ("repo_path", Bind::Param("repo_path")),
                ("remote_url", Bind::Param("remote_url")),
                ("org", Bind::Param("org")),
            ],
        )
        .params(&[
            param("name", "Name", text(64, "papayapos"), true),
            param("repo_path", "Checkout path", text(512, "~/catalogs/papayapos"), true),
            param("remote_url", "Remote URL (optional)", text(512, "git@github.com:org/catalog.git"), false),
            param("org", "Org", ParamKind::Options { source: OptionSource::Orgs }, true),
        ]),
    ),
    update: None,
    delete: Some(
        ActionSpec::new("catalog.remove", "Remove", "catalog_remove_catalog", &[("name", Bind::Record("name"))])
            .confirm("Removes the catalog from fleet's config. Its checkout stays on disk; open cards on it are withdrawn."),
    ),
    actions: &[],
    create_flow: None,
    variant_by: None,
};

/// Every resource type.
pub const RESOURCES: &[ResourceType] = &[ORG, TRACKER, CATALOG];

pub fn resource(id: &str) -> Option<&'static ResourceType> {
    RESOURCES.iter().find(|r| r.id == id)
}

/// Every desktop command a resource or a page action names, for the
/// handler-list check.
pub fn commands() -> Vec<&'static str> {
    let mut out: Vec<&'static str> = Vec::new();
    for r in RESOURCES {
        out.push(r.list);
        for a in r.actions() {
            out.push(a.command);
        }
        if r.create_flow.is_some() {
            out.extend(["flow_start", "flow_submit", "flow_back", "flow_cancel"]);
        }
    }
    out.extend(super::actions::PAGE_ACTIONS.iter().map(|a| a.command));
    out.sort_unstable();
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resources_are_well_formed() {
        let mut ids = std::collections::BTreeSet::new();
        for r in RESOURCES {
            assert!(ids.insert(r.id), "duplicate resource {}", r.id);
            assert!(r.field(r.title_field).is_some(), "{}: title field", r.id);
            if let Some(c) = r.color_field {
                assert!(r.field(c).is_some(), "{}: colour field", r.id);
            }
            let mut fields = std::collections::BTreeSet::new();
            for f in r.fields {
                assert!(fields.insert(f.id), "{}: duplicate field {}", r.id, f.id);
                assert!(
                    f.help.ends_with('.'),
                    "{}.{}: help is a sentence",
                    r.id,
                    f.id
                );
                if f.edit.is_some() {
                    assert!(
                        r.update.is_some(),
                        "{}.{}: editable with no update action",
                        r.id,
                        f.id
                    );
                    assert!(
                        !matches!(f.kind, FieldKind::Items { .. }),
                        "{}.{}: a list is changed by its own actions",
                        r.id,
                        f.id
                    );
                }
                if let Some(Badge::Set { .. }) = f.badge {
                    assert_eq!(f.kind, FieldKind::Inherit, "{}.{}", r.id, f.id);
                }
                if let Some(Badge::Label) = f.badge {
                    assert!(
                        matches!(f.kind, FieldKind::Choice { .. }),
                        "{}.{}: a label badge is a choice's",
                        r.id,
                        f.id
                    );
                }
                if !f.merge_path.is_empty() {
                    assert!(f.edit.is_some(), "{}.{}: merged into what?", r.id, f.id);
                }
            }
            if let Some(flow) = r.create_flow {
                assert!(
                    super::super::flows::FLOWS.iter().any(|(id, _)| *id == flow),
                    "{}: create_flow {flow} is not a flow",
                    r.id
                );
            }
            let mut actions = std::collections::BTreeSet::new();
            for a in r.actions() {
                assert!(actions.insert(a.id), "{}: duplicate action {}", r.id, a.id);
                assert!(
                    a.id.starts_with(&format!("{}.", r.id)),
                    "{}: action {} is namespaced",
                    r.id,
                    a.id
                );
                for (arg, bind) in a.bind {
                    if let Bind::Param(p) = bind {
                        assert!(
                            a.params.iter().any(|x| x.name == *p),
                            "{}: {arg} binds an undeclared param {p}",
                            a.id
                        );
                    }
                    if let Bind::Record(f) = bind {
                        assert!(
                            *f == r.id_field || r.field(f).is_some(),
                            "{}: {arg} binds an unknown field {f}",
                            a.id
                        );
                    }
                }
                for p in a.params {
                    assert!(
                        a.bind.iter().any(|(_, b)| *b == Bind::Param(p.name)),
                        "{}: param {} is asked for and never sent",
                        a.id,
                        p.name
                    );
                }
                if let Some(c) = a.confirm {
                    assert!(c.ends_with('.'), "{}: confirm is a sentence", a.id);
                }
                if !a.variants.is_empty() {
                    let by = r.variant_by.expect("variants need a variant_by field");
                    let Some(FieldSpec {
                        kind: FieldKind::Choice { options },
                        ..
                    }) = r.field(by)
                    else {
                        panic!("{}: variant_by {by} is a choice field", r.id);
                    };
                    for v in a.variants {
                        assert!(
                            options.iter().any(|(o, _)| o == v),
                            "{}: {v} is not a {by}",
                            a.id
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn sub_item_actions_bind_the_item_and_record_actions_the_record() {
        let org = resource("org").unwrap();
        assert_eq!(
            super::super::flows::FLOWS.len(),
            1,
            "a new flow: name the commands of its resource here"
        );
        for f in org.fields {
            if let FieldKind::Items { remove, .. } = &f.kind {
                let rm = remove.expect("every org list can be shortened");
                assert!(
                    rm.bind
                        .iter()
                        .any(|(_, b)| matches!(b, Bind::Item | Bind::ItemField(_))),
                    "{}: a remove names the item",
                    rm.id
                );
            }
        }
        assert_eq!(
            commands(),
            [
                "add_org",
                "add_org_rule",
                "assign_host_org",
                "assign_tracker_org",
                "catalog_add_catalog",
                "catalog_admit_catalog",
                "catalog_list_catalogs",
                "catalog_remove_catalog",
                "catalog_unadmit_catalog",
                "flow_back",
                "flow_cancel",
                "flow_start",
                "flow_submit",
                "list_orgs",
                "list_trackers",
                "remove_org",
                "remove_org_rule",
                "remove_tracker",
                "set_tracker_credential",
                "test_tracker",
                "update_org",
                "update_tracker",
                "work_retention_sweep",
            ]
        );
    }
}
