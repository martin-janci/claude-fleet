//! Control's Library (Orbit Fleet redesign step 9.7): the files a person put
//! on a host, by the Library's Upload… or as a prompt's attachment
//! (`library_items`, migration 143). The Library shows them beside the
//! downloads (`service::downloads`) and the repos the sessions work in; those
//! two already have their own rows and are read from there, not copied here.
//!
//! The bytes stay on the host: the desktop puts them there over its own ssh
//! (`upload_attachments`), then records them with `add`. A row names a path on
//! the session's host, so who sees it is the same `own` tier as a download
//! ([`super::downloads::visible`]): the index into a host's disk is the
//! session owner's.

use crate::ipc_error::{codes, IpcError};
use crate::service::view_scope::ViewScope;
use crate::store::{LibraryItemRow, NewLibraryItem, SessionRow, Store};

/// `kind` values.
pub const KIND_UPLOAD: &str = "upload";
pub const KIND_ATTACHMENT: &str = "attachment";

/// At most this many rows a list answers (and its default).
pub const LIST_LIMIT: usize = 200;
/// At most this many files one `add` records.
pub const ADD_LIMIT: usize = 20;
const MAX_PATH: usize = 4096;
const MAX_NAME: usize = 255;

/// One file `add` records.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct LibraryFile {
    /// Where it is on the session's host.
    pub path: String,
    /// Its name; the path's last part when absent.
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub size: Option<i64>,
}

/// Arguments of `add`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct AddArgs {
    /// `upload` | `attachment`.
    pub kind: String,
    /// The session the files were put beside; its host is where they are.
    pub session_id: i64,
    pub files: Vec<LibraryFile>,
}

/// Arguments of `list`.
#[derive(
    Debug, Clone, Default, serde::Serialize, serde::Deserialize, rmcp::schemars::JsonSchema,
)]
#[schemars(crate = "rmcp::schemars")]
pub struct ListArgs {
    /// Only this session's files.
    #[serde(default)]
    pub session_id: Option<i64>,
    /// Only this host's files.
    #[serde(default)]
    pub host_alias: Option<String>,
    /// At most this many, newest first (default and cap 200).
    #[serde(default)]
    pub limit: Option<usize>,
}

/// What `list` answers.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LibraryList {
    pub items: Vec<LibraryItemRow>,
}

/// Whether `scope` sees `row`: the download rule, for a path on a host.
pub fn visible(s: &Store, scope: &ViewScope, row: &LibraryItemRow) -> bool {
    let sess = row
        .session_id
        .and_then(|id| s.get_session_by_id(id).ok().flatten());
    visible_with(scope, row, sess.as_ref())
}

/// [`visible`] with `row`'s session already read (`None`: it has none, or
/// it is gone).
fn visible_with(scope: &ViewScope, row: &LibraryItemRow, sess: Option<&SessionRow>) -> bool {
    if scope.host.as_deref().is_some_and(|h| h != row.host_alias) {
        return false;
    }
    match sess {
        Some(sess) => scope.may_own(sess),
        // The session is gone: its org still fences the row, and the owner
        // recorded with it (migration 147) is the person half. Without that
        // half every person in the org saw the file (review r04 F3).
        None => {
            scope.org.sees_session_org_only(&row.host_alias, row.org_id)
                && scope.may_own_person_row(row.org_id, row.owner_person_id)
        }
    }
}

pub fn list(s: &Store, scope: &ViewScope, args: &ListArgs) -> Result<LibraryList, IpcError> {
    let limit = args.limit.unwrap_or(LIST_LIMIT).clamp(1, LIST_LIMIT);
    // One session read per session, not per item: many items share one, and
    // a member who sees few of up to `KEEP` rows walks them all (review r16).
    let mut sessions: std::collections::HashMap<i64, Option<SessionRow>> =
        std::collections::HashMap::new();
    let items = s
        .library_items()?
        .into_iter()
        .filter(|r| args.session_id.is_none_or(|id| r.session_id == Some(id)))
        .filter(|r| args.host_alias.as_deref().is_none_or(|h| r.host_alias == h))
        .filter(|r| {
            let sess = match r.session_id {
                Some(id) => sessions
                    .entry(id)
                    .or_insert_with(|| s.get_session_by_id(id).ok().flatten())
                    .as_ref(),
                None => None,
            };
            visible_with(scope, r, sess)
        })
        .take(limit)
        .collect();
    Ok(LibraryList { items })
}

fn invalid(msg: impl Into<String>) -> IpcError {
    IpcError::new(codes::E_INVALID, msg)
}

/// The path's last part.
fn base_name(path: &str) -> &str {
    path.trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(path)
}

/// Record files a person put beside a session. The session must be one the
/// caller owns (`E_NOTFOUND` otherwise, like any id that is not theirs); its
/// host, name and org come from its row, never from the caller.
pub fn add(s: &Store, scope: &ViewScope, args: &AddArgs) -> Result<Vec<LibraryItemRow>, IpcError> {
    if args.kind != KIND_UPLOAD && args.kind != KIND_ATTACHMENT {
        return Err(invalid(format!(
            "kind must be upload | attachment, got {:?}",
            args.kind
        )));
    }
    if args.files.is_empty() || args.files.len() > ADD_LIMIT {
        return Err(invalid(format!("files: 1 to {ADD_LIMIT}")));
    }
    for f in &args.files {
        if f.path.trim().is_empty() || f.path.len() > MAX_PATH || f.path.contains('\0') {
            return Err(invalid("each file needs a path"));
        }
        if f.name
            .as_deref()
            .is_some_and(|n| n.len() > MAX_NAME || n.contains('\0'))
        {
            return Err(invalid(format!("a name is at most {MAX_NAME} bytes")));
        }
    }
    let session = match s.get_session_by_id(args.session_id)? {
        Some(r) if scope.host.is_none() && scope.may_own(&r) => r,
        _ => {
            return Err(IpcError::new(
                codes::E_NOTFOUND,
                format!("session {} not found", args.session_id),
            ))
        }
    };
    let mut out = Vec::with_capacity(args.files.len());
    for f in &args.files {
        let name = f
            .name
            .as_deref()
            .filter(|n| !n.trim().is_empty())
            .unwrap_or_else(|| base_name(&f.path));
        out.push(s.insert_library_item(&NewLibraryItem {
            kind: &args.kind,
            host_alias: &session.host_alias,
            session_id: Some(session.id),
            session_name: Some(&session.tmux_name),
            org_id: session.org_id,
            path: &f.path,
            name,
            size: f.size.filter(|n| *n >= 0),
        })?);
    }
    Ok(out)
}

/// Drop a row from the index (the file stays on the host). `false` when it
/// was gone or is not the caller's to see.
pub fn remove(s: &Store, scope: &ViewScope, id: i64) -> Result<bool, IpcError> {
    match s.library_item(id)? {
        Some(r) if visible(s, scope, &r) => Ok(s.delete_library_item(id)?),
        _ => Ok(false),
    }
}

#[cfg(test)]
mod tests;
