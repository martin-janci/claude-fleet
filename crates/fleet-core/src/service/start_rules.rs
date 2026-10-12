//! Start rules (Orbit Fleet redesign step 8.11, "From AI to rule"): a task
//! key pattern that names the project, and optionally the host, a start of
//! a matching task lands in. The plan's order for every question is a rule
//! first, then Jev, then an LLM; this is the rule for "which project does
//! this task start in".
//!
//! **Where a rule decides.** `trackers::tickets::plan_resolved` asks
//! [`matching`] before the key's history (the newest place its prefix ran)
//! and so before Jev K1 (`decide::start_project`), which is only asked when
//! neither knows: a rule match records no decision run and costs no call.
//! A project or host the caller names still wins over the rule.
//!
//! **How a rule is offered.** Every PERSON's start of a `PREFIX-N` task
//! that no rule decided is tallied ([`tally`]): the same prefix landing in
//! the same project [`OFFER_AFTER`] times in a row turns fleet's tally row
//! `offered`, and the start preview of a matching task carries it as
//! `rule_offer` ("Add rule PD-* → papaya-pos?"). A start of the prefix in
//! another project resets the streak. A person accepts the offer (it
//! becomes `active`, theirs, and replaces any other active rule of the same
//! pattern) or dismisses it, after which it is never offered again. An
//! agent's start is never tallied.
//!
//! **Who may read and change a rule** follows routines: its org's boundary
//! first, then its owner (fleet's own rows have none: the one person of a
//! single-person fleet, or the hub's own reader), and an org member reads
//! while only an admin changes. Anyone else gets the answer of an id that
//! does not exist. A rule decides for the tasks of its org only.

use crate::ipc_error::{codes, lock, IpcError};
use crate::service::orgs;
use crate::service::view_scope::ViewScope;
use crate::store::{now_unix, StartRuleLaunch, StartRuleRow, Store, AGENT_CLAUDE, AGENT_CODEX};
use serde::{Deserialize, Serialize};
use std::sync::Mutex;

/// Identical person starts in a row before fleet offers a rule (the plan's
/// decision: five).
pub const OFFER_AFTER: i64 = 5;
/// A pattern's length, in characters.
pub const PATTERN_MAX_CHARS: usize = 64;

/// What a person writes (`start_rules { action: save, rule }`): the whole
/// rule.
#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, rmcp::schemars::JsonSchema,
)]
#[schemars(crate = "rmcp::schemars")]
#[serde(deny_unknown_fields)]
pub struct StartRuleInput {
    /// A task key pattern, `*` for any run of characters: `PD-*`.
    pub pattern: String,
    pub project_id: i64,
    /// Where it runs; absent = the project's last host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_alias: Option<String>,
    /// The org whose tasks it decides for; absent = tasks of no org. Only on
    /// a new rule: a saved rule keeps its org.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_id: Option<i64>,
    /// Where a start lands when the host is unreachable ("mac, else
    /// mercury"); with no `host_alias`, when the project's last host is.
    /// Absent = none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_host: Option<String>,
    /// The account: a credential profile on the host the session bills.
    /// Absent = the host's own login. Claude Code only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    /// `claude --model` (`opus`, `sonnet[1m]`, a model id). Absent = the
    /// host's default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// low | medium | high | xhigh | max. Absent = the default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// The agent the session runs: claude | codex. Absent = Claude Code.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
}

fn not_found(id: i64) -> IpcError {
    orgs::not_found("start rule", id)
}

fn invalid(msg: impl Into<String>) -> IpcError {
    IpcError::new(codes::E_INVALID, msg.into())
}

// --- patterns -----------------------------------------------------------------

/// PURE: `pattern` trimmed, or why it is not one. Letters, digits and
/// `- _ . : / # *`, at least one of them not `*`, at most
/// [`PATTERN_MAX_CHARS`].
pub fn check_pattern(pattern: &str) -> Result<String, IpcError> {
    let p = pattern.trim();
    if p.is_empty() {
        return Err(invalid("a rule needs a pattern, like PD-*"));
    }
    if p.chars().count() > PATTERN_MAX_CHARS {
        return Err(invalid(format!(
            "a pattern is at most {PATTERN_MAX_CHARS} characters"
        )));
    }
    if let Some(c) = p
        .chars()
        .find(|c| !(c.is_ascii_alphanumeric() || "-_.:/#*".contains(*c)))
    {
        return Err(invalid(format!(
            "a pattern holds letters, digits, - _ . : / # and *, not {c:?}"
        )));
    }
    if specificity(p) == 0 {
        return Err(invalid(
            "a pattern needs at least one character besides *: it would match every task",
        ));
    }
    Ok(p.to_string())
}

