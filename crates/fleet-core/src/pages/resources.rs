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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ParamKind {
    Text {
        max: usize,
        placeholder: &'static str,
    },
    Color,
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

/// An org's rule, added one kind at a time: each form asks for one thing.
const ADD_OWNER_RULE: ActionSpec = ActionSpec {
    id: "org.add_owner_rule",
    label: "Add owner rule",
    command: "add_org_rule",
    envelope: Envelope::Args,
    bind: &[
        ORG_ID,
        ("owner", Bind::Param("owner")),
        ("repo", Bind::Param("repo")),
    ],
    params: &[
        ParamSpec {
            name: "owner",
            label: "GitHub owner",
            kind: ParamKind::Text {
                max: 100,
                placeholder: "acme",
            },
            required: true,
        },
        ParamSpec {
            name: "repo",
            label: "Repository (optional)",
            kind: ParamKind::Text {
                max: 100,
                placeholder: "api",
            },
            required: false,
        },
    ],
    confirm: None,
};

const ADD_PATH_RULE: ActionSpec = ActionSpec {
    id: "org.add_path_rule",
    label: "Add path rule",
    command: "add_org_rule",
    envelope: Envelope::Args,
    bind: &[ORG_ID, ("path_prefix", Bind::Param("path_prefix"))],
    params: &[ParamSpec {
        name: "path_prefix",
        label: "Path prefix",
        kind: ParamKind::Text {
            max: 1024,
            placeholder: "/home/me/work/acme",
        },
        required: true,
    }],
    confirm: None,
};

const ADD_HOST_RULE: ActionSpec = ActionSpec {
    id: "org.add_host_rule",
    label: "Add host rule",
    command: "add_org_rule",
    envelope: Envelope::Args,
    bind: &[ORG_ID, ("host_alias", Bind::Param("host_alias"))],
    params: &[ParamSpec {
        name: "host_alias",
        label: "Host",
        kind: ParamKind::Text {
            max: 100,
            placeholder: "hetzner-a",
        },
        required: true,
    }],
    confirm: None,
};

/// Every resource type.
pub const RESOURCES: &[ResourceType] = &[ResourceType {
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
        FieldSpec {
            id: "name",
            label: "Name",
            help: "What the scope selector and the Work view call it.",
            kind: FieldKind::Text { max: 80 },
            edit: Some("name"),
            badge: None,
            confirm: None,
        },
        FieldSpec {
            id: "color",
            label: "Colour",
            help: "Marks its sessions in the sidebar.",
            kind: FieldKind::Color,
            edit: Some("color"),
            badge: None,
            confirm: None,
        },
        FieldSpec {
            id: "rules",
            label: "Rules",
            help: "Which sessions belong to it: a GitHub owner (or one repository), a path prefix, or a host.",
            kind: FieldKind::Items {
                item_label: ItemLabel::OrgRule,
                remove: Some(ActionSpec {
                    id: "org.remove_rule",
                    label: "Remove rule",
                    command: "remove_org_rule",
                    envelope: Envelope::Args,
                    bind: &[("rule_id", Bind::ItemField("id"))],
                    params: &[],
                    confirm: None,
                }),
                add: &[ADD_OWNER_RULE, ADD_PATH_RULE, ADD_HOST_RULE],
            },
            edit: None,
            badge: None,
            confirm: None,
        },
        FieldSpec {
            id: "hosts",
            label: "Hosts",
            help: "A host in an org: its per-host token reads only this org's (and unassigned) work.",
            kind: FieldKind::Items {
                item_label: ItemLabel::Plain,
                remove: Some(ActionSpec {
                    id: "org.unassign_host",
                    label: "Take the host out",
                    command: "assign_host_org",
                    envelope: Envelope::Args,
                    bind: &[("host_alias", Bind::Item), ("org_id", Bind::Null)],
                    params: &[],
                    confirm: None,
                }),
                add: &[ActionSpec {
                    id: "org.assign_host",
                    label: "Add host",
                    command: "assign_host_org",
                    envelope: Envelope::Args,
                    bind: &[("host_alias", Bind::Param("host")), ORG_ID],
                    params: &[ParamSpec {
                        name: "host",
                        label: "Host",
                        kind: ParamKind::Options {
                            source: OptionSource::Hosts,
                        },
                        required: true,
                    }],
                    confirm: None,
                }],
            },
            edit: None,
            badge: None,
            confirm: None,
        },
        FieldSpec {
            id: "trackers",
            label: "Trackers",
            help: "Trackers whose tickets belong to this org.",
            kind: FieldKind::Items {
                item_label: ItemLabel::Field("name"),
                remove: Some(ActionSpec {
                    id: "org.unassign_tracker",
                    label: "Take the tracker out",
                    command: "assign_tracker_org",
                    envelope: Envelope::Args,
                    bind: &[("tracker_id", Bind::ItemField("id")), ("org_id", Bind::Null)],
                    params: &[],
                    confirm: None,
                }),
                add: &[ActionSpec {
                    id: "org.assign_tracker",
                    label: "Add tracker",
                    command: "assign_tracker_org",
                    envelope: Envelope::Args,
                    bind: &[("tracker_id", Bind::Param("tracker")), ORG_ID],
                    params: &[ParamSpec {
                        name: "tracker",
                        label: "Tracker",
                        kind: ParamKind::Options {
                            source: OptionSource::Trackers,
                        },
                        required: true,
                    }],
                    confirm: None,
                }],
            },
            edit: None,
            badge: None,
            confirm: None,
        },
        FieldSpec {
            id: "isolate_sessions",
            label: "Isolate sessions",
            help: "Also hide this org's sessions from other orgs' hosts, and theirs from its hosts: list, peer status, messages. Work data is fenced either way.",
            kind: FieldKind::Bool {
                on_off: false,
                default: false,
            },
            edit: Some("isolate_sessions"),
            badge: Some(Badge::True {
                text: "isolates sessions",
            }),
            confirm: Some("Hosts outside this org will no longer list or message its sessions, and its hosts will not see other orgs' sessions. It can break a controller that dispatches across companies."),
        },
        FieldSpec {
            id: "auto_tidy",
            label: "Auto-tidy",
            help: "Auto-tidy for this org's sessions: on or off whatever the fleet-wide setting says, or inherit it. Safe kill only, never a session in use.",
            kind: FieldKind::Inherit,
            edit: Some("auto_tidy"),
            badge: Some(Badge::Set { text: "auto-tidy" }),
            confirm: None,
        },
        FieldSpec {
            id: "jev_allowed",
            label: "Send to Jev",
            help: "Let this org's redacted prompts and ticket titles go to TypeSafe's decision model when Decisions (Jev) is on. Only ids and numbers are recorded; an answer is at most a suggestion.",
            kind: FieldKind::Bool {
                on_off: true,
                default: false,
            },
            edit: Some("jev"),
            badge: Some(Badge::True {
                text: "sends to Jev",
            }),
            confirm: Some("This org's redacted prompts and ticket titles will be sent to TypeSafe when Decisions (Jev) is on."),
        },
        FieldSpec {
            id: "bound_sees_unassigned",
            label: "Bound devices see unassigned",
            help: "Devices paired to this org (fleet-hub pair --org) also see work and sessions that belong to no org, as a host does. Off: only this org's own.",
            kind: FieldKind::Bool {
                on_off: false,
                default: true,
            },
            edit: Some("bound_sees_unassigned"),
            badge: Some(Badge::False {
                text: "bound devices: own only",
            }),
            confirm: None,
        },
    ],
    create: Some(ActionSpec {
        id: "org.add",
        label: "Add organisation",
        command: "add_org",
        envelope: Envelope::Args,
        bind: &[
            ("name", Bind::Param("name")),
            ("color", Bind::Param("color")),
        ],
        params: &[
            ParamSpec {
                name: "name",
                label: "Name",
                kind: ParamKind::Text {
                    max: 80,
                    placeholder: "Company A",
                },
                required: true,
            },
            ParamSpec {
                name: "color",
                label: "Colour",
                kind: ParamKind::Color,
                required: false,
            },
        ],
        confirm: None,
    }),
    update: Some(ActionSpec {
        id: "org.update",
        label: "Apply",
        command: "update_org",
        envelope: Envelope::Args,
        bind: &[ORG_ID],
        params: &[],
        confirm: None,
    }),
    delete: Some(ActionSpec {
        id: "org.remove",
        label: "Remove organisation",
        command: "remove_org",
        envelope: Envelope::Args,
        bind: &[ORG_ID],
        params: &[],
        confirm: Some("Its rules go with it, and its hosts and trackers become unassigned. Sessions are not touched."),
    }),
}];

pub fn resource(id: &str) -> Option<&'static ResourceType> {
    RESOURCES.iter().find(|r| r.id == id)
}

/// Every desktop command a resource names, for the handler-list check.
pub fn commands() -> Vec<&'static str> {
    let mut out: Vec<&'static str> = Vec::new();
    for r in RESOURCES {
        out.push(r.list);
        for a in r.actions() {
            out.push(a.command);
        }
    }
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
            }
        }
    }

    #[test]
    fn sub_item_actions_bind_the_item_and_record_actions_the_record() {
        let org = resource("org").unwrap();
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
                "list_orgs",
                "remove_org",
                "remove_org_rule",
                "update_org",
            ]
        );
    }
}
