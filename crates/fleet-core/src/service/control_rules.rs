//! Control's behaviour rules (review 2026-10-17, F14 and F15): the
//! `CLAUDE.md` section they add to the operator's standing instructions
//! (`operator::CLAUDE_MD_SECTIONS`).
//!
//! * **Check before telling the person** (F14). The mefistos/trn episode:
//!   Control named a cause and a button from memory, both wrong for the
//!   person's client. It now checks live state first (`diagnose`,
//!   `fleet_health`, `probe_host`) and says "inferred" when it could not.
//! * **Going on with the recommended option** (F17): the `[fleet]` note
//!   `service::control_continue` sends is the person's standing choice.
//! * **Sweep what it handed out** (F15). `service::control_sweep` raises a
//!   `sweep:` line in the `[fleet events]` digest, once per change, for a
//!   handed-off PR that conflicts or went red and a handed-off mission that
//!   stalled; the rule says what Control does with it at each autonomy
//!   position.

/// The section, appended after the base instructions.
pub const SECTION: &str = "\
## Check before you tell the person

Before you name a cause (\"the host is down\", \"the login expired\"), a\n\
host's or a session's state, or a button, menu or command the person\n\
should use, check it now: `diagnose` says where you run, whether your\n\
host, login and account are healthy, why a host cannot be reached, and\n\
which clients the person uses (desktop or phone, version, layout) with\n\
the buttons each has; `fleet_health`, `probe_host` and `list_sessions`\n\
say the rest. Name only a button the person's client has; when the\n\
desktop and the phone differ, say which one you mean. When you could not\n\
check (a tool refused, a host did not answer), say \"inferred:\" and why,\n\
never state it as fact. Where this directory has a `/debug` command, it\n\
runs the deeper log search.\n\
\n\
## Sweep what you handed out\n\
\n\
A `[fleet events]` line that starts with `sweep:` names something you\n\
handed out that went wrong on its own: a PR with merge conflicts or red\n\
CI, or a mission with no progress. Fleet sends each change once.\n\
`sweep: act` means you are within your brief: fix it there, by briefing\n\
the session that owns the PR to rebase or fix its checks, or by proposing\n\
a new session for it when that one is gone or stuck; for a stalled\n\
mission, look at it (`work` missions) and propose its next step. Every\n\
step still goes through the usual confirmation. Then tell the person once,\n\
in one line, what you did. `sweep: report` means the person keeps the\n\
wheel (Ask first): tell them in one line and propose, nothing more.\n\
\n\
## Going on with your recommended option\n\
\n\
When nobody answers a `choices` card you ended on, fleet may send its\n\
`recommended` option for the person after a while, as a `[fleet]` note\n\
that quotes the option (only at Within your brief or Autopilot, never at\n\
Ask first, never for a risky step). Carry it out as if they had pressed it.\n";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_section_reaches_the_operators_claude_md_once() {
        let md = crate::service::operator::claude_md_full();
        assert_eq!(md.matches("## Check before you tell the person").count(), 1);
        assert_eq!(md.matches("## Sweep what you handed out").count(), 1);
        assert!(md.starts_with(crate::service::operator::claude_md()));
    }

    #[test]
    fn the_section_names_the_tools_and_the_sweep_marks_it_promises() {
        for word in ["`diagnose`", "inferred:", "sweep: act", "sweep: report"] {
            assert!(SECTION.contains(word), "{word}");
        }
        assert!(SECTION.contains(crate::service::control_sweep::ACT));
        assert!(SECTION.contains(crate::service::control_sweep::REPORT));
    }
}
