//! `search` (search phase 3): one query over the full-text index
//! (`store/search.rs`, migration 163) — tasks and tickets, sessions,
//! conversations' first prompts, pull requests, the work journal, and the
//! transcript chunks the transcript pass copies in when
//! `search.index_transcripts` is on.
//!
//! Every hit is fenced before it leaves: a session's (and its
//! conversation's, transcript's or pull request's) through
//! `sees_session_row`; a conversation whose session is gone through
//! `sees_past_conversation`; a work item through the org's visible items;
//! anything that names neither only for the hub's own unnarrowed reader.
//! The index is over-read so a fenced-out match does not leave the page
//! short.

use crate::ipc_error::{codes, lock, IpcError};
use crate::service::trackers::tickets::item_visible;
use crate::service::view_scope::ViewScope;
use crate::store::{SearchDocHit, SessionRow, Store, MARK_CLOSE, MARK_OPEN};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Mutex;

/// What a hit can be.
pub const SEARCH_KINDS: [&str; 6] = [
    "item",
    "session",
    "conversation",
    "pr",
    "journal",
    "transcript",
];
pub const SEARCH_DEFAULT_LIMIT: usize = 20;
pub const SEARCH_MAX_LIMIT: usize = 100;
/// Words of a query that count; the rest are dropped.
const MAX_TERMS: usize = 12;
const MAX_QUERY_CHARS: usize = 200;

#[derive(Debug, Clone, Default, Deserialize, Serialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct SearchArgs {
    /// Words to find, every one, in any order; case and accents ignored, and
    /// each word matches as a prefix ("prihl" finds "prihlásenie").
    pub query: String,
    /// Only these kinds: item (tasks and tickets), session, conversation
    /// (a conversation's first prompt), pr, journal (a conversation's end,
    /// a handover, a summary) and transcript (conversation text, only when
    /// the hub indexes transcripts). Empty: every kind.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub kinds: Vec<String>,
    /// 1–100, default 20.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
}

/// One match the caller may see.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchHit {
    /// One of [`SEARCH_KINDS`].
    pub kind: String,
    /// The source row's id (a transcript chunk: `<claude id>:<offset>`).
    #[serde(rename = "ref")]
    pub ref_id: String,
    pub title: String,
    /// `[start, end)` of each match in `title`, in UTF-16 code units (what
    /// a JavaScript or Kotlin string indexes by).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub title_marks: Vec<[u32; 2]>,
    /// A few words around the matches in the text ("…" where it is cut).
    pub snippet: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub snippet_marks: Vec<[u32; 2]>,
    /// When the thing happened or last changed, unix seconds.
    pub at: i64,
    /// The live session it belongs to, when there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_alias: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claude_session_id: Option<String>,
    /// A work item hit: the task (`item:<id>`) and its key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchPage {
    pub hits: Vec<SearchHit>,
    /// The hub copies conversation text into the index
    /// (`search.index_transcripts`); without it a conversation is found by
    /// its first prompt and its journal only.
    pub transcripts_indexed: bool,
}

/// The FTS5 expression for `query`: each word a quoted prefix term, all of
/// them required. `None` when no word has a letter or digit.
pub fn fts_query(query: &str) -> Option<String> {
    let terms: Vec<String> = query
        .split_whitespace()
        .map(|w| w.replace('"', ""))
        .filter(|w| w.chars().any(char::is_alphanumeric))
        .take(MAX_TERMS)
        .map(|w| format!("\"{w}\"*"))
        .collect();
    (!terms.is_empty()).then(|| terms.join(" AND "))
}

/// The text without its match markers, and where the marked runs were.
fn unmark(raw: &str) -> (String, Vec<[u32; 2]>) {
    let mut text = String::with_capacity(raw.len());
    let mut marks = Vec::new();
    let mut at: u32 = 0;
    let mut open: Option<u32> = None;
    for c in raw.chars() {
        match c {
            MARK_OPEN => open = Some(at),
            MARK_CLOSE => {
                if let Some(s) = open.take() {
                    if s < at {
                        marks.push([s, at]);
                    }
                }
            }
            c => {
                text.push(c);
                at += c.len_utf16() as u32;
            }
        }
    }
    (text, marks)
}

