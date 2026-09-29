//! Whose words a submitted prompt is: the person's, or Claude Code's own.
//!
//! The UserPromptSubmit hook sees every user turn, and Claude Code submits
//! some of them itself: a background task finishing (`<task-notification>`),
//! a slash command's echo and output (`<command-name>`,
//! `<local-command-stdout>`), a `!` bash line (`<bash-input>`), a reminder
//! (`<system-reminder>`). None of them is something a person typed, so none
//! may be a conversation's first prompt, a work-detection signal, a census
//! text or a benchmark case. [`human_part`] is the one test all of them use
//! (through `work::detect::loop_guard`, which the census and the J1
//! benchmark call via `FleetPrompts`).
//!
//! The rule follows `transcript::parse_conversation`: a harness block is
//! recognised only at the HEAD of the prompt, so a person who pastes a
//! transcript or mentions a tag keeps their prompt.

use std::borrow::Cow;

/// Tags Claude Code opens a user turn with — the ONE list both readers of
/// a user turn use: this module (the hook's prompt, a stored first prompt)
/// and `transcript::parse_conversation` (through [`is_harness_tag`]), so a
/// turn the conversation view folds away is never a conversation's first
/// prompt, and the reverse. Only a listed tag is the harness: an unlisted
/// one, however harness-shaped (`<my-widget>x</my-widget> renders blank`),
/// is the person's text. A listed block that is never closed runs to the
/// end of the prompt: a stored first prompt is cut at 200 characters, and a
/// task notification is longer than that.
///
/// The list is what 600 local transcripts opened user turns with
/// (2026-09-28): `task-notification` by far the most, then
/// `system-reminder` (usually followed by the person's words), the slash
/// command and `!` bash echoes, the IDE's context blocks, and the desktop
/// app's `ci-monitor-event`, `scheduled-task` and `create-pr-command`.
/// `<pasted_content …>` is the person's paste and stays theirs. A tag
/// Claude Code starts writing tomorrow is added here, once, for both.
pub const HARNESS_TAGS: &[&str] = &[
    "task-notification",
    "system-reminder",
    "ci-monitor-event",
    "scheduled-task",
    "create-pr-command",
    "command-name",
    "command-message",
    "command-args",
    "local-command-stdout",
    "local-command-stderr",
    "local-command-caveat",
    "bash-input",
    "bash-stdout",
    "bash-stderr",
    "user-prompt-submit-hook",
    "teammate-message",
    "ide_opened_file",
    "ide_selection",
];

/// Whether `tag` is one Claude Code opens a user turn with
/// ([`HARNESS_TAGS`]). The one predicate: the transcript parser asks it too.
pub fn is_harness_tag(tag: &str) -> bool {
    HARNESS_TAGS.contains(&tag)
}

/// The person's part of `prompt`: the text left once every harness block at
/// its head is taken off, trimmed. `None` when nothing is left — the prompt
/// is Claude Code talking (or it is empty).
///
/// One harness block carries the person's words: a slash command's
/// `<command-args>`. `/fix PAY-7` reaches the hook as
/// `<command-name>/fix</command-name><command-args>PAY-7</command-args>`,
/// and the `PAY-7` is what they typed — so a command with arguments reads
/// back as `/fix PAY-7` (owned, hence the `Cow`), where work detection
/// still sees the key. A command without arguments is the harness's echo.
pub fn human_part(prompt: &str) -> Option<Cow<'_, str>> {
    let mut t = prompt.trim();
    let mut name: Option<&str> = None;
    let mut args: Option<&str> = None;
    while let Some((tag, inner, rest)) = split_harness_block(t) {
        match tag {
            "command-name" => name = Some(inner.trim()),
            "command-args" => args = Some(inner.trim()),
            _ => {}
        }
        t = rest.trim_start();
    }
    let command = args.filter(|a| !a.is_empty()).map(|a| match name {
        Some(n) if !n.is_empty() => format!("{n} {a}"),
        _ => a.to_string(),
    });
    match (command, t.is_empty()) {
        (None, true) => None,
        (None, false) => Some(Cow::Borrowed(t)),
        (Some(c), true) => Some(Cow::Owned(c)),
        (Some(c), false) => Some(Cow::Owned(format!("{c}\n{t}"))),
    }
}

