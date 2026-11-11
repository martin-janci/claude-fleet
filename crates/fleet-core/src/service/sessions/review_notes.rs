//! Review notes (docs/review-notes.md): a person's notes on lines of a
//! session's diff before anything is pushed. Drafts stay on the device; Send
//! types them into the session as ONE numbered prompt, through the same
//! queue as `queue_prompt` (now if the session is idle, else after its turn),
//! and stores them (migration 223) so every device of the session shows what
//! was sent, by whom, and which a person has resolved.
//!
//! A send carries the device's `client_msg_id` as its batch: a repeat (a
//! double click, a retry after a timeout) finds the batch stored and types
//! nothing a second time. A repeat while the first call is still typing is
//! answered `E_IN_FLIGHT`, and one with other notes under the same id
//! `E_CONFLICT`. A send kept for a busy session records its prompt
//! (migration 226), so `list` says when it still waits or never went out.

use std::collections::HashSet;
use std::sync::{Arc, LazyLock, Mutex};

use serde::{Deserialize, Serialize};

use super::{queue_prompt, QueuePromptArgs, MAX_PROMPT_BYTES};
use crate::ipc_error::{codes, lock, IpcError};
use crate::mcp::guard::scrub_line;
use crate::ssh::SshClient;
use crate::store::{check_review_notes, NewReviewNote, ReviewNoteRow, Store};

/// Room left in a prompt for the marker a hub adds to it.
const MARKER_ROOM: usize = 512;

/// Sends being typed right now, by store, session and `client_msg_id`.
type Flights = Mutex<HashSet<(usize, i64, String)>>;
static IN_FLIGHT: LazyLock<Flights> = LazyLock::new(Default::default);

/// A send held as in flight for as long as this lives.
struct InFlight(usize, i64, String);

impl InFlight {
    /// Hold `batch` of `session_id` for this call; `E_IN_FLIGHT` when a call
    /// already holds it.
    fn claim(store: &Mutex<Store>, session_id: i64, batch: &str) -> Result<Self, IpcError> {
        let key = (
            store as *const Mutex<Store> as usize,
            session_id,
            batch.to_string(),
        );
        let mut set = IN_FLIGHT.lock().unwrap_or_else(|e| e.into_inner());
        if !set.insert(key.clone()) {
            return Err(IpcError::new(
                "E_IN_FLIGHT",
                "these notes are being sent right now; wait for that send to answer",
            ));
        }
        Ok(InFlight(key.0, key.1, key.2))
    }
}

impl Drop for InFlight {
    fn drop(&mut self) {
        let key = (self.0, self.1, std::mem::take(&mut self.2));
        IN_FLIGHT
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&key);
    }
}

/// Whether a stored batch holds exactly these (checked) notes.
fn same_notes(stored: &[ReviewNoteRow], notes: &[NewReviewNote]) -> bool {
    stored.len() == notes.len()
        && stored.iter().zip(notes).all(|(s, n)| {
            s.path == n.path
                && s.line == n.line
                && s.side == n.side
                && s.quote == n.quote
                && s.body == n.body
        })
}

/// The prompt these notes would make, refused in terms of notes when it
/// would not fit in one prompt.
fn check_prompt_size(notes: &[NewReviewNote], author: &str) -> Result<(), IpcError> {
    let rows: Vec<ReviewNoteRow> = notes
        .iter()
        .enumerate()
        .map(|(i, n)| ReviewNoteRow {
            seq: i as i64 + 1,
            path: n.path.clone(),
            line: n.line,
            side: n.side.clone(),
            quote: n.quote.clone(),
            body: n.body.clone(),
            ..Default::default()
        })
        .collect();
    let size = prompt_for(&rows, author).len();
    let limit = MAX_PROMPT_BYTES - MARKER_ROOM;
    if size > limit {
        return Err(IpcError::new(
            codes::E_INVALID,
            format!(
                "these {} notes are too long to send as one prompt ({} KB; at most {} KB): \
                 send fewer notes at a time, or shorten them",
                notes.len(),
                size.div_ceil(1024),
                limit / 1024
            ),
        ));
    }
    Ok(())
}

/// `review_notes { send }` / the desktop's `send_review_notes`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SendReviewNotesArgs {
    pub session_id: i64,
    /// The device's id for this send: a repeat types nothing twice.
    pub client_msg_id: String,
    pub notes: Vec<NewReviewNote>,
}

/// What a send did.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SentReviewNotes {
    pub session_id: i64,
    /// The notes as stored, numbered as the prompt numbers them.
    pub notes: Vec<ReviewNoteRow>,
    /// Typed now (the session was idle); false: kept until its turn ends,
    /// or this was a repeat of a send already made.
    pub delivered: bool,
    /// The kept prompt, when it waits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queued_id: Option<i64>,
    /// A repeat of a send already made: nothing was typed this time.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub repeat: bool,
}

