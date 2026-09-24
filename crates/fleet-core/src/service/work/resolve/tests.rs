//! Table tests over [`resolve`]: the rules of design §0.3, one situation per
//! row. Every row states the links, the signals and the changes expected.

use super::*;

const CONV: &str = "conv-2";

fn ev(signal: Signal, text: &str) -> Evidence {
    Evidence {
        signal,
        rule: String::new(),
        text: text.into(),
        snippet: None,
        at: 100,
        conversation: Some(CONV.into()),
        note: None,
    }
}

fn cand(target: &str, signal: Signal, strength: Strength) -> Candidate {
    Candidate {
        target: target.into(),
        signal,
        strength,
        ambiguous: false,
        first_prompt_sole: false,
        tracker_id: None,
        untracked: false,
        evidence: ev(signal, target),
    }
}

fn branch(target: &str) -> Candidate {
    cand(target, Signal::Branch, Strength::Strong)
}

fn link(id: i64, target: &str, state: &str, source: &str) -> ExistingLink {
    ExistingLink {
        id,
        target: target.into(),
        state: state.into(),
        source: source.into(),
        strength: AUTO_SOURCES.contains(&source).then_some(Strength::Strong),
        claude_session_id: Some(CONV.into()),
        is_primary: false,
        decided_at: 10 + id,
        evidence_len: 1,
    }
}

fn primary(mut l: ExistingLink) -> ExistingLink {
    l.is_primary = true;
    l
}

fn old_window(mut l: ExistingLink) -> ExistingLink {
    l.claude_session_id = Some("conv-1".into());
    l
}

fn input() -> ResolveInput {
    ResolveInput {
        conversation: Some(CONV.into()),
        branch: Some(vec![]),
        pr: None,
        events: vec![],
        links: vec![],
        trusted: false,
    }
}

/// `(kind, target-or-id, state/rule)` summary of each change, for readable
/// table assertions.
fn summary(changes: &[LinkChange]) -> Vec<String> {
    changes
        .iter()
        .map(|c| match c {
            LinkChange::Create {
                target,
                state,
                rule,
                preselected,
                source,
                ..
            } => format!(
                "create {target} {} {rule} {source}{}",
                match state {
                    NewState::Suggested => "suggested",
                    NewState::Confirmed => "confirmed",
                },
                if *preselected { " pre" } else { "" }
            ),
            LinkChange::End { link_id, reason } => format!("end {link_id} {reason}"),
            LinkChange::Withdraw { link_id } => format!("withdraw {link_id}"),
            LinkChange::Promote { link_id, rule, .. } => format!("promote {link_id} {rule}"),
            LinkChange::Decay { link_id } => format!("decay {link_id}"),
            LinkChange::Touch {
                link_id,
                preselected,
                ..
            } => format!("touch {link_id}{}", if *preselected { " pre" } else { "" }),
            LinkChange::Primary(PrimaryRef::Link(id)) => format!("primary {id}"),
            LinkChange::Primary(PrimaryRef::Target(t)) => format!("primary {t}"),
            LinkChange::Primary(PrimaryRef::None) => "primary none".into(),
        })
        .collect()
}

fn run(i: ResolveInput) -> Vec<String> {
    summary(&resolve(&i))
}

