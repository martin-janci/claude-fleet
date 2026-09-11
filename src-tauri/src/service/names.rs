//! Docker-style memorable session names: `<adjective>-<noun>` ("blue-sirius").
//!
//! Rust twin of `src/lib/names.ts`. Both sides read the SAME word lists —
//! `src/lib/names.json` is compiled in here via `include_str!` — so a name
//! minted by the backend (when `new_session` receives an empty `name`) comes
//! from exactly the vocabulary the dialog's dice button uses.
//!
//! Collision policy mirrors Docker's `GetRandomName`: draw random pairs
//! until one is free; after `MAX_TRIES` misses fall back to the last pair
//! with a counting suffix (`blue-sirius-2`, `-3`, …).

use std::collections::HashSet;
use std::sync::OnceLock;

use rand::{Rng, RngExt};

const WORDS_JSON: &str = include_str!("../../../src/lib/names.json");

pub const SEPARATOR: &str = "-";
/// Random draws before falling back to a numeric suffix.
pub const MAX_TRIES: usize = 24;
/// Shared-prefix length that makes a pair redundant ("lunar luna").
pub const SAME_ROOT_PREFIX: usize = 4;

/// True when the adjective and noun share a root and read as a stutter.
pub fn same_root(adjective: &str, noun: &str) -> bool {
    adjective
        .bytes()
        .zip(noun.bytes())
        .take_while(|(a, b)| a == b)
        .count()
        >= SAME_ROOT_PREFIX
}

/// Lowercase, every run of non-alphanumerics becomes one `-`, no leading or
/// trailing dash: `"Blue Sirius"` → `"blue-sirius"`. Used to put friendly
/// names in the same slug space as generated pairs (the TS side uses
/// `finalizeBranchSlug`, which agrees on every value the generator emits).
pub fn slugify(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

/// Make a name acceptable to `validate::tmux_name`: `.` and `:` (tmux's
/// target separators) become `-`.
pub fn tmux_safe(s: &str) -> String {
    s.replace(['.', ':'], "-")
}

#[derive(serde::Deserialize)]
struct Words {
    adjectives: Vec<String>,
    nouns: Vec<String>,
}

fn words() -> &'static Words {
    static WORDS: OnceLock<Words> = OnceLock::new();
    WORDS.get_or_init(|| {
        serde_json::from_str(WORDS_JSON).expect("src/lib/names.json is valid — checked by tests")
    })
}

pub fn adjectives() -> &'static [String] {
    &words().adjectives
}

pub fn nouns() -> &'static [String] {
    &words().nouns
}

/// Mint a name not present in `existing` (compared case-insensitively;
/// `existing` should hold slugs — worktree names, tmux-name suffixes).
pub fn generate_name<R: Rng + ?Sized>(existing: &HashSet<String>, rng: &mut R) -> String {
    let taken: HashSet<String> = existing.iter().map(|s| s.to_lowercase()).collect();
    let adj = adjectives();
    let noun = nouns();
    let mut last = String::new();
    for _ in 0..MAX_TRIES {
        let a = &adj[rng.random_range(0..adj.len())];
        let n = &noun[rng.random_range(0..noun.len())];
        // A same-root draw still spends a try, so the budget stays MAX_TRIES.
        if same_root(a, n) {
            continue;
        }
        last = format!("{a}{SEPARATOR}{n}");
        if !taken.contains(&last) {
            return last;
        }
    }
    if last.is_empty() {
        last = format!("{}{SEPARATOR}{}", adj[0], noun[0]);
    }
    let mut n = 2u64;
    loop {
        let candidate = format!("{last}{SEPARATOR}{n}");
        if !taken.contains(&candidate) {
            return candidate;
        }
        n += 1;
    }
}

/// `generate_name` with the thread-local OS-seeded rng.
pub fn generate_name_default(existing: &HashSet<String>) -> String {
    generate_name(existing, &mut rand::rng())
}

