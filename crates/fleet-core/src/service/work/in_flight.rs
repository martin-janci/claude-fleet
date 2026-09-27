//! The in-flight registry of work keys a `start` or `resume` is spawning a
//! session for (work graph M14.1a). The store lock is not held across a
//! spawn (an SSH round trip), so the guards `plan_start` and `plan_resume`
//! check could let two callers — desktop and phone, a double click — each
//! spawn a session for one key. A claim taken under the store lock, after
//! the guards are re-checked and before the spawn, and held until the new
//! session is linked, turns the second into `E_EXISTS` before it spawns
//! anything.
//!
//! A claim is for a whole key, or (a multi-repo start's sibling) for a key
//! in one project. Two claims clash when their keys are equal and either
//! is whole-key or both name the same project, so the siblings of one
//! multi-repo start never fence each other, but a start or resume of the
//! whole key fences them all.

use crate::ipc_error::{codes, IpcError};
use crate::store::Store;
use std::sync::Mutex;

/// Who holds a claim, for the refusal's sentence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verb {
    Start,
    Resume,
}

impl Verb {
    fn ing(self) -> &'static str {
        match self {
            Verb::Start => "started",
            Verb::Resume => "resumed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    /// `Store::instance_id`: one fleet's registry from another's (which only
    /// matters to tests — and, unlike the store's address, is never reused
    /// by a store built where a dropped one was).
    store: u64,
    key: String,
    project: Option<i64>,
    verb: Verb,
}

impl Entry {
    fn clashes(&self, store: u64, key: &str, project: Option<i64>) -> bool {
        self.store == store
            && self.key == key
            && match (self.project, project) {
                (Some(a), Some(b)) => a == b,
                _ => true,
            }
    }
}

static IN_FLIGHT: Mutex<Vec<Entry>> = Mutex::new(Vec::new());

/// One key's claim; released on drop, whatever the start's or resume's
/// outcome.
#[derive(Debug)]
pub struct Claim(Entry);

impl Claim {
    /// Claim `key` (in `project`, for a multi-repo sibling) for `verb`.
    /// `E_EXISTS` when a clashing claim is held. Call it under the store
    /// lock, right after the re-checked guards.
    pub fn take(
        store: &Store,
        key: &str,
        project: Option<i64>,
        verb: Verb,
    ) -> Result<Self, IpcError> {
        let mut set = IN_FLIGHT
            .lock()
            .map_err(|_| IpcError::new(codes::E_LOCK, "the in-flight registry is poisoned"))?;
        let id = store.instance_id();
        if let Some(held) = set.iter().find(|e| e.clashes(id, key, project)) {
            return Err(IpcError::new(
                codes::E_EXISTS,
                format!(
                    "{key} is being {} already; wait for that session, then jump to it",
                    held.verb.ing()
                ),
            ));
        }
        let entry = Entry {
            store: id,
            key: key.to_string(),
            project,
            verb,
        };
        set.push(entry.clone());
        Ok(Claim(entry))
    }
}

impl Drop for Claim {
    fn drop(&mut self) {
        if let Ok(mut set) = IN_FLIGHT.lock() {
            if let Some(i) = set.iter().position(|e| *e == self.0) {
                set.swap_remove(i);
            }
        }
    }
}

/// Whether any claim is held for `store` (tests: every claim is released).
#[cfg(test)]
pub fn any_held(store: &Store) -> bool {
    let id = store.instance_id();
    IN_FLIGHT.lock().unwrap().iter().any(|e| e.store == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The registry is keyed by the store's instance id, never its address:
    /// two stores never alias — not even one built where a dropped one was —
    /// and a claim is released on drop.
    #[test]
    fn the_in_flight_registry_never_aliases_two_stores() {
        let a = Store::open_in_memory().unwrap();
        let first = Claim::take(&a, "ABC-1", None, Verb::Resume).unwrap();
        assert_eq!(
            Claim::take(&a, "ABC-1", None, Verb::Resume)
                .unwrap_err()
                .code,
            codes::E_EXISTS
        );
        let b = Store::open_in_memory().unwrap();
        assert_ne!(a.instance_id(), b.instance_id());
        let _other = Claim::take(&b, "ABC-1", None, Verb::Resume)
            .expect("another store's key is not this one's");
        drop(first);
        let again = Claim::take(&a, "ABC-1", None, Verb::Resume).expect("released on drop");
        let a_id = a.instance_id();
        drop(again);
        drop(a);
        let c = Store::open_in_memory().unwrap();
        assert_ne!(c.instance_id(), a_id, "an id is never reused");
        let _fresh =
            Claim::take(&c, "ABC-1", None, Verb::Resume).expect("a new store starts clean");
    }

    /// A start and a resume of one key fence each other, and the refusal
    /// says which one is in flight.
    #[test]
    fn a_start_and_a_resume_of_one_key_fence_each_other() {
        let s = Store::open_in_memory().unwrap();
        let start = Claim::take(&s, "ABC-1", None, Verb::Start).unwrap();
        let err = Claim::take(&s, "ABC-1", None, Verb::Resume).unwrap_err();
        assert_eq!(err.code, codes::E_EXISTS);
        assert!(err.message.contains("being started"), "{}", err.message);
        drop(start);
        let resume = Claim::take(&s, "ABC-1", None, Verb::Resume).unwrap();
        let err = Claim::take(&s, "ABC-1", None, Verb::Start).unwrap_err();
        assert!(err.message.contains("being resumed"), "{}", err.message);
        drop(resume);
        assert!(!any_held(&s));
    }

    /// Siblings of one multi-repo start (one key, other projects) do not
    /// fence each other; the same project does, and so does the whole key,
    /// either way round.
    #[test]
    fn per_project_claims_clash_only_on_their_project_or_the_whole_key() {
        let s = Store::open_in_memory().unwrap();
        let one = Claim::take(&s, "ABC-1", Some(1), Verb::Start).unwrap();
        let _two = Claim::take(&s, "ABC-1", Some(2), Verb::Start).expect("another project");
        assert!(Claim::take(&s, "ABC-1", Some(1), Verb::Start).is_err());
        assert!(Claim::take(&s, "ABC-1", None, Verb::Resume).is_err());
        let _other_key = Claim::take(&s, "ABC-2", None, Verb::Start).expect("another key");
        drop(one);
        let _one_again = Claim::take(&s, "ABC-1", Some(1), Verb::Start).expect("released");

        let t = Store::open_in_memory().unwrap();
        let _whole = Claim::take(&t, "ABC-1", None, Verb::Start).unwrap();
        assert!(Claim::take(&t, "ABC-1", Some(3), Verb::Start).is_err());
    }
}
