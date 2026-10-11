//! Guides stored at runtime (declarative pages, layout L9 `guide`).
//!
//! A guide is a `fleet.page/1` spec whose layout is `guide`: a few steps,
//! each a section of settings fields, notices, links, page actions and read
//! values. Guides compiled into the app are ordinary pages; this module is
//! for the ones an **agent** writes while the app runs — typically a Claude
//! session on a host using the `fleet-guides` catalog skill.
//!
//! The rules:
//! - A guide is data, validated by the same `pages::validate` as every page,
//!   against the registries of this build. It can only NAME settings,
//!   sources, page actions and pages; it cannot hold code or markup, and its
//!   fields write through the person's own settings controls.
//! - Its id is `guide.<name>`, it is none of the compiled pages, and it sits
//!   under the Guides page ([`GUIDES_PAGE`]).
//! - An agent **proposes**; only a person approves (the work graph's rule
//!   R11 applied to pages). An approved guide is re-checked against the
//!   registries each time the pages are read, so a guide naming a setting a
//!   later build removed is left out rather than shown broken.

use crate::ipc_error::{codes, IpcError};
use crate::pages::model::{Item, Layout, Page, SPEC_VERSION};
use crate::pages::{self, validate};
use crate::service::settings::{self, Actor, KindDesc};
use crate::store::{GuideProposalRow, NewGuideProposal, Store};

/// The page every stored guide sits under (`crates/fleet-core/pages/guides.json`).
pub const GUIDES_PAGE: &str = "guides";
/// Every stored guide's id starts with this.
pub const ID_PREFIX: &str = "guide.";
/// The largest spec a proposal may carry, in bytes of JSON.
pub const MAX_SPEC_BYTES: usize = 16 * 1024;
/// Proposals waiting at once. A newer proposal for an id replaces that id's
/// pending one, so this bounds distinct guides, not attempts.
pub const MAX_PENDING: usize = 20;
/// Approved guides at once.
pub const MAX_APPROVED: usize = 50;
/// The longest `why` a proposal may give.
pub const WHY_MAX_CHARS: usize = 500;

/// The result of checking a spec: the page, or every problem with where it
/// is, in the words an author fixes it from.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Checked {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page_id: Option<String>,
    pub problems: Vec<String>,
}

/// A proposal as a reviewer sees it.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GuideProposal {
    pub id: i64,
    pub at: i64,
    pub page_id: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_detail: Option<String>,
    /// Whether it replaces a guide that is live now.
    pub replaces: bool,
    /// The spec itself, for a preview.
    pub page: Page,
}

/// What the Guides page shows: the live guides, what waits, and whether
/// this caller may decide.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GuidesView {
    pub guides: Vec<Page>,
    pub proposals: Vec<GuideProposal>,
    pub can_write: bool,
    /// Where each live guide came from (Guide board, "proposed by a session,
    /// approved by Martin on 6 Oct"), one per entry of `guides`. Absent from
    /// a hub that predates it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub approvals: Vec<GuideApproval>,
}

/// Who proposed a live guide and who approved it, when.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GuideApproval {
    pub page_id: String,
    /// `agent` | `person`, as on the proposal.
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_detail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approved_at: Option<i64>,
    /// The approver as the decision recorded it (`person`, `person (pixel)`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approved_by: Option<String>,
}

fn invalid(msg: impl Into<String>) -> IpcError {
    IpcError::new(codes::E_INVALID, msg.into())
}

fn actor_text(a: &Actor<'_>) -> String {
    match a.detail() {
        Some(d) => format!("{} ({d})", a.word()),
        None => a.word().to_string(),
    }
}

/// Parse and check `spec` as a stored guide, next to the compiled pages and
/// the other live guides (a guide may link to one).
pub fn check(s: &Store, spec: &serde_json::Value) -> Checked {
    let mut problems = Vec::new();
    let text = spec.to_string();
    if text.len() > MAX_SPEC_BYTES {
        problems.push(format!(
            "the spec is {} bytes, over the {MAX_SPEC_BYTES} a guide may have",
            text.len()
        ));
        return Checked {
            ok: false,
            page_id: None,
            problems,
        };
    }
    let page: Page = match serde_json::from_value(spec.clone()) {
        Ok(p) => p,
        Err(e) => {
            problems.push(format!("not a fleet.page/1 spec: {e}"));
            return Checked {
                ok: false,
                page_id: None,
                problems,
            };
        }
    };
    let id = page.id.clone();
    problems.extend(rules(&page));
    let mut all: Vec<Page> = pages::all().to_vec();
    all.extend(live(s).into_iter().filter(|g| g.id != id));
    all.push(page);
    problems.extend(
        validate::validate(&all)
            .into_iter()
            .filter(|p| p.page == id)
            .map(|p| p.to_string()),
    );
    Checked {
        ok: problems.is_empty(),
        page_id: Some(id),
        problems,
    }
}

