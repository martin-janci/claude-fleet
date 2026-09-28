//! The natural language a piece of text is written in, as one word of a
//! fixed vocabulary (the Jev evaluation's language axis, decisions D40–D46:
//! `docs/superpowers/specs/2026-09-27-jev-language-census-design.md`).
//!
//! The same reading serves the language census now (`census`) and the
//! static routing table (D41) later, so a census number and a routing
//! decision can never disagree about what "Slovak" means.
//!
//! How a text is read:
//! 1. Code is taken out first: fenced blocks, inline backticks, ticket keys,
//!    URLs, paths and identifier-shaped words. What stays is the prose; its
//!    share of the text is the [`CodeDensity`].
//! 2. Mostly non-Latin letters → `other`. Fewer than [`MIN_WORDS`] words of
//!    prose → `unknown` (a "fix", an "ok", a lone key).
//! 3. lingua, limited to the six languages of D44, names the language of
//!    the prose.
//! 4. Two language families, each holding at least [`MIXED_SHARE_PCT`] of the
//!    words of the sentences long enough to judge → `mixed`. Slovak and
//!    Czech count as one family here: a text split between them is far more
//!    likely one of them misread than both.
//! 5. Slovak vs Czech is settled by the words and letters only one of them
//!    uses (`sa`/`se`, `som`/`jsem`, `-cia`/`-ce`, `ä ô ľ`/`ě ř ů`). This is
//!    what reads Slovak and Czech typed WITHOUT diacritics, the case lingua
//!    alone gets wrong most often; `folded` marks it.
//!
//! Accuracy is pinned by the fixtures in `testdata/nl/`: `cases.jsonl` was
//! used while writing the rules, `holdout.jsonl` was written after them and
//! never tuned against (see the tests).

pub mod census;

use serde::{Deserialize, Serialize};

/// Named in every census header and evaluation, so a number can be traced
/// to the reader that produced it. Bump it when a rule or a model changes.
pub const DETECTOR_VERSION: &str = "nl-1 (lingua 1.8: en sk cs de pl hu; sk/cs markers v1)";

/// Fewer words of prose than this is `unknown`.
pub const MIN_WORDS: usize = 3;
/// A sentence needs this many words to count toward `mixed`.
const MIXED_SENTENCE_WORDS: usize = 4;
/// Each of two families must hold this share of the judged words for `mixed`.
pub const MIXED_SHARE_PCT: usize = 25;
/// At or above this share of code characters the text is `high` density.
pub const CODE_HIGH_PCT: usize = 30;

/// The language of a text, as one word.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NlBucket {
    En,
    Sk,
    Cs,
    De,
    Pl,
    Hu,
    /// Mostly a script none of the six uses (Cyrillic, CJK, Arabic, Greek…).
    Other,
    /// Two language families in one text, each with a real share of it.
    Mixed,
    /// Too little prose to say.
    Unknown,
}

impl NlBucket {
    pub const ALL: [NlBucket; 9] = [
        NlBucket::En,
        NlBucket::Sk,
        NlBucket::Cs,
        NlBucket::De,
        NlBucket::Pl,
        NlBucket::Hu,
        NlBucket::Other,
        NlBucket::Mixed,
        NlBucket::Unknown,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            NlBucket::En => "en",
            NlBucket::Sk => "sk",
            NlBucket::Cs => "cs",
            NlBucket::De => "de",
            NlBucket::Pl => "pl",
            NlBucket::Hu => "hu",
            NlBucket::Other => "other",
            NlBucket::Mixed => "mixed",
            NlBucket::Unknown => "unknown",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|b| b.as_str() == s)
    }

    /// Slovak and Czech are one family for `mixed`.
    fn family(self) -> Self {
        if self == NlBucket::Cs {
            NlBucket::Sk
        } else {
            self
        }
    }
}

/// How much of a text is code rather than prose.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CodeDensity {
    /// No code at all.
    None,
    /// Some, under [`CODE_HIGH_PCT`] of the characters.
    Low,
    /// [`CODE_HIGH_PCT`] of the characters or more.
    High,
}

