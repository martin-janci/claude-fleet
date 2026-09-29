//! The language census (`fleet-hub census languages`, Jev evaluation D40):
//! which languages the texts a decision model would see are written in, per
//! org and per source, as counts.
//!
//! Local and read-only by construction: it reads the store the hub already
//! keeps, reads each text once in this process, and keeps only the counts.
//! Nothing is recorded and nothing is sent. A count under
//! [`SUPPRESS_BELOW`] shows as `<5`, in the lines and in the JSON alike.
//!
//! Sources, each named in the output:
//! - `prompt` — a conversation's first prompt (the first 200 characters the
//!   UserPromptSubmit hook keeps): the input of work-link decisions (J1).
//!   Prompts fleet typed itself (start, resume, handover and safe-kill
//!   requests, quick-reply chips, anything `[claude-fleet`-marked) and
//!   prompts Claude Code submitted itself (a `<task-notification>`, a slash
//!   command's echo: `prompt_origin`) are counted as `fleet_typed` and left
//!   out of the buckets.
//! - `title:<provider>` / `description:<provider>` — work items' titles and
//!   cached descriptions: J1's candidates.
//! - `journal:<kind>` — journal bodies written by a person or by Claude,
//!   never fleet's own rows: a stand-in for the language of Claude's replies.
//! - `pairs` — confirmed links, read from both ends: the language of the
//!   prompt that opened the conversation against the language of the item's
//!   title. How much of J1 is matching across languages.

use super::{CodeDensity, LabeledCase, NlBucket, Ranker};
use crate::ipc_error::{codes, IpcError};
use crate::service::quick_replies::{self, QuickReply};
use crate::store::{Store, NL_CENSUS_MIN_SCHEMA};
use serde::{Serialize, Serializer};
use std::collections::BTreeMap;

/// `days` when none is given.
pub const DEFAULT_DAYS: u32 = 90;
/// The longest window.
pub const MAX_DAYS: u32 = 730;
/// Rows read per source when no cap is given (newest first).
pub const DEFAULT_MAX_PER_SOURCE: u32 = 5000;
/// The largest cap.
pub const MAX_PER_SOURCE: u32 = 100_000;
/// Counts from 1 up to this (exclusive) show as `<5`.
pub const SUPPRESS_BELOW: u64 = 5;

/// What the store does not hold, so the census cannot read it.
pub const NOT_COUNTED: &[&str] = &[
    "prompts after a conversation's first (only the first 200 characters of the first are stored)",
    "what Claude Code submits itself (a <task-notification>, a slash command's echo): never a person's prompt; rows stored before fleet skipped it at capture are counted as fleet-typed",
    "conversations of deleted sessions (they go with the session row)",
    "Claude's replies as such (not stored; the journal stands in for them)",
    "commit subjects and PR titles (they live on the hosts)",
    "tracker items fleet never synced (the sync's per-view caps)",
];

/// A count as the census shows it: exact from [`SUPPRESS_BELOW`] up (and
/// at 0), `<5` below.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Shown(pub u64);

impl Shown {
    fn bump(&mut self) {
        self.0 += 1;
    }
}

impl std::fmt::Display for Shown {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.0 > 0 && self.0 < SUPPRESS_BELOW {
            write!(f, "<{SUPPRESS_BELOW}")
        } else {
            write!(f, "{}", self.0)
        }
    }
}

impl Serialize for Shown {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        if self.0 > 0 && self.0 < SUPPRESS_BELOW {
            s.serialize_str(&self.to_string())
        } else {
            s.serialize_u64(self.0)
        }
    }
}

/// What to count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CensusOptions {
    pub days: u32,
    /// Only this org's rows.
    pub org: Option<i64>,
    pub max_per_source: u32,
    pub now: i64,
}

impl CensusOptions {
    pub fn new(
        days: Option<u32>,
        org: Option<i64>,
        max_per_source: Option<u32>,
        now: i64,
    ) -> Result<Self, IpcError> {
        let days = days.unwrap_or(DEFAULT_DAYS);
        if days == 0 || days > MAX_DAYS {
            return Err(IpcError::new(
                codes::E_INVALID,
                format!("days must be 1-{MAX_DAYS}, got {days}"),
            ));
        }
        let max = max_per_source.unwrap_or(DEFAULT_MAX_PER_SOURCE);
        if max == 0 || max > MAX_PER_SOURCE {
            return Err(IpcError::new(
                codes::E_INVALID,
                format!("max per source must be 1-{MAX_PER_SOURCE}, got {max}"),
            ));
        }
        Ok(Self {
            days,
            org,
            max_per_source: max,
            now,
        })
    }