/// PURE: the characters of `pattern` that are not `*`: the more, the more
/// specific (`PD-1*` beats `PD-*`).
pub fn specificity(pattern: &str) -> usize {
    pattern.chars().filter(|&c| c != '*').count()
}

/// PURE: does `key` match `pattern`? `*` is any run of characters
/// (none included); everything else matches itself, without regard to
/// ASCII case.
pub fn glob_match(pattern: &str, key: &str) -> bool {
    let p: Vec<char> = pattern.chars().map(|c| c.to_ascii_uppercase()).collect();
    let k: Vec<char> = key.chars().map(|c| c.to_ascii_uppercase()).collect();
    // The classic two-pointer walk with one backtrack point per `*`.
    let (mut pi, mut ki) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while ki < k.len() {
        if pi < p.len() && p[pi] == '*' {
            star = Some((pi, ki));
            pi += 1;
        } else if pi < p.len() && p[pi] == k[ki] {
            pi += 1;
            ki += 1;
        } else if let Some((sp, sk)) = star {
            pi = sp + 1;
            ki = sk + 1;
            star = Some((sp, sk + 1));
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|&c| c == '*')
}

/// PURE: the pattern fleet tallies a start of `key` under: `PREFIX-*` for a
/// `PREFIX-N` key (the prefix letters, digits or `_`), `None` for anything
/// else (a GitHub issue, which its repository already places; an Asana
/// task; free text).
pub fn prefix_pattern(key: &str) -> Option<String> {
    if crate::store::github_ref(key).is_some() {
        return None;
    }
    let (prefix, rest) = key.split_once('-')?;
    let prefix_ok = !prefix.is_empty()
        && prefix
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
        && prefix.chars().any(|c| c.is_ascii_alphabetic());
    let rest_ok = !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit());
    (prefix_ok && rest_ok).then(|| format!("{}-*", prefix.to_ascii_uppercase()))
}

/// PURE: of `rules`, the one that decides `key`: the most specific match,
/// then the newest change, then the newest row.
fn best<'a>(rules: &'a [StartRuleRow], key: &str) -> Option<&'a StartRuleRow> {
    rules
        .iter()
        .filter(|r| glob_match(&r.pattern, key))
        .max_by_key(|r| (specificity(&r.pattern), r.updated_at, r.id))
}

// --- deciding and tallying ----------------------------------------------------

/// The active rule that decides a start of `key`, a task of `org`.
pub fn matching(s: &Store, org: Option<i64>, key: &str) -> Result<Option<StartRuleRow>, IpcError> {
    let rules = s.start_rules_in(org, "active")?;
    Ok(best(&rules, key).cloned())
}

/// The rule fleet offers for `key`, a task of `org`, if `scope` may accept
/// it. `None` once an active rule decides the key: nothing is left to offer.
pub fn offer_for(
    s: &Store,
    scope: &ViewScope,
    org: Option<i64>,
    key: &str,
) -> Result<Option<StartRuleRow>, IpcError> {
    if matching(s, org, key)?.is_some() {
        return Ok(None);
    }
    let offered = s.start_rules_in(org, "offered")?;
    match best(&offered, key) {
        Some(r) if may_change(s, scope, r)? => Ok(Some(r.clone())),
        _ => Ok(None),
    }
}

