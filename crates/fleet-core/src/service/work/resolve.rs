//! The work-link resolver (work graph M4.3, design §0.3): a PURE function
//! from what a session shows now — its state signals, the event signals just
//! seen, and the links it already has — to the link changes that follow.
//! `detect` gathers the input from the store and applies the output; nothing
//! here reads a clock, a store or a setting.
//!
//! Tiers, not scores: `explicit > strong > weak`, and inside a tier the most
//! recent decision. Nothing is learned. The rules are numbered so a link's
//! evidence can name the one that made it:
//!
//! | Rule | Condition | Outcome |
//! |---|---|---|
//! | R1 | a person's `confirmed` / `rejected` decision | final: never touched |
//! | R2 | `explicit` (`started`, `agent`, carried links) | confirmed (not made here) |
//! | R3 | exactly one strong state candidate, trusted project | confirmed, auto |
//! | R3b | the same, untrusted project | a pre-selected suggestion |
//! | R4 | several strong state candidates | all suggested, none pre-selected |
//! | R5 | a ticket URL in a prompt | confirmed when it is the sole candidate of a conversation's first prompt, else suggested |
//! | R6 | a weak candidate (prompt key, trailer, `#n`) | suggested; decays at the next conversation boundary unless seen again |
//! | R7 | a state signal's value changes | the auto link it made ENDS; suggestions it made go |
//! | R8 | a key two trackers claim | never automatic: a suggestion |
//! | R9 | a rejected (participant, target) pair | never proposed again, from any signal |
//!
//! R10 (reviews and workers inherit the parent's primary) is M2.2's carry.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// How much a candidate says (design §0.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Strength {
    Weak,
    Strong,
    Explicit,
}

impl Strength {
    pub fn as_str(self) -> &'static str {
        match self {
            Strength::Weak => "weak",
            Strength::Strong => "strong",
            Strength::Explicit => "explicit",
        }
    }

    pub fn parse(s: &str) -> Option<Strength> {
        match s {
            "weak" => Some(Strength::Weak),
            "strong" => Some(Strength::Strong),
            "explicit" => Some(Strength::Explicit),
            _ => None,
        }
    }
}

/// Where a candidate came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Signal {
    /// The session's current branch (state).
    Branch,
    /// The PR's head branch (state).
    PrHead,
    /// A PR's closing issue reference (state).
    PrClosing,
    /// A key in the PR's title or body (weak event).
    PrText,
    /// A commit trailer (`Refs:`, `Fixes`, `Jira:`, `Closes #n`) (weak event).
    Trailer,
    /// A ticket URL in a prompt.
    PromptUrl,
    /// A key in a prompt.
    PromptKey,
    /// A bare `#n` in a prompt.
    PromptIssue,
}

impl Signal {
    /// The link `source` a link created from this signal carries.
    pub fn source(self) -> &'static str {
        match self {
            Signal::Branch => "branch",
            Signal::PrHead | Signal::PrClosing | Signal::PrText => "pr",
            Signal::Trailer => "trailer",
            Signal::PromptUrl => "url",
            Signal::PromptKey | Signal::PromptIssue => "prompt",
        }
    }

    /// A state signal: only its present value is a candidate (C10).
    pub fn is_state(self) -> bool {
        matches!(self, Signal::Branch | Signal::PrHead | Signal::PrClosing)
    }
}

/// Link sources the resolver itself writes. A link with any other source is
/// a decision (a person's, an agent's, or a carry) and is never changed here.
pub const AUTO_SOURCES: &[&str] = &["branch", "pr", "trailer", "url", "prompt"];

/// One line of a link's explanation, stored denormalised on the link (C13).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Evidence {
    pub signal: Signal,
    /// The rule that read it (R3, R5 …).
    pub rule: String,
    /// What was seen: the branch name, the matched text (≤ 80 chars).
    pub text: String,
    /// ±40 characters of prompt around the match, redacted; absent when
    /// snippets are off or the signal is not a prompt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snippet: Option<String>,
    pub at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation: Option<String>,
    /// `reference` for a key from a reference list (the dump guard).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Most evidence lines one link keeps (the newest).
pub const EVIDENCE_MAX: usize = 6;

/// A candidate target seen by one signal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// Normalised target: `ABC-123`, `owner/repo#42`, `asana:<id>`.
    pub target: String,
    pub signal: Signal,
    pub strength: Strength,
    /// Two trackers claim this key (R8).
    pub ambiguous: bool,
    /// The event is the sole candidate of a conversation's first prompt.
    pub first_prompt_sole: bool,
    /// The tracker a URL's host named, for binding.
    pub tracker_id: Option<i64>,
    pub evidence: Evidence,
}

