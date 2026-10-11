//! Task attachments (migration 165): `work_link { attach }` /
//! `{ attachment_delete }` and `work { attachment }`, shared by the MCP tools
//! and the desktop's Routed commands. An attachment is about the ITEM, as a
//! comment is ([`super::local::comment`]): an item outside the caller's
//! scope answers as an unknown id, and so does an attachment on one.

use super::local::visible_parent;
use super::{WorkArgs, WorkLinkArgs};
use crate::ipc_error::{codes, lock, IpcError};
use crate::service::orgs::OrgScope;
use crate::service::view_scope::ViewScope;
use crate::store::{AttachmentRow, NewAttachment, Store};
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use std::sync::Mutex;

/// `work { attachment }`: one attachment and its bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttachmentData {
    pub attachment: AttachmentRow,
    pub data_base64: String,
}

/// `work_link { action: attach, item_id, name, mime, data_base64,
/// comment_id? }`: a file on a task the caller sees, at most
/// `work.attachment_max_mb`. The bytes are decoded before the store is
/// locked.
pub fn attach(
    args: &WorkLinkArgs,
    store: &Mutex<Store>,
    scope: &OrgScope,
    author: &str,
    person: Option<i64>,
) -> Result<AttachmentRow, IpcError> {
    let item_id = args
        .item_id
        .ok_or_else(|| IpcError::new(codes::E_INVALID, "attach needs item_id"))?;
    let name = args
        .name
        .as_deref()
        .ok_or_else(|| IpcError::new(codes::E_INVALID, "attach needs name: the file's name"))?;
    let data = args
        .data_base64
        .as_deref()
        .ok_or_else(|| IpcError::new(codes::E_INVALID, "attach needs data_base64"))?;
    // A base64 string longer than the largest allowed file could encode is
    // refused before decoding it.
    let max = {
        let s = lock(store)?;
        crate::service::settings::attachment_max_bytes(&s)
    };
    if data.len() > max.div_ceil(3) * 4 + 4 {
        return Err(IpcError::new(
            codes::E_INVALID,
            format!("the file is over the {max}-byte attachment limit (work.attachment_max_mb)"),
        ));
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data.trim())
        .map_err(|e| IpcError::new(codes::E_INVALID, format!("data_base64 is not base64: {e}")))?;
    let s = lock(store)?;
    visible_parent(&s, scope, item_id)?;
    let mut a = s.add_attachment(
        &NewAttachment {
            item_id,
            name,
            mime: args.mime.as_deref().unwrap_or_default(),
            author,
            author_person_id: person,
            comment_id: args.comment_id,
        },
        &bytes,
        max,
    )?;
    a.mine = true;
    Ok(a)
}

/// `work_link { action: attachment_delete, attachment_id }`: its author's
/// alone. An attachment on an item outside the scope answers as an unknown
/// one.
pub fn attachment_delete(
    args: &WorkLinkArgs,
    store: &Mutex<Store>,
    scope: &OrgScope,
    author: &str,
    person: Option<i64>,
) -> Result<AttachmentRow, IpcError> {
    let id = args
        .attachment_id
        .ok_or_else(|| IpcError::new(codes::E_INVALID, "attachment_delete needs attachment_id"))?;
    let unknown = || IpcError::new(codes::E_NOTFOUND, format!("attachment {id} not found"));
    let s = lock(store)?;
    let a = s.get_attachment(id)?.ok_or_else(unknown)?;
    visible_parent(&s, scope, a.item_id).map_err(|_| unknown())?;
    s.delete_attachment(id, person, author)?.ok_or_else(unknown)
}