/// After a PERSON's start of `key` (a task of `org`) landed in
/// `project_id`, and no rule decided it: count it toward `PREFIX-* →
/// project` and reset the streak of the prefix's other projects. Returns
/// the row when this start made it an offer. A key with no prefix, and a
/// pattern a person dismissed for this project, count nothing.
pub fn tally(
    s: &Store,
    org: Option<i64>,
    key: &str,
    project_id: i64,
    now: i64,
) -> Result<Option<StartRuleRow>, IpcError> {
    let Some(pattern) = prefix_pattern(key) else {
        return Ok(None);
    };
    // Another project for the same prefix breaks the streak: "identical"
    // confirmations, in a row. An offer still waiting goes back to counting.
    for state in ["counting", "offered"] {
        for r in s.start_rules_in(org, state)? {
            if r.project_id != project_id && r.pattern.eq_ignore_ascii_case(&pattern) {
                s.set_start_rule_tally(r.id, 0, "counting", now)?;
            }
        }
    }
    match s.find_start_rule(org, &pattern, project_id)? {
        None => {
            let state = if OFFER_AFTER <= 1 {
                "offered"
            } else {
                "counting"
            };
            let r = s.insert_start_rule(org, None, &pattern, project_id, None, state, 1, now)?;
            Ok((state == "offered").then_some(r))
        }
        Some(r) if r.state == "counting" || r.state == "offered" => {
            let n = r.confirmations + 1;
            let state = if n >= OFFER_AFTER {
                "offered"
            } else {
                "counting"
            };
            s.set_start_rule_tally(r.id, n, state, now)?;
            let became = r.state == "counting" && state == "offered";
            Ok(if became {
                s.get_start_rule(r.id)?
            } else {
                None
            })
        }
        // Active: a rule of this very shape that did not decide (a more
        // specific one did, or the caller named the project); dismissed: a
        // person said no. Neither is counted.
        Some(_) => Ok(None),
    }
}

/// Where a start that rule `r` decides lands: its host while that host is
/// reachable, else its fallback ("mac, else mercury"). With no host of its
/// own, the fallback stands in when `last_host` (the project's last host)
/// is unreachable. `None` = the rule leaves the host to the key's history.
/// The flag says the fallback was taken.
pub fn rule_host(
    s: &Store,
    r: &StartRuleRow,
    last_host: Option<&str>,
) -> Result<(Option<String>, bool), IpcError> {
    let primary = r.host_alias.as_deref().or(last_host);
    let Some(fallback) = r.fallback_host.as_deref() else {
        return Ok((r.host_alias.clone(), false));
    };
    let reachable = |h: &str| -> Result<bool, IpcError> {
        Ok(s.list_hosts()?.iter().any(|x| x.alias == h && x.reachable))
    };
    match primary {
        Some(p) if reachable(p)? => Ok((r.host_alias.clone(), false)),
        Some(_) if reachable(fallback)? => Ok((Some(fallback.to_string()), true)),
        _ => Ok((r.host_alias.clone(), false)),
    }
}

/// The placement rule (work_rules) whose "its sessions start here" applies
/// to a start of task `item_id` / `key` in `repo` (`owner/repo`): the rule
/// that places the task in the Work view (the oldest enabled one matching
/// it), when it names a host or an account. `None` otherwise.
pub fn placement_start(
    s: &Store,
    item_id: Option<i64>,
    key: &str,
    title: &str,
    repo: Option<&str>,
) -> Result<Option<crate::store::WorkRule>, IpcError> {
    let item = match item_id {
        Some(id) => s.work_view_item(id)?,
        None => None,
    };
    let repos: Vec<String> = repo.map(str::to_string).into_iter().collect();
    Ok(s.work_rules()?
        .into_iter()
        .find(|r| {
            r.enabled
                && crate::service::work::view::rule_matches(
                    r,
                    item.as_ref(),
                    Some(key),
                    title,
                    &repos,
                )
        })
        .filter(|r| r.host_alias.is_some() || r.profile.is_some()))
}

/// [`tally`] behind the store's lock, best effort: a start never fails on
/// its tally.
pub fn tally_logged(store: &Mutex<Store>, item_id: Option<i64>, key: &str, project_id: i64) {
    let Ok(s) = lock(store) else {
        return;
    };
    let org = match item_id.map(|id| s.item_org(id)).transpose() {
        Ok(o) => o.flatten(),
        Err(e) => {
            tracing::warn!("[start_rules] tally skipped: {}", e.message);
            return;
        }
    };
    if let Err(e) = tally(&s, org, key, project_id, now_unix()) {
        tracing::warn!("[start_rules] tally not recorded: {}", e.message);
    }
}

/// One more start `rule_id` decided, best effort.
pub fn note_hit(store: &Mutex<Store>, rule_id: i64) {
    if let Ok(s) = lock(store) {
        if let Err(e) = s.note_start_rule_hit(rule_id, now_unix()) {
            tracing::warn!("[start_rules] hit not recorded: {}", e.message);
        }
    }
}

// --- who may read and change ----------------------------------------------------

fn role_in(s: &Store, scope: &ViewScope, org: Option<i64>) -> Result<Option<String>, IpcError> {
    match (org, scope.person) {
        (Some(o), Some(p)) => s.org_role(o, p),
        _ => Ok(None),
    }
}