impl CodeDensity {
    pub const ALL: [CodeDensity; 3] = [CodeDensity::None, CodeDensity::Low, CodeDensity::High];

    pub fn as_str(self) -> &'static str {
        match self {
            CodeDensity::None => "none",
            CodeDensity::Low => "low",
            CodeDensity::High => "high",
        }
    }

    fn of(code_chars: usize, total_chars: usize) -> Self {
        if code_chars == 0 || total_chars == 0 {
            CodeDensity::None
        } else if code_chars * 100 >= total_chars * CODE_HIGH_PCT {
            CodeDensity::High
        } else {
            CodeDensity::Low
        }
    }
}

/// What the detector read from one text. Holds no text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NlReading {
    pub bucket: NlBucket,
    /// Slovak or Czech written without a single diacritic.
    pub folded: bool,
    pub code: CodeDensity,
    /// Words of prose after the code was taken out.
    pub words: usize,
}

/// The prose of a text, with what was taken out of it counted.
#[derive(Debug, Default, PartialEq, Eq)]
struct Prose {
    text: String,
    words: usize,
    code_chars: usize,
    total_chars: usize,
    latin_letters: usize,
    other_letters: usize,
}

/// `ABC-123`: an upper-case prefix, a dash, digits.
fn is_ticket_key(w: &str) -> bool {
    let w = w.trim_matches(|c: char| !c.is_alphanumeric());
    let Some((prefix, num)) = w.split_once('-') else {
        return false;
    };
    prefix
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_uppercase())
        && prefix
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
        && !num.is_empty()
        && num.chars().all(|c| c.is_ascii_digit())
}

/// A URL, a path, an address or an identifier: not a word of prose.
fn is_code_word(w: &str) -> bool {
    const MARKS: &[&str] = &[
        "://", "/", "\\", "@", "::", "_", "(", "=", "{", "<", "->", "=>",
    ];
    if MARKS.iter().any(|m| w.contains(m)) {
        return true;
    }
    // camelCase / PascalCase with an inner capital, all alphanumeric.
    let letters_only = w.trim_matches(|c: char| !c.is_alphanumeric());
    letters_only.chars().all(char::is_alphanumeric)
        && letters_only.chars().any(char::is_lowercase)
        && letters_only.chars().skip(1).any(char::is_uppercase)
        && letters_only.chars().skip(1).any(char::is_lowercase)
}

fn is_latin(c: char) -> bool {
    c.is_ascii_alphabetic() || ('\u{00C0}'..='\u{024F}').contains(&c)
}

fn prose(text: &str) -> Prose {
    let mut p = Prose {
        total_chars: text.chars().count(),
        ..Default::default()
    };
    // Fenced blocks first, so their backticks are not read as inline code.
    let mut unfenced = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find("```") {
        unfenced.push_str(&rest[..open]);
        let after = &rest[open + 3..];
        match after.find("```") {
            Some(close) => {
                p.code_chars += after[..close].chars().count() + 6;
                unfenced.push('\n');
                rest = &after[close + 3..];
            }
            None => {
                p.code_chars += after.chars().count() + 3;
                rest = "";
            }
        }
    }
    unfenced.push_str(rest);
    // Inline code.
    let mut plain = String::with_capacity(unfenced.len());
    let mut in_code = false;
    for c in unfenced.chars() {
        if c == '`' {
            in_code = !in_code;
            p.code_chars += 1;
            plain.push(' ');
        } else if in_code {
            p.code_chars += 1;
        } else {
            plain.push(c);
        }
    }
    // Words, line by line so sentence ends at line breaks survive.
    for line in plain.split_inclusive('\n') {
        for w in line.split_whitespace() {
            if is_ticket_key(w) || is_code_word(w) {
                p.code_chars += w.chars().count();
                continue;
            }
            if !w.chars().any(char::is_alphabetic) {
                continue;
            }
            for c in w.chars().filter(|c| c.is_alphabetic()) {
                if is_latin(c) {
                    p.latin_letters += 1;
                } else {
                    p.other_letters += 1;
                }
            }
            p.words += 1;
            p.text.push_str(w);
            p.text.push(' ');
        }
        if line.ends_with('\n') {
            p.text.push('\n');
        }
    }
    p
}