    pub fn since(&self) -> i64 {
        self.now - i64::from(self.days) * 86_400
    }
}

/// One source's counts in one org.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct SourceCensus {
    pub source: String,
    /// Texts read.
    pub read: Shown,
    /// Prompts fleet typed itself, left out of every count below.
    #[serde(skip_serializing_if = "is_zero")]
    pub fleet_typed: Shown,
    /// Per [`NlBucket`] word; buckets with nothing are left out.
    pub languages: BTreeMap<&'static str, Shown>,
    /// Slovak or Czech written without diacritics.
    pub no_diacritics: Shown,
    /// Per [`CodeDensity`] word.
    pub code: BTreeMap<&'static str, Shown>,
}

fn is_zero(s: &Shown) -> bool {
    s.0 == 0
}

/// Confirmed links, prompt language × title language.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct PairCensus {
    pub read: Shown,
    /// Prompt fleet typed itself: left out.
    #[serde(skip_serializing_if = "is_zero")]
    pub fleet_typed: Shown,
    /// Both ends in a language (not `unknown`/`mixed`/`other`), and different.
    pub cross_language: Shown,
    /// Both ends in the same language.
    pub same_language: Shown,
    pub cells: Vec<PairCell>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PairCell {
    pub prompt: &'static str,
    pub title: &'static str,
    pub count: Shown,
}

/// One org's census. `org_id: None` is the rows no org claims.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct OrgCensus {
    pub org_id: Option<i64>,
    pub org: String,
    pub sources: Vec<SourceCensus>,
    pub pairs: PairCensus,
}

/// The whole census.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CensusReport {
    pub detector: &'static str,
    pub schema_version: i64,
    pub days: u32,
    pub since: i64,
    pub max_per_source: u32,
    /// Sources that hit `max_per_source`: their counts are the newest rows only.
    pub capped: Vec<String>,
    pub orgs: Vec<OrgCensus>,
    pub not_counted: Vec<&'static str>,
}

/// Tells fleet's own prompts from a person's.
pub struct FleetPrompts {
    templates: Vec<Template>,
    chips: Vec<String>,
}

/// A fleet prompt template, as far as a stored first prompt can show it: a
/// head, and when the head alone is short, the words after the key.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Template {
    head: String,
    tail: Option<String>,
}

/// Rendered into each template where its key or nonce goes.
const SENTINEL: &str = "ZQXSENTINEL-9";
/// A head this long is distinctive on its own.
const HEAD_CHARS: usize = 60;
/// Words after the key checked when the head is shorter.
const TAIL_CHARS: usize = 30;
/// `conversations.first_prompt` keeps this many characters.
const FIRST_PROMPT_CHARS: usize = 200;

impl Template {
    fn of(rendered: &str) -> Self {
        let (before, after) = match rendered.find(SENTINEL) {
            Some(i) => (&rendered[..i], Some(&rendered[i + SENTINEL.len()..])),
            None => (rendered, None),
        };
        if before.chars().count() >= HEAD_CHARS || after.is_none() {
            return Template {
                head: before.chars().take(HEAD_CHARS).collect(),
                tail: None,
            };
        }
        Template {
            head: before.to_string(),
            tail: after
                .map(|a| a.chars().take(TAIL_CHARS).collect::<String>())
                .filter(|t| !t.trim().is_empty()),
        }
    }

    fn matches(&self, prompt: &str) -> bool {
        prompt.starts_with(&self.head) && self.tail.as_deref().is_none_or(|t| prompt.contains(t))
    }
}