/// A live link the session already has.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExistingLink {
    pub id: i64,
    pub target: String,
    /// confirmed | rejected | suggested.
    pub state: String,
    pub source: String,
    pub strength: Option<Strength>,
    pub claude_session_id: Option<String>,
    pub is_primary: bool,
    /// `decided_at`, else `created_at`.
    pub decided_at: i64,
    pub evidence_len: usize,
}

impl ExistingLink {
    /// Made by the resolver (not a decision).
    fn is_auto(&self) -> bool {
        AUTO_SOURCES.contains(&self.source.as_str())
    }

    /// The tier the primary choice ranks it by.
    fn tier(&self) -> Strength {
        if self.is_auto() {
            self.strength.unwrap_or(Strength::Strong)
        } else {
            Strength::Explicit
        }
    }
}

/// Everything one resolution looks at.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResolveInput {
    /// The session's current Claude conversation (its window).
    pub conversation: Option<String>,
    /// Branch candidates; `None` when the branch is not known at all (its
    /// links are then left alone), `Some(vec![])` when it names no work.
    pub branch: Option<Vec<Candidate>>,
    /// PR head and closing-ref candidates; `None` when never probed.
    pub pr: Option<Vec<Candidate>>,
    /// Event candidates: prompt matches, PR text, trailers.
    pub events: Vec<Candidate>,
    /// Live links (every state).
    pub links: Vec<ExistingLink>,
    /// The session's project trusts branch keys (R3 vs R3b).
    pub trusted: bool,
}

/// What a created link is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NewState {
    Suggested,
    Confirmed,
}

