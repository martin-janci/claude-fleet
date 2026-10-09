//! Generates the settings documentation from the registry
//! (`service::settings::SPECS`), so a label, a range or a default is written
//! once, in Rust, and the docs cannot drift from it:
//!
//! - `docs/settings-reference.md`: every registered setting, grouped by key
//!   prefix;
//! - the settings table in `docs/work-graph.md` (`work.*`) and in
//!   `docs/decisions.md` (`decide.*`), each between its
//!   `<!-- BEGIN GENERATED: settings <prefix> -->` / `END` markers.
//!
//! The `settings_docs_are_current` test enforces all three. Regenerate after
//! changing any spec with:
//!   REGEN_SETTINGS_DOCS=1 cargo fleet-test -- settings_docs_are_current
//!
//! Entirely `#[cfg(test)]`, like `mcp::doc_gen`.

use super::settings::{Danger, Kind, Restart, Spec, Tag, Unit, SPECS};

const REFERENCE_REL_PATH: &str = "docs/settings-reference.md";

/// The guides that carry one prefix's table, spliced between markers.
const GUIDES: &[(&str, &str)] = &[
    ("docs/work-graph.md", "work."),
    ("docs/decisions.md", "decide."),
];

fn begin_marker(prefix: &str) -> String {
    format!("<!-- BEGIN GENERATED: settings {prefix} -->")
}

fn end_marker(prefix: &str) -> String {
    format!("<!-- END GENERATED: settings {prefix} -->")
}

/// The allowed values of a spec, in words: "1–365 days, `0` = forever".
pub(crate) fn range_text(spec: &Spec) -> String {
    let unit = spec.unit.word();
    let with_unit = |n: String| match spec.unit {
        Unit::None | Unit::Count => n,
        Unit::Percent => format!("{n}%"),
        _ => format!("{n} {unit}"),
    };
    let base = match spec.kind {
        Kind::Bool => "on / off".to_string(),
        Kind::Secs if spec.unit == Unit::Seconds => "seconds".to_string(),
        Kind::Secs => format!("seconds, shown in {unit}"),
        Kind::SecsMin(min) => format!("≥ {min} seconds"),
        Kind::Int { min, max } => with_unit(format!("{min}–{max}")),
        Kind::Choice(options) => options
            .iter()
            .map(|o| format!("`{o}`"))
            .collect::<Vec<_>>()
            .join(" / "),
        Kind::ChoiceSet(options) => format!(
            "any of {}",
            options
                .iter()
                .map(|o| format!("`{o}`"))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Kind::PathMap => "JSON map: host alias → path".to_string(),
        Kind::IdSet => "JSON array of ids".to_string(),
        Kind::PriceMap => "JSON map: model → USD per million tokens".to_string(),
        Kind::Text { max } => format!("text, up to {max} characters"),
        Kind::TimeRange => "a daily time range `HH:MM-HH:MM`, or empty for none".to_string(),
    };
    match spec.zero {
        Some(zero) => format!("{base}, `0` = {zero}"),
        None => base,
    }
}

/// The help text plus the notes a reader needs before changing it.
fn what_it_does(spec: &Spec) -> String {
    let mut out = spec.help.to_string();
    if spec.tags.contains(&Tag::Experimental) {
        out.push_str(" Experimental.");
    }
    match spec.restart {
        Restart::None => {}
        Restart::App => out.push_str(" Applies after a restart."),
        Restart::Hooks => out.push_str(" Applies when the hooks are next installed."),
    }
    if let Danger::Confirm(_) = spec.danger {
        out.push_str(" Asks to confirm.");
    }
    if let Some(how) = spec.owned_by {
        out.push_str(&format!(" Read-only here: change it with {how}."));
    }
    out.replace('|', "\\|")
}

fn render_rows<'a>(specs: impl Iterator<Item = &'a Spec>) -> String {
    let mut out = String::from(
        "| Setting | Default | Range | Scope | What it does |\n|---|---|---|---|---|\n",
    );
    for spec in specs {
        out.push_str(&format!(
            "| `{}` | `{}` | {} | {} | {} |\n",
            spec.key,
            spec.default,
            range_text(spec),
            spec.scope().word(),
            what_it_does(spec)
        ));
    }
    out
}

/// One prefix's table, with its markers, as spliced into a guide.
pub(crate) fn render_table(prefix: &str) -> String {
    format!(
        "{}\n<!-- Generated from service/settings.rs: REGEN_SETTINGS_DOCS=1 cargo fleet-test -- settings_docs_are_current -->\n{}{}",
        begin_marker(prefix),
        render_rows(SPECS.iter().filter(|s| s.key.starts_with(prefix))),
        end_marker(prefix)
    )
}

