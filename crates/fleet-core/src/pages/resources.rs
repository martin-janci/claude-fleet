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
    /// JSON `true` (a grant).
    True,
    /// JSON `false` (taking a grant back).
    False,
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
    /// Paired devices, by name (what `list_devices` answers).
    Devices,
    /// Asset catalogs, by name.
    Catalogs,
    /// The people this hub knows, by name (what `list_people` answers;
    /// M15 step G2.10).
    People,
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
    /// One of a fixed set (`(value, label)`); the first is preselected.
    Choice {
        options: &'static [(&'static str, &'static str)],
    },
    /// Free text with the source's values offered as you type: pick one
    /// that exists or type a new one (a person, M15 step G2.10).
    Suggest {
        max: usize,
        placeholder: &'static str,
        source: OptionSource,
    },
    /// A switch, sent as a boolean; `default` is where it starts.
    Toggle {
        default: bool,
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
    /// The command's answer is shown to the person, by this closed
    /// formatter, instead of only re-reading the list.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<ResultView>,
    /// The loader shown beside the form while the command runs, for a
    /// command that takes a while (linking two hubs exchanges keys).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub busy: Option<BusyLoader>,
    /// A read-only command run with the form's arguments while the person
    /// fills it in; its answer's `sentence` is shown under the form (an org
    /// rule's live impact, M15 step G2.10). It writes nothing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preview: Option<&'static str>,
}

/// A loader an action shows while it runs: a closed set from the loader kit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum BusyLoader {
    /// Two hubs talking to each other.
    CounterOrbit,
}