#[test]
fn the_rule_table() {
    struct Row {
        name: &'static str,
        input: ResolveInput,
        expect: &'static [&'static str],
    }
    let rows = vec![
        Row {
            name: "R3: one branch key in a trusted project is confirmed and primary",
            input: ResolveInput {
                branch: Some(vec![branch("ABC-123")]),
                trusted: true,
                ..input()
            },
            expect: &["create ABC-123 confirmed R3 branch", "primary ABC-123"],
        },
        Row {
            name: "R3b: the same in an untrusted project is a pre-selected suggestion",
            input: ResolveInput {
                branch: Some(vec![branch("ABC-123")]),
                ..input()
            },
            expect: &["create ABC-123 suggested R3b branch pre"],
        },
        Row {
            name: "R4: a branch key and a different PR closing ref are both suggested",
            input: ResolveInput {
                branch: Some(vec![branch("ABC-1")]),
                pr: Some(vec![cand("o/r#42", Signal::PrClosing, Strength::Strong)]),
                trusted: true,
                ..input()
            },
            expect: &[
                "create ABC-1 suggested R4 branch",
                "create o/r#42 suggested R4 pr",
            ],
        },
        Row {
            name: "the branch and the PR head naming one key are one candidate",
            input: ResolveInput {
                branch: Some(vec![branch("ABC-1")]),
                pr: Some(vec![cand("ABC-1", Signal::PrHead, Strength::Strong)]),
                trusted: true,
                ..input()
            },
            expect: &["create ABC-1 confirmed R3 branch", "primary ABC-1"],
        },
        Row {
            name: "R3u: a sole closing ref no tracker can resolve is only suggested, even trusted",
            input: ResolveInput {
                pr: Some(vec![Candidate {
                    untracked: true,
                    ..cand("o/r#42", Signal::PrClosing, Strength::Strong)
                }]),
                trusted: true,
                ..input()
            },
            expect: &["create o/r#42 suggested R3u pr pre"],
        },
        Row {
            name: "R3u: a tracked sighting of the same target makes it tracked again",
            input: ResolveInput {
                pr: Some(vec![
                    Candidate {
                        untracked: true,
                        ..cand("o/r#42", Signal::PrClosing, Strength::Strong)
                    },
                    cand("o/r#42", Signal::PrHead, Strength::Strong),
                ]),
                trusted: true,
                ..input()
            },
            expect: &["create o/r#42 confirmed R3 pr", "primary o/r#42"],
        },
        Row {
            name: "R8: a key two trackers claim is never automatic",
            input: ResolveInput {
                branch: Some(vec![Candidate {
                    ambiguous: true,
                    ..branch("ABC-1")
                }]),
                trusted: true,
                ..input()
            },
            expect: &["create ABC-1 suggested R8 branch"],
        },
        Row {
            name: "R7: a branch change ends the auto link and makes the new one",
            input: ResolveInput {
                branch: Some(vec![branch("ABC-130")]),
                links: vec![primary(link(1, "ABC-123", "confirmed", "branch"))],
                trusted: true,
                ..input()
            },
            expect: &[
                "end 1 branch_changed",
                "create ABC-130 confirmed R3 branch",
                "primary ABC-130",
            ],
        },
        Row {
            name: "R7: a branch change never ends a manual link, which stays primary",
            input: ResolveInput {
                branch: Some(vec![branch("ABC-130")]),
                links: vec![
                    primary(link(1, "PAY-7", "confirmed", "manual")),
                    link(2, "ABC-123", "confirmed", "branch"),
                ],
                trusted: true,
                ..input()
            },
            expect: &["end 2 branch_changed", "create ABC-130 confirmed R3 branch"],
        },
        Row {
            name: "R7: started and agent links survive a branch with no key",
            input: ResolveInput {
                branch: Some(vec![]),
                links: vec![
                    primary(link(1, "ABC-1", "confirmed", "started")),
                    link(2, "ABC-2", "confirmed", "agent"),
                ],
                ..input()
            },
            expect: &[],
        },
        Row {
            name: "R7: a suggestion from the old branch is withdrawn, not ended",
            input: ResolveInput {
                branch: Some(vec![]),
                links: vec![link(1, "ABC-1", "suggested", "branch")],
                ..input()
            },
            expect: &["withdraw 1"],
        },
        Row {
            name: "an unknown branch leaves branch links alone",
            input: ResolveInput {
                branch: None,
                links: vec![primary(link(1, "ABC-1", "confirmed", "branch"))],
                ..input()
            },
            expect: &[],
        },
        Row {
            name: "R9: a rejected branch key is never proposed again",
            input: ResolveInput {
                branch: Some(vec![branch("ABC-1")]),
                links: vec![link(1, "ABC-1", "rejected", "manual")],
                trusted: true,
                ..input()
            },
            expect: &[],
        },
        Row {
            name: "R9: a rejected key is not proposed from a prompt either",
            input: ResolveInput {
                events: vec![Candidate {
                    first_prompt_sole: true,
                    ..cand("ABC-1", Signal::PromptUrl, Strength::Strong)
                }],
                links: vec![link(1, "ABC-1", "rejected", "manual")],
                ..input()
            },
            expect: &[],
        },
        Row {
            name: "R5: a sole URL in a first prompt is confirmed",
            input: ResolveInput {
                events: vec![Candidate {
                    first_prompt_sole: true,
                    ..cand("ABC-9", Signal::PromptUrl, Strength::Strong)
                }],
                ..input()
            },
            expect: &["create ABC-9 confirmed R5 url", "primary ABC-9"],
        },
        Row {
            name: "R5: a URL later in the conversation is a suggestion",
            input: ResolveInput {
                events: vec![cand("ABC-9", Signal::PromptUrl, Strength::Strong)],
                ..input()
            },
            expect: &["create ABC-9 suggested R5 url"],
        },
        Row {
            name: "R5: a sole key in a first prompt is a pre-selected suggestion",
            input: ResolveInput {
                events: vec![Candidate {
                    first_prompt_sole: true,
                    ..cand("ABC-99", Signal::PromptKey, Strength::Strong)
                }],
                ..input()
            },
            expect: &["create ABC-99 suggested R5 prompt pre"],
        },
        Row {
            name: "R6: a key later in a conversation is a weak suggestion",
            input: ResolveInput {
                events: vec![cand("ABC-99", Signal::PromptKey, Strength::Weak)],
                ..input()
            },
            expect: &["create ABC-99 suggested R6 prompt"],
        },
        Row {
            name: "R6: seen again, a suggestion is touched, not duplicated",
            input: ResolveInput {
                events: vec![cand("ABC-99", Signal::PromptKey, Strength::Weak)],
                links: vec![old_window(link(4, "ABC-99", "suggested", "prompt"))],
                ..input()
            },
            expect: &["touch 4"],
        },
        Row {
            name: "R6: an event suggestion of an earlier window decays",
            input: ResolveInput {
                links: vec![
                    old_window(link(4, "ABC-99", "suggested", "prompt")),
                    link(5, "ABC-98", "suggested", "prompt"),
                ],
                ..input()
            },
            expect: &["decay 4"],
        },
        Row {
            name: "a confirmed link is never decayed or touched by a prompt",
            input: ResolveInput {
                events: vec![cand("ABC-1", Signal::PromptKey, Strength::Weak)],
                links: vec![primary(old_window(link(1, "ABC-1", "confirmed", "prompt")))],
                ..input()
            },
            expect: &[],
        },
        Row {
            name: "a key that becomes sole promotes its trusted suggestion (R4 → R3)",
            input: ResolveInput {
                branch: Some(vec![branch("ABC-1")]),
                links: vec![link(1, "ABC-1", "suggested", "branch")],
                trusted: true,
                ..input()
            },
            expect: &["promote 1 R3", "primary 1"],
        },
        Row {
            name: "PR text re-read in the same window is not news",
            input: ResolveInput {
                pr: Some(vec![]),
                events: vec![cand("ABC-5", Signal::PrText, Strength::Weak)],
                links: vec![link(1, "ABC-5", "suggested", "pr")],
                ..input()
            },
            expect: &[],
        },
        Row {
            name: "a closed PR withdraws the suggestions its text made",
            input: ResolveInput {
                pr: Some(vec![]),
                links: vec![link(1, "ABC-5", "suggested", "pr")],
                ..input()
            },
            expect: &["withdraw 1"],
        },
        Row {
            name: "primary: the latest conversation's decision wins over an older manual one",
            input: ResolveInput {
                events: vec![Candidate {
                    first_prompt_sole: true,
                    ..cand("NEW-1", Signal::PromptUrl, Strength::Strong)
                }],
                links: vec![primary(old_window(link(1, "OLD-1", "confirmed", "manual")))],
                ..input()
            },
            expect: &["create NEW-1 confirmed R5 url", "primary NEW-1"],
        },
        Row {
            name: "primary: in one window, explicit beats strong",
            input: ResolveInput {
                branch: Some(vec![branch("ABC-2")]),
                links: vec![primary(link(1, "ABC-1", "confirmed", "manual"))],
                trusted: true,
                ..input()
            },
            expect: &["create ABC-2 confirmed R3 branch"],
        },
        Row {
            name: "primary: ending the primary auto link hands primary to what is left",
            input: ResolveInput {
                branch: Some(vec![]),
                links: vec![
                    primary(link(1, "ABC-1", "confirmed", "branch")),
                    old_window(link(2, "PAY-1", "confirmed", "manual")),
                ],
                ..input()
            },
            expect: &["end 1 branch_changed", "primary 2"],
        },
        Row {
            name: "primary: nothing left clears it",
            input: ResolveInput {
                branch: Some(vec![]),
                links: vec![primary(link(1, "ABC-1", "confirmed", "branch"))],
                ..input()
            },
            expect: &["end 1 branch_changed", "primary none"],
        },
        Row {
            name: "idempotent: the same state again changes nothing",
            input: ResolveInput {
                branch: Some(vec![branch("ABC-1")]),
                links: vec![primary(link(1, "ABC-1", "confirmed", "branch"))],
                trusted: true,
                ..input()
            },
            expect: &[],
        },
    ];
    for r in rows {
        assert_eq!(run(r.input), r.expect, "{}", r.name);
    }
}

#[test]
fn evidence_names_the_rule_and_keeps_every_sighting() {
    let changes = resolve(&ResolveInput {
        branch: Some(vec![branch("ABC-1")]),
        pr: Some(vec![cand("ABC-1", Signal::PrHead, Strength::Strong)]),
        trusted: true,
        ..input()
    });
    let LinkChange::Create { evidence, .. } = &changes[0] else {
        panic!("{changes:?}");
    };
    assert_eq!(evidence.len(), 2);
    assert!(evidence.iter().all(|e| e.rule == "R3"));
    assert_eq!(evidence[0].signal, Signal::Branch);
    assert_eq!(evidence[1].signal, Signal::PrHead);
}

#[test]
fn it_is_deterministic() {
    let i = ResolveInput {
        branch: Some(vec![branch("B-1"), branch("A-1")]),
        events: vec![
            cand("C-1", Signal::PromptKey, Strength::Weak),
            cand("D-1", Signal::PromptUrl, Strength::Strong),
        ],
        links: vec![old_window(link(9, "E-1", "suggested", "prompt"))],
        ..input()
    };
    let first = resolve(&i);
    for _ in 0..5 {
        assert_eq!(resolve(&i), first);
    }
}