impl FleetPrompts {
    /// `chips` are the fleet's quick replies (stored, else the defaults).
    pub fn new(chips: &[QuickReply]) -> Self {
        let templates = vec![
            Template::of(&crate::service::trackers::tickets::start_prompt(SENTINEL)),
            Template::of(&crate::service::work::resume::start_prompt(SENTINEL)),
            Template::of(&crate::service::safe_kill::build_safe_kill_prompt(SENTINEL)),
            Template::of(&crate::service::work::agent_handover::build_prompt(
                SENTINEL, "n",
            )),
        ];
        let chips = chips
            .iter()
            .map(|c| {
                c.text
                    .chars()
                    .take(FIRST_PROMPT_CHARS)
                    .collect::<String>()
                    .trim()
                    .to_string()
            })
            .filter(|t| !t.is_empty())
            .collect();
        Self { templates, chips }
    }

    pub fn from_store(store: &Store) -> Result<Self, IpcError> {
        let chips = match store.get_setting(quick_replies::SETTING_KEY)? {
            Some(raw) => serde_json::from_str::<Vec<QuickReply>>(&raw)
                .unwrap_or_else(|_| quick_replies::defaults()),
            None => quick_replies::defaults(),
        };
        Ok(Self::new(&chips))
    }

    /// Fleet typed `prompt`, or Claude Code did (a `<task-notification>`, a
    /// slash command's echo: `prompt_origin`, through the loop guard).
    pub fn is_fleet(&self, prompt: &str, last_prompt: Option<&str>) -> bool {
        let p = prompt.trim();
        crate::service::work::detect::loop_guard(p, last_prompt, &[]).is_some()
            || self.templates.iter().any(|t| t.matches(p))
            || self.chips.iter().any(|c| c == p)
    }

    /// The words a person typed in `prompt`: `None` when fleet or Claude
    /// Code typed it ([`Self::is_fleet`]), else the prompt without the
    /// harness blocks at its head (rows stored before the hook took them
    /// off).
    pub fn person_text<'a>(
        &self,
        prompt: &'a str,
        last_prompt: Option<&str>,
    ) -> Option<std::borrow::Cow<'a, str>> {
        let p = crate::service::prompt_origin::human_part(prompt)?;
        (!self.is_fleet(&p, last_prompt)).then_some(p)
    }
}

#[derive(Default)]
struct Acc {
    sources: BTreeMap<String, SourceCensus>,
    pairs: PairCensus,
    cells: BTreeMap<(NlBucket, NlBucket), Shown>,
}

impl Acc {
    fn source(&mut self, name: &str) -> &mut SourceCensus {
        self.sources
            .entry(name.to_string())
            .or_insert_with(|| SourceCensus {
                source: name.to_string(),
                ..Default::default()
            })
    }
}

fn count(s: &mut SourceCensus, r: super::NlReading) {
    s.read.bump();
    s.languages.entry(r.bucket.as_str()).or_default().bump();
    if r.folded {
        s.no_diacritics.bump();
    }
    s.code.entry(r.code.as_str()).or_default().bump();
}

fn is_language(b: NlBucket) -> bool {
    !matches!(b, NlBucket::Unknown | NlBucket::Mixed | NlBucket::Other)
}