/// May `scope` read `r`?
pub fn sees(s: &Store, scope: &ViewScope, r: &StartRuleRow) -> Result<bool, IpcError> {
    if scope.may_own_person_row(r.org_id, r.owner_person_id) {
        return Ok(true);
    }
    Ok(scope.org.sees_org(r.org_id) && role_in(s, scope, r.org_id)?.is_some())
}

/// May `scope` change `r`?
pub fn may_change(s: &Store, scope: &ViewScope, r: &StartRuleRow) -> Result<bool, IpcError> {
    if scope.may_own_person_row(r.org_id, r.owner_person_id) {
        return Ok(true);
    }
    Ok(scope.org.sees_org(r.org_id)
        && role_in(s, scope, r.org_id)?.as_deref() == Some(crate::store::ROLE_ADMIN))
}

/// May `scope` add a rule for the tasks of `org`? The same as changing an
/// ownerless rule of that org.
fn may_add(s: &Store, scope: &ViewScope, org: Option<i64>) -> Result<bool, IpcError> {
    if scope.may_own_person_row(org, scope.person) {
        // A person's own row, but a rule decides for everyone in its org:
        // only its admins may add one there.
        return Ok(org.is_none()
            || scope.is_internal()
            || role_in(s, scope, org)?.as_deref() == Some(crate::store::ROLE_ADMIN));
    }
    Ok(false)
}

fn visible(s: &Store, scope: &ViewScope, id: i64) -> Result<StartRuleRow, IpcError> {
    match s.get_start_rule(id)? {
        // Fleet's private tally is nobody's to read by id.
        Some(r) if r.state != "counting" && sees(s, scope, &r)? => Ok(r),
        _ => Err(not_found(id)),
    }
}

fn changeable(s: &Store, scope: &ViewScope, id: i64) -> Result<StartRuleRow, IpcError> {
    let r = visible(s, scope, id)?;
    if may_change(s, scope, &r)? {
        Ok(r)
    } else {
        Err(IpcError::new(
            codes::E_FORBIDDEN,
            format!(
                "the rule {} is its owner's; an admin of its organisation may change it too",
                r.pattern
            ),
        ))
    }
}

// --- the tool's actions -----------------------------------------------------------

/// One rule as `start_rules { list }` answers it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartRuleView {
    #[serde(flatten)]
    pub rule: StartRuleRow,
    /// `owner/repo` of its project, for the list's line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// Whether this caller may change it: the UI's buttons, not a fence.
    #[serde(default)]
    pub may_change: bool,
}

fn view_of(s: &Store, scope: &ViewScope, rule: StartRuleRow) -> Result<StartRuleView, IpcError> {
    let project = s
        .get_project(rule.project_id)?
        .map(|p| format!("{}/{}", p.owner, p.repo));
    Ok(StartRuleView {
        may_change: may_change(s, scope, &rule)?,
        project,
        rule,
    })
}

/// `start_rules { action: list }`: the offered, active and dismissed rules
/// this caller may read, offers first, then active rules, then dismissed
/// ones; fleet's private tally is left out.
pub fn list(store: &Mutex<Store>, scope: &ViewScope) -> Result<Vec<StartRuleView>, IpcError> {
    let s = lock(store)?;
    let mut out = Vec::new();
    for r in s.list_start_rules()? {
        if r.state != "counting" && sees(&s, scope, &r)? {
            out.push(view_of(&s, scope, r)?);
        }
    }
    let rank = |st: &str| match st {
        "offered" => 0,
        "active" => 1,
        _ => 2,
    };
    out.sort_by_key(|v| (rank(&v.rule.state), v.rule.id));
    Ok(out)
}

/// `v` trimmed, `None` when absent or empty.
fn trimmed(v: Option<&str>) -> Option<String> {
    v.map(str::trim)
        .filter(|x| !x.is_empty())
        .map(str::to_string)
}

/// A host alias a rule names, trimmed, or `E_NOTFOUND` when fleet has no
/// such host.
fn known_host(s: &Store, h: Option<&str>) -> Result<Option<String>, IpcError> {
    let Some(h) = trimmed(h) else {
        return Ok(None);
    };
    if !s.list_hosts()?.iter().any(|x| x.alias == h) {
        return Err(IpcError::new(
            codes::E_NOTFOUND,
            format!("host {h} not found"),
        ));
    }
    Ok(Some(h))
}