/// Sentences of the prose, split at `. ! ?` and line breaks.
fn sentences(prose: &str) -> Vec<&str> {
    prose
        .split(['.', '!', '?', '\n'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect()
}

/// Every Slovak and Czech letter with a diacritic, both cases.
const DIACRITICS: &str = "áäčďéíĺľňóôŕšťúýžěřůÁÄČĎÉÍĹĽŇÓÔŔŠŤÚÝŽĚŘŮ";
/// Letters Czech uses and Slovak does not.
const CS_ONLY_LETTERS: &str = "ěřůĚŘŮ";
/// Letters Slovak uses and Czech does not.
const SK_ONLY_LETTERS: &str = "äôľĺŕÄÔĽĹŔ";
/// Letters Polish uses and neither Slovak nor Czech does.
const PL_ONLY_LETTERS: &str = "ąęłśźżĄĘŁŚŹŻ";

/// Slovak function words (written without diacritics) Czech does not use.
const SK_WORDS: &[&str] = &[
    "sa",
    "aj",
    "som",
    "sme",
    "ste",
    "su",
    "ked",
    "preco",
    "este",
    "teraz",
    "treba",
    "potrebujem",
    "ktory",
    "ktora",
    "ktore",
    "tiez",
    "nieco",
    "vsetko",
    "mozes",
    "pozri",
    "alebo",
    "lebo",
    "pretoze",
    "vyzera",
    "zle",
    "ci",
    "sprav",
    "tu",
];
/// Czech function words (written without diacritics) Slovak does not use.
const CS_WORDS: &[&str] = &[
    "se",
    "jsem",
    "jsi",
    "jsme",
    "jste",
    "jsou",
    "neni",
    "kdyz",
    "proc",
    "jeste",
    "ted",
    "taky",
    "tohle",
    "muzes",
    "podivej",
    "nejdriv",
    "potrebuju",
    "udelej",
    "mrkni",
    "ktery",
    "ktera",
    "ktere",
    "neco",
    "vsechno",
    "spatne",
    "vypada",
    "jestli",
    "protoze",
    "nebo",
    "tady",
    "zda",
];

fn fold(c: char) -> char {
    match c {
        'á' | 'ä' => 'a',
        'č' => 'c',
        'ď' => 'd',
        'é' | 'ě' => 'e',
        'í' => 'i',
        'ĺ' | 'ľ' => 'l',
        'ň' => 'n',
        'ó' | 'ô' => 'o',
        'ŕ' | 'ř' => 'r',
        'š' => 's',
        'ť' => 't',
        'ú' | 'ů' => 'u',
        'ý' => 'y',
        'ž' => 'z',
        other => other,
    }
}

/// (Slovak, Czech) evidence: function words and the `-cia` / `-ce` noun
/// endings (`aplikacia` / `aplikace`).
fn sk_cs_markers(prose: &str) -> (usize, usize) {
    let (mut sk, mut cs) = (0, 0);
    for w in prose.split(|c: char| !c.is_alphabetic()) {
        if w.is_empty() {
            continue;
        }
        let w: String = w.to_lowercase().chars().map(fold).collect();
        if SK_WORDS.contains(&w.as_str()) {
            sk += 1;
        }
        if CS_WORDS.contains(&w.as_str()) {
            cs += 1;
        }
        if w.len() > 5 && ["cia", "cie", "cii", "ciu"].iter().any(|e| w.ends_with(e)) {
            sk += 1;
        }
        if w.len() > 5
            && ["kace", "kaci", "zace", "zaci", "cace"]
                .iter()
                .any(|e| w.ends_with(e))
        {
            cs += 1;
        }
    }
    (sk, cs)
}

/// The language ranking a reader gives for one piece of prose, best first.
/// A seam so the rules above can be tested without a model.
pub trait Ranker {
    fn rank(&self, prose: &str) -> Vec<(NlBucket, f64)>;
}

/// Read `text` with `ranker`.
pub fn read_with(ranker: &dyn Ranker, text: &str) -> NlReading {
    let p = prose(text);
    let code = CodeDensity::of(p.code_chars, p.total_chars);
    let reading = |bucket, folded| NlReading {
        bucket,
        folded,
        code,
        words: p.words,
    };
    if p.other_letters > p.latin_letters {
        return reading(NlBucket::Other, false);
    }
    if p.words < MIN_WORDS {
        return reading(NlBucket::Unknown, false);
    }
    let Some(&(mut top, _)) = ranker.rank(&p.text).first() else {
        return reading(NlBucket::Unknown, false);
    };

    // Mixed: families by sentence, weighted by words.
    let mut shares: Vec<(NlBucket, usize)> = Vec::new();
    let mut judged = 0;
    for s in sentences(&p.text) {
        let n = s.split_whitespace().count();
        if n < MIXED_SENTENCE_WORDS {
            continue;
        }
        if let Some(&(lang, _)) = ranker.rank(s).first() {
            let fam = lang.family();
            match shares.iter_mut().find(|(f, _)| *f == fam) {
                Some((_, w)) => *w += n,
                None => shares.push((fam, n)),
            }
            judged += n;
        }
    }
    let big = shares
        .iter()
        .filter(|(_, n)| n * 100 >= judged * MIXED_SHARE_PCT)
        .count();
    if judged > 0 && big >= 2 {
        return reading(NlBucket::Mixed, false);
    }

    // Slovak vs Czech, and Slovak/Czech the model gave to a neighbour.
    let (sk, cs) = sk_cs_markers(&p.text);
    let pl_letters = p.text.chars().any(|c| PL_ONLY_LETTERS.contains(c));
    if matches!(top, NlBucket::Sk | NlBucket::Cs) {
        if sk > cs {
            top = NlBucket::Sk;
        } else if cs > sk {
            top = NlBucket::Cs;
        }
    } else if !pl_letters
        && ((top == NlBucket::Pl && sk + cs >= 2) || (top == NlBucket::En && sk + cs >= 3))
    {
        top = if cs > sk { NlBucket::Cs } else { NlBucket::Sk };
    }
    // Letters only one of them has outrank the words.
    if matches!(top, NlBucket::Sk | NlBucket::Cs) {
        let cs_l = p.text.chars().any(|c| CS_ONLY_LETTERS.contains(c));
        let sk_l = p.text.chars().any(|c| SK_ONLY_LETTERS.contains(c));
        if cs_l && !sk_l {
            top = NlBucket::Cs;
        } else if sk_l && !cs_l {
            top = NlBucket::Sk;
        }
    }
    let folded = matches!(top, NlBucket::Sk | NlBucket::Cs)
        && !p.text.chars().any(|c| DIACRITICS.contains(c));
    reading(top, folded)
}

/// The production reader: lingua over the six languages of D44. Models load
/// on first use; build one and share it.
pub struct Detector {
    inner: lingua::LanguageDetector,
}

impl Default for Detector {
    fn default() -> Self {
        Self::new()
    }
}

impl Detector {
    pub fn new() -> Self {
        use lingua::Language::{Czech, English, German, Hungarian, Polish, Slovak};
        Self {
            inner: lingua::LanguageDetectorBuilder::from_languages(&[
                English, Slovak, Czech, German, Polish, Hungarian,
            ])
            .build(),
        }
    }

    pub fn read(&self, text: &str) -> NlReading {
        read_with(self, text)
    }
}

impl Ranker for Detector {
    fn rank(&self, prose: &str) -> Vec<(NlBucket, f64)> {
        use lingua::Language;
        self.inner
            .compute_language_confidence_values(prose)
            .into_iter()
            .map(|(lang, conf)| {
                let b = match lang {
                    Language::English => NlBucket::En,
                    Language::Slovak => NlBucket::Sk,
                    Language::Czech => NlBucket::Cs,
                    Language::German => NlBucket::De,
                    Language::Polish => NlBucket::Pl,
                    Language::Hungarian => NlBucket::Hu,
                    #[allow(unreachable_patterns)]
                    _ => NlBucket::Other,
                };
                (b, conf)
            })
            .collect()
    }
}

/// One labeled text: the fixtures' format, and what `fleet-hub census
/// languages --export-sample` writes for a person to check (D46).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LabeledCase {
    pub text: String,
    /// The bucket a person says is right.
    pub expect: NlBucket,
    #[serde(default)]
    pub folded: bool,
    /// `prompt`, `title`, `note`… — where such a text comes from.
    #[serde(default)]
    pub kind: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
    /// `false` while `expect` is still the detector's own guess: an exported
    /// sample nobody has checked yet. Evaluation skips unchecked cases.
    #[serde(default = "yes")]
    pub checked: bool,
}

fn yes() -> bool {
    true
}

/// How a reader did on labeled cases.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Evaluation {
    pub detector: String,
    /// Checked cases read.
    pub cases: u64,
    /// Cases an unchecked `expect` left out.
    pub unchecked: u64,
    pub bucket_correct: u64,
    pub folded_correct: u64,
    /// `(expected, read)` → count, mistakes only.
    pub confusion: Vec<(NlBucket, NlBucket, u64)>,
}

