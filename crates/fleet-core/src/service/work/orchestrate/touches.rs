//! Work that would collide does not run side by side (lanes design
//! 2026-10-23 §4.1, §5.4, L5). A task's touches are the paths it says it
//! will change (`work_items.touches_json`, from the planner's `create_item`).
//!
//! * **Item execution** ([`gate_items`]): an implement run whose touches
//!   meet those of a run already open in the same repository waits. A task
//!   that names no touches is not held: nothing is known about it, and
//!   holding it would end today's parallel runs (a deviation from §5.4,
//!   which assumed an overlap).
//! * **Lane execution** ([`gate_lane_starts`]): a lane already under way
//!   goes on. A lane not yet started waits while another lane in the same
//!   repository is under way, unless both name the touches of every task
//!   and they do not meet: two branches of one repository otherwise end in
//!   an integration (§4.1). Lanes in different repositories never wait on
//!   each other.
//!
//! A repository that is not known (`None`) is taken as the same as any.

use std::collections::{HashMap, HashSet};

use super::steps::Step;
use crate::store::touches_overlap;

fn same_repo(a: Option<i64>, b: Option<i64>) -> bool {
    a.is_none() || b.is_none() || a == b
}

/// What a task is known to change: its repository and its touches.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ItemTouch {
    pub project: Option<i64>,
    pub touches: Vec<String>,
}

/// PURE: item execution's implement runs, without those whose touches meet
/// a run already open (`running`, by item) or one taken earlier in this
/// pass, in the same repository. `only` narrows the gate to some items (a
/// lane mission's tasks in no lane); `None` gates every step.
pub fn gate_items(
    steps: Vec<Step>,
    known: &HashMap<i64, ItemTouch>,
    running: &[i64],
    only: Option<&HashSet<i64>>,
) -> Vec<Step> {
    let mut held: Vec<ItemTouch> = running
        .iter()
        .filter_map(|i| known.get(i))
        .filter(|t| !t.touches.is_empty())
        .cloned()
        .collect();
    steps
        .into_iter()
        .filter(|st| {
            if !st.is_impl() {
                return true;
            }
            let Some(item) = st.item_id else {
                return true;
            };
            if only.is_some_and(|o| !o.contains(&item)) {
                return true;
            }
            let Some(t) = known.get(&item).filter(|t| !t.touches.is_empty()) else {
                return true;
            };
            if held
                .iter()
                .any(|h| same_repo(t.project, h.project) && touches_overlap(&t.touches, &h.touches))
            {
                return false;
            }
            held.push(t.clone());
            true
        })
        .collect()
}

/// A lane as the start gate sees it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LaneTouch {
    pub id: i64,
    pub project: Option<i64>,
    /// Every path its tasks name.
    pub touches: Vec<String>,
    /// Every task of it names its touches.
    pub complete: bool,
    /// Started (a lead, or a task attempted) and not done.
    pub under_way: bool,
}

/// PURE: lane execution's implement runs, without those of a lane not yet
/// under way that would collide with one that is (module docs).
pub fn gate_lane_starts(
    steps: Vec<Step>,
    lane_of: &HashMap<i64, i64>,
    lanes: &[LaneTouch],
) -> Vec<Step> {
    let mut under_way: Vec<LaneTouch> = lanes.iter().filter(|l| l.under_way).cloned().collect();
    steps
        .into_iter()
        .filter(|st| {
            if !st.is_impl() {
                return true;
            }
            let Some(lane) = st
                .item_id
                .and_then(|i| lane_of.get(&i))
                .and_then(|id| lanes.iter().find(|l| l.id == *id))
            else {
                return true;
            };
            if under_way.iter().any(|u| u.id == lane.id) {
                return true;
            }
            let collides = under_way.iter().any(|u| {
                same_repo(lane.project, u.project)
                    && (!lane.complete || !u.complete || touches_overlap(&lane.touches, &u.touches))
            });
            if collides {
                return false;
            }
            under_way.push(lane.clone());
            true
        })
        .collect()
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

    fn items(steps: &[Step]) -> Vec<i64> {
        steps.iter().filter_map(|s| s.item_id).collect()
    }

    fn t(project: i64, paths: &[&str]) -> ItemTouch {
        ItemTouch {
            project: Some(project),
            touches: paths.iter().map(|p| p.to_string()).collect(),
        }
    }

    #[test]
    fn a_task_waits_for_one_that_changes_the_same_files() {
        let known = HashMap::from([
            (1, t(10, &["src/store/**"])),
            (2, t(10, &["src/store/mod.rs"])),
            (3, t(10, &["src/ui"])),
            (4, t(11, &["src/store/**"])),
        ]);
        // 1 runs: 2 meets it, 3 does not, 4 is another repository, 5 says
        // nothing and is not held.
        let got = gate_items(vec![run(2), run(3), run(4), run(5)], &known, &[1], None);
        assert_eq!(items(&got), [3, 4, 5]);
        // In one pass, the first of two that meet goes.
        let got = gate_items(vec![run(1), run(2)], &known, &[], None);
        assert_eq!(items(&got), [1]);
        // Narrowed to some items, the rest pass.
        let only = HashSet::from([3]);
        let got = gate_items(vec![run(2)], &known, &[1], Some(&only));
        assert_eq!(items(&got), [2]);
    }

    fn lane(id: i64, project: i64, paths: &[&str], complete: bool, under_way: bool) -> LaneTouch {
        LaneTouch {
            id,
            project: Some(project),
            touches: paths.iter().map(|p| p.to_string()).collect(),
            complete,
            under_way,
        }
    }

    #[test]
    fn a_lane_starts_beside_another_of_its_repo_only_when_both_are_known_apart() {
        let lane_of = HashMap::from([(1, 100), (2, 200), (3, 300), (4, 400)]);
        let lanes = [
            lane(100, 10, &["src/store"], true, true),
            lane(200, 10, &["src/ui"], true, false),
            lane(300, 10, &["src/ui"], false, false),
            lane(400, 11, &[], false, false),
        ];
        // 100 goes on; 200 is known apart; 300 does not say all it
        // changes; 400 is another repository.
        let got = gate_lane_starts(vec![run(1), run(2), run(3), run(4)], &lane_of, &lanes);
        assert_eq!(items(&got), [1, 2, 4]);
        // Two new lanes that meet: the first starts.
        let lanes = [
            lane(100, 10, &["src/store"], true, false),
            lane(200, 10, &["src/store/mod.rs"], true, false),
        ];
        let got = gate_lane_starts(vec![run(1), run(2)], &lane_of, &lanes);
        assert_eq!(items(&got), [1]);
    }
}
