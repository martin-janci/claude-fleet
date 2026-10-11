//! What the support agent writes to Jira (step 7), as actions: the
//! investigation comment (or the question when it needs more), a bug in
//! the rule's `bug_project` linked to the ticket, or a link to the open bug
//! it matches. Ported from support-agent's `plugins/jira` (`act`,
//! `buildInvestigationCommentAdf`, `postNeedsMoreInfo`,
//! `buildBugDescriptionAdf`, `dedup`). Every payload carries exactly what
//! will be sent: the ADF for Jira Cloud and the same text as wiki markup
//! for Jira Data Center, which is also what a person reads before
//! approving it.

use super::fingerprint::{self, FOLLOW_UP_AT, REUSE_AT};
use super::output::Investigation;
use super::SupportConfig;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub const JIRA_COMMENT: &str = "jira_comment";
pub const JIRA_CREATE_BUG: &str = "jira_create_bug";
pub const JIRA_LINK: &str = "jira_link";
pub const KINDS: [&str; 3] = [JIRA_COMMENT, JIRA_CREATE_BUG, JIRA_LINK];

/// A comment on the ticket.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommentPayload {
    pub tracker_id: i64,
    pub key: String,
    /// Wiki markup: what Jira Data Center gets and a person reads.
    pub body: String,
    /// The same as ADF, for Jira Cloud.
    pub adf: Value,
}

/// A bug to create in `project`, then linked to the ticket.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateBugPayload {
    pub tracker_id: i64,
    pub source_key: String,
    pub project: String,
    pub issue_type: String,
    pub summary: String,
    pub body: String,
    pub adf: Value,
    pub labels: Vec<String>,
    pub link_type: String,
    pub fingerprint: Option<String>,
}

/// A link from the open bug to the ticket.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LinkPayload {
    pub tracker_id: i64,
    pub source_key: String,
    pub bug_key: String,
    pub link_type: String,
    pub fingerprint: Option<String>,
    /// Why this bug: the fingerprint's record, or the summary's score.
    pub why: String,
    pub body: String,
}

// ── Document building: one tree, two renderings ─────────────────────────────