/// What a stored guide is beyond any page: its layout, id and place.
fn rules(page: &Page) -> Vec<String> {
    let mut out = Vec::new();
    if page.spec != SPEC_VERSION {
        out.push(format!("spec must be {SPEC_VERSION:?}"));
    }
    if page.layout != Layout::Guide {
        out.push("a stored page is a guide: \"layout\": \"guide\"".into());
    }
    if !page.id.starts_with(ID_PREFIX) || page.id.len() == ID_PREFIX.len() {
        out.push(format!(
            "a guide's id is {ID_PREFIX}<name>, e.g. guide.work_graph"
        ));
    }
    if pages::get(&page.id).is_some() {
        out.push(format!(
            "`{}` is a page of the app: choose another id",
            page.id
        ));
    }
    if page.parent.as_deref() != Some(GUIDES_PAGE) {
        out.push(format!("a guide's parent is \"{GUIDES_PAGE}\""));
    }
    out
}

/// The approved guides that still check against this build, in approval
/// order. One that no longer does is left out and logged.
pub fn live(s: &Store) -> Vec<Page> {
    let rows = s.guide_proposals_in("approved").unwrap_or_default();
    let parsed: Vec<Page> = rows
        .iter()
        .filter_map(|r| serde_json::from_str::<Page>(&r.spec).ok())
        .collect();
    let mut all: Vec<Page> = pages::all().to_vec();
    all.extend(parsed.iter().cloned());
    let problems = validate::validate(&all);
    parsed
        .into_iter()
        .filter(|g| {
            let bad: Vec<_> = problems.iter().filter(|p| p.page == g.id).collect();
            if !bad.is_empty() {
                tracing::warn!(guide = %g.id, problems = bad.len(), "[guides] left out: no longer checks");
            }
            bad.is_empty() && rules(g).is_empty()
        })
        .collect()
}

/// Propose `spec` as a guide on behalf of `actor`. Refused: a spec that does
/// not check (every problem in the message), and a queue that is full.
pub fn propose(
    s: &Store,
    spec: &serde_json::Value,
    why: Option<&str>,
    actor: Actor<'_>,
) -> Result<GuideProposalRow, IpcError> {
    let checked = check(s, spec);
    if !checked.ok {
        return Err(invalid(format!(
            "the guide does not check:\n{}",
            checked.problems.join("\n")
        )));
    }
    let why = why.map(str::trim).filter(|w| !w.is_empty());
    if why.is_some_and(|w| w.chars().count() > WHY_MAX_CHARS) {
        return Err(invalid(format!(
            "why is at most {WHY_MAX_CHARS} characters"
        )));
    }
    let page: Page = serde_json::from_value(spec.clone()).map_err(|e| invalid(e.to_string()))?;
    let pending = s.guide_proposals_in("pending").map_err(IpcError::from)?;
    if pending.len() >= MAX_PENDING && !pending.iter().any(|p| p.page_id == page.id) {
        return Err(IpcError::new(
            codes::E_RATE_LIMITED,
            format!("{MAX_PENDING} guides already wait for review: a person decides those first"),
        ));
    }
    let text = serde_json::to_string(&page).map_err(|e| invalid(e.to_string()))?;
    let row = s
        .insert_guide_proposal(&NewGuideProposal {
            page_id: &page.id,
            title: &page.title,
            spec: &text,
            why,
            source: actor.word(),
            source_detail: actor.detail(),
        })
        .map_err(IpcError::from)?;
    Ok(row)
}

/// The proposals waiting, oldest first.
pub fn pending(s: &Store) -> Result<Vec<GuideProposal>, IpcError> {
    let live_ids: Vec<String> = s
        .guide_proposals_in("approved")
        .map_err(IpcError::from)?
        .into_iter()
        .map(|r| r.page_id)
        .collect();
    let rows = s.guide_proposals_in("pending").map_err(IpcError::from)?;
    Ok(rows
        .into_iter()
        .filter_map(|r| {
            let page = serde_json::from_str::<Page>(&r.spec).ok()?;
            Some(GuideProposal {
                replaces: live_ids.contains(&r.page_id),
                id: r.id,
                at: r.at,
                page_id: r.page_id,
                title: r.title,
                why: r.why,
                source: r.source,
                source_detail: r.source_detail,
                page,
            })
        })
        .collect())
}