/// A non-empty prompt with nothing of a person's in it.
pub fn is_harness(prompt: &str) -> bool {
    !prompt.trim().is_empty() && human_part(prompt).is_none()
}

/// A stored first prompt as a reader should see it: [`human_part`] of it,
/// owned. Rows written before the hook took harness blocks off (#370) hold
/// a `<task-notification>` or a reminder; every read of
/// `conversations.first_prompt` (the conversation list, the handover
/// brief) goes through this, so no consumer shows Claude Code's words as
/// what the person asked.
pub fn clean_first_prompt(stored: Option<String>) -> Option<String> {
    let p = stored?;
    match human_part(&p)? {
        Cow::Borrowed(b) if b.len() == p.len() => Some(p),
        c => Some(c.into_owned()),
    }
}

/// The listed harness block `text` starts with, as `(tag, what is
/// inside)` — for the transcript parser, which folds a harness-only entry
/// under it.
pub fn head_block(text: &str) -> Option<(&str, &str)> {
    split_harness_block(text).map(|(tag, inner, _)| (tag, inner))
}

/// The harness block `text` starts with: `(tag, what is inside, the text
/// after it)`, or `None` when it does not start with a listed one. An
/// unclosed block runs to the end.
fn split_harness_block(text: &str) -> Option<(&str, &str, &str)> {
    let rest = text.strip_prefix('<')?;
    let name_end = rest
        .find(|c: char| c == '>' || c.is_whitespace())
        .unwrap_or(rest.len());
    let tag = &rest[..name_end];
    if !is_harness_tag(tag) {
        return None;
    }
    let body_at = rest[name_end..]
        .find('>')
        .map(|i| 1 + name_end + i + 1)
        .unwrap_or(text.len());
    let close = format!("</{tag}>");
    match text[body_at..].find(&close) {
        Some(i) => Some((
            tag,
            &text[body_at..body_at + i],
            &text[body_at + i + close.len()..],
        )),
        None => Some((tag, &text[body_at..], "")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Shaped like the ones the production hub stored (2026-09-28), cut at
    /// the 200 characters `conversations.first_prompt` keeps.
    fn stored(p: &str) -> String {
        p.chars().take(200).collect()
    }

    #[test]
    fn what_claude_code_submits_itself_is_not_a_persons() {
        for p in [
            stored(
                "<task-notification>\n<task-type>artifact-watch-lifecycle</task-type>\n\
                 <summary>Stopped watching Artifact \"Release notes\": the artifact was deleted.\
                 </summary>\n<status>stopped</status>\n</task-notification>",
            ),
            stored(
                "<task-notification>\n<task-id>afb11347d54b0e640</task-id>\n\
                 <tool-use-id>toolu_01B2ZchuQiWa4VttKoDRjQUd</tool-use-id>\n\
                 <output-file>/tmp/claude-1000/-home-dev-projects-github-com-acme-api/tasks/\
                 afb11347d54b0e640.output</output-file>\n<status>completed</status>\n\
                 <summary>Agent \"Fix PAY-7\" completed</summary>\n</task-notification>",
            ),
            "<system-reminder>\nThe TodoWrite tool hasn't been used recently.\n</system-reminder>"
                .into(),
            "<command-message>review</command-message>\n<command-name>/review</command-name>\n\
             <command-args></command-args>"
                .into(),
            "<command-name>/clear</command-name>".into(),
            "<local-command-caveat>Caveat: The messages below were generated by the user while \
             running local commands.</local-command-caveat>"
                .into(),
            "<local-command-stdout>Set model to opus</local-command-stdout>".into(),
            "<bash-input>git status</bash-input>".into(),
            "<bash-stdout>On branch main</bash-stdout><bash-stderr></bash-stderr>".into(),
            "  <teammate-message teammate_id=\"lead\">Pick up PAY-7</teammate-message>\n".into(),
            // A long reminder ahead of the person's words: the stored 200
            // characters hold none of them.
            stored(&format!(
                "<system-reminder>\n{}\n</system-reminder>\nfix the PAY-7 retry bug",
                "The task tools haven't been used recently. ".repeat(6)
            )),
            // The desktop app's own, cut at 200 characters.
            stored(&format!(
                "<ci-monitor-event>\nCI finished for pull request #412: 3 checks failed. {}",
                "Read the failing logs and fix them. ".repeat(6)
            )),
            // A known block cut off inside its open tag.
            "<task-notification".into(),
        ] {
            assert!(is_harness(&p), "{p}");
            assert_eq!(human_part(&p).as_deref(), None, "{p}");
        }
    }

    #[test]
    fn a_persons_text_after_the_harness_blocks_is_kept() {
        assert_eq!(
            human_part(
                "<system-reminder>Plan mode is on.</system-reminder>\n\n  fix the PAY-7 retry bug"
            )
            .as_deref(),
            Some("fix the PAY-7 retry bug")
        );
        assert_eq!(
            human_part(
                "<ide_opened_file>The user opened src/pay.rs</ide_opened_file> why does this panic?"
            )
            .as_deref(),
            Some("why does this panic?")
        );
    }

    /// `/fix PAY-7` as the hook receives it: the arguments are what the
    /// person typed, so the key stays visible to work detection.
    #[test]
    fn a_slash_commands_arguments_are_the_persons() {
        for (p, want) in [
            (
                "<command-message>fix</command-message>\n<command-name>/fix</command-name>\n\
                 <command-args>PAY-7</command-args>",
                "/fix PAY-7",
            ),
            (
                "<command-name>/fix</command-name><command-args>PAY-7</command-args>",
                "/fix PAY-7",
            ),
            (
                "<command-args>PAY-7 and the retry</command-args>",
                "PAY-7 and the retry",
            ),
            (
                "<command-name>/fix</command-name><command-args>PAY-7</command-args>\nplease",
                "/fix PAY-7\nplease",
            ),
        ] {
            assert!(!is_harness(p), "{p}");
            assert_eq!(human_part(p).as_deref(), Some(want), "{p}");
        }
    }

    /// Only a LISTED tag is the harness: a closed block of an unlisted,
    /// harness-shaped tag at the head is the person's (a pasted component),
    /// and the transcript parser agrees because it asks the same predicate.
    #[test]
    fn an_unlisted_tag_is_the_persons_even_when_closed() {
        for p in [
            "<my-widget>x</my-widget> renders blank",
            "<my-widget>x</my-widget>",
            "<foo-bar>\nsome config\n</foo-bar>",
        ] {
            assert!(!is_harness(p), "{p}");
            assert_eq!(human_part(p).as_deref(), Some(p), "{p}");
        }
        assert!(is_harness_tag("task-notification"));
        assert!(is_harness_tag("ide_selection"));
        assert!(!is_harness_tag("my-widget"));
        assert!(!is_harness_tag("pasted_content"));
    }

    /// A first prompt stored before #370 is read back without Claude Code's
    /// blocks; a harness-only one reads as none at all.
    #[test]
    fn an_old_stored_first_prompt_is_cleaned_on_read() {
        assert_eq!(
            clean_first_prompt(Some(
                "<task-notification>\n<task-id>a1</task-id>\n<status>completed</status>\n\
                 <summary>Agent \"Fix PAY-7\" completed</summary>\n</task-notification>"
                    .into()
            )),
            None
        );
        assert_eq!(
            clean_first_prompt(Some(
                "<system-reminder>Plan mode.</system-reminder>\nfix the login bug".into()
            ))
            .as_deref(),
            Some("fix the login bug")
        );
        assert_eq!(
            clean_first_prompt(Some("fix the login bug".into())).as_deref(),
            Some("fix the login bug")
        );
        assert_eq!(clean_first_prompt(None), None);
    }

    #[test]
    fn a_persons_prompt_stays_a_prompt() {
        for p in [
            "Fix the login bug on mobile",
            "Pozri sa preco padá build na main",
            // A pasted HTML element or config, a tag in the middle.
            "<div>hello</div> is rendered twice, why?",
            "<my_config>on</my_config> — is this valid?",
            "<my-widget> renders blank on Safari",
            "<pasted_content id=\"1\">SELECT * FROM orders",
            "look at this: <task-notification>…</task-notification>",
            "[claude-fleet: message from task #3] done",
        ] {
            assert!(!is_harness(p), "{p}");
            assert_eq!(human_part(p).as_deref(), Some(p.trim()), "{p}");
        }
        assert!(!is_harness(""));
        assert!(!is_harness("   "));
        assert_eq!(human_part("  ").as_deref(), None);
    }
}