/// One change to apply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkChange {
    Create {
        target: String,
        state: NewState,
        source: &'static str,
        strength: Strength,
        rule: &'static str,
        preselected: bool,
        tracker_id: Option<i64>,
        evidence: Vec<Evidence>,
    },
    /// A confirmed auto link whose state signal moved on (R7): it ends, with
    /// a snapshot, and stays as history.
    End { link_id: i64, reason: &'static str },
    /// A suggestion whose state signal moved on: it was never decided, so it
    /// goes.
    Withdraw { link_id: i64 },
    /// A suggestion that now meets an automatic rule.
    Promote {
        link_id: i64,
        rule: &'static str,
        strength: Strength,
        evidence: Vec<Evidence>,
    },
    /// An event suggestion from an earlier conversation, not seen in this
    /// one (R6).
    Decay { link_id: i64 },
    /// Seen again: new evidence, and a suggestion's window moves along.
    Touch {
        link_id: i64,
        evidence: Vec<Evidence>,
        conversation: Option<String>,
        /// Raise a suggestion's strength / pre-selection.
        strength: Option<Strength>,
        preselected: bool,
    },
    /// The session's primary link is now this one (a live link, or the
    /// link just created for `target`).
    Primary(PrimaryRef),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrimaryRef {
    Link(i64),
    Target(String),
    /// No confirmed link is left.
    None,
}

/// One target's candidates merged: the strongest sighting leads, and every
/// sighting is evidence.
#[derive(Debug, Clone)]
struct Merged {
    lead: Candidate,
    evidence: Vec<Evidence>,
}

/// Merge candidates by target, strongest first, evidence together.
fn merge(cands: impl IntoIterator<Item = Candidate>) -> BTreeMap<String, Merged> {
    let mut out: BTreeMap<String, Merged> = BTreeMap::new();
    for c in cands {
        match out.get_mut(&c.target) {
            None => {
                out.insert(
                    c.target.clone(),
                    Merged {
                        evidence: vec![c.evidence.clone()],
                        lead: c,
                    },
                );
            }
            Some(m) => {
                m.evidence.push(c.evidence.clone());
                let (ambiguous, sole, tracker) = (c.ambiguous, c.first_prompt_sole, c.tracker_id);
                if c.strength > m.lead.strength
                    || (c.strength == m.lead.strength && c.signal < m.lead.signal)
                {
                    let old = std::mem::replace(&mut m.lead, c);
                    m.lead.ambiguous |= old.ambiguous;
                    m.lead.first_prompt_sole |= old.first_prompt_sole;
                    m.lead.tracker_id = m.lead.tracker_id.or(old.tracker_id);
                } else {
                    m.lead.ambiguous |= ambiguous;
                    m.lead.first_prompt_sole |= sole;
                    m.lead.tracker_id = m.lead.tracker_id.or(tracker);
                }
            }
        }
    }
    out
}

fn evidence_of(m: &Merged, rule: &str) -> Vec<Evidence> {
    m.evidence
        .iter()
        .cloned()
        .map(|mut e| {
            e.rule = rule.to_string();
            e
        })
        .collect()
}

/// Resolve one session. Deterministic: the same input gives the same changes.
pub fn resolve(input: &ResolveInput) -> Vec<LinkChange> {
    let mut changes = Vec::new();
    let conv = input.conversation.clone();

    // R9: a rejected pair is never proposed again, from any signal.
    let rejected: BTreeSet<&str> = input
        .links
        .iter()
        .filter(|l| l.state == "rejected")
        .map(|l| l.target.as_str())
        .collect();
    let by_target: BTreeMap<&str, &ExistingLink> = input
        .links
        .iter()
        .filter(|l| l.state != "rejected")
        .map(|l| (l.target.as_str(), l))
        .collect();

    // ── state signals ──
    let branch_targets: Option<BTreeSet<String>> = input
        .branch
        .as_ref()
        .map(|v| v.iter().map(|c| c.target.clone()).collect());
    let pr_targets: Option<BTreeSet<String>> = input
        .pr
        .as_ref()
        .map(|v| v.iter().map(|c| c.target.clone()).collect());

    // R7: an auto link a state signal made, whose value moved on.
    let mut gone: BTreeSet<i64> = BTreeSet::new();
    for l in &input.links {
        let current = match l.source.as_str() {
            "branch" => branch_targets.as_ref(),
            // `pr` links from the PR's text are events, but they are only
            // re-derived while the PR exists; a closed / changed PR takes
            // them too.
            "pr" => pr_targets.as_ref(),
            _ => continue,
        };
        let Some(current) = current else { continue };
        let still = current.contains(&l.target)
            || (l.source == "pr"
                && input
                    .events
                    .iter()
                    .any(|e| e.signal == Signal::PrText && e.target == l.target));
        if still {
            continue;
        }
        match l.state.as_str() {
            "confirmed" => {
                let reason = if l.source == "branch" {
                    "branch_changed"
                } else {
                    "pr_changed"
                };
                changes.push(LinkChange::End {
                    link_id: l.id,
                    reason,
                });
                gone.insert(l.id);
            }
            "suggested" => {
                changes.push(LinkChange::Withdraw { link_id: l.id });
                gone.insert(l.id);
            }
            _ => {}
        }
    }

    let state = merge(
        input
            .branch
            .iter()
            .flatten()
            .chain(input.pr.iter().flatten())
            .filter(|c| !rejected.contains(c.target.as_str()))
            .cloned(),
    );
    let strong = state
        .values()
        .filter(|m| m.lead.strength >= Strength::Strong)
        .count();
    let sole = strong == 1;
    let mut touched: BTreeSet<i64> = BTreeSet::new();
    let mut created: Vec<(String, Strength)> = Vec::new();
    for m in state.values() {
        let c = &m.lead;
        let (rule, confirm, pre) = if c.ambiguous {
            ("R8", false, false)
        } else if c.strength < Strength::Strong {
            ("R6", false, false)
        } else if !sole {
            ("R4", false, false)
        } else if input.trusted {
            ("R3", true, false)
        } else {
            ("R3b", false, true)
        };
        match by_target.get(c.target.as_str()) {
            Some(l) if gone.contains(&l.id) => {}
            Some(l) if l.state == "suggested" => {
                touched.insert(l.id);
                if confirm {
                    changes.push(LinkChange::Promote {
                        link_id: l.id,
                        rule,
                        strength: c.strength,
                        evidence: evidence_of(m, rule),
                    });
                } else if l.claude_session_id != conv || l.evidence_len == 0 {
                    changes.push(LinkChange::Touch {
                        link_id: l.id,
                        evidence: evidence_of(m, rule),
                        conversation: conv.clone(),
                        strength: (l.strength < Some(c.strength)).then_some(c.strength),
                        preselected: pre,
                    });
                }
            }
            // Confirmed already (auto or a decision): R1 — nothing.
            Some(l) => {
                touched.insert(l.id);
            }
            None => {
                changes.push(LinkChange::Create {
                    target: c.target.clone(),
                    state: if confirm {
                        NewState::Confirmed
                    } else {
                        NewState::Suggested
                    },
                    source: c.signal.source(),
                    strength: c.strength,
                    rule,
                    preselected: pre,
                    tracker_id: c.tracker_id,
                    evidence: evidence_of(m, rule),
                });
                if confirm {
                    created.push((c.target.clone(), c.strength));
                }
            }
        }
    }

    // ── event signals ──
    let events = merge(
        input
            .events
            .iter()
            .filter(|c| !rejected.contains(c.target.as_str()) && !state.contains_key(&c.target))
            .cloned(),
    );
    for m in events.values() {
        let c = &m.lead;
        let url = c.signal == Signal::PromptUrl;
        // PR text and trailers are re-read from the stored probe on every
        // run: seeing them again is news only in a new window.
        let rederived = matches!(c.signal, Signal::PrText | Signal::Trailer);
        let (rule, confirm, pre) = if c.ambiguous {
            ("R8", false, false)
        } else if url {
            ("R5", c.first_prompt_sole, false)
        } else if c.first_prompt_sole && c.strength >= Strength::Strong {
            ("R5", false, true)
        } else {
            ("R6", false, false)
        };
        match by_target.get(c.target.as_str()) {
            Some(l) if gone.contains(&l.id) => {}
            Some(l) if l.state == "suggested" => {
                touched.insert(l.id);
                if confirm {
                    changes.push(LinkChange::Promote {
                        link_id: l.id,
                        rule,
                        strength: c.strength,
                        evidence: evidence_of(m, rule),
                    });
                } else if !rederived || l.claude_session_id != conv {
                    changes.push(LinkChange::Touch {
                        link_id: l.id,
                        evidence: evidence_of(m, rule),
                        conversation: conv.clone(),
                        strength: (l.strength < Some(c.strength)).then_some(c.strength),
                        preselected: pre,
                    });
                }
            }
            Some(l) => {
                touched.insert(l.id);
            }
            None => {
                changes.push(LinkChange::Create {
                    target: c.target.clone(),
                    state: if confirm {
                        NewState::Confirmed
                    } else {
                        NewState::Suggested
                    },
                    source: c.signal.source(),
                    strength: c.strength,
                    rule,
                    preselected: pre,
                    tracker_id: c.tracker_id,
                    evidence: evidence_of(m, rule),
                });
                if confirm {
                    created.push((c.target.clone(), c.strength));
                }
            }
        }
    }

    // R6: an event suggestion from an earlier window, not seen again, decays
    // at the boundary. State suggestions follow R7 instead.
    if conv.is_some() {
        for l in &input.links {
            let event = matches!(l.source.as_str(), "prompt" | "url" | "trailer");
            if l.state == "suggested"
                && event
                && !touched.contains(&l.id)
                && !gone.contains(&l.id)
                && l.claude_session_id != conv
            {
                changes.push(LinkChange::Decay { link_id: l.id });
                gone.insert(l.id);
            }
        }
    }

    // ── the primary link ──
    // Among live confirmed links: decided in the current conversation first
    // ("the latest conversation decides"), then explicit > strong > weak,
    // then the most recent decision.
    let promoted: BTreeSet<i64> = changes
        .iter()
        .filter_map(|c| match c {
            LinkChange::Promote { link_id, .. } => Some(*link_id),
            _ => None,
        })
        .collect();
    type Rank = (bool, Strength, i64, i64);
    let mut best: Option<(Rank, PrimaryRef)> = None;
    let mut consider = |rank: Rank, r: PrimaryRef| {
        if best.as_ref().is_none_or(|(b, _)| rank > *b) {
            best = Some((rank, r));
        }
    };
    let promoted_strength = |id: i64| {
        changes.iter().find_map(|c| match c {
            LinkChange::Promote {
                link_id, strength, ..
            } if *link_id == id => Some(*strength),
            _ => None,
        })
    };
    for l in &input.links {
        if gone.contains(&l.id) {
            continue;
        }
        if l.state == "confirmed" {
            let current = conv.is_some() && l.claude_session_id == conv;
            consider(
                (current, l.tier(), l.decided_at, l.id),
                PrimaryRef::Link(l.id),
            );
        } else if promoted.contains(&l.id) {
            let tier = promoted_strength(l.id).unwrap_or(Strength::Strong);
            consider(
                (conv.is_some(), tier, i64::MAX, l.id),
                PrimaryRef::Link(l.id),
            );
        }
    }
    for (target, strength) in &created {
        consider(
            (conv.is_some(), *strength, i64::MAX, i64::MAX),
            PrimaryRef::Target(target.clone()),
        );
    }
    let current_primary = input
        .links
        .iter()
        .find(|l| l.is_primary && l.state == "confirmed" && !gone.contains(&l.id))
        .map(|l| l.id);
    // The primary moves only when this run gave a reason: it lost its
    // primary, or it confirmed a link that outranks it. A primary a person
    // or a carry set is otherwise left alone (R1).
    let reason = current_primary.is_none() || !created.is_empty() || !promoted.is_empty();
    match best {
        _ if !reason => {}
        Some((_, PrimaryRef::Link(id))) if Some(id) == current_primary => {}
        Some((_, r)) => changes.push(LinkChange::Primary(r)),
        None => {
            let had = input
                .links
                .iter()
                .any(|l| l.is_primary && l.state == "confirmed");
            if had {
                changes.push(LinkChange::Primary(PrimaryRef::None));
            }
        }
    }
    changes
}

#[cfg(test)]
mod tests;