pub fn view(s: &Store, can_write: bool) -> Result<GuidesView, IpcError> {
    let guides = live(s);
    let approvals = approvals(s, &guides)?;
    Ok(GuidesView {
        guides,
        proposals: pending(s)?,
        can_write,
        approvals,
    })
}

/// The provenance of each guide in `guides`, from its approved row.
fn approvals(s: &Store, guides: &[Page]) -> Result<Vec<GuideApproval>, IpcError> {
    let rows = s.guide_proposals_in("approved").map_err(IpcError::from)?;
    Ok(rows
        .into_iter()
        .filter(|r| guides.iter().any(|g| g.id == r.page_id))
        .map(|r| GuideApproval {
            page_id: r.page_id,
            source: r.source,
            source_detail: r.source_detail,
            approved_at: r.decided_at,
            approved_by: r.decided_by,
        })
        .collect())
}

/// Approve (or reject) a pending proposal as `actor`, a person. Approving
/// checks the spec again: the registries may have moved since it was
/// proposed.
pub fn decide(s: &Store, id: i64, approve: bool, actor: Actor<'_>) -> Result<GuidesView, IpcError> {
    let row = s
        .guide_proposal(id)
        .map_err(IpcError::from)?
        .filter(|r| r.state == "pending")
        .ok_or_else(|| IpcError::new(codes::E_INVALID, format!("no guide proposal {id} waits")))?;
    let by = actor_text(&actor);
    // No actor approves its own proposal. The feature's guarantee is "an
    // agent proposes; only a person approves", but the master token is BOTH a
    // proposer (`Actor::Agent("control API")`) and a settings writer, so one
    // control-API caller could `propose` and then `decide { approve: true }`
    // with no second party. A guide an agent proposed from a HOST session
    // carries that host's detail, so an operator's master token still
    // approves it — which is the intended flow.
    //
    // Here rather than in the MCP tool so the hub CLI and the desktop are held
    // to it too.
    if approve
        && row.source == actor.word()
        && row.source_detail.as_deref() == actor.detail()
        && actor.detail().is_some()
    {
        return Err(IpcError::new(
            codes::E_FORBIDDEN,
            format!(
                "{by} proposed this guide, so it does not also approve it — a person decides, in \
                 Settings → Guides or with fleet-hub guides"
            ),
        ));
    }
    if approve {
        let spec: serde_json::Value =
            serde_json::from_str(&row.spec).map_err(|e| invalid(e.to_string()))?;
        let checked = check(s, &spec);
        if !checked.ok {
            return Err(invalid(format!(
                "the guide no longer checks:\n{}",
                checked.problems.join("\n")
            )));
        }
        let live = s.guide_proposals_in("approved").map_err(IpcError::from)?;
        if live.len() >= MAX_APPROVED && !live.iter().any(|r| r.page_id == row.page_id) {
            return Err(IpcError::new(
                codes::E_RATE_LIMITED,
                format!("{MAX_APPROVED} guides are live: remove one first"),
            ));
        }
        s.approve_guide_proposal(id, &by).map_err(IpcError::from)?;
    } else {
        s.close_guide_proposal(id, "pending", "rejected", &by)
            .map_err(IpcError::from)?;
    }
    view(s, true)
}

/// Take a live guide off the pages, by its page id.
pub fn remove(s: &Store, page_id: &str, actor: Actor<'_>) -> Result<GuidesView, IpcError> {
    let row = s
        .guide_proposals_in("approved")
        .map_err(IpcError::from)?
        .into_iter()
        .find(|r| r.page_id == page_id)
        .ok_or_else(|| IpcError::new(codes::E_INVALID, format!("no live guide `{page_id}`")))?;
    s.close_guide_proposal(row.id, "approved", "removed", &actor_text(&actor))
        .map_err(IpcError::from)?;
    view(s, true)
}

/// What a guide may name, for an author: the rules, the items, and every
/// setting, page, page action and read-only source by id, with no values.
#[derive(Debug, serde::Serialize)]
pub struct AuthoringCatalog {
    pub spec: &'static str,
    pub layout: &'static str,
    pub id_rule: String,
    pub parent: &'static str,
    pub max_steps: usize,
    pub limits: serde_json::Value,
    pub items: &'static [&'static str],
    pub conditions: &'static [&'static str],
    pub settings: Vec<serde_json::Value>,
    pub pages: Vec<serde_json::Value>,
    pub actions: Vec<serde_json::Value>,
    pub sources: Vec<serde_json::Value>,
    pub example: serde_json::Value,
}