enum Block {
    Heading(u8, String),
    Para(String, bool),
    Bullets(Vec<String>),
    Code(String, &'static str),
}

fn paras(text: &str) -> Vec<Block> {
    text.replace("\r\n", "\n")
        .split("\n\n")
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(|p| Block::Para(p.to_string(), false))
        .collect()
}

fn text_node(t: &str, bold: bool) -> Vec<Value> {
    let mut out = Vec::new();
    for (i, line) in t.split('\n').enumerate() {
        if i > 0 {
            out.push(json!({ "type": "hardBreak" }));
        }
        if !line.is_empty() {
            let mut n = json!({ "type": "text", "text": line });
            if bold {
                n["marks"] = json!([{ "type": "strong" }]);
            }
            out.push(n);
        }
    }
    out
}

fn adf(blocks: &[Block]) -> Value {
    let content: Vec<Value> = blocks
        .iter()
        .map(|b| match b {
            Block::Heading(l, t) => json!({ "type": "heading", "attrs": { "level": l }, "content": text_node(t, false) }),
            Block::Para(t, bold) => json!({ "type": "paragraph", "content": text_node(t, *bold) }),
            Block::Bullets(items) => json!({
                "type": "bulletList",
                "content": items.iter().map(|i| json!({
                    "type": "listItem",
                    "content": [{ "type": "paragraph", "content": text_node(i, false) }],
                })).collect::<Vec<_>>(),
            }),
            Block::Code(t, lang) => json!({
                "type": "codeBlock",
                "attrs": { "language": lang },
                "content": [{ "type": "text", "text": t }],
            }),
        })
        .collect();
    json!({ "type": "doc", "version": 1, "content": content })
}

fn wiki(blocks: &[Block]) -> String {
    blocks
        .iter()
        .map(|b| match b {
            Block::Heading(l, t) => format!("h{l}. {t}"),
            Block::Para(t, true) => format!("*{}*", t.replace('\n', " ")),
            Block::Para(t, false) => t.clone(),
            Block::Bullets(items) => items
                .iter()
                .map(|i| format!("* {i}"))
                .collect::<Vec<_>>()
                .join("\n"),
            Block::Code(t, lang) => format!("{{code:{lang}}}\n{t}\n{{code}}"),
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn cut(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let head: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{head}…")
    }
}

/// What a comment says about the bug it goes with.
pub enum BugNote<'a> {
    None,
    Matches(&'a str),
    Proposed(&'a str),
}

/// PURE: the investigation comment, in at most `max_chars` (the diff and
/// the long fields are shortened, then the diff dropped, to fit).
pub fn investigation_comment(
    inv: &Investigation,
    cfg: &SupportConfig,
    job_id: i64,
    fp: Option<&str>,
    bug: BugNote,
) -> (String, Value) {
    let build = |field_cap: usize, with_diff: bool| {
        let mut b = vec![Block::Heading(3, format!("🔍 {}", cfg.marker))];
        if !inv.summary.is_empty() && inv.summary != inv.root_cause {
            b.push(Block::Para(inv.summary.replace('\n', " "), true));
        }
        b.push(Block::Heading(3, "Root Cause".into()));
        b.extend(paras(&cut(&inv.root_cause, field_cap)));
        b.push(Block::Heading(3, "Details".into()));
        let mut details = vec![
            format!("Confidence: {}", inv.confidence),
            format!("Severity: {}", inv.severity),
            format!("Component: {}", inv.affected_component),
        ];
        if !inv.affected_version.is_empty() {
            details.push(format!("Version: {}", inv.affected_version));
        }
        if let Some(fp) = fp {
            details.push(format!("Fingerprint: {}", &fp[..fp.len().min(12)]));
        }
        b.push(Block::Bullets(details));
        if !inv.suggested_fix.is_empty() {
            b.push(Block::Heading(3, "Suggested Fix".into()));
            b.extend(paras(&cut(&inv.suggested_fix, field_cap)));
        }
        if with_diff && !inv.diff.is_empty() {
            b.push(Block::Heading(3, "Diff".into()));
            b.push(Block::Code(cut(&inv.diff, field_cap), "diff"));
        }
        if !inv.steps_to_reproduce.is_empty() {
            b.push(Block::Heading(3, "Steps to Reproduce".into()));
            b.extend(paras(&cut(&inv.steps_to_reproduce, field_cap)));
        }
        b.push(Block::Heading(3, "Next Steps".into()));
        let mut next = vec![format!(
            "Investigated by claude-fleet's support agent (job {job_id})."
        )];
        match bug {
            BugNote::None => {}
            BugNote::Matches(k) => next.push(format!("Matches the open bug {k}.")),
            BugNote::Proposed(p) => next.push(format!("A bug is proposed in {p}.")),
        }
        b.push(Block::Bullets(next));
        b
    };
    for (cap, diff) in [
        (4000, true),
        (1500, true),
        (800, true),
        (800, false),
        (300, false),
    ] {
        let blocks = build(cap, diff);
        let text = wiki(&blocks);
        if text.chars().count() <= cfg.max_body_chars {
            return (text, adf(&blocks));
        }
    }
    let blocks = build(120, false);
    (wiki(&blocks), adf(&blocks))
}

/// PURE: the question for the reporter, with the marker so it never
/// triggers the agent again.
pub fn needs_info_comment(question: &str, cfg: &SupportConfig) -> (String, Value) {
    let mut b = vec![
        Block::Heading(3, "❓ Potrebujem doplniť info".into()),
        Block::Para(format!("[{}]", cfg.marker), false),
    ];
    b.extend(paras(&cut(
        question,
        cfg.max_body_chars.saturating_sub(200),
    )));
    (wiki(&b), adf(&b))
}

/// PURE: a Jira label: no spaces, at most 255 characters.
fn label(s: &str) -> String {
    let l: String = s
        .trim()
        .chars()
        .map(|c| if c.is_whitespace() { '-' } else { c })
        .collect();
    cut(&l, 255)
}

/// PURE: the bug for an investigation that found an exception.
pub fn bug(
    inv: &Investigation,
    cfg: &SupportConfig,
    tracker_id: i64,
    project: &str,
    source_key: &str,
    fp: Option<&str>,
    related: Option<&str>,
) -> CreateBugPayload {
    let message_head = cut(inv.error_message.trim(), 80);
    let summary = cut(
        &format!(
            "[{}] {}: {}",
            inv.affected_component, inv.exception_class, message_head
        ),
        250,
    );
    let mut b = vec![Block::Heading(2, "Problem description".into())];
    b.extend(paras(&cut(&inv.summary, 1500)));
    b.push(Block::Heading(2, "Probable root cause".into()));
    b.extend(paras(&cut(&inv.root_cause, 1500)));
    b.push(Block::Heading(2, "Relevant log excerpt".into()));
    b.push(Block::Code(
        cut(
            if inv.stack_trace.is_empty() {
                "(none)"
            } else {
                &inv.stack_trace
            },
            1500,
        ),
        "text",
    ));
    if !inv.steps_to_reproduce.is_empty() {
        b.push(Block::Heading(2, "Steps to reproduce".into()));
        b.extend(paras(&cut(&inv.steps_to_reproduce, 800)));
    }
    b.push(Block::Heading(2, "Context".into()));
    let mut ctx = vec![
        format!("Incident ticket: {source_key}"),
        format!("Component: {}", inv.affected_component),
        format!(
            "Version: {}",
            if inv.affected_version.is_empty() {
                "unknown"
            } else {
                &inv.affected_version
            }
        ),
        format!("Severity: {}", inv.severity),
        format!("Confidence: {}", inv.confidence),
    ];
    if let Some(fp) = fp {
        ctx.push(format!("Fingerprint: {fp}"));
    }
    if let Some(r) = related {
        ctx.push(format!("Possibly related: {r}"));
    }
    b.push(Block::Bullets(ctx));
    let mut labels = vec![label(&inv.affected_component)];
    if !inv.affected_version.is_empty() {
        labels.push(label(&format!("v{}", inv.affected_version)));
    }
    labels.retain(|l| !l.is_empty());
    CreateBugPayload {
        tracker_id,
        source_key: source_key.into(),
        project: project.into(),
        issue_type: cfg.bug_type.clone(),
        summary,
        body: wiki(&b),
        adf: adf(&b),
        labels,
        link_type: cfg.link_type.clone(),
        fingerprint: fp.map(str::to_string),
    }
}

/// An open bug a search found: its key and summary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub key: String,
    pub summary: String,
}

/// PURE: the JQL that finds the open bugs an exception may already have.
pub fn candidates_jql(project: &str, bug_type: &str, exception_class: &str) -> Option<String> {
    let clean = |s: &str| {
        s.chars()
            .filter(|c| !matches!(c, '"' | '\\'))
            .collect::<String>()
    };
    let class = clean(exception_class.trim());
    if class.is_empty() {
        return None;
    }
    Some(format!(
        "project = \"{}\" AND issuetype = \"{}\" AND statusCategory != Done AND text ~ \"\\\"{class}\\\"\" ORDER BY updated DESC",
        clean(project),
        clean(bug_type)
    ))
}

fn json_text<T: Serialize>(v: &T) -> String {
    serde_json::to_string(v).unwrap_or_default()
}

/// One planned action: its kind, payload and whether the rule runs it
/// without a person.
#[derive(Debug, Clone, PartialEq)]
pub struct Planned {
    pub kind: &'static str,
    pub payload: String,
    pub auto: bool,
}

pub struct PlanInput<'a> {
    pub inv: &'a Investigation,
    pub cfg: &'a SupportConfig,
    pub level: &'a str,
    pub tracker_id: i64,
    pub key: &'a str,
    pub job_id: i64,
    pub fingerprint: Option<&'a str>,
    /// The bug the fingerprint's record names, inside the incident window.
    pub known_bug: Option<&'a str>,
    /// Open bugs the search found.
    pub candidates: &'a [Candidate],
}

/// PURE: the writes an investigation asks for, at `propose` or `act`.
pub fn plan(p: &PlanInput) -> Vec<Planned> {
    let auto = |kind: &str| {
        p.level == "act"
            && p.cfg.auto.iter().any(|a| a == kind)
            && (kind != JIRA_CREATE_BUG || (p.inv.confidence == "high" && p.fingerprint.is_some()))
    };
    let planned = |kind: &'static str, payload: String| Planned {
        kind,
        payload,
        auto: auto(kind),
    };
    let mut out = Vec::new();
    if !p.inv.needs_more_info.is_empty() {
        let (body, adf) = needs_info_comment(&p.inv.needs_more_info, p.cfg);
        let c = CommentPayload {
            tracker_id: p.tracker_id,
            key: p.key.into(),
            body,
            adf,
        };
        out.push(planned(JIRA_COMMENT, json_text(&c)));
        return out;
    }
    // The bug: the fingerprint's own, else a search's match, else a new one.
    let mut note = BugNote::None;
    let mut bug_action = None;
    if let (Some(project), false) = (&p.cfg.bug_project, p.inv.exception_class.is_empty()) {
        let best = p
            .candidates
            .iter()
            .map(|c| {
                let s = fingerprint::similarity(
                    &c.summary,
                    &p.inv.exception_class,
                    &p.inv.affected_component,
                    &p.inv.error_message,
                );
                (s, c)
            })
            .max_by_key(|(s, _)| *s);
        let link = |bug_key: &str, why: String| {
            let l = LinkPayload {
                tracker_id: p.tracker_id,
                source_key: p.key.into(),
                bug_key: bug_key.into(),
                link_type: p.cfg.link_type.clone(),
                fingerprint: p.fingerprint.map(str::to_string),
                body: format!("{} {} {}", bug_key, p.cfg.link_type.to_lowercase(), p.key),
                why,
            };
            (JIRA_LINK, json_text(&l))
        };
        if let Some(k) = p.known_bug {
            note = BugNote::Matches(k);
            bug_action = Some(link(
                k,
                "the same fingerprint, inside the incident window".into(),
            ));
        } else if let Some((s, c)) = best.filter(|(s, _)| *s >= REUSE_AT) {
            note = BugNote::Matches(&c.key);
            bug_action = Some(link(&c.key, format!("its summary scores {s}")));
        } else {
            let related = best
                .filter(|(s, _)| *s >= FOLLOW_UP_AT)
                .map(|(_, c)| c.key.as_str());
            let b = bug(
                p.inv,
                p.cfg,
                p.tracker_id,
                project,
                p.key,
                p.fingerprint,
                related,
            );
            note = BugNote::Proposed(project);
            bug_action = Some((JIRA_CREATE_BUG, json_text(&b)));
        }
    }
    let (body, adf) = investigation_comment(p.inv, p.cfg, p.job_id, p.fingerprint, note);
    let c = CommentPayload {
        tracker_id: p.tracker_id,
        key: p.key.into(),
        body,
        adf,
    };
    out.push(planned(JIRA_COMMENT, json_text(&c)));
    if let Some((kind, payload)) = bug_action {
        out.push(planned(kind, payload));
    }
    out
}