/// How an action's answer is shown: a closed set the renderer implements.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResultView {
    /// `pair_device`'s `{ url, code, expires_in_s, qr }`: the one-time
    /// code, its URL to open and the QR to scan, until it expires.
    Pairing,
    /// A command's `{ exit_code, output, truncated }` (a debug device's
    /// logs, an install): its text in a scrolling block.
    Output,
    /// `{ caption, mime, data }`: one image, base64 (a debug device's
    /// screenshot).
    Image,
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
            result: None,
            busy: None,
            preview: None,
        }
    }
    pub const fn preview(self, command: &'static str) -> Self {
        ActionSpec {
            preview: Some(command),
            ..self
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
    pub const fn result(self, view: ResultView) -> Self {
        ActionSpec {
            result: Some(view),
            ..self
        }
    }
    pub const fn busy(self, loader: BusyLoader) -> Self {
        ActionSpec {
            busy: Some(loader),
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
    /// One of a source's values, picked from a select inside the edit form
    /// (M15 step G7.14: a device's org and person). `none` labels the empty
    /// choice, which clears the value; without it a value must be picked.
    Pick {
        source: OptionSource,
        #[serde(skip_serializing_if = "Option::is_none")]
        none: Option<&'static str>,
    },
    /// Unix seconds, shown as how long ago ("5 min ago", "never").
    Time,
    /// A whole number the backend counts (an org's live sessions); shown,
    /// never edited. An absent value reads `0`.
    Count,
    /// An estimated cost in micro-USD, shown as dollars ("$12.34"); never
    /// edited. An absent value leaves the field out (the caller may not see
    /// spend).
    Money,
    /// An estimated cost per day, `[{ day, cost_micros }]` oldest first,
    /// shown as bars with each day's amount; never edited. An absent value
    /// leaves the field out, as for `money`.
    MoneySeries,
    /// A transfer in progress, `{ done, total, both_ways, since? }` in the
    /// record (Orbit Fleet 11.12, a hub link's sync): while `total` is above
    /// `done`, a Constellation with the real count ("412 of 1 280 messages ·
    /// 18 s", `since` being when the transfer last moved); while the two
    /// ends trade both ways with nothing queued, a Counter-orbit. Never
    /// edited; an absent value (idle) leaves the field out.
    Sync {
        unit: &'static str,
    },
    /// The record's per-org settings (`OrgSetting` rows: the setting
    /// described with the fleet's value, and the record's own): each one
    /// shown as a settings row that inherits the fleet's value or takes its
    /// own, written through `set` with `key` and `value` (absent: inherit).
    /// An absent list leaves the field out.
    Settings {
        set: ActionSpec,
    },
    /// A list of sub-items, each shown with `label` and changed through
    /// `remove` / `add` actions rather than the record's Apply.
    Items {
        item_label: ItemLabel,
        #[serde(skip_serializing_if = "Option::is_none")]
        remove: Option<ActionSpec>,
        add: &'static [ActionSpec],
        /// Actions on one item besides `remove`, each a button on the item
        /// (M15 step G4.7: a share's "Narrow to watch").
        #[serde(skip_serializing_if = "<[ActionSpec]>::is_empty")]
        each: &'static [ActionSpec],
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
    /// A paired device: its name, then `read-only` / `trusted` when so.
    Device,
    /// An org's member (phase D): their name, then their role.
    Member,
    /// One of an org's "Needs an admin" (`service::org_needs::AdminNeed`):
    /// what it is in a line, then why or when, listed rather than chipped.
    AdminNeed,
    /// One person's share of an org's spend (`service::org_spend::
    /// PersonSpend`): a table row of who, then today, 7 days and the month
    /// in dollars; nobody's reads "Routines, missions and unclaimed".
    PersonSpend,
    /// An org's catalog project (M15 step G2.10): its name, then its remote,
    /// path and hosts when set.
    OrgProject,
    /// M15 step G4.7: a share on an org's session (`service::orgs::OrgShare`),
    /// a table row of session · owner, shared with, level and since; a
    /// session the caller may not see reads "a private session".
    OrgShare,
    /// G4.7: a member and their live sessions with states, then how many
    /// more are private (`service::orgs::TeamMember`).
    TeamMember,
    /// G4.7: a former member, "removed 3 d ago", and the shares they still
    /// hold (`service::orgs::RemovedMember`).
    RemovedMember,
    /// G4.7: a Claude account the org's hosts use, and on which hosts.
    OrgAccount,
}

/// A line under a field's value where its section shows tiles: a closed
/// set of formatters over other keys of the record, never a template.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Sub {
    /// The record's budget for this amount, in whole USD at `field`:
    /// "of $60 budget", or "82% of $750" once something is spent. Nothing
    /// when there is no budget (`0` or absent).
    Budget { field: &'static str },
    /// The count at `field`, then `text` ("2 need you"); nothing at `0`.
    Count {
        field: &'static str,
        text: &'static str,
    },
    /// The roles of the members listed at `field` (M15 step G7.14): "2
    /// admins · 2 members · 1 viewer"; nothing when the list is absent.
    Roles { field: &'static str },
}

/// A badge in the list and the detail header while a field has a value.
/// `{hub}` in a badge's text is the hub's address when the desktop is paired
/// to one, and nothing otherwise ("owns the hub fleet.example", M15 step
/// G7.14).
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
    /// Its label. `{title}` is the one placeholder: the renderer puts the
    /// record's title there ("Allow Jev (decision model) for Acme's work").
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
    /// Its line under the value in a `tiles` section.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sub: Option<Sub>,
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
            sub: None,
        }
    }
    pub const fn sub(self, sub: Sub) -> Self {
        FieldSpec {
            sub: Some(sub),
            ..self
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
            if let FieldKind::Items {
                remove, add, each, ..
            } = &f.kind
            {
                out.extend(remove.iter());
                out.extend(add.iter());
                out.extend(each.iter());
            }
            if let FieldKind::Settings { set } = &f.kind {
                out.push(set);
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

/// A person: one this hub knows, picked as you type, or a new name.
const fn person() -> ParamKind {
    ParamKind::Suggest {
        max: 64,
        placeholder: "jane",
        source: OptionSource::People,
    }
}

/// What an org rule matches by (M15 step G2.10): `add_org_rule` turns the
/// one value into the rule's owner / repo, path prefix or host
/// (`service::orgs::rule_from_match`).
pub const ORG_RULE_MATCH: &[(&str, &str)] = &[
    ("repository", "Repository"),
    ("path", "Path"),
    ("host", "Host"),
    ("owner", "Owner"),
];

/// An org's rule: one form, a "Match by" choice and its value, with the live
/// impact under it while it is typed.
const ORG_RULE_ADDS: &[ActionSpec] = &[ActionSpec::new(
    "org.add_rule",
    "Add rule",
    "add_org_rule",
    &[
        ORG_ID,
        ("match_by", Bind::Param("match_by")),
        ("value", Bind::Param("value")),
    ],
)
.params(&[
    param(
        "match_by",
        "Match by",
        ParamKind::Choice {
            options: ORG_RULE_MATCH,
        },
        true,
    ),
    param(
        "value",
        "Matches",
        text(1024, "acme/api · /home/me/work/acme · hetzner-a · acme"),
        true,
    ),
])
.preview("org_rule_preview")];

/// The roles of an org member (`store::ORG_ROLES`), as a form offers them.
pub const ORG_ROLE_CHOICES: &[(&str, &str)] = &[
    (
        "member",
        "Member — sees its work and what is shared with it",
    ),
    ("admin", "Admin — administers the org"),
    ("viewer", "Viewer — reads only"),
];

/// The caller's own role in an org (`OrgDetail::my_role`), as its header
/// says it (M15 step G7.14).
const MY_ROLES: &[(&str, &str)] = &[
    ("admin", "you are an admin"),
    ("member", "you are a member"),
    ("viewer", "you are a viewer"),
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
        FieldSpec::new("session_count", "Live sessions", "Its live sessions that you can see.", FieldKind::Count)
            .sub(Sub::Count { field: "needs_you", text: "need you" }),
        FieldSpec::new("needs_you", "Need you", "Of those, the ones waiting on a person: a question, a permission, a stop.", FieldKind::Count),
        FieldSpec::new("member_count", "Members", "Who is in it, by role. Shown to its own people and the fleet's administrator.", FieldKind::Count)
            .sub(Sub::Roles { field: "members" }),
        FieldSpec::new("spent_today_micros", "Spent today", "Estimated cost of its sessions today (UTC). Shown when you can see every session.", FieldKind::Money)
            .sub(Sub::Budget { field: "budget_daily_usd" }),
        FieldSpec::new("spent_week_micros", "Last 7 days", "Estimated cost of its sessions over the last 7 days.", FieldKind::Money),
        FieldSpec::new("spent_month_micros", "This month", "Estimated cost of its sessions this calendar month (UTC).", FieldKind::Money)
            .sub(Sub::Budget { field: "budget_monthly_usd" }),
        FieldSpec::new(
            "spend_series",
            "Last 14 days",
            "Estimated cost of its sessions on each of the last 14 days (UTC), today last.",
            FieldKind::MoneySeries,
        ),
        FieldSpec::new(
            "spend_by_person",
            "By person",
            "Whose sessions spent it: today, the last 7 days and this month (UTC). Shown only to an admin who sees every session; otherwise it is hidden whole, never in part.",
            FieldKind::Items { item_label: ItemLabel::PersonSpend, remove: None, add: &[], each: &[] },
        ),
        FieldSpec::new(
            "needs_admin",
            "Needs an admin",
            "What its admins should look at: a budget at 80% or past it (Fleet only warns; it never stops a session), a device that may prompt but is not trusted yet (trust it in Settings → Devices), sessions on its hosts nobody has claimed.",
            FieldKind::Items { item_label: ItemLabel::AdminNeed, remove: None, add: &[], each: &[] },
        ),
        FieldSpec::new(
            "settings",
            "Its own settings",
            "Settings this org may set for itself; the rest of the fleet keeps its value. Inherit takes the fleet's again.",
            FieldKind::Settings {
                set: ActionSpec::new(
                    "org.set_setting",
                    "Set",
                    "set_org_setting",
                    &[ORG_ID, ("key", Bind::Param("key")), ("value", Bind::Param("value"))],
                )
                // Filled by the settings row, never shown as a form: the key
                // is the row's, the value what the person chose (empty:
                // inherit).
                .params(&[param("key", "Setting", text(128, ""), true), param("value", "Value", text(4096, ""), false)]),
            },
        ),
        FieldSpec::new(
            "rules",
            "Rules",
            "Which sessions belong to it: a GitHub owner (or one repository), a path prefix, or a host.",
            FieldKind::Items {
                item_label: ItemLabel::OrgRule,
                remove: Some(ActionSpec::new("org.remove_rule", "Remove rule", "remove_org_rule", &[("rule_id", Bind::ItemField("id"))])),
                add: ORG_RULE_ADDS,
                each: &[],
            },
        ),
        FieldSpec::new(
            "projects",
            "Projects",
            "The org's project catalog: a project its people work on, with its remote, where it is checked out and the hosts it may run on (none: every host of the org).",
            FieldKind::Items {
                item_label: ItemLabel::OrgProject,
                remove: Some(ActionSpec::new(
                    "org.remove_project",
                    "Remove the project",
                    "remove_org_project",
                    &[("project_id", Bind::ItemField("id"))],
                )),
                add: &[ActionSpec::new(
                    "org.add_project",
                    "Add project",
                    "add_org_project",
                    &[
                        ORG_ID,
                        ("name", Bind::Param("name")),
                        ("remote", Bind::Param("remote")),
                        ("path", Bind::Param("path")),
                        ("hosts", Bind::Param("hosts")),
                    ],
                )
                .params(&[
                    param("name", "Name", text(80, "api"), true),
                    param("remote", "Remote (optional)", text(1024, "git@github.com:acme/api.git"), false),
                    param("path", "Path on hosts (optional)", text(1024, "~/src/api"), false),
                    param("hosts", "Hosts allowed (optional; every host of the org when empty)", text(1024, "hetzner-a, hetzner-b"), false),
                ])],
                each: &[],
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
                each: &[],
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
                each: &[],
            },
        ),
        FieldSpec::new(
            "accounts",
            "Accounts",
            "The Claude accounts its hosts are signed in to. Add or switch one on Settings → Hosts.",
            FieldKind::Items { item_label: ItemLabel::OrgAccount, remove: None, add: &[], each: &[] },
        ),
        FieldSpec::new(
            "catalogs",
            "Catalogs",
            "Asset catalogs this org owns; its hosts receive them. Add or remove one in Settings → Catalogs.",
            FieldKind::Items { item_label: ItemLabel::Plain, remove: None, add: &[], each: &[] },
        ),
        FieldSpec::new(
            "devices",
            "Devices",
            "Phones, browsers and desktops bound to this org: they see only its work (and unassigned work, if allowed below). Pair a new one in Settings → Devices. Shown to the hub's operator only.",
            FieldKind::Items {
                item_label: ItemLabel::Device,
                remove: Some(ActionSpec::new("org.unbind_device", "Unbind the device", "bind_device_org", &[("device", Bind::ItemField("name"))])),
                add: &[ActionSpec::new("org.bind_device", "Bind a device", "bind_device_org", &[("device", Bind::Param("device")), ORG_ID])
                    .params(&[param("device", "Device", ParamKind::Options { source: OptionSource::Devices }, true)])],
                each: &[],
            },
        ),
        FieldSpec::new(
            "members",
            "Members",
            "Who is in the company. An admin administers it (its settings, members and their devices); a member sees its work and what is shared with it; a viewer only reads. Nobody sees another person's private sessions.",
            FieldKind::Items {
                item_label: ItemLabel::Member,
                remove: Some(
                    ActionSpec::new(
                        "org.remove_member",
                        "Remove from the org",
                        "remove_org_member",
                        &[ORG_ID, ("person_id", Bind::ItemField("person_id"))],
                    )
                    .confirm("What was shared with them on this org's sessions is taken back too. Their own sessions stay theirs; if this was their last org, their devices see nothing of any org."),
                ),
                add: &[ActionSpec::new(
                    "org.set_member",
                    "Add or change a member",
                    "set_org_member",
                    &[ORG_ID, ("person", Bind::Param("person")), ("role", Bind::Param("role"))],
                )
                .params(&[
                    param("person", "Person", person(), true),
                    param("role", "Role", ParamKind::Choice { options: ORG_ROLE_CHOICES }, true),
                ])],
                each: &[],
            },
        ),
        FieldSpec::new(
            "removed_members",
            "Removed members",
            "Who left the company, and the shares on its sessions they still hold. Take those back here. Shown to its admins.",
            FieldKind::Items {
                item_label: ItemLabel::RemovedMember,
                remove: None,
                add: &[],
                each: &[ActionSpec::new(
                    "org.revoke_former_grants",
                    "Take back their shares",
                    "revoke_org_member_grants",
                    &[ORG_ID, ("person_id", Bind::ItemField("person_id"))],
                )
                .confirm("Every share they still hold on this org's sessions is taken back. The sessions' owners can share again.")],
            },
        ),
        FieldSpec::new(
            "team",
            "Team",
            "Who is working on what: each member's live sessions in this org that you can see, with their state, and how many more are private.",
            FieldKind::Items { item_label: ItemLabel::TeamMember, remove: None, add: &[], each: &[] },
        ),
        FieldSpec::new(
            "shares",
            "Sharing",
            "Every share on this org's sessions: whose session, with whom, at what level, since when. An admin can take one back or narrow it to watch, never widen it. A session you cannot see reads \"a private session\".",
            FieldKind::Items {
                item_label: ItemLabel::OrgShare,
                remove: Some(
                    ActionSpec::new("org.revoke_share", "Revoke", "revoke_org_share", &[ORG_ID, ("grant_id", Bind::ItemField("id"))])
                        .confirm("They lose this session at once. Its owner can share it again."),
                ),
                add: &[],
                each: &[ActionSpec::new("org.narrow_share", "Narrow to watch", "narrow_org_share", &[ORG_ID, ("grant_id", Bind::ItemField("id"))])
                    .confirm("They keep reading the session and can no longer answer or drive it.")],
            },
        ),
        FieldSpec::new(
            "owns_hub",
            "Owns this hub",
            "This company owns the hub: its admins administer hosts — route them into orgs and see how many sessions on them nobody has claimed. One org at most. Only the hub's owner changes it.",
            FieldKind::Bool { on_off: false, default: false },
        )
        .edit("owns_hub")
        .badge(Badge::True { text: "owns the hub {hub}" })
        .confirm("This org's admins will administer every host on the hub."),
        FieldSpec::new("my_role", "Your role", "What you may do in it: an admin administers it, a member works in it, a viewer reads.", FieldKind::Choice { options: MY_ROLES })
            .badge(Badge::Label),
        FieldSpec::new(
            "admins_see_unclaimed",
            "Admins see unclaimed sessions",
            "Its admins see how many sessions on its hosts nobody has claimed — a count, never the sessions. Off by default; only the hub's owner changes it.",
            FieldKind::Bool { on_off: false, default: false },
        )
        .edit("admins_see_unclaimed"),
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
            "members_own_sessions_only",
            "Members see only their own sessions",
            "On (the default): a member sees their own sessions and what is shared with them. Off: the org's members also watch each other's sessions in it — read only; answering or driving one still needs a share. Only the hub's owner turns it off; an org admin can turn it back on.",
            FieldKind::Bool { on_off: false, default: true },
        )
        .edit("members_own_sessions_only")
        .badge(Badge::False { text: "members see the team's sessions" })
        .confirm("Its members will watch each other's sessions in this org: what each one's Claude is doing and has said. They still cannot answer or drive them without a share."),
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
            "Allow Jev (decision model) for {title}'s work",
            "Let this org's redacted prompts and ticket titles go to TypeSafe's decision model when Decisions (Jev) is on. Only ids and numbers are recorded; an answer is at most a suggestion.",
            FieldKind::Bool { on_off: true, default: false },
        )
        .edit("jev")
        .badge(Badge::True { text: "sends to Jev" })
        .confirm("This org's redacted prompts and ticket titles will be sent to TypeSafe when Decisions (Jev) is on."),
        FieldSpec::new(
            "jev_reply_allowed",
            "Also allow Claude's reply text (turn outcome)",
            "On top of Jev above: let the end of this org's sessions' screens — Claude's last reply, redacted, code replaced by placeholders, a few lines — go to TypeSafe's decision model, so Jev can tell a finished turn from a question when hooks say nothing. Off by default (D48).",
            FieldKind::Bool { on_off: true, default: false },
        )
        .edit("jev_reply")
        .badge(Badge::True { text: "sends replies to Jev" })
        .confirm("The end of this org's sessions' screens (Claude's reply text, redacted) will be sent to TypeSafe when Decisions (Jev) and its turn outcome use case are on."),
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
        ActionSpec::new(
            "org.add",
            "Add organisation",
            "add_org",
            &[
                ("name", Bind::Param("name")),
                ("color", Bind::Param("color")),
                ("isolate_sessions", Bind::Param("isolate_sessions")),
            ],
        )
        .params(&[
            param("name", "Name", text(80, "Company A"), true),
            param("color", "Colour", ParamKind::Color, false),
            param(
                "isolate_sessions",
                "Its sessions are visible only to its members",
                ParamKind::Toggle { default: false },
                false,
            ),
        ]),
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
    help: "Git repos of assets fleet syncs to hosts. `personal` is yours; an org's catalog reaches that org's hosts and the org-less hosts that admit it. A GitHub org with SSO must allow the catalog's deploy key — an org admin does that once in the org's settings. Which device may change a catalog is set in Settings → Devices.",
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
                each: &[],
            },
        ),
        FieldSpec::new(
            "granted",
            "Granted to",
            "Paired desktops that may change this catalog. Granted in Settings → Devices.",
            FieldKind::Items { item_label: ItemLabel::Plain, remove: None, add: &[], each: &[] },
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

const DEVICE: (&str, Bind) = ("device", Bind::Record("name"));

/// A device's modes as `DeviceSummary.mode` says them.
const DEVICE_MODES: &[(&str, &str)] = &[
    ("full", "Full"),
    ("answer", "Answer only"),
    ("readonly", "Watch only"),
];

/// `DeviceSummary.kind`.
const DEVICE_KINDS: &[(&str, &str)] = &[("desktop", "Desktop"), ("phone", "Phone")];

const DEVICE_RESOURCE: ResourceType = ResourceType {
    id: "device",
    label: "Device",
    plural: "Devices",
    help: "The phones, browsers and desktops paired to the hub. A device bound to an org sees only that org's work; a trusted one's prompts reach agents unmarked, and it may change these settings. Links to other fleets' hubs are under Federation; updater tokens are managed on the hub.",
    list: "list_devices",
    id_field: "name",
    title_field: "name",
    color_field: None,
    empty: "No paired devices. Pair a phone or a browser with Pair a device.",
    fields: &[
        FieldSpec::new("name", "Name", "How the apps and the audit name it. Its grants, catalogs and person follow a new name.", FieldKind::Text { max: 64 }).edit("name"),
        FieldSpec::new("mode", "Mode", "Full drives sessions; answer only answers their questions and permission prompts and never types a prompt; watch only reads. A viewer's device watches only whatever it says here.", FieldKind::Choice { options: DEVICE_MODES })
            .edit("mode")
            .badge(Badge::Label),
        FieldSpec::new("this_device", "This device", "The device you are using now. It cannot revoke, untrust, bind, hand over or make itself read-only.", FieldKind::Bool { on_off: false, default: false })
            .badge(Badge::True { text: "this device" }),
        FieldSpec::new(
            "trusted",
            "Trusted",
            "Its prompts and messages reach agents unmarked, and it may change the company's orgs, devices and settings. Trust a device you type on, never an agent's.",
            FieldKind::Bool { on_off: false, default: false },
        )
        .edit("trusted")
        .badge(Badge::True { text: "trusted" })
        .confirm("Its prompts will reach agents unmarked, and it will be able to change the company's orgs, devices and settings."),
        FieldSpec::new(
            "org",
            "Org",
            "The org it is bound to; none sees every org.",
            FieldKind::Pick { source: OptionSource::Orgs, none: Some("No org — every org") },
        )
        .edit("org")
        .confirm("From its next request it sees only the org picked here (every org, with none)."),
        FieldSpec::new(
            "person",
            "Belongs to",
            "Whose device it is: it sees their sessions and the ones shared with them.",
            FieldKind::Pick { source: OptionSource::People, none: None },
        )
        .edit("person")
        .confirm("From its next request it sees that person's sessions and the ones shared with them, and no others."),
        FieldSpec::new("kind", "Kind", "A desktop or a phone, as its app last said.", FieldKind::Choice { options: DEVICE_KINDS }),
        FieldSpec::new("app", "App", "What it runs, as its app last said; nothing until it has called the hub.", FieldKind::Text { max: 80 }),
        FieldSpec::new(
            "catalogs",
            "May change catalogs",
            "Asset catalogs it may edit and sync. Only a full device bound to no org can hold one.",
            FieldKind::Items {
                item_label: ItemLabel::Plain,
                remove: Some(ActionSpec::new(
                    "device.ungrant_catalog",
                    "Take the catalog back",
                    "grant_device_catalog",
                    &[DEVICE, ("catalog", Bind::Item), ("on", Bind::False)],
                )),
                add: &[ActionSpec::new("device.grant_catalog", "Grant a catalog", "grant_device_catalog", &[DEVICE, ("catalog", Bind::Param("catalog")), ("on", Bind::True)])
                    .params(&[param("catalog", "Catalog", ParamKind::Options { source: OptionSource::Catalogs }, true)])],
                each: &[],
            },
        ),
        FieldSpec::new("last_seen_at", "Last seen", "Its last request to the hub.", FieldKind::Time),
        FieldSpec::new("created_at", "Paired", "When it was paired.", FieldKind::Time),
    ],
    create: Some(
        ActionSpec::new(
            "device.pair",
            "Pair a device",
            "pair_device",
            &[
                ("device", Bind::Param("device")),
                ("mode", Bind::Param("mode")),
                ("org", Bind::Param("org")),
                ("person", Bind::Param("person")),
            ],
        )
        .params(&[
            param("device", "Name", text(64, "ada-phone"), true),
            param("mode", "Mode", ParamKind::Choice { options: DEVICE_MODES }, true),
            param("org", "Bind to org (optional)", ParamKind::Options { source: OptionSource::Orgs }, false),
            param("person", "Belongs to (optional; you when empty)", person(), false),
        ])
        .result(ResultView::Pairing),
    ),
    update: Some(ActionSpec::new("device.update", "Apply", "update_device", &[DEVICE])),
    delete: Some(
        ActionSpec::new("device.revoke", "Revoke", "revoke_device", &[DEVICE])
            .confirm("Its next request is refused and the name is free again. Pair it again to bring it back."),
    ),
    // The org and the person are picked in the edit form (M15 step G7.14);
    // Apply sends them through `update_device`.
    actions: &[],
    create_flow: None,
    variant_by: None,
};

const PERSON_RESOURCE: ResourceType = ResourceType {
    id: "person",
    label: "Person",
    plural: "People",
    help: "The people this hub knows. A session belongs to the person whose device started it and is private to them unless they share it. The hub's owner cannot be disabled.",
    list: "list_people",
    id_field: "id",
    title_field: "name",
    color_field: None,
    empty: "Nobody yet. Add a person, or pair a device to them.",
    fields: &[
        FieldSpec::new("name", "Name", "What grants and devices are addressed to.", FieldKind::Text { max: 64 }).edit("name"),
        FieldSpec::new("display_name", "Shown as", "How the apps show them; empty shows the name.", FieldKind::Text { max: 80 }).edit("display_name"),
        FieldSpec::new("owner", "Owner", "This hub's own owner.", FieldKind::Bool { on_off: false, default: false }).badge(Badge::True { text: "owner" }),
        FieldSpec::new("devices", "Devices", "Their live paired devices. Manage them in Settings → Devices.", FieldKind::Items { item_label: ItemLabel::Plain, remove: None, add: &[], each: &[] }),
        FieldSpec::new("created_at", "Added", "When the hub first heard of them.", FieldKind::Time),
        FieldSpec::new("disabled_at", "Disabled", "When their devices and the shares made to them were revoked.", FieldKind::Time),
    ],
    create: Some(
        ActionSpec::new(
            "person.add",
            "+ Person",
            "add_person",
            &[("name", Bind::Param("name")), ("display_name", Bind::Param("display_name"))],
        )
        .params(&[
            param("name", "Name", text(64, "jana"), true),
            param("display_name", "Shown as (optional)", text(80, "Jana Nováková"), false),
        ]),
    ),
    update: Some(ActionSpec::new("person.update", "Apply", "rename_person", &[("person_id", Bind::Record("id"))])),
    delete: Some(
        ActionSpec::new("person.disable", "Disable", "disable_person", &[("person_id", Bind::Record("id"))])
            .confirm("Revokes every device of theirs and every share made to them, at once. Their own sessions stay private and theirs. There is no re-enable."),
    ),
    actions: &[],
    create_flow: None,
    variant_by: None,
};

/// Every resource type.
const DEBUG_DEVICE_ID: (&str, Bind) = ("id", Bind::Record("id"));

/// `DebugDevice.platform`, `.kind` and `.state` as `service::debug_devices`
/// says them.
const DEBUG_PLATFORMS: &[(&str, &str)] = &[("android", "Android"), ("ios", "iOS")];
const DEBUG_KINDS: &[(&str, &str)] = &[
    ("physical", "Device"),
    ("emulator", "Emulator"),
    ("simulator", "Simulator"),
];
const DEBUG_STATES: &[(&str, &str)] = &[
    ("online", "Online"),
    ("booted", "Booted"),
    ("booting", "Starting"),
    ("offline", "Offline"),
    ("unauthorized", "Not authorized"),
    ("shutdown", "Stopped"),
    ("missing", "Missing"),
];

const DEBUG_DEVICE: ResourceType = ResourceType {
    id: "debug_device",
    label: "Debug device",
    plural: "Debug devices",
    help: "Phones, emulators and simulators plugged into or running on your hosts. Each is used where it is attached; a shared one can be used by sessions on every host of its org through fleet's control API.",
    list: "list_debug_devices",
    id_field: "id",
    title_field: "title",
    color_field: None,
    empty: "No debug devices yet. Plug a phone into a host (USB debugging on), start an emulator or boot a simulator; hosts are scanned every few minutes, or rescan one now.",
    fields: &[
        FieldSpec::new("title", "Device", "Its label, or the name it reports.", FieldKind::Text { max: 128 }),
        FieldSpec::new("label", "Label", "Your name for it, used by sessions to pick it; empty uses its own name.", FieldKind::Text { max: 64 }).edit("label"),
        FieldSpec::new("host", "Host", "The host it is attached to; every command runs there.", FieldKind::Text { max: 128 }),
        FieldSpec::new("platform", "Platform", "Android or iOS.", FieldKind::Choice { options: DEBUG_PLATFORMS }).badge(Badge::Label),
        FieldSpec::new("kind", "Kind", "A physical device, an Android emulator or an iOS simulator.", FieldKind::Choice { options: DEBUG_KINDS }),
        FieldSpec::new("state", "State", "What the host's last scan saw.", FieldKind::Choice { options: DEBUG_STATES }).badge(Badge::Label),
        FieldSpec::new("os_version", "OS", "The version it runs.", FieldKind::Text { max: 64 }),
        FieldSpec::new("model", "Model", "The hardware model it reports.", FieldKind::Text { max: 128 }),
        FieldSpec::new("serial", "Serial", "What adb, simctl or devicectl address it by.", FieldKind::Text { max: 256 }),
        FieldSpec::new(
            "shared",
            "Shared",
            "Sessions on other hosts of this host's org may use it; off, only sessions on its own host do.",
            FieldKind::Bool { on_off: false, default: false },
        )
        .edit("shared")
        .badge(Badge::True { text: "shared" })
        .confirm("Sessions on every host of its org will be able to install and launch apps on it, and on a simulator that runs them on its Mac."),
        FieldSpec::new("claimed_by", "Claimed by", "Who is using it now; nobody else may until the claim ends.", FieldKind::Text { max: 256 }),
        FieldSpec::new("claim_note", "For", "What the claim says it is for.", FieldKind::Text { max: 200 }),
        FieldSpec::new("claimed_until", "Claim ends", "When the claim lapses unless its holder keeps using it.", FieldKind::Time),
        FieldSpec::new("last_seen_at", "Last seen", "When a scan last found it.", FieldKind::Time),
    ],
    create: None,
    update: Some(ActionSpec::new("debug_device.update", "Apply", "update_debug_device", &[DEBUG_DEVICE_ID])),
    delete: Some(
        ActionSpec::new("debug_device.forget", "Forget", "forget_debug_device", &[DEBUG_DEVICE_ID])
            .confirm("It leaves the list with its label and sharing. A device still attached comes back on the next scan."),
    ),
    actions: &[
        ActionSpec::new("debug_device.rescan", "Rescan host", "scan_debug_devices", &[("host", Bind::Record("host"))]),
        ActionSpec::new("debug_device.claim", "Claim", "claim_debug_device", &[DEBUG_DEVICE_ID, ("note", Bind::Param("note"))])
            .params(&[param("note", "For (optional)", text(200, "testing the login flow"), false)]),
        ActionSpec::new(
            "debug_device.install",
            "Install app…",
            "install_debug_device",
            &[
                DEBUG_DEVICE_ID,
                ("path", Bind::Param("path")),
                ("host", Bind::Param("host")),
                ("claim", Bind::Param("claim")),
                ("note", Bind::Param("note")),
            ],
        )
            .params(&[
                param("path", "App (.apk, .app or .ipa)", text(1024, "~/app/build/outputs/apk/debug/app-debug.apk"), true),
                param("host", "On host (optional; the device's own when empty)", ParamKind::Options { source: OptionSource::Hosts }, false),
                // M15 step G7.14: claimed first, so others see it in use.
                param("claim", "Claim it while installing (others see it in use)", ParamKind::Toggle { default: true }, false),
                param("note", "For (optional)", text(200, "testing the login flow"), false),
            ])
            .result(ResultView::Output),
        ActionSpec::new("debug_device.logs", "Logs", "debug_device_logs", &[DEBUG_DEVICE_ID]).result(ResultView::Output),
        ActionSpec::new("debug_device.screenshot", "Screenshot", "debug_device_screenshot", &[DEBUG_DEVICE_ID]).result(ResultView::Image),
        ActionSpec::new("debug_device.release", "Release claim", "release_debug_device", &[DEBUG_DEVICE_ID])
            .confirm("Whoever holds it loses the claim; another session may then use the device."),
        ActionSpec::new("debug_device.boot", "Start", "boot_debug_device", &[DEBUG_DEVICE_ID]).variants(&["emulator", "simulator"]),
        ActionSpec::new("debug_device.shutdown", "Stop", "shutdown_debug_device", &[DEBUG_DEVICE_ID])
            .variants(&["emulator", "simulator"])
            .confirm("Anything running on it stops."),
    ],
    create_flow: None,
    variant_by: Some("kind"),
};

/// `PeerLinkSummary.role` and `.state` as `store::peer_links` says them.
const PEER_ROLES: &[(&str, &str)] = &[("dialer", "We dial"), ("listener", "They dial")];
const PEER_STATES: &[(&str, &str)] = &[
    ("connected", "Connected"),
    ("retrying", "Retrying"),
    ("refused", "Refused"),
    ("incompatible", "Incompatible"),
];

/// Orbit Fleet 11.5: this fleet's links to other fleets' hubs. Federation is
/// hub to hub, so the desktop shows its hub's links (`list_peer_links`
/// routes) and links a new hub through it with the one-time code that hub's
/// owner minted (`fleet-hub pair --mode peer --name <label>` there).
const PEER_LINK: ResourceType = ResourceType {
    id: "peer_link",
    label: "Linked hub",
    plural: "Linked hubs",
    help: "Other fleets' hubs this hub exchanges messages with: sessions here can message sessions there, and back. Each link shows how it is doing and what it carried.",
    list: "list_peer_links",
    id_field: "id",
    title_field: "title",
    color_field: None,
    empty: "No linked hubs. Ask the other fleet's owner for a link code (fleet-hub pair --mode peer --name <label> on their hub), then Link a hub.",
    fields: &[
        FieldSpec::new("title", "Fleet", "The other fleet's id once it has answered, else its hub's address.", FieldKind::Text { max: 256 }),
        FieldSpec::new("state", "State", "How the last exchange went.", FieldKind::Choice { options: PEER_STATES }).badge(Badge::Label),
        FieldSpec::new("role", "Direction", "Which hub opens the connection: this one dials out, or the other dials in.", FieldKind::Choice { options: PEER_ROLES }),
        FieldSpec::new("url", "Address", "The hub this one dials; a link the other side dials has none.", FieldKind::Text { max: 512 }),
        FieldSpec::new("sync", "Syncing", "Messages carried today against those still queued, while the queue drains.", FieldKind::Sync { unit: "messages" }),
        FieldSpec::new("latency", "Latency", "The round trip of the last exchange this hub dialed.", FieldKind::Text { max: 32 }),
        FieldSpec::new("messages_today", "Messages today", "Carried either way since midnight (UTC).", FieldKind::Count),
        FieldSpec::new("messages_total", "Messages in all", "Carried either way since the link was made.", FieldKind::Count),
        FieldSpec::new("pending", "Waiting", "Messages queued for the other fleet.", FieldKind::Count),
        FieldSpec::new("last_exchange_at", "Last exchange", "When the hubs last traded messages.", FieldKind::Time),
        FieldSpec::new("last_error", "Last error", "Why the last exchange failed, while it is failing.", FieldKind::Text { max: 512 }),
        FieldSpec::new("retry", "Retrying", "How often this hub tries the link again while it is down (it backs off to once a minute).", FieldKind::Text { max: 32 }),
    ],
    create: Some(
        ActionSpec::new("peer_link.add", "Link a hub", "link_peer_hub", &[("url", Bind::Param("url")), ("code", Bind::Param("code"))]).params(&[
            param("url", "Hub address", text(512, "https://hub.example.com"), true),
            param("code", "Link code", ParamKind::Secret, true),
        ])
        .busy(BusyLoader::CounterOrbit),
    ),
    update: None,
    delete: Some(
        ActionSpec::new("peer_link.remove", "Unlink", "unlink_peer_hub", &[("id", Bind::Record("id"))])
            .confirm("Sessions here can no longer message that fleet, and messages waiting for it fail back to their senders. Linking again needs a new code."),
    ),
    actions: &[],
    create_flow: None,
    variant_by: None,
};

pub const RESOURCES: &[ResourceType] = &[
    ORG,
    TRACKER,
    CATALOG,
    DEVICE_RESOURCE,
    PERSON_RESOURCE,
    DEBUG_DEVICE,
    PEER_LINK,
];

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
            out.extend(a.preview);
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
                let bare = f.label.replace("{title}", "");
                assert!(
                    !bare.contains('{') && !bare.contains('}'),
                    "{}.{}: `{{title}}` is a label's one placeholder",
                    r.id,
                    f.id
                );
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
            if let FieldKind::Items {
                remove, add, each, ..
            } = &f.kind
            {
                // Every action on one item names it.
                for a in *each {
                    assert!(
                        a.bind
                            .iter()
                            .any(|(_, b)| matches!(b, Bind::Item | Bind::ItemField(_))),
                        "{}: an item's action names the item",
                        a.id
                    );
                }
                // The org's catalogs are shown, never changed here: they are
                // added and removed on Settings → Catalogs. Spend by person
                // and what needs an admin are read-only reports; so are the
                // team and the accounts its hosts use (G4.7). A former
                // member is never re-added from here, only their shares
                // taken back.
                if [
                    "catalogs",
                    "needs_admin",
                    "spend_by_person",
                    "team",
                    "accounts",
                    "removed_members",
                ]
                .contains(&f.id)
                {
                    assert!(remove.is_none() && add.is_empty(), "{}", f.id);
                    continue;
                }
                let rm = remove.expect("every other org list can be shortened");
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
                "add_org_project",
                "add_org_rule",
                "add_person",
                "assign_host_org",
                "assign_tracker_org",
                "bind_device_org",
                "boot_debug_device",
                "catalog_add_catalog",
                "catalog_admit_catalog",
                "catalog_list_catalogs",
                "catalog_remove_catalog",
                "catalog_unadmit_catalog",
                "claim_debug_device",
                "debug_device_logs",
                "debug_device_screenshot",
                "disable_person",
                "flow_back",
                "flow_cancel",
                "flow_start",
                "flow_submit",
                "forget_debug_device",
                "grant_device_catalog",
                "install_debug_device",
                "link_peer_hub",
                "list_debug_devices",
                "list_devices",
                "list_orgs",
                "list_peer_links",
                "list_people",
                "list_trackers",
                "narrow_org_share",
                "org_rule_preview",
                "pair_device",
                "release_debug_device",
                "remove_org",
                "remove_org_member",
                "remove_org_project",
                "remove_org_rule",
                "remove_tracker",
                "rename_person",
                "repair_workspaces_now",
                "restore_all_lost_sessions",
                "revoke_device",
                "revoke_org_member_grants",
                "revoke_org_share",
                "scan_debug_devices",
                "set_org_member",
                "set_org_setting",
                "set_tracker_credential",
                "shutdown_debug_device",
                "test_tracker",
                "unlink_peer_hub",
                "update_debug_device",
                "update_device",
                "update_org",
                "update_tracker",
                "work_retention_sweep",
            ]
        );
    }
}