pub fn authoring_catalog(s: &Store) -> AuthoringCatalog {
    // Where each setting lives: a guide links there for the rest of a topic.
    let mut home = std::collections::BTreeMap::new();
    for page in pages::all().iter().filter(|p| p.layout != Layout::Guide) {
        validate::for_each_item(page, |_, item| {
            if let Item::Field { key, .. } = item {
                home.entry(key.clone()).or_insert_with(|| page.id.clone());
            }
        });
    }
    let settings = settings::SPECS
        .iter()
        .map(|spec| {
            let mut v = serde_json::json!({
                "key": spec.key,
                "label": spec.label,
                "help": spec.help,
                "kind": KindDesc::from(spec.kind),
                "unit": spec.unit,
                "default": spec.default,
            });
            if let Some(z) = spec.zero {
                v["zero_means"] = z.into();
            }
            if let Some(h) = home.get(spec.key) {
                v["page"] = h.clone().into();
            }
            if spec.owned_by.is_some() {
                v["read_only"] = true.into();
            }
            if !spec.option_labels.is_empty() {
                v["options"] = spec
                    .option_labels
                    .iter()
                    .map(|(value, label)| serde_json::json!({ "value": value, "label": label }))
                    .collect();
            }
            v
        })
        .collect();
    let mut page_list: Vec<serde_json::Value> = pages::navigable()
        .iter()
        .map(|p| serde_json::json!({ "id": p.id, "title": p.title }))
        .collect();
    page_list.extend(
        live(s)
            .iter()
            .map(|p| serde_json::json!({ "id": p.id, "title": p.title })),
    );
    let actions = pages::actions::PAGE_ACTIONS
        .iter()
        .map(|a| serde_json::json!({ "id": a.id, "label": a.label, "help": a.help }))
        .collect();
    let sources = pages::sources::SOURCES
        .iter()
        .filter(|src| src.live.is_none())
        .filter(|src| matches!(src.shape.name(), "scalar" | "record"))
        .map(|src| {
            let fields = serde_json::to_value(src)
                .ok()
                .and_then(|v| v.get("fields").cloned())
                .unwrap_or_default();
            serde_json::json!({
                "id": src.id, "label": src.label, "shape": src.shape.name(), "fields": fields,
            })
        })
        .collect();
    AuthoringCatalog {
        spec: SPEC_VERSION,
        layout: "guide",
        id_rule: format!("{ID_PREFIX}<lowercase_name>, not an id the app already has"),
        parent: GUIDES_PAGE,
        max_steps: validate::MAX_GUIDE_STEPS,
        limits: serde_json::json!({
            "title_chars": validate::MAX_TITLE,
            "titles": "the guide's and every step's",
            "text_chars": validate::MAX_TEXT,
            "text": "every notice and the guide's and a step's intro",
            "secs_fields": "stored in seconds; the field shows and takes the setting's unit",
            "hint_chars": validate::MAX_HINT,
            "spec_bytes": MAX_SPEC_BYTES,
            "plain_text": "no < or >; no markup",
        }),
        items: crate::pages::catalog::layout_item_types(Layout::Guide),
        conditions: &[
            "{\"key\": K, \"eq\": V}",
            "{\"key\": K, \"in\": [V, ...]}",
            "{\"key\": K, \"truthy\": true|false}",
            "{\"all\": [C, ...]}",
            "{\"any\": [C, ...]}",
            "{\"not\": C}",
        ],
        settings,
        pages: page_list,
        actions,
        sources,
        example: example(),
    }
}

/// A small guide that checks: the shape an author starts from.
pub fn example() -> serde_json::Value {
    serde_json::json!({
        "spec": SPEC_VERSION,
        "id": "guide.cleanup",
        "title": "Let fleet tidy up idle sessions",
        "parent": GUIDES_PAGE,
        "layout": "guide",
        "intro": "Three steps: what cleanup does, turn it on, and when it acts.",
        "sections": [
            { "title": "What it does", "items": [
                { "type": "notice", "tone": "info",
                  "text": "Garbage collection stops background sessions that sat idle, so they stop using a host." }
            ] },
            { "title": "Turn it on", "items": [
                { "type": "field", "key": "gc.enabled" }
            ] },
            { "title": "When it acts", "when": { "key": "gc.enabled", "truthy": true }, "items": [
                { "type": "field", "key": "gc.bg_idle_secs", "hint": "A day suits most fleets." },
                { "type": "link", "page": "settings.automation", "label": "Every automation setting" }
            ] }
        ]
    })
}

/// The settings keys a guide's fields name, for a reviewer.
pub fn keys_of(page: &Page) -> Vec<String> {
    let mut out = Vec::new();
    validate::for_each_item(page, |_, item| {
        if let Item::Field { key, .. } = item {
            out.push(key.clone());
        }
    });
    out
}

#[cfg(test)]
mod tests;
