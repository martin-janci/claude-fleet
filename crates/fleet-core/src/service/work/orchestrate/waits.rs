//! Why a mission's ready work waits (mission improvements M3, design
//! 2026-10-26 §3): each gate of the loop's tick that holds back an
//! implement run names its cause, and the tick books them
//! (`Store::account_mission_waits`).

use super::steps::Step;
use std::collections::BTreeSet;

/// The items whose implement run is among `steps`.
pub fn impl_items(steps: &[Step]) -> BTreeSet<i64> {
    steps
        .iter()
        .filter(|st| st.is_impl())
        .filter_map(|st| st.item_id)
        .collect()
}

/// PURE: whether a gate held back an implement run that was in `before`.
pub fn held(before: &BTreeSet<i64>, after: &[Step]) -> bool {
    let after = impl_items(after);
    before.iter().any(|i| !after.contains(i))
}

/// PURE: the cause of the runs the slots left out: `wanted` is the plan
/// with no cap on runs open at once, `planned` the plan under it. A plan
/// that asks a person about its spent attempts waits on the person (the
/// ask says so), not on a slot.
pub fn slot_cause(wanted: &[Step], planned: &[Step]) -> Option<&'static str> {
    if !held(&impl_items(wanted), planned) {
        return None;
    }
    let attempts_spent = planned
        .iter()
        .any(|st| st.kind == "ask" && st.item_id.is_none());
    Some(if attempts_spent { "person" } else { "slot" })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(item: i64) -> Step {
        Step {
            kind: "run".into(),
            item_id: Some(item),
            role: Some("implement".into()),
            reason: String::new(),
            context: None,
            auto: true,
        }
    }

    fn ask() -> Step {
        Step {
            kind: "ask".into(),
            item_id: None,
            role: None,
            reason: "the mission used its 30 attempts".into(),
            context: None,
            auto: false,
        }
    }

    #[test]
    fn a_run_the_cap_left_out_waits_on_a_slot() {
        assert_eq!(slot_cause(&[run(1), run(2)], &[run(1)]), Some("slot"));
        assert_eq!(slot_cause(&[run(1)], &[run(1)]), None);
        assert_eq!(
            slot_cause(&[run(1), run(2)], &[run(1), ask()]),
            Some("person"),
            "spent attempts are a person's call"
        );
    }

    #[test]
    fn a_gate_holds_when_a_run_it_got_is_gone() {
        let before = impl_items(&[run(1), run(2), ask()]);
        assert!(held(&before, &[run(1)]));
        assert!(!held(&before, &[run(2), run(1)]));
    }
}