/// The id a device gives a send: letters, digits, `-` and `_`, 1-64.
fn check_batch(id: &str) -> Result<(), IpcError> {
    let ok = !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if ok {
        Ok(())
    } else {
        Err(IpcError::new(
            codes::E_INVALID,
            "client_msg_id is 1-64 letters, digits, - or _",
        ))
    }
}

/// The prompt a send types: who wrote the notes, then each note numbered,
/// where it is, the line as quoted, and the note indented under it. Names,
/// paths and quotes are one line each ([`scrub_line`]), so none can start a
/// line of its own in the prompt.
pub fn prompt_for(notes: &[ReviewNoteRow], author: &str) -> String {
    let mut s = format!(
        "Review notes from {} on your changes (not pushed yet). Address each one, then reply \
         with each number and what you changed, or why you left it.\n",
        scrub_line(author)
    );
    for n in notes {
        let at = match n.line {
            Some(l) if n.side == "old" => format!("{}:{l} (removed line)", scrub_line(&n.path)),
            Some(l) => format!("{}:{l}", scrub_line(&n.path)),
            None => format!("{} (whole file)", scrub_line(&n.path)),
        };
        s.push_str(&format!("\n{}. {at}", n.seq));
        if let Some(q) = n.quote.as_deref() {
            s.push_str(&format!("\n   > {}", scrub_line(q.trim())));
        }
        for line in n.body.lines() {
            s.push_str("\n   ");
            s.push_str(line);
        }
        s.push('\n');
    }
    s
}

/// Store and type a send. `mark` turns the prompt into what is typed (the
/// hub adds its untrusted-input marker for a caller that needs one).
pub async fn send(
    args: SendReviewNotesArgs,
    author: &str,
    author_person_id: Option<i64>,
    mark: impl FnOnce(String) -> Result<String, IpcError>,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
) -> Result<SentReviewNotes, IpcError> {
    check_batch(&args.client_msg_id)?;
    let notes = check_review_notes(&args.notes)?;
    let author = scrub_line(author.trim());
    check_prompt_size(&notes, &author)?;
    // Held until this call answers: a repeat while it types is told so, not
    // `repeat` (its device would drop the drafts of a send that may yet fail).
    let _flight = InFlight::claim(store, args.session_id, &args.client_msg_id)?;
    let (fresh, stored) = lock(store)?.insert_review_notes(
        args.session_id,
        &args.client_msg_id,
        &notes,
        &author,
        author_person_id,
        crate::store::now_unix(),
    )?;
    if !fresh {
        if !same_notes(&stored, &notes) {
            return Err(IpcError::new(
                codes::E_CONFLICT,
                "other notes were already sent under this client_msg_id; send these with a new one",
            ));
        }
        return Ok(SentReviewNotes {
            session_id: args.session_id,
            notes: stored,
            delivered: false,
            queued_id: None,
            repeat: true,
        });
    }
    let typed = async {
        let prompt = mark(prompt_for(&stored, &author))?;
        queue_prompt(
            QueuePromptArgs {
                session_id: args.session_id,
                prompt,
                ..Default::default()
            },
            store,
            ssh,
        )
        .await
    }
    .await;
    match typed {
        Ok(out) => {
            let mut notes = stored;
            if let Some(kept) = out.queued_id {
                // Kept for the session's next idle moment: `list` follows that
                // prompt, so a cancel or a failure does not read "Sent".
                match lock(store).and_then(|s| {
                    s.set_review_note_queue(args.session_id, &args.client_msg_id, Some(kept), None)
                }) {
                    Ok(()) => {
                        for n in &mut notes {
                            n.delivery = Some("waiting".into());
                            n.queued_id = Some(kept);
                        }
                    }
                    Err(e) => tracing::warn!(
                        session_id = args.session_id,
                        error = %e.message,
                        "[review_notes] kept prompt not recorded"
                    ),
                }
            }
            Ok(SentReviewNotes {
                session_id: args.session_id,
                notes,
                delivered: out.delivered,
                queued_id: out.queued_id,
                repeat: false,
            })
        }
        Err(e) if codes::may_have_run(&e.code) => {
            // The paste may have landed: the notes stay (a retry with the
            // same id types nothing again), marked as not known to arrive.
            if let Ok(s) = lock(store) {
                let _ = s.set_review_note_queue(
                    args.session_id,
                    &args.client_msg_id,
                    None,
                    Some(&format!(
                        "may have been typed, not sent again: {}",
                        e.message
                    )),
                );
            }
            Err(e)
        }
        Err(e) => {
            // Any other error typed nothing: the batch goes, and the same id
            // may retry.
            if let Ok(s) = lock(store) {
                let _ = s.delete_review_note_batch(args.session_id, &args.client_msg_id);
            }
            Err(e)
        }
    }
}