/// May `view` see this match? Returns the session row it belongs to, when
/// that row exists and is visible, for the hit's name and host.
fn fence(
    s: &Store,
    view: &ViewScope,
    d: &SearchDocHit,
    sessions: &mut HashMap<i64, Option<SessionRow>>,
) -> Result<(bool, Option<SessionRow>), IpcError> {
    if d.kind == "item" {
        let Some(id) = d.item_id else {
            return Ok((false, None));
        };
        if view.is_unrestricted() {
            return Ok((true, None));
        }
        let Some(item) = s.get_work_item(id)? else {
            return Ok((false, None));
        };
        return Ok((item_visible(&view.org, s, &item)?, None));
    }
    if let Some(sid) = d.session_id {
        let row = match sessions.get(&sid) {
            Some(r) => r.clone(),
            None => {
                let r = s.get_session_by_id(sid)?;
                sessions.insert(sid, r.clone());
                r
            }
        };
        if let Some(row) = row {
            return Ok(if view.sees_session_row(&row).is_visible() {
                (true, Some(row))
            } else {
                (false, None)
            });
        }
    }
    if let Some(cid) = d.claude_session_id.as_deref() {
        return Ok((view.sees_past_conversation(s, cid)?, None));
    }
    Ok((view.is_unrestricted(), None))
}

/// `search { query, kinds?, limit? }`.
pub fn search(
    store: &Mutex<Store>,
    view: &ViewScope,
    args: &SearchArgs,
) -> Result<SearchPage, IpcError> {
    if args.query.chars().count() > MAX_QUERY_CHARS {
        return Err(IpcError::new(
            codes::E_INVALID,
            format!("query is longer than {MAX_QUERY_CHARS} characters"),
        ));
    }
    for k in &args.kinds {
        if !SEARCH_KINDS.contains(&k.as_str()) {
            return Err(IpcError::new(
                codes::E_INVALID,
                format!(
                    "kinds holds item, session, conversation, pr, journal or transcript, not {k:?}"
                ),
            ));
        }
    }
    let limit = args
        .limit
        .unwrap_or(SEARCH_DEFAULT_LIMIT)
        .clamp(1, SEARCH_MAX_LIMIT);
    let s = lock(store)?;
    let transcripts_indexed =
        crate::service::settings::get_bool(&s, crate::service::settings::SEARCH_INDEX_TRANSCRIPTS);
    let Some(q) = fts_query(&args.query) else {
        return Ok(SearchPage {
            hits: Vec::new(),
            transcripts_indexed,
        });
    };
    let kinds: Vec<&str> = args.kinds.iter().map(String::as_str).collect();
    // Over-read: a page of fenced-out matches must not read as "nothing".
    let raw = s.search_docs_matching(&q, &kinds, (limit * 5).min(500))?;
    let mut sessions = HashMap::new();
    let mut hits = Vec::new();
    for d in raw {
        if hits.len() >= limit {
            break;
        }
        let (visible, row) = fence(&s, view, &d, &mut sessions)?;
        if !visible {
            continue;
        }
        let (title, title_marks) = unmark(&d.title);
        let (snippet, snippet_marks) = unmark(&d.snippet);
        let key = match d.item_id {
            Some(id) if d.kind == "item" => s.get_work_item(id)?.and_then(|i| i.key),
            _ => None,
        };
        hits.push(SearchHit {
            kind: d.kind.clone(),
            ref_id: d.ref_id,
            title,
            title_marks,
            snippet,
            snippet_marks,
            at: d.at,
            session_id: row.as_ref().map(|r| r.id),
            session_name: row.as_ref().map(|r| {
                r.friendly_name
                    .clone()
                    .filter(|n| !n.is_empty())
                    .unwrap_or_else(|| r.tmux_name.clone())
            }),
            host_alias: row.as_ref().map(|r| r.host_alias.clone()),
            claude_session_id: d.claude_session_id,
            task_id: (d.kind == "item")
                .then(|| d.item_id.map(|i| format!("item:{i}")))
                .flatten(),
            key,
        });
    }
    Ok(SearchPage {
        hits,
        transcripts_indexed,
    })
}

#[cfg(test)]
#[path = "search_tests.rs"]
mod tests;
