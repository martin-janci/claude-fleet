//! A small BM25 over a handful of documents (a case's candidates), the
//! benchmark's lexical baseline (test map card J1, "BM25 over titles").
//!
//! The tokenizer is language-agnostic but aware enough of fleet's languages
//! (D44: en, sk, cs, de): lower-cased, diacritics folded by
//! [`crate::service::nl::fold`] (`č` → `c`, `ä` → `a`, `ß` → `ss`), split on
//! anything that is not a letter or a digit, and cut to a
//! [`STEM_CHARS`]-character prefix — a crude stemmer that lets Slovak and
//! Czech inflections (`prihlásenie` / `prihlásenia`) and English plurals
//! meet. Tokens under [`MIN_TOKEN_CHARS`] are dropped.

use crate::service::nl::fold;
use std::collections::{BTreeSet, HashMap};

/// Named in the benchmark's header; bump it when the tokenizer or the
/// scoring changes.
pub const BM25_VERSION: &str = "bm25-1 (k1 1.2, b 0.75, fold+prefix6, title x2)";
/// The prefix a token is cut to.
pub const STEM_CHARS: usize = 6;
/// Shorter tokens are dropped.
pub const MIN_TOKEN_CHARS: usize = 2;
const K1: f64 = 1.2;
const B: f64 = 0.75;

/// PURE: one word folded and cut to its stem.
pub fn stem(word: &str) -> String {
    fold(word).chars().take(STEM_CHARS).collect()
}

/// PURE: the tokens of `text`: folded words, cut to [`STEM_CHARS`], at
/// least [`MIN_TOKEN_CHARS`] long.
pub fn tokenize(text: &str) -> Vec<String> {
    fold(text)
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.chars().count() >= MIN_TOKEN_CHARS)
        .map(|w| w.chars().take(STEM_CHARS).collect())
        .collect()
}

/// BM25 over a fixed set of documents.
pub struct Bm25 {
    docs: Vec<HashMap<String, u32>>,
    lens: Vec<f64>,
    avg_len: f64,
    df: HashMap<String, u32>,
}

impl Bm25 {
    /// Index `docs` (each a document's text).
    pub fn new<S: AsRef<str>>(docs: &[S]) -> Self {
        let mut df: HashMap<String, u32> = HashMap::new();
        let mut out = Vec::with_capacity(docs.len());
        let mut lens = Vec::with_capacity(docs.len());
        for d in docs {
            let toks = tokenize(d.as_ref());
            lens.push(toks.len() as f64);
            let mut tf: HashMap<String, u32> = HashMap::new();
            for t in toks {
                *tf.entry(t).or_default() += 1;
            }
            for t in tf.keys() {
                *df.entry(t.clone()).or_default() += 1;
            }
            out.push(tf);
        }
        let avg_len = if lens.is_empty() {
            0.0
        } else {
            lens.iter().sum::<f64>() / lens.len() as f64
        };
        Bm25 {
            docs: out,
            lens,
            avg_len,
            df,
        }
    }

    /// The Lucene-style idf: always positive, so a word every candidate
    /// shares still counts a little.
    fn idf(&self, term: &str) -> f64 {
        let n = self.docs.len() as f64;
        let df = f64::from(*self.df.get(term).unwrap_or(&0));
        (1.0 + (n - df + 0.5) / (df + 0.5)).ln()
    }

    /// Every document's score against `query` (its distinct tokens).
    pub fn scores(&self, query: &str) -> Vec<f64> {
        let terms: BTreeSet<String> = tokenize(query).into_iter().collect();
        self.docs
            .iter()
            .zip(&self.lens)
            .map(|(tf, &len)| {
                terms
                    .iter()
                    .filter_map(|t| tf.get(t).map(|&f| (t, f64::from(f))))
                    .map(|(t, f)| {
                        let norm = if self.avg_len > 0.0 {
                            len / self.avg_len
                        } else {
                            1.0
                        };
                        self.idf(t) * f * (K1 + 1.0) / (f + K1 * (1.0 - B + B * norm))
                    })
                    .sum()
            })
            .collect()
    }

    /// The best document and its score; ties go to the lower index. `None`
    /// when nothing scores above 0.
    pub fn best(&self, query: &str) -> Option<(usize, f64)> {
        let mut best: Option<(usize, f64)> = None;
        for (i, s) in self.scores(query).into_iter().enumerate() {
            if s > 0.0 && best.is_none_or(|(_, b)| s > b) {
                best = Some((i, s));
            }
        }
        best
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_folded_split_stemmed_and_short_ones_dropped() {
        assert_eq!(
            tokenize("Oprav prihlásenie: login_page/redirect a X"),
            vec!["oprav", "prihla", "login", "page", "redire"]
        );
        // Inflections meet at the stem.
        assert_eq!(tokenize("prihlásenia"), tokenize("prihlasenie"));
        assert!(tokenize("  ,, - ").is_empty());
    }

    #[test]
    fn the_matching_document_wins() {
        let bm = Bm25::new(&[
            "Billing export to CSV",
            "Login redirect loops on mobile",
            "Upgrade the database driver",
        ]);
        let (i, s) = bm
            .best("the login page keeps redirecting on my phone")
            .unwrap();
        assert_eq!(i, 1);
        assert!(s > 0.0);
        assert!(bm.best("completely unrelated words").is_none());
    }

    #[test]
    fn rare_terms_outweigh_common_ones() {
        let bm = Bm25::new(&["fix build pipeline", "fix login", "fix billing"]);
        let s = bm.scores("fix login");
        assert!(s[1] > s[0] && s[1] > s[2], "{s:?}");
        assert!(s[0] > 0.0, "a shared word still counts a little");
    }

    #[test]
    fn slovak_without_diacritics_matches_a_title_with_them() {
        let bm = Bm25::new(&["Opraviť prihlásenie cez SSO", "Export faktúr do CSV"]);
        assert_eq!(bm.best("oprav prihlasenia cez sso").map(|b| b.0), Some(0));
        assert_eq!(bm.best("exportuj faktury").map(|b| b.0), Some(1));
    }

    #[test]
    fn ties_go_to_the_first_and_empty_sets_score_nothing() {
        let bm = Bm25::new(&["same title", "same title"]);
        assert_eq!(bm.best("same").map(|b| b.0), Some(0));
        let empty = Bm25::new::<&str>(&[]);
        assert!(empty.best("anything").is_none());
    }
}
