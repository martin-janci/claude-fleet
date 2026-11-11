//! Review notes (docs/review-notes.md): a person's notes on lines of a
//! session's diff before anything is pushed. Drafts stay on the device; Send
//! types them into the session as ONE numbered prompt, through the same
//! queue as `queue_prompt` (now if the session is idle, else after its turn),
//! and stores them (migration 223) so every device of the session shows what
//! was sent, by whom, and which a person has resolved.
//!
//! A send carries the device's `client_msg_id` as its batch: a repeat (a
//! double click, a retry after a timeout) finds the batch stored and types
//! nothing a second time.

use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use super::{queue_prompt, QueuePromptArgs};
use crate::ipc_error::{codes, lock, IpcError};
use crate::ssh::SshClient;
use crate::store::{check_review_notes, NewReviewNote, ReviewNoteRow};

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

/// One line of text, so a name or a quote can never start a line of its own
/// in the prompt.
fn one_line(s: &str) -> String {
    s.chars()
        .map(|c| {
            if crate::store::breaks_a_line(c) {
                ' '
            } else {
                c
            }
        })
        .collect()
}

/// The prompt a send types: who wrote the notes, then each note numbered,
/// where it is, the line as quoted, and the note indented under it.
pub fn prompt_for(notes: &[ReviewNoteRow], author: &str) -> String {
    let mut s = format!(
        "Review notes from {} on your changes (not pushed yet). Address each one, then reply \
         with each number and what you changed, or why you left it.\n",
        one_line(author)
    );
    for n in notes {
        let at = match n.line {
            Some(l) if n.side == "old" => format!("{}:{l} (removed line)", one_line(&n.path)),
            Some(l) => format!("{}:{l}", one_line(&n.path)),
            None => format!("{} (whole file)", one_line(&n.path)),
        };
        s.push_str(&format!("\n{}. {at}", n.seq));
        if let Some(q) = n.quote.as_deref() {
            s.push_str(&format!("\n   > {}", one_line(q.trim())));
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
    let author = one_line(author.trim());
    let fresh = {
        let s = lock(store)?;
        s.insert_review_notes(
            args.session_id,
            &args.client_msg_id,
            &notes,
            &author,
            author_person_id,
            crate::store::now_unix(),
        )?
    };
    let stored = lock(store)?.review_note_batch(args.session_id, &args.client_msg_id)?;
    if !fresh {
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
        Ok(out) => Ok(SentReviewNotes {
            session_id: args.session_id,
            notes: stored,
            delivered: out.delivered,
            queued_id: out.queued_id,
            repeat: false,
        }),
        Err(e) => {
            // A timeout or a lost agent answer may have typed the notes
            // (`queue_prompt` keeps such a row as "may have been typed, not
            // sent again"): the batch stays, so a retry with the same id
            // answers `repeat` and types nothing twice. Any other error
            // typed nothing: the batch goes, and the same id may retry.
            if !codes::may_have_run(&e.code) {
                if let Ok(s) = lock(store) {
                    let _ = s.delete_review_note_batch(args.session_id, &args.client_msg_id);
                }
            }
            Err(e)
        }
    }
}

use crate::store::Store;

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
    let (author, person) = {
        let s = lock(store)?;
        let owner = s.personal_owner_id()?;
        let name = owner
            .and_then(|id| s.get_person(id).ok().flatten())
            .map(|p| p.display_name.unwrap_or(p.name))
            .unwrap_or_else(|| "you".to_string());
        (name, owner)
    };
    match args.action.as_deref().unwrap_or("list") {
        "list" => to_json(&lock(store)?.review_notes(args.session_id)?),
        "send" => {
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
            let row = lock(store)?
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
            args,
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
    }

    #[tokio::test]
    async fn a_send_that_may_have_typed_keeps_its_batch_and_a_retry_types_nothing() {
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
            client_msg_id: "rn-2".into(),
            notes: vec![NewReviewNote {
                path: "a.rs".into(),
                line: Some(2),
                side: "new".into(),
                quote: None,
                body: "x".into(),
            }],
        };
        for code in [
            codes::E_SSH_TIMEOUT,
            codes::E_TIMEOUT,
            codes::E_AGENT_PROTOCOL,
        ] {
            assert!(codes::may_have_run(code), "{code}");
        }
        let err = send(
            args.clone(),
            "Ada",
            None,
            |_| Err(IpcError::new(codes::E_SSH_TIMEOUT, "ssh timed out")),
            &store,
            &ssh,
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, codes::E_SSH_TIMEOUT);
        assert_eq!(
            lock(&store).unwrap().review_notes(sid).unwrap().len(),
            1,
            "a send that may have typed stays listed on every device"
        );
        let again = send(
            args,
            "Ada",
            None,
            |_| panic!("a retry of a send that may have typed types nothing"),
            &store,
            &ssh,
        )
        .await
        .unwrap();
        assert!(again.repeat && !again.delivered);
    }
}