/// A rule's host and account (a start rule's, a placement rule's "its
/// sessions start here"), checked: a host fleet knows, a well-formed
/// credential profile name.
pub fn check_host_and_profile(
    s: &Store,
    host: Option<&str>,
    profile: Option<&str>,
) -> Result<(Option<String>, Option<String>), IpcError> {
    let host = known_host(s, host)?;
    let profile = trimmed(profile);
    if let Some(p) = profile.as_deref() {
        crate::validate::claude_profile(p)?;
    }
    Ok((host, profile))
}

fn check_target(
    s: &Store,
    input: &StartRuleInput,
) -> Result<(String, Option<String>, StartRuleLaunch), IpcError> {
    let pattern = check_pattern(&input.pattern)?;
    match s.get_project(input.project_id)? {
        Some(p) if !p.system => {}
        _ => {
            return Err(IpcError::new(
                codes::E_NOTFOUND,
                format!("project {} not found", input.project_id),
            ))
        }
    }
    let (host, profile) =
        check_host_and_profile(s, input.host_alias.as_deref(), input.profile.as_deref())?;
    let fallback_host = known_host(s, input.fallback_host.as_deref())?;
    if fallback_host.is_some() && fallback_host == host {
        return Err(invalid(
            "the fallback host is the rule's own host; pick another",
        ));
    }
    let model = trimmed(input.model.as_deref());
    if let Some(m) = model.as_deref() {
        crate::validate::claude_model(m)?;
    }
    let effort = trimmed(input.effort.as_deref());
    if let Some(e) = effort.as_deref() {
        crate::validate::effort_level(e)?;
    }
    let agent = trimmed(input.agent.as_deref());
    match agent.as_deref() {
        None | Some(AGENT_CLAUDE) => {}
        Some(AGENT_CODEX) if profile.is_some() => {
            return Err(invalid(
                "an account applies to Claude Code sessions; Codex keeps its own login",
            ))
        }
        Some(AGENT_CODEX) => {}
        Some(other) => {
            return Err(invalid(format!(
                "a rule starts claude or codex, not {other:?}"
            )))
        }
    }
    Ok((
        pattern,
        host,
        StartRuleLaunch {
            fallback_host,
            profile,
            model,
            effort,
            agent,
        },
    ))
}

/// The same pattern and project already has a row: one person's rule, or
/// fleet's tally, which the save takes over.
fn clash(
    s: &Store,
    org: Option<i64>,
    pattern: &str,
    project_id: i64,
    except: Option<i64>,
) -> Result<Option<StartRuleRow>, IpcError> {
    Ok(s.find_start_rule(org, pattern, project_id)?
        .filter(|r| Some(r.id) != except))
}

/// `start_rules { action: save, rule, rule_id? }`: add an active rule, or
/// rewrite one. A new rule over fleet's tally (or a dismissed offer) of the
/// same pattern and project takes that row over.
pub fn save(
    store: &Mutex<Store>,
    scope: &ViewScope,
    id: Option<i64>,
    input: &StartRuleInput,
) -> Result<StartRuleView, IpcError> {
    let s = lock(store)?;
    let (pattern, host, launch) = check_target(&s, input)?;
    let now = now_unix();
    let saved = match id {
        Some(id) => {
            let r = changeable(&s, scope, id)?;
            if let Some(other) = clash(&s, r.org_id, &pattern, input.project_id, Some(id))? {
                if other.state == "active" || other.state == "offered" {
                    return Err(IpcError::new(
                        codes::E_EXISTS,
                        format!("a rule {pattern} for that project already exists"),
                    ));
                }
                s.delete_start_rule(other.id)?;
            }
            s.update_start_rule(
                id,
                &pattern,
                input.project_id,
                host.as_deref(),
                &launch,
                now,
            )?;
            id
        }
        None => {
            let org = input.org_id;
            if !may_add(&s, scope, org)? {
                return Err(IpcError::new(
                    codes::E_FORBIDDEN,
                    "only an admin of the organisation adds a rule for its tasks",
                ));
            }
            match clash(&s, org, &pattern, input.project_id, None)? {
                Some(r) if r.state == "active" => {
                    return Err(IpcError::new(
                        codes::E_EXISTS,
                        format!("a rule {pattern} for that project already exists"),
                    ))
                }
                Some(r) => {
                    s.update_start_rule(
                        r.id,
                        &pattern,
                        input.project_id,
                        host.as_deref(),
                        &launch,
                        now,
                    )?;
                    s.set_start_rule_state(r.id, "active", scope.person, now)?;
                    r.id
                }
                None => {
                    let id = s
                        .insert_start_rule(
                            org,
                            scope.person,
                            &pattern,
                            input.project_id,
                            host.as_deref(),
                            "active",
                            0,
                            now,
                        )?
                        .id;
                    s.update_start_rule(
                        id,
                        &pattern,
                        input.project_id,
                        host.as_deref(),
                        &launch,
                        now,
                    )?;
                    id
                }
            }
        }
    };
    let row = s.get_start_rule(saved)?.ok_or_else(|| not_found(saved))?;
    view_of(&s, scope, row)
}