/// Count every source. `detector` reads each text once.
pub fn census(
    store: &Store,
    detector: &dyn Ranker,
    opts: &CensusOptions,
) -> Result<CensusReport, IpcError> {
    let schema_version = store.schema_version()?;
    if schema_version < NL_CENSUS_MIN_SCHEMA {
        return Err(IpcError::new(
            codes::E_INVALID,
            format!(
                "this database is at schema {schema_version}; the census needs {NL_CENSUS_MIN_SCHEMA} \
                 or later (open it once with a current fleet to migrate it)"
            ),
        ));
    }
    let since = opts.since();
    let max = opts.max_per_source;
    let fleet = FleetPrompts::from_store(store)?;
    let wanted = |org: Option<i64>| opts.org.is_none() || opts.org == org;
    let mut by_org: BTreeMap<Option<i64>, Acc> = BTreeMap::new();
    let mut capped = Vec::new();

    let prompts = store.nl_census_prompts(since, max)?;
    if prompts.len() as u32 >= max {
        capped.push("prompt".to_string());
    }
    for p in prompts.iter().filter(|p| wanted(p.org_id)) {
        let s = by_org.entry(p.org_id).or_default().source("prompt");
        let Some(text) = fleet.person_text(&p.text, p.last_prompt.as_deref()) else {
            s.fleet_typed.bump();
            continue;
        };
        count(s, super::read_with(detector, &text));
    }

    let items = store.nl_census_items(since, max)?;
    if items.len() as u32 >= max {
        capped.push("title/description".to_string());
    }
    for it in items.iter().filter(|i| wanted(i.org_id)) {
        let acc = by_org.entry(it.org_id).or_default();
        let r = super::read_with(detector, &it.title);
        count(acc.source(&format!("title:{}", it.provider)), r);
        if let Some(d) = it.description.as_deref().filter(|d| !d.trim().is_empty()) {
            let r = super::read_with(detector, d);
            count(acc.source(&format!("description:{}", it.provider)), r);
        }
    }

    let journal = store.nl_census_journal(since, max)?;
    if journal.len() as u32 >= max {
        capped.push("journal".to_string());
    }
    for j in journal.iter().filter(|j| wanted(j.org_id)) {
        let r = super::read_with(detector, &j.body);
        count(
            by_org
                .entry(j.org_id)
                .or_default()
                .source(&format!("journal:{}", j.kind)),
            r,
        );
    }

    let pairs = store.nl_census_pairs(since, max)?;
    if pairs.len() as u32 >= max {
        capped.push("pairs".to_string());
    }
    for pr in pairs.iter().filter(|p| wanted(p.org_id)) {
        let acc = by_org.entry(pr.org_id).or_default();
        acc.pairs.read.bump();
        let Some(prompt) = fleet.person_text(&pr.prompt, pr.last_prompt.as_deref()) else {
            acc.pairs.fleet_typed.bump();
            continue;
        };
        let a = super::read_with(detector, &prompt).bucket;
        let b = super::read_with(detector, &pr.title).bucket;
        if is_language(a) && is_language(b) {
            if a == b {
                acc.pairs.same_language.bump();
            } else {
                acc.pairs.cross_language.bump();
            }
        }
        acc.cells.entry((a, b)).or_default().bump();
    }

    let names: BTreeMap<i64, String> = store
        .list_orgs()?
        .into_iter()
        .map(|o| (o.id, o.name))
        .collect();
    let orgs = by_org
        .into_iter()
        .map(|(org_id, mut acc)| {
            acc.pairs.cells = acc
                .cells
                .iter()
                .map(|(&(a, b), &n)| PairCell {
                    prompt: a.as_str(),
                    title: b.as_str(),
                    count: n,
                })
                .collect();
            acc.pairs
                .cells
                .sort_by(|x, y| y.count.cmp(&x.count).then(x.prompt.cmp(y.prompt)));
            OrgCensus {
                org_id,
                org: match org_id {
                    Some(id) => names
                        .get(&id)
                        .cloned()
                        .unwrap_or_else(|| format!("org #{id}")),
                    None => "(no org)".to_string(),
                },
                sources: acc.sources.into_values().collect(),
                pairs: acc.pairs,
            }
        })
        .collect();

    Ok(CensusReport {
        detector: super::DETECTOR_VERSION,
        schema_version,
        days: opts.days,
        since,
        max_per_source: max,
        capped,
        orgs,
        not_counted: NOT_COUNTED.to_vec(),
    })
}