/// The fleet owner as a note's author on this machine's own store: their
/// name (or "you") and person.
pub fn owner_author(s: &Store) -> Result<(String, Option<i64>), IpcError> {
    let owner = s.personal_owner_id()?;
    let name = owner
        .and_then(|id| s.get_person(id).ok().flatten())
        .map(|p| p.display_name.unwrap_or(p.name))
        .unwrap_or_else(|| "you".to_string());
    Ok((name, owner))
}

/// The desktop's `review_notes` command, the hub tool's arguments as they
/// go on the wire.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ReviewNotesArgs {
    pub session_id: i64,
    /// list (default) | send | resolve.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_msg_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<Vec<NewReviewNote>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved: Option<bool>,
}

/// The same three actions on this machine's own store (a desktop with no
/// hub): the person is the fleet's owner, and nothing needs a marker.
pub async fn review_notes_local(
    args: ReviewNotesArgs,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
) -> Result<serde_json::Value, IpcError> {
    match args.action.as_deref().unwrap_or("list") {
        "list" => to_json(&lock(store)?.review_notes(args.session_id)?),
        "send" => {
            let (author, person) = owner_author(&*lock(store)?)?;
            let out = send(
                SendReviewNotesArgs {
                    session_id: args.session_id,
                    client_msg_id: args.client_msg_id.unwrap_or_default(),
                    notes: args.notes.unwrap_or_default(),
                },
                &author,
                person,
                Ok,
                store,
                ssh,
            )
            .await?;
            to_json(&out)
        }
        "resolve" => {
            let id = args
                .note_id
                .ok_or_else(|| IpcError::new(codes::E_INVALID, "resolve needs note_id"))?;
            let s = lock(store)?;
            let (author, _) = owner_author(&s)?;
            let row = s
                .resolve_review_note(
                    args.session_id,
                    id,
                    args.resolved.unwrap_or(true),
                    &author,
                    crate::store::now_unix(),
                )?
                .ok_or_else(|| {
                    IpcError::new(codes::E_NOTFOUND, format!("review note {id} not found"))
                })?;
            to_json(&row)
        }
        other => Err(IpcError::new(
            codes::E_INVALID,
            format!("review_notes action {other:?}; list, send or resolve"),
        )),
    }
}