impl Evaluation {
    pub fn bucket_pct(&self) -> f64 {
        pct(self.bucket_correct, self.cases)
    }
    pub fn folded_pct(&self) -> f64 {
        pct(self.folded_correct, self.cases)
    }

    pub fn lines(&self) -> Vec<String> {
        let mut v = vec![
            format!("detector: {}", self.detector),
            format!(
                "cases: {} checked ({} unchecked left out)",
                self.cases, self.unchecked
            ),
        ];
        if self.cases == 0 {
            v.push(
                "nothing to measure: set `checked` to true on the cases a person has corrected"
                    .into(),
            );
            return v;
        }
        v.extend([
            format!(
                "language right: {}/{} ({:.1}%)",
                self.bucket_correct,
                self.cases,
                self.bucket_pct()
            ),
            format!(
                "no-diacritics flag right: {}/{} ({:.1}%)",
                self.folded_correct,
                self.cases,
                self.folded_pct()
            ),
        ]);
        if self.confusion.is_empty() {
            v.push("mistakes: none".into());
        } else {
            v.push("mistakes (expected -> read: count):".into());
            for (e, g, n) in &self.confusion {
                v.push(format!("  {} -> {}: {n}", e.as_str(), g.as_str()));
            }
        }
        v
    }
}

fn pct(n: u64, of: u64) -> f64 {
    if of == 0 {
        0.0
    } else {
        n as f64 * 100.0 / of as f64
    }
}