impl CensusReport {
    /// The census as lines for a terminal.
    pub fn lines(&self) -> Vec<String> {
        let mut v = vec![
            format!("language census — detector {}", self.detector),
            format!(
                "window: {} days; newest {} rows per source; schema {}",
                self.days, self.max_per_source, self.schema_version
            ),
            format!(
                "counts from 1 to {} show as <{0}; no text is printed or kept",
                SUPPRESS_BELOW - 1
            )
            .replace(
                &format!("<{}", SUPPRESS_BELOW - 1),
                &format!("<{SUPPRESS_BELOW}"),
            ),
        ];
        if self.orgs.is_empty() {
            v.push(String::new());
            v.push("nothing to count in this window".into());
        }
        for o in &self.orgs {
            v.push(String::new());
            v.push(match o.org_id {
                Some(id) => format!("org {} (#{id})", o.org),
                None => o.org.clone(),
            });
            for s in &o.sources {
                let mut line = format!("  {:<22} read {}", s.source, s.read);
                if s.fleet_typed.0 > 0 {
                    line.push_str(&format!(", fleet-typed {} (left out)", s.fleet_typed));
                }
                v.push(line);
                if s.languages.is_empty() {
                    continue;
                }
                let langs: Vec<String> = NlBucket::ALL
                    .iter()
                    .filter_map(|b| {
                        s.languages
                            .get(b.as_str())
                            .map(|n| format!("{} {n}", b.as_str()))
                    })
                    .collect();
                v.push(format!("    languages: {}", langs.join(", ")));
                let code: Vec<String> = CodeDensity::ALL
                    .iter()
                    .filter_map(|c| {
                        s.code
                            .get(c.as_str())
                            .map(|n| format!("{} {n}", c.as_str()))
                    })
                    .collect();
                v.push(format!(
                    "    no diacritics (sk/cs): {}; code: {}",
                    s.no_diacritics,
                    code.join(", ")
                ));
            }
            let p = &o.pairs;
            if p.read.0 > 0 {
                v.push(format!(
                    "  pairs (confirmed links, prompt × title): read {}, same language {}, cross-language {}{}",
                    p.read,
                    p.same_language,
                    p.cross_language,
                    if p.fleet_typed.0 > 0 {
                        format!(", fleet-typed {} (left out)", p.fleet_typed)
                    } else {
                        String::new()
                    }
                ));
                for c in &p.cells {
                    v.push(format!("    {:>7} × {:<7} {}", c.prompt, c.title, c.count));
                }
            }
        }
        if !self.capped.is_empty() {
            v.push(String::new());
            v.push(format!(
                "capped at {} rows (newest only): {}",
                self.max_per_source,
                self.capped.join(", ")
            ));
        }
        v.push(String::new());
        v.push("not counted:".into());
        for n in &self.not_counted {
            v.push(format!("  - {n}"));
        }
        v
    }
}