/// The suffix a tmux name carries after the project prefix:
/// `dev-<owner>-<repo>--fix-login-term` → `Some("fix-login")`. `None` for a
/// bare `dev-<owner>-<repo>` or a name that does not follow the convention.
pub fn tmux_name_suffix(tmux_name: &str, owner: &str, repo: &str) -> Option<String> {
    let prefix = format!("dev-{owner}-{repo}--");
    let rest = tmux_name.strip_prefix(&prefix)?;
    let rest = rest.strip_suffix("-term").unwrap_or(rest);
    if rest.is_empty() {
        None
    } else {
        Some(rest.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::convert::Infallible;

    /// rand 0.10 dropped `StepRng`; a constant generator is all these tests
    /// need (every draw lands on the same pair).
    struct ConstRng(u64);
    impl rand::TryRng for ConstRng {
        type Error = Infallible;
        fn try_next_u32(&mut self) -> Result<u32, Infallible> {
            Ok(self.0 as u32)
        }
        fn try_next_u64(&mut self) -> Result<u64, Infallible> {
            Ok(self.0)
        }
        fn try_fill_bytes(&mut self, dst: &mut [u8]) -> Result<(), Infallible> {
            dst.fill(self.0 as u8);
            Ok(())
        }
    }

    #[test]
    fn lists_match_frontend() {
        // Same numbers as `src/lib/names.test.ts` asserts. Both read one JSON
        // file, so this can only fail if someone edits the list and forgets
        // to update one of the two assertions — which is the intended tripwire.
        assert_eq!(adjectives().len(), 67);
        assert_eq!(nouns().len(), 124);
    }

    #[test]
    fn every_word_is_lowercase_ascii_3_to_8_letters_and_unique() {
        let mut seen = HashSet::new();
        for w in adjectives().iter().chain(nouns().iter()) {
            assert!(
                w.len() >= 3 && w.len() <= 8 && w.chars().all(|c| c.is_ascii_lowercase()),
                "bad word {w:?}"
            );
            assert!(seen.insert(w.clone()), "duplicate word {w:?}");
        }
    }

    #[test]
    fn generates_adjective_noun_from_lists() {
        let mut rng = ConstRng(0);
        let n = generate_name(&HashSet::new(), &mut rng);
        let (a, b) = n.split_once('-').expect("has separator");
        assert!(adjectives().iter().any(|w| w == a));
        assert!(nouns().iter().any(|w| w == b));
    }

    #[test]
    fn skips_taken_names_and_falls_back_to_numeric_suffix() {
        // A constant rng always draws the same pair.
        let mut rng = ConstRng(0);
        let first = generate_name(&HashSet::new(), &mut rng);
        let mut taken: HashSet<String> = [first.to_uppercase()].into_iter().collect();
        let second = generate_name(&taken, &mut ConstRng(0));
        assert_eq!(second, format!("{first}-2"));
        taken.insert(second);
        let third = generate_name(&taken, &mut ConstRng(0));
        assert_eq!(third, format!("{first}-3"));
    }

    #[test]
    fn never_returns_an_existing_name() {
        let mut existing = HashSet::new();
        for _ in 0..500 {
            let n = generate_name_default(&existing);
            assert!(!existing.contains(&n));
            existing.insert(n);
        }
    }

    #[test]
    fn never_pairs_same_root_words() {
        assert!(same_root("lunar", "luna"));
        assert!(same_root("cosmic", "cosmos"));
        assert!(!same_root("coral", "corona"));
        for _ in 0..500 {
            let n = generate_name_default(&HashSet::new());
            let (a, b) = n.split_once('-').unwrap();
            assert!(!same_root(a, b), "{n}");
        }
    }

    #[test]
    fn adjectives_are_sorted_without_fleet() {
        let mut sorted = adjectives().to_vec();
        sorted.sort();
        assert_eq!(sorted, adjectives());
        assert!(!adjectives().iter().any(|a| a == "fleet"));
    }

    #[test]
    fn slugify_and_tmux_safe() {
        assert_eq!(slugify("Blue Sirius"), "blue-sirius");
        assert_eq!(slugify("  Fix: the login bug! "), "fix-the-login-bug");
        assert_eq!(slugify(""), "");
        assert_eq!(tmux_safe("dev-o-r--v1.2:x"), "dev-o-r--v1-2-x");
    }

    #[test]
    fn tmux_name_suffix_strips_prefix_and_term() {
        assert_eq!(
            tmux_name_suffix("dev-o-r--blue-sirius", "o", "r").as_deref(),
            Some("blue-sirius")
        );
        assert_eq!(
            tmux_name_suffix("dev-o-r--blue-sirius-term", "o", "r").as_deref(),
            Some("blue-sirius")
        );
        assert_eq!(tmux_name_suffix("dev-o-r", "o", "r"), None);
        assert_eq!(tmux_name_suffix("other", "o", "r"), None);
    }
}