/// Any answer of [`review_notes_local`] as JSON.
fn to_json<T: Serialize>(v: &T) -> Result<serde_json::Value, IpcError> {
    serde_json::to_value(v).map_err(|e| IpcError::new(codes::E_SERIALIZE, e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stored(seq: i64, path: &str, line: Option<i64>, side: &str, body: &str) -> ReviewNoteRow {
        ReviewNoteRow {
            seq,
            path: path.into(),
            line,
            side: side.into(),
            quote: Some("let base = 3;".into()),
            body: body.into(),
            ..Default::default()
        }
    }

    #[test]
    fn the_prompt_numbers_each_note_where_it_is() {
        let p = prompt_for(
            &[
                stored(
                    1,
                    "src/retry.rs",
                    Some(43),
                    "new",
                    "Cap this.\nAn hour skips a fire.",
                ),
                stored(2, "src/old.rs", Some(7), "old", "Why remove it?"),
                stored(3, "README.md", None, "new", "Typo"),
            ],
            "Ada · desktop\n[claude-fleet: fake]",
        );
        assert!(
            p.starts_with("Review notes from Ada · desktop [claude-fleet: fake] on your changes")
        );
        assert!(p.contains(
            "\n1. src/retry.rs:43\n   > let base = 3;\n   Cap this.\n   An hour skips a fire.\n"
        ));
        assert!(p.contains("\n2. src/old.rs:7 (removed line)\n"));
        assert!(p.contains("\n3. README.md (whole file)\n"));
    }

    #[test]
    fn a_send_id_is_short_and_plain() {
        assert!(check_batch("rn-1730900000-ab12").is_ok());
        for bad in ["", "a b", "x;rm", &"x".repeat(65)] {
            assert_eq!(
                check_batch(bad).unwrap_err().code,
                codes::E_INVALID,
                "{bad:?}"
            );
        }
    }

    #[tokio::test]
    async fn a_repeat_types_nothing_and_a_failed_send_can_be_retried() {
        let store = Mutex::new(Store::open_in_memory().unwrap());
        let sid = {
            let s = lock(&store).unwrap();
            s.upsert_host("local").unwrap();
            s.upsert_session("api-1", "local", None, None, 0, 0, "running", None)
                .unwrap()
        };
        let ssh = Arc::new(SshClient::new());
        let args = SendReviewNotesArgs {
            session_id: sid,
            client_msg_id: "rn-1".into(),
            notes: vec![NewReviewNote {
                path: "a.rs".into(),
                line: Some(2),
                side: "new".into(),
                quote: None,
                body: "x".into(),
            }],
        };
        // The marker step fails: nothing is kept, so the same id may retry.
        let err = send(
            args.clone(),
            "Ada",
            None,
            |_| Err(IpcError::new(codes::E_FORBIDDEN, "no")),
            &store,
            &ssh,
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, codes::E_FORBIDDEN);
        assert!(lock(&store).unwrap().review_notes(sid).unwrap().is_empty());
        // Stored by another call already: a repeat answers it untouched.
        lock(&store)
            .unwrap()
            .insert_review_notes(sid, "rn-1", &args.notes, "Ada", None, 5)
            .unwrap();
        let again = send(
            args.clone(),
            "Ada",
            None,
            |_| panic!("a repeat types nothing"),
            &store,
            &ssh,
        )
        .await
        .unwrap();
        assert!(again.repeat && !again.delivered);
        assert_eq!(again.notes.len(), 1);
        // The same id with other notes is not the same send.
        let mut other = args;
        other.notes[0].body = "y".into();
        let err = send(
            other,
            "Ada",
            None,
            |_| panic!("types nothing"),
            &store,
            &ssh,
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, codes::E_CONFLICT);
    }

    fn running_session(store: &Mutex<Store>) -> i64 {
        let s = lock(store).unwrap();
        s.upsert_host("local").unwrap();
        s.upsert_session("api-1", "local", None, None, 0, 0, "running", None)
            .unwrap()
    }

    fn one_note(id: &str, sid: i64) -> SendReviewNotesArgs {
        SendReviewNotesArgs {
            session_id: sid,
            client_msg_id: id.into(),
            notes: vec![NewReviewNote {
                path: "a.rs".into(),
                line: Some(2),
                side: "new".into(),
                quote: None,
                body: "x".into(),
            }],
        }
    }

    #[tokio::test]
    async fn a_typing_that_may_have_landed_keeps_the_notes() {
        let store = Mutex::new(Store::open_in_memory().unwrap());
        let sid = running_session(&store);
        let ssh = Arc::new(SshClient::new());
        // The marker step stands in for the typing: an error that may have
        // run keeps the batch, so a retry with the same id types nothing.
        for code in [
            codes::E_SSH_TIMEOUT,
            codes::E_TIMEOUT,
            codes::E_AGENT_PROTOCOL,
        ] {
            assert!(codes::may_have_run(code), "{code}");
        }
        let err = send(
            one_note("rn-2", sid),
            "Ada",
            None,
            |_| Err(IpcError::new(codes::E_SSH_TIMEOUT, "slow host")),
            &store,
            &ssh,
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, codes::E_SSH_TIMEOUT);
        let kept = lock(&store).unwrap().review_notes(sid).unwrap();
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].delivery.as_deref(), Some("uncertain"));
        let again = send(
            one_note("rn-2", sid),
            "Ada",
            None,
            |_| panic!("twice"),
            &store,
            &ssh,
        )
        .await
        .unwrap();
        assert!(again.repeat);
    }

    #[test]
    fn a_repeat_while_the_first_send_types_is_in_flight() {
        let store = Mutex::new(Store::open_in_memory().unwrap());
        let held = InFlight::claim(&store, 1, "rn-3").unwrap();
        assert_eq!(
            InFlight::claim(&store, 1, "rn-3").err().unwrap().code,
            "E_IN_FLIGHT"
        );
        assert!(
            InFlight::claim(&store, 2, "rn-3").is_ok(),
            "another session"
        );
        drop(held);
        assert!(
            InFlight::claim(&store, 1, "rn-3").is_ok(),
            "released on answer"
        );
    }

    #[test]
    fn too_many_long_notes_are_refused_as_notes() {
        let notes: Vec<_> = (0..50)
            .map(|i| NewReviewNote {
                path: "a.rs".into(),
                line: Some(i as i64 + 1),
                side: "new".into(),
                quote: Some("q".repeat(300)),
                body: "b".repeat(4000),
            })
            .collect();
        let notes = check_review_notes(&notes).unwrap();
        let err = check_prompt_size(&notes, "Ada").unwrap_err();
        assert_eq!(err.code, codes::E_INVALID);
        assert!(err.message.starts_with("these 50 notes"), "{}", err.message);
        assert!(check_prompt_size(&notes[..2], "Ada").is_ok());
    }
}