/// Up to `n` first prompts a person wrote, spread evenly over the window,
/// each with the detector's guess as `expect` and `checked: false`: the
/// file a person corrects to measure the detector on their own texts (D46).
/// The texts are the store's own; the caller writes them to a local file.
pub fn sample_prompts(
    store: &Store,
    detector: &dyn Ranker,
    opts: &CensusOptions,
    n: usize,
) -> Result<Vec<LabeledCase>, IpcError> {
    let fleet = FleetPrompts::from_store(store)?;
    // One case per distinct text: a repeated prompt is labeled once.
    let mut seen = std::collections::HashSet::new();
    let mine: Vec<String> = store
        .nl_census_prompts(opts.since(), opts.max_per_source)?
        .into_iter()
        .filter(|p| opts.org.is_none() || opts.org == p.org_id)
        .filter_map(|p| {
            fleet
                .person_text(&p.text, p.last_prompt.as_deref())
                .map(std::borrow::Cow::into_owned)
        })
        .filter(|t| seen.insert(t.clone()))
        .collect();
    if n == 0 || mine.is_empty() {
        return Ok(Vec::new());
    }
    let step = (mine.len() as f64 / n as f64).max(1.0);
    let mut out = Vec::new();
    let mut at = 0.0;
    while (at as usize) < mine.len() && out.len() < n {
        let text = &mine[at as usize];
        let r = super::read_with(detector, text);
        out.push(LabeledCase {
            text: text.clone(),
            expect: r.bucket,
            folded: r.folded,
            kind: "prompt".into(),
            note: String::new(),
            checked: false,
        });
        at += step;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::nl::NlBucket;

    /// Reads by keyword so the tests do not depend on a model: "the" is
    /// English, "sa" Slovak, "und" German, anything else Czech.
    struct Words;
    impl Ranker for Words {
        fn rank(&self, p: &str) -> Vec<(NlBucket, f64)> {
            let has = |w: &str| p.split_whitespace().any(|x| x.eq_ignore_ascii_case(w));
            let b = if has("the") {
                NlBucket::En
            } else if has("sa") {
                NlBucket::Sk
            } else if has("und") {
                NlBucket::De
            } else {
                NlBucket::Cs
            };
            vec![(b, 1.0)]
        }
    }

    /// Rows the store stamps itself (items, journal) carry the real clock.
    static NOW: std::sync::LazyLock<i64> = std::sync::LazyLock::new(crate::store::now_unix);

    fn item(s: &Store, tracker: i64, key: &str, title: &str) {
        s.conn_for_test()
            .execute(
                "INSERT INTO work_items (source, tracker_id, external_id, key, title, created_at, updated_at) \
                 VALUES ('jira', ?1, ?2, ?2, ?3, ?4, ?4)",
                rusqlite::params![tracker, key, title, *NOW],
            )
            .unwrap();
    }

    fn opts() -> CensusOptions {
        CensusOptions::new(None, None, None, *NOW).unwrap()
    }

    fn session(s: &Store, name: &str, host: &str) -> i64 {
        s.upsert_host(host).unwrap();
        s.upsert_session(name, host, None, None, 1, 1, "running", None)
            .unwrap()
    }

    fn conversation(s: &Store, session: i64, cid: &str, at: i64, prompt: &str) {
        s.conn_for_test()
            .execute(
                "INSERT INTO conversations (session_id, claude_session_id, started_at, start_source, first_prompt) \
                 VALUES (?1, ?2, ?3, 'startup', ?4)",
                rusqlite::params![session, cid, at, prompt],
            )
            .unwrap();
    }

    fn org_for_host(s: &Store, name: &str, host: &str) -> i64 {
        let o = s.add_org(name, None, false).unwrap();
        s.set_host_org(host, Some(o.id)).unwrap();
        o.id
    }

    fn source<'a>(r: &'a CensusReport, org: Option<i64>, name: &str) -> &'a SourceCensus {
        r.orgs
            .iter()
            .find(|o| o.org_id == org)
            .and_then(|o| o.sources.iter().find(|s| s.source == name))
            .unwrap_or_else(|| panic!("no {name} for {org:?}: {r:#?}"))
    }

    #[test]
    fn prompts_are_counted_per_org_and_fleets_own_are_left_out() {
        let s = Store::open_in_memory().unwrap();
        let a = session(&s, "a", "h1");
        let b = session(&s, "b", "h2");
        let acme = org_for_host(&s, "Acme", "h1");
        let recent = *NOW - 86_400;
        for i in 0..6 {
            conversation(
                &s,
                a,
                &format!("a{i}"),
                recent,
                "Pozri sa preco padá build na main",
            );
        }
        conversation(
            &s,
            a,
            "a-fleet",
            recent,
            &crate::service::trackers::tickets::start_prompt("PAY-7"),
        );
        conversation(&s, a, "a-chip", recent, "Continue where you left off.");
        conversation(&s, b, "b0", recent, "Look at the failing build on main");
        conversation(
            &s,
            b,
            "b-old",
            *NOW - 400 * 86_400,
            "Look at the old thing on main",
        );

        let r = census(&s, &Words, &opts()).unwrap();
        let p = source(&r, Some(acme), "prompt");
        assert_eq!(p.read, Shown(6));
        assert_eq!(p.fleet_typed, Shown(2));
        assert_eq!(p.languages.get("sk"), Some(&Shown(6)));
        let n = source(&r, None, "prompt");
        assert_eq!(
            n.read,
            Shown(1),
            "the prompt outside the window is not read"
        );
        assert_eq!(n.languages.get("en"), Some(&Shown(1)));
    }

    #[test]
    fn the_org_filter_keeps_one_org() {
        let s = Store::open_in_memory().unwrap();
        let a = session(&s, "a", "h1");
        let b = session(&s, "b", "h2");
        let acme = org_for_host(&s, "Acme", "h1");
        conversation(&s, a, "a0", *NOW - 10, "Pozri sa preco to pada");
        conversation(&s, b, "b0", *NOW - 10, "Look at the build on main");
        let o = CensusOptions::new(None, Some(acme), None, *NOW).unwrap();
        let r = census(&s, &Words, &o).unwrap();
        assert_eq!(r.orgs.len(), 1);
        assert_eq!(r.orgs[0].org, "Acme");
    }

    #[test]
    fn items_and_journal_are_counted_by_provider_and_kind() {
        let s = Store::open_in_memory().unwrap();
        let t = s
            .add_tracker("jira", "Acme Jira", "https://acme.atlassian.net")
            .unwrap();
        let acme = s.add_org("Acme", None, false).unwrap().id;
        s.conn_for_test()
            .execute(
                "UPDATE trackers SET org_id = ?1 WHERE id = ?2",
                rusqlite::params![acme, t.id],
            )
            .unwrap();
        item(&s, t.id, "PAY-1", "Fix the login page on mobile");
        let a = session(&s, "a", "h1");
        conversation(&s, a, "c1", *NOW - 10, "Look at the build on main");
        s.append_journal(
            Some("c1"),
            None,
            "note",
            "agent",
            Some("Oprava je hotova und test tiez"),
            None,
        )
        .unwrap();
        s.append_journal(
            Some("c1"),
            None,
            "status_change",
            "fleet",
            Some("To Do -> Done"),
            None,
        )
        .unwrap();

        let r = census(&s, &Words, &opts()).unwrap();
        let title = source(&r, Some(acme), "title:jira");
        assert_eq!(title.languages.get("en"), Some(&Shown(1)));
        let note = source(&r, None, "journal:note");
        assert_eq!(note.read, Shown(1));
        assert!(
            r.orgs.iter().all(|o| o
                .sources
                .iter()
                .all(|x| x.source != "journal:status_change")),
            "fleet's own journal rows are never read"
        );
    }

    #[test]
    fn confirmed_links_become_language_pairs() {
        let s = Store::open_in_memory().unwrap();
        let t = s
            .add_tracker("jira", "Acme Jira", "https://acme.atlassian.net")
            .unwrap();
        item(&s, t.id, "PAY-1", "Fix the login page on mobile");
        let a = session(&s, "a", "h1");
        s.conn_for_test()
            .execute(
                "UPDATE sessions SET claude_session_id = 'c1' WHERE id = ?1",
                [a],
            )
            .unwrap();
        conversation(&s, a, "c1", *NOW - 10, "Pozri sa preco nejde prihlasenie");
        s.link_session_work(a, crate::store::WorkTarget::Key("PAY-1"), "manual")
            .unwrap();

        let r = census(&s, &Words, &opts()).unwrap();
        let pairs = &r
            .orgs
            .iter()
            .find(|o| o.pairs.read.0 > 0)
            .expect("a pair")
            .pairs;
        assert_eq!(pairs.cross_language, Shown(1));
        assert_eq!(
            pairs.cells,
            vec![PairCell {
                prompt: "sk",
                title: "en",
                count: Shown(1)
            }]
        );
    }

    #[test]
    fn small_counts_show_as_under_five_in_json_and_lines() {
        assert_eq!(
            serde_json::to_value(Shown(3)).unwrap(),
            serde_json::json!("<5")
        );
        assert_eq!(
            serde_json::to_value(Shown(0)).unwrap(),
            serde_json::json!(0)
        );
        assert_eq!(
            serde_json::to_value(Shown(5)).unwrap(),
            serde_json::json!(5)
        );
        assert_eq!(Shown(1).to_string(), "<5");
        let s = Store::open_in_memory().unwrap();
        let a = session(&s, "a", "h1");
        conversation(&s, a, "a0", *NOW - 10, "Look at the build on main");
        let lines = census(&s, &Words, &opts()).unwrap().lines().join("\n");
        assert!(lines.contains("read <5"), "{lines}");
        assert!(lines.contains("show as <5"), "{lines}");
    }

    #[test]
    fn the_report_never_holds_a_text() {
        let s = Store::open_in_memory().unwrap();
        let a = session(&s, "a", "h1");
        conversation(
            &s,
            a,
            "a0",
            *NOW - 10,
            "Secret plan for the ACME merger talks",
        );
        let r = census(&s, &Words, &opts()).unwrap();
        let json = serde_json::to_string(&r).unwrap();
        let lines = r.lines().join("\n");
        for out in [json, lines] {
            assert!(!out.contains("Secret") && !out.contains("merger"), "{out}");
        }
    }

    #[test]
    fn every_fleet_template_is_recognised() {
        let f = FleetPrompts::new(&quick_replies::defaults());
        for p in [
            crate::service::trackers::tickets::start_prompt("ABC-12"),
            crate::service::work::resume::start_prompt("ABC-12"),
            crate::service::safe_kill::build_safe_kill_prompt("f00d"),
            crate::service::work::agent_handover::build_prompt("ABC-12", "n1"),
            "[claude-fleet: message from task #3] done".to_string(),
        ] {
            let stored: String = p.chars().take(FIRST_PROMPT_CHARS).collect();
            assert!(f.is_fleet(&stored, None), "{stored}");
        }
        for chip in quick_replies::defaults() {
            let stored: String = chip.text.chars().take(FIRST_PROMPT_CHARS).collect();
            assert!(f.is_fleet(&stored, None), "{stored}");
        }
        assert!(!f.is_fleet("Start on the login bug, it is urgent", None));
        assert!(!f.is_fleet("Pozri sa na ten build", None));
    }

    /// Shaped like the six of 25 exported on the production hub
    /// (2026-09-28), as the store keeps them: cut at 200 characters.
    const TASK_NOTIFICATION: &str = "<task-notification>\n<task-type>artifact-watch-lifecycle\
        </task-type>\n<summary>Stopped watching Artifact \"Release notes\": the artifact was \
        deleted.</summary>\n<status>stopped</status>\n<note>Nothing to do.</note>\n\
        </task-notification>";

    #[test]
    fn what_claude_code_submits_itself_is_left_out_like_fleets_own() {
        let f = FleetPrompts::new(&quick_replies::defaults());
        let stored: String = TASK_NOTIFICATION.chars().take(FIRST_PROMPT_CHARS).collect();
        assert!(f.is_fleet(&stored, None));
        assert_eq!(f.person_text(&stored, None), None);
        assert_eq!(
            f.person_text(
                "<system-reminder>Plan mode.</system-reminder>\nfix PAY-7 on mobile",
                None
            )
            .as_deref(),
            Some("fix PAY-7 on mobile"),
            "a row stored with a harness head is read without it"
        );
        assert_eq!(
            f.person_text("  Pozri sa na ten build ", None).as_deref(),
            Some("Pozri sa na ten build")
        );

        let s = Store::open_in_memory().unwrap();
        let a = session(&s, "a", "h1");
        conversation(&s, a, "c0", *NOW - 10, "Look at the build on main");
        conversation(&s, a, "c1", *NOW - 10, &stored);
        conversation(
            &s,
            a,
            "c2",
            *NOW - 10,
            "<command-message>review</command-message>\n<command-name>/review</command-name>",
        );
        let r = census(&s, &Words, &opts()).unwrap();
        let p = source(&r, None, "prompt");
        assert_eq!(p.fleet_typed, Shown(2));
        assert_eq!(p.languages.get("en"), Some(&Shown(1)));
        let got = sample_prompts(&s, &Words, &opts(), 10).unwrap();
        assert_eq!(got.len(), 1, "{got:?}");
    }

    #[test]
    fn options_are_bounded() {
        assert!(CensusOptions::new(Some(0), None, None, *NOW).is_err());
        assert!(CensusOptions::new(Some(MAX_DAYS + 1), None, None, *NOW).is_err());
        assert!(CensusOptions::new(None, None, Some(0), *NOW).is_err());
        assert_eq!(opts().since(), *NOW - 90 * 86_400);
    }

    #[test]
    fn an_old_schema_is_refused_with_the_version_named() {
        let s = Store::open_in_memory().unwrap();
        s.conn_for_test()
            .execute("DELETE FROM schema_version WHERE version >= 50", [])
            .unwrap();
        let e = census(&s, &Words, &opts()).unwrap_err();
        assert!(e.message.contains("schema"), "{}", e.message);
    }

    #[test]
    fn a_sample_spreads_over_the_window_and_is_unchecked() {
        let s = Store::open_in_memory().unwrap();
        let a = session(&s, "a", "h1");
        for i in 0..10 {
            conversation(
                &s,
                a,
                &format!("c{i}"),
                *NOW - 100 - i,
                &format!("Look at the build number {i} now"),
            );
        }
        conversation(&s, a, "fleet", *NOW - 50, "Continue where you left off.");
        conversation(&s, a, "again", *NOW - 60, "Look at the build number 3 now");
        let got = sample_prompts(&s, &Words, &opts(), 4).unwrap();
        assert_eq!(got.len(), 4);
        assert!(got.iter().all(|c| !c.checked && c.expect == NlBucket::En));
        assert!(got.iter().all(|c| !c.text.starts_with("Continue")));
        let texts: std::collections::HashSet<_> = got.iter().map(|c| c.text.clone()).collect();
        assert_eq!(texts.len(), got.len(), "no text twice");
    }
}
