//! A work link's confidence as a number (redesign step 6.5): what Review
//! shows as a percentage, and what "Confirm all high-confidence" selects by.
//!
//! The resolver decides in tiers, not scores (`resolve`'s module doc), and
//! this does not change that: the number is read off what the resolver
//! already stored on the link — the rule that made it, its strength, and
//! how many different signals its evidence names. Pure, so the same link
//! always reads the same; nothing is learned and nothing is stored.

use std::collections::BTreeSet;

/// A link at or above this is "high confidence": Review's
/// "Confirm all high-confidence" ticks exactly these. Mirrored in
/// `src/lib/work.ts` (`HIGH_CONFIDENCE_PCT`).
pub const HIGH_CONFIDENCE: u8 = 85;

/// What one corroborating signal adds, and the most they add together.
const CORROBORATION_STEP: u8 = 3;
const CORROBORATION_MAX: u8 = 9;

/// The score of the situation a rule names, before corroboration. `None`
/// for a link no resolver rule made (older links, carries): its strength
/// decides instead.
fn rule_base(rule: &str, strength: Option<&str>) -> Option<u8> {
    Some(match rule {
        // One strong state signal in a trusted project: linked by itself.
        "R3" => 95,
        // The same, untrusted: shown ticked.
        "R3b" => 90,
        // A ticket URL in a prompt (confirmed when it was the sole
        // candidate of a conversation's first prompt), or a strong key that
        // was: a prompt's own words, read at face value.
        "R5" if strength == Some("strong") => 85,
        "R5" => 80,
        // One strong state signal that no tracker can resolve.
        "R3u" => 75,
        // Several strong state signals compete; the agent's own guess.
        "R4" | "R11" => 60,
        // Two trackers claim the key.
        "R8" => 40,
        // A key in passing: prompt, trailer, `#n`.
        "R6" => 35,
        _ => return None,
    })
}

fn strength_base(strength: Option<&str>) -> u8 {
    match strength {
        Some("strong") => 80,
        Some("inferred") => 60,
        _ => 35,
    }
}

/// The link's confidence, 0–100. A person's decision (or an agent's
/// explicit one) is 100; anything the resolver proposed stays below it,
/// however much evidence agrees — a guess never reads as certain.
pub fn confidence(
    source: &str,
    strength: Option<&str>,
    rule: Option<&str>,
    evidence: &[serde_json::Value],
) -> u8 {
    let auto = super::resolve::AUTO_SOURCES.contains(&source);
    if !auto || strength == Some("explicit") {
        return 100;
    }
    let base = rule
        .and_then(|r| rule_base(r, strength))
        .unwrap_or_else(|| strength_base(strength));
    // Every signal beyond the first that saw the same target corroborates
    // it: a branch AND a PR AND a prompt beat a branch alone.
    let signals: BTreeSet<&str> = evidence
        .iter()
        .filter_map(|e| e.get("signal").and_then(|s| s.as_str()))
        .collect();
    let extra = u8::try_from(signals.len().saturating_sub(1)).unwrap_or(u8::MAX);
    let bonus = extra
        .saturating_mul(CORROBORATION_STEP)
        .min(CORROBORATION_MAX);
    base.saturating_add(bonus).min(99)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ev(signals: &[&str]) -> Vec<serde_json::Value> {
        signals.iter().map(|s| json!({ "signal": s })).collect()
    }

    /// The scores Review shows, pinned: changing one is a visible change.
    #[test]
    fn detection_scores_are_pinned() {
        let one = ev(&["branch"]);
        let cases: &[(&str, Option<&str>, Option<&str>, u8)] = &[
            ("branch", Some("strong"), Some("R3"), 95),
            ("branch", Some("strong"), Some("R3b"), 90),
            ("url", Some("strong"), Some("R5"), 85),
            ("prompt", Some("weak"), Some("R5"), 80),
            ("pr", Some("strong"), Some("R3u"), 75),
            ("branch", Some("strong"), Some("R4"), 60),
            ("agent_inferred", Some("inferred"), Some("R11"), 60),
            ("prompt", Some("weak"), Some("R8"), 40),
            ("prompt", Some("weak"), Some("R6"), 35),
            ("trailer", Some("weak"), Some("R6"), 35),
            // No rule recorded: the strength decides.
            ("branch", Some("strong"), None, 80),
            ("agent_inferred", Some("inferred"), None, 60),
            ("prompt", Some("weak"), None, 35),
            ("prompt", None, None, 35),
            // A decision is certain.
            ("started", Some("explicit"), None, 100),
            ("agent", None, None, 100),
            ("manual", Some("strong"), Some("R3"), 100),
            ("branch", Some("explicit"), Some("R3"), 100),
        ];
        for (source, strength, rule, want) in cases {
            assert_eq!(
                confidence(source, *strength, *rule, &one),
                *want,
                "{source} {strength:?} {rule:?}"
            );
        }
    }

    #[test]
    fn other_signals_corroborate_up_to_a_cap_and_never_reach_certain() {
        let c = |sigs: &[&str]| confidence("prompt", Some("weak"), Some("R6"), &ev(sigs));
        assert_eq!(c(&[]), 35);
        assert_eq!(c(&["prompt_key"]), 35);
        // The same signal again is not corroboration.
        assert_eq!(c(&["prompt_key", "prompt_key", "prompt_key"]), 35);
        assert_eq!(c(&["prompt_key", "trailer"]), 38);
        assert_eq!(c(&["prompt_key", "trailer", "pr_text"]), 41);
        assert_eq!(
            c(&["prompt_key", "trailer", "pr_text", "branch", "pr_head"]),
            44
        );
        let top = confidence(
            "branch",
            Some("strong"),
            Some("R3"),
            &ev(&["branch", "pr_head", "pr_closing", "trailer"]),
        );
        assert_eq!(top, 99);
    }

    #[test]
    fn high_confidence_takes_a_sole_strong_signal_and_leaves_a_contest() {
        let one = ev(&["branch"]);
        assert!(confidence("branch", Some("strong"), Some("R3b"), &one) >= HIGH_CONFIDENCE);
        assert!(confidence("url", Some("strong"), Some("R5"), &one) >= HIGH_CONFIDENCE);
        assert!(confidence("branch", Some("strong"), Some("R4"), &one) < HIGH_CONFIDENCE);
        assert!(confidence("pr", Some("strong"), Some("R3u"), &one) < HIGH_CONFIDENCE);
        assert!(
            confidence("agent_inferred", Some("inferred"), Some("R11"), &one) < HIGH_CONFIDENCE
        );
    }
}