/// `work { action: attachment, attachment_id }`: the attachment and its
/// bytes, for a caller who sees its item (the org fence, `view.org`). The
/// author is withheld from a caller who sees neither every device nor its
/// own, exactly as `work { task }` withholds it; `mine` is the reader's
/// side's to set.
pub fn attachment(
    args: &WorkArgs,
    store: &Mutex<Store>,
    view: &ViewScope,
) -> Result<AttachmentData, IpcError> {
    let scope = &view.org;
    let id = args
        .attachment_id
        .ok_or_else(|| IpcError::new(codes::E_INVALID, "attachment needs attachment_id"))?;
    let unknown = || IpcError::new(codes::E_NOTFOUND, format!("attachment {id} not found"));
    let (mut attachment, bytes) = {
        let s = lock(store)?;
        let a = s.get_attachment(id)?.ok_or_else(unknown)?;
        visible_parent(&s, scope, a.item_id).map_err(|_| unknown())?;
        let bytes = s.attachment_bytes(id)?.ok_or_else(unknown)?;
        (a, bytes)
    };
    let own = matches!(
        (attachment.author_person_id, view.person),
        (Some(a), Some(p)) if a == p
    );
    if !(view.is_unrestricted() || view.is_sole_person() || own) {
        attachment.author = String::new();
        attachment.author_person_id = None;
    }
    Ok(AttachmentData {
        attachment,
        data_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{NativeItem, Store};

    fn store_with_item() -> (Mutex<Store>, i64) {
        let s = Store::open_in_memory().unwrap();
        let id = s
            .create_native_item(&NativeItem {
                title: "Fix login",
                parent_id: None,
                project_id: None,
                notes: None,
            })
            .unwrap()
            .id;
        (Mutex::new(s), id)
    }

    fn attach_args(item_id: i64, data: &str) -> WorkLinkArgs {
        WorkLinkArgs {
            action: "attach".into(),
            item_id: Some(item_id),
            name: Some("notes.txt".into()),
            mime: Some("text/plain".into()),
            data_base64: Some(data.into()),
            ..Default::default()
        }
    }

    #[test]
    fn attach_read_and_delete_round_trip() {
        let (st, id) = store_with_item();
        let a = attach(
            &attach_args(id, "aGVsbG8="),
            &st,
            &OrgScope::All,
            "desktop",
            None,
        )
        .unwrap();
        assert!(a.mine);
        assert_eq!(a.size, 5);
        let read = attachment(
            &WorkArgs {
                attachment_id: Some(a.id),
                ..Default::default()
            },
            &st,
            &ViewScope::internal(),
        )
        .unwrap();
        assert_eq!(read.data_base64, "aGVsbG8=");
        assert_eq!(read.attachment.author, "desktop");
        let gone = attachment_delete(
            &WorkLinkArgs {
                action: "attachment_delete".into(),
                attachment_id: Some(a.id),
                ..Default::default()
            },
            &st,
            &OrgScope::All,
            "desktop",
            None,
        )
        .unwrap();
        assert_eq!(gone.id, a.id);
    }

    #[test]
    fn bad_base64_and_an_oversized_payload_are_refused() {
        let (st, id) = store_with_item();
        let err = attach(
            &attach_args(id, "not base64!"),
            &st,
            &OrgScope::All,
            "desktop",
            None,
        )
        .unwrap_err();
        assert_eq!(err.code, codes::E_INVALID);
        crate::service::settings::set(
            &st.lock().unwrap(),
            crate::service::settings::WORK_ATTACHMENT_MAX_MB,
            "1",
        )
        .unwrap();
        let big = base64::engine::general_purpose::STANDARD.encode(vec![0u8; 1024 * 1024 + 1]);
        let err = attach(&attach_args(id, &big), &st, &OrgScope::All, "desktop", None).unwrap_err();
        assert_eq!(err.code, codes::E_INVALID);
        assert!(
            err.message.contains("work.attachment_max_mb"),
            "{}",
            err.message
        );
    }

    /// The scope fence: an item a scoped caller cannot see answers exactly
    /// as an unknown id, for attaching, reading and deleting.
    #[test]
    fn an_item_outside_the_scope_answers_as_unknown() {
        let (st, id) = store_with_item();
        let a = attach(
            &attach_args(id, "aGk="),
            &st,
            &OrgScope::All,
            "desktop",
            None,
        )
        .unwrap();
        // A local item with no links is visible to no org-bound caller.
        let scoped = OrgScope::Host {
            alias: "a".into(),
            org: Some(1),
            isolated: Default::default(),
        };
        let err = attach(&attach_args(id, "aGk="), &st, &scoped, "host:a", None).unwrap_err();
        assert_eq!(err.code, codes::E_NOTFOUND);
        let unknown =
            attach(&attach_args(999_999, "aGk="), &st, &scoped, "host:a", None).unwrap_err();
        assert_eq!(
            err.message.replace(&id.to_string(), "N"),
            unknown.message.replace("999999", "N")
        );
        let read = attachment(
            &WorkArgs {
                attachment_id: Some(a.id),
                ..Default::default()
            },
            &st,
            &ViewScope::internal().with_org(scoped.clone()),
        )
        .unwrap_err();
        assert_eq!(read.code, codes::E_NOTFOUND);
        assert_eq!(read.message, format!("attachment {} not found", a.id));
        let del = attachment_delete(
            &WorkLinkArgs {
                action: "attachment_delete".into(),
                attachment_id: Some(a.id),
                ..Default::default()
            },
            &st,
            &scoped,
            "desktop",
            None,
        )
        .unwrap_err();
        assert_eq!(del.code, codes::E_NOTFOUND);
    }
}