/// Read every checked case and count the mistakes.
pub fn evaluate(ranker: &dyn Ranker, cases: &[LabeledCase]) -> Evaluation {
    let mut e = Evaluation {
        detector: DETECTOR_VERSION.to_string(),
        ..Default::default()
    };
    let mut confusion: std::collections::BTreeMap<(NlBucket, NlBucket), u64> = Default::default();
    for c in cases {
        if !c.checked {
            e.unchecked += 1;
            continue;
        }
        e.cases += 1;
        let r = read_with(ranker, &c.text);
        if r.bucket == c.expect {
            e.bucket_correct += 1;
        } else {
            *confusion.entry((c.expect, r.bucket)).or_default() += 1;
        }
        if r.folded == c.folded {
            e.folded_correct += 1;
        }
    }
    e.confusion = confusion.into_iter().map(|((a, b), n)| (a, b, n)).collect();
    e
}

/// Parse JSON lines of [`LabeledCase`]; blank lines are skipped, a bad line
/// is an error naming its number.
pub fn parse_cases(jsonl: &str) -> Result<Vec<LabeledCase>, String> {
    jsonl
        .lines()
        .enumerate()
        .filter(|(_, l)| !l.trim().is_empty())
        .map(|(i, l)| serde_json::from_str(l).map_err(|e| format!("line {}: {e}", i + 1)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A ranker that answers one fixed language for everything.
    struct Always(NlBucket);
    impl Ranker for Always {
        fn rank(&self, _: &str) -> Vec<(NlBucket, f64)> {
            vec![(self.0, 1.0)]
        }
    }

    /// A ranker that says English for sentences with "the", else its default.
    struct EnglishIfThe(NlBucket);
    impl Ranker for EnglishIfThe {
        fn rank(&self, p: &str) -> Vec<(NlBucket, f64)> {
            let the = p.split_whitespace().any(|w| w.eq_ignore_ascii_case("the"));
            vec![(if the { NlBucket::En } else { self.0 }, 1.0)]
        }
    }

    #[test]
    fn code_keys_and_urls_are_not_prose() {
        let p = prose("Fix `parse_since` for PROJ-12, see https://x.io/a and src/main.rs now");
        assert_eq!(
            p.text.split_whitespace().collect::<Vec<_>>(),
            ["Fix", "for", "see", "and", "now"]
        );
        assert!(p.code_chars > 0);
    }

    #[test]
    fn a_fenced_block_is_code_even_unclosed() {
        let p = prose("Look here:\n```rust\nlet x = 1;\n```\nand here ```fn main()");
        assert_eq!(p.words, 4);
        assert_eq!(
            CodeDensity::of(p.code_chars, p.total_chars),
            CodeDensity::High
        );
    }

    #[test]
    fn camel_case_is_code_but_a_capitalised_word_is_not() {
        assert!(is_code_word("useEffect"));
        assert!(is_code_word("TrackerView"));
        assert!(!is_code_word("Jira"));
        assert!(!is_code_word("CI"));
        assert!(!is_code_word("prosím,"));
    }

    #[test]
    fn short_text_is_unknown_whatever_the_model_says() {
        for t in ["ok", "fix it", "PROJ-1 PROJ-2", "`cargo test`", "👍"] {
            assert_eq!(
                read_with(&Always(NlBucket::En), t).bucket,
                NlBucket::Unknown,
                "{t}"
            );
        }
    }

    #[test]
    fn mostly_foreign_script_is_other() {
        let r = read_with(
            &Always(NlBucket::En),
            "Исправь, пожалуйста, нестабильный тест входа.",
        );
        assert_eq!(r.bucket, NlBucket::Other);
    }

    #[test]
    fn two_families_with_real_shares_are_mixed() {
        let r = read_with(
            &EnglishIfThe(NlBucket::Sk),
            "Oprav prosím ten test v CI. Then make sure the linter passes before you push.",
        );
        assert_eq!(r.bucket, NlBucket::Mixed);
    }

    #[test]
    fn slovak_and_czech_halves_are_not_mixed() {
        struct Split;
        impl Ranker for Split {
            fn rank(&self, p: &str) -> Vec<(NlBucket, f64)> {
                vec![(
                    if p.contains("jsem") {
                        NlBucket::Cs
                    } else {
                        NlBucket::Sk
                    },
                    1.0,
                )]
            }
        }
        let r = read_with(
            &Split,
            "Pozri sa na ten build dnes. Ja jsem to tam asi rozbil včera.",
        );
        assert_ne!(r.bucket, NlBucket::Mixed);
    }

    #[test]
    fn markers_settle_slovak_against_czech() {
        let sk = "Pozri sa preco to pada ked je session prazdna";
        let cs = "Podivej se proc to pada kdyz je session prazdna";
        assert_eq!(read_with(&Always(NlBucket::Cs), sk).bucket, NlBucket::Sk);
        assert_eq!(read_with(&Always(NlBucket::Sk), cs).bucket, NlBucket::Cs);
    }

    #[test]
    fn letters_only_one_language_has_outrank_markers() {
        // "se" is a Czech marker, but ř is Czech only and ô Slovak only.
        assert_eq!(
            read_with(&Always(NlBucket::Sk), "Oprav to ř prosím, ať se to hne").bucket,
            NlBucket::Cs
        );
        assert_eq!(
            read_with(
                &Always(NlBucket::Cs),
                "Oprav to hneď, bolo to v pôvodnej verzii"
            )
            .bucket,
            NlBucket::Sk
        );
    }

    #[test]
    fn slovak_the_model_gave_to_polish_comes_back_unless_polish_letters_say_so() {
        let folded_sk = "Oprav prosim ten test, ked sa to da este dnes";
        assert_eq!(
            read_with(&Always(NlBucket::Pl), folded_sk).bucket,
            NlBucket::Sk
        );
        let polish = "Napraw proszę ten test, gdy się da jeszcze dziś";
        assert_eq!(
            read_with(&Always(NlBucket::Pl), polish).bucket,
            NlBucket::Pl
        );
    }

    #[test]
    fn folded_is_slovak_or_czech_without_any_diacritic() {
        let r = read_with(
            &Always(NlBucket::Sk),
            "Pozri sa preco to pada ked je prazdna",
        );
        assert!(r.folded);
        let r = read_with(
            &Always(NlBucket::Sk),
            "Pozri sa, prečo to padá, keď je prázdna",
        );
        assert!(!r.folded);
        let r = read_with(
            &Always(NlBucket::En),
            "Look at why it fails when it is empty",
        );
        assert!(!r.folded);
    }

    #[test]
    fn bucket_words_round_trip() {
        for b in NlBucket::ALL {
            assert_eq!(NlBucket::parse(b.as_str()), Some(b));
            assert_eq!(
                serde_json::to_value(b).unwrap(),
                serde_json::json!(b.as_str())
            );
        }
    }

    #[test]
    fn an_unchecked_case_is_left_out_of_the_evaluation() {
        let cases = parse_cases(
            r#"{"text":"Look at why the build fails on main","expect":"en"}
{"text":"Look at why the build fails on main","expect":"de","checked":false}"#,
        )
        .unwrap();
        let e = evaluate(&Always(NlBucket::En), &cases);
        assert_eq!((e.cases, e.unchecked, e.bucket_correct), (1, 1, 1));
        let none = evaluate(&Always(NlBucket::En), &cases[1..]);
        assert!(
            none.lines()
                .iter()
                .any(|l| l.starts_with("nothing to measure")),
            "{:?}",
            none.lines()
        );
    }

    #[test]
    fn a_bad_line_names_its_number() {
        let err = parse_cases("{\"text\":\"a b c\",\"expect\":\"en\"}\n\nnot json").unwrap_err();
        assert!(err.starts_with("line 3:"), "{err}");
    }

    fn fixture(name: &str) -> Vec<LabeledCase> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src/service/testdata/nl")
            .join(name);
        parse_cases(&std::fs::read_to_string(&path).unwrap()).unwrap()
    }

    /// The cases the rules were written against. A drop here is a rule or a
    /// model that changed: look at the mistakes before moving the bar.
    #[test]
    fn the_detector_reads_the_written_against_cases() {
        let e = evaluate(&Detector::new(), &fixture("cases.jsonl"));
        assert!(e.cases >= 200, "{}", e.cases);
        assert!(e.bucket_pct() >= 97.0, "{}", e.lines().join("\n"));
        assert!(e.folded_pct() >= 98.0, "{}", e.lines().join("\n"));
        // Every mistake stays inside the Slovak/Czech family.
        assert!(
            e.confusion
                .iter()
                .all(|(a, b, _)| matches!(a, NlBucket::Sk | NlBucket::Cs)
                    && matches!(b, NlBucket::Sk | NlBucket::Cs)),
            "{}",
            e.lines().join("\n")
        );
    }

    /// Written after the rules and never tuned against: the honest number.
    #[test]
    fn the_detector_reads_the_holdout() {
        let e = evaluate(&Detector::new(), &fixture("holdout.jsonl"));
        assert!(e.cases >= 40, "{}", e.cases);
        assert!(e.bucket_pct() >= 90.0, "{}", e.lines().join("\n"));
    }
}