/// A group heading for a key prefix (the part before the first dot).
fn group_title(prefix: &str) -> &str {
    match prefix {
        "reconcile" => "Reconcile tick",
        "sessions" => "Sessions",
        "restore" => "Restoring lost sessions",
        "playbooks" => "Playbooks",
        "gc" => "Garbage collection",
        "projects" => "Projects",
        "tasks" => "Tasks",
        "repair" => "Workspace repair",
        "move" => "Move to host",
        "usage" => "Usage",
        "reports" => "Error reports",
        "health" => "Health",
        "work" => "Work graph",
        "decide" => "Decisions (Jev)",
        "hub" => "Hub daemon (read-only)",
        "mcp" => "Control API (read-only)",
        other => other,
    }
}

/// The whole of `docs/settings-reference.md`.
pub(crate) fn render_reference() -> String {
    let mut out = String::from(
        "<!-- GENERATED FILE — do not edit by hand.\n     \
Regenerate with: REGEN_SETTINGS_DOCS=1 cargo fleet-test -- settings_docs_are_current -->\n\n\
# Settings reference\n\n\
Every operator setting fleet stores, generated from the registry in \
`crates/fleet-core/src/service/settings.rs`. Change one in Settings on the \
desktop, or over the control API with the master token (`set_setting`); \
`get_settings { describe: true }` returns this same metadata with each \
setting's current value.\n\n\
Scope says where a value lives: *fleet* is one value for the whole fleet, \
kept on the hub when the desktop is paired with one; *fleet, per org* is \
the same, and an org may set its own value that its sessions read instead; \
*per process* is the running app's or hub's own.\n",
    );
    let mut prefixes: Vec<&str> = Vec::new();
    for spec in SPECS {
        let prefix = spec.key.split('.').next().unwrap_or(spec.key);
        if !prefixes.contains(&prefix) {
            prefixes.push(prefix);
        }
    }
    for prefix in prefixes {
        let dotted = format!("{prefix}.");
        out.push_str(&format!("\n## {}\n\n", group_title(prefix)));
        out.push_str(&render_rows(
            SPECS.iter().filter(|s| s.key.starts_with(&dotted)),
        ));
    }
    out
}

/// Replace the block from `prefix`'s begin marker to its end marker (both
/// included) in `doc` with `table`.
pub(crate) fn splice(doc: &str, prefix: &str, table: &str) -> Result<String, String> {
    let begin = begin_marker(prefix);
    let end = end_marker(prefix);
    let start = doc
        .find(&begin)
        .ok_or_else(|| format!("no `{begin}` marker"))?;
    let stop = doc[start..]
        .find(&end)
        .map(|i| start + i + end.len())
        .ok_or_else(|| format!("no `{end}` marker after `{begin}`"))?;
    Ok(format!("{}{}{}", &doc[..start], table, &doc[stop..]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn repo_path(rel: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(rel)
    }

    #[test]
    fn settings_docs_are_current() {
        let regen = std::env::var("REGEN_SETTINGS_DOCS").is_ok();
        let mut stale = Vec::new();

        let reference = render_reference();
        let path = repo_path(REFERENCE_REL_PATH);
        if regen {
            std::fs::write(&path, &reference).expect("write settings-reference.md");
        } else if std::fs::read_to_string(&path).unwrap_or_default() != reference {
            stale.push(REFERENCE_REL_PATH.to_string());
        }

        for (rel, prefix) in GUIDES {
            let path = repo_path(rel);
            let doc = std::fs::read_to_string(&path).expect("read guide");
            let spliced = splice(&doc, prefix, &render_table(prefix))
                .unwrap_or_else(|e| panic!("{rel}: {e}"));
            if regen {
                std::fs::write(&path, &spliced).expect("write guide");
            } else if spliced != doc {
                stale.push(rel.to_string());
            }
        }

        assert!(
            stale.is_empty(),
            "\n\n{} out of date with service/settings.rs. Regenerate with:\n  \
REGEN_SETTINGS_DOCS=1 cargo fleet-test -- settings_docs_are_current\n",
            stale.join(", ")
        );
    }

    #[test]
    fn range_text_reads_the_kind_unit_and_zero() {
        let spec = |key| super::super::settings::spec(key).unwrap();
        assert_eq!(range_text(spec("work.recent_days")), "1–365 days");
        assert_eq!(
            range_text(spec("work.retention.journal_days")),
            "0–3650 days, `0` = forever"
        );
        assert_eq!(
            range_text(spec("gc.bg_idle_secs")),
            "seconds, shown in hours, `0` = never"
        );
        assert_eq!(
            range_text(spec("repair.tick_interval_secs")),
            "≥ 60 seconds"
        );
        assert_eq!(range_text(spec("health.context_red_pct")), "1–100%");
        assert_eq!(
            range_text(spec("decide.jev.status_map")),
            "`off` / `shadow` / `assist`"
        );
        assert_eq!(range_text(spec("gc.enabled")), "on / off");
    }

    #[test]
    fn splice_replaces_only_the_marked_block() {
        let doc = format!(
            "intro\n{}\nold\n{}\noutro\n",
            begin_marker("x."),
            end_marker("x.")
        );
        let out = splice(&doc, "x.", "NEW").unwrap();
        assert_eq!(out, "intro\nNEW\noutro\n");
        assert!(splice("no markers", "x.", "NEW").is_err());
    }
}