/// `start_rules { action: accept, rule_id }`: an offer becomes the caller's
/// active rule, and replaces any other active rule of the same pattern (the
/// same task cannot start in two places by rule).
pub fn accept(store: &Mutex<Store>, scope: &ViewScope, id: i64) -> Result<StartRuleView, IpcError> {
    let s = lock(store)?;
    let r = changeable(&s, scope, id)?;
    if r.state != "offered" && r.state != "dismissed" {
        return Err(IpcError::new(
            codes::E_INVALID_STATE,
            format!("the rule {} is already {}", r.pattern, r.state),
        ));
    }
    let now = now_unix();
    for other in s.start_rules_in(r.org_id, "active")? {
        if other.id != id && other.pattern.eq_ignore_ascii_case(&r.pattern) {
            s.delete_start_rule(other.id)?;
        }
    }
    s.set_start_rule_state(id, "active", scope.person, now)?;
    let row = s.get_start_rule(id)?.ok_or_else(|| not_found(id))?;
    view_of(&s, scope, row)
}

/// `start_rules { action: dismiss, rule_id }`: "Not now" on an offer, for
/// good; or an active rule turned off without losing it.
pub fn dismiss(
    store: &Mutex<Store>,
    scope: &ViewScope,
    id: i64,
) -> Result<StartRuleView, IpcError> {
    let s = lock(store)?;
    changeable(&s, scope, id)?;
    s.set_start_rule_state(id, "dismissed", None, now_unix())?;
    let row = s.get_start_rule(id)?.ok_or_else(|| not_found(id))?;
    view_of(&s, scope, row)
}

/// `start_rules { action: delete, rule_id }`. A deleted offer's tally starts
/// over.
pub fn delete(store: &Mutex<Store>, scope: &ViewScope, id: i64) -> Result<bool, IpcError> {
    let s = lock(store)?;
    changeable(&s, scope, id)?;
    s.delete_start_rule(id)
}

/// One `start_rules` call: the MCP tool's parameters and the desktop
/// command's arguments alike.
#[derive(Debug, Clone, Default, Serialize, Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct StartRulesArgs {
    /// list | save | accept | dismiss | delete.
    pub action: String,
    /// Every action but list, and save of a change.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule_id: Option<i64>,
    /// save: the whole rule.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule: Option<StartRuleInput>,
}

/// Run one `start_rules` call for `scope`: `list` answers the rules, every
/// change answers the rule as it now stands, `delete` answers `{ removed }`.
pub fn run(
    store: &Mutex<Store>,
    scope: &ViewScope,
    args: &StartRulesArgs,
) -> Result<serde_json::Value, IpcError> {
    let id = || {
        args.rule_id
            .ok_or_else(|| invalid(format!("{} needs rule_id", args.action)))
    };
    match args.action.as_str() {
        "list" => to_json(&list(store, scope)?),
        "save" => {
            let input = args
                .rule
                .as_ref()
                .ok_or_else(|| invalid("save needs rule"))?;
            to_json(&save(store, scope, args.rule_id, input)?)
        }
        "accept" => to_json(&accept(store, scope, id()?)?),
        "dismiss" => to_json(&dismiss(store, scope, id()?)?),
        "delete" => Ok(serde_json::json!({ "removed": delete(store, scope, id()?)? })),
        other => Err(invalid(format!(
            "action must be list | save | accept | dismiss | delete, got {other:?}"
        ))),
    }
}

fn to_json<T: Serialize>(v: &T) -> Result<serde_json::Value, IpcError> {
    serde_json::to_value(v).map_err(|e| IpcError::new(codes::E_SERIALIZE, e.to_string()))
}

#[cfg(test)]
#[path = "start_rules_tests.rs"]
mod tests;
