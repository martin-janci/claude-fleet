//! The `peer_exchange` wire. Field doc comments are one line each: they are
//! served in the tool's input schema and count against the tool budget.

use rmcp::schemars;
use serde::{Deserialize, Serialize};

pub const PROTO: u32 = 1;
pub const PEER_BODY_MAX: usize = 32 * 1024;
pub const PEER_BATCH_MAX: usize = 50;
pub const PEER_WAIT_MAX_MS: u64 = 25_000;
/// The longest `from_addr` / `to_addr` that may cross a link, in bytes. A
/// session address is a 36-byte fleet, a host and a tmux name; this bounds
/// what a peer can make a recipient's hook label carry.
pub const PEER_ADDR_MAX: usize = 256;
/// A page (the dialer's `send`, the listener's `messages`) is cut once its
/// items' JSON reaches this many bytes — always at least one item. Items are
/// double-encoded and SSE-framed on the way, and control-heavy bodies grow
/// several-fold in JSON, so 50 × [`PEER_BODY_MAX`] can exceed a proxy's body
/// limit or the dialer's response cap; a page that never fits is re-sent
/// forever.
pub const PEER_PAGE_MAX_BYTES: usize = 512 * 1024;

/// PURE: the longest prefix of `items` whose serialized size stays within
/// [`PEER_PAGE_MAX_BYTES`], never fewer than one item (an item over the cap
/// on its own still goes, alone). The flag says whether anything was cut.
pub fn cap_page(mut items: Vec<WireMessage>) -> (Vec<WireMessage>, bool) {
    let mut total = 0usize;
    let mut keep = 0usize;
    for m in &items {
        let size = serde_json::to_string(m).map(|s| s.len()).unwrap_or(0);
        if keep > 0 && total + size > PEER_PAGE_MAX_BYTES {
            break;
        }
        total += size;
        keep += 1;
    }
    let cut = keep < items.len();
    items.truncate(keep);
    (items, cut)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct WireRef {
    pub fleet: String,
    pub id: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct WireMessage {
    /// The sending hub's message id.
    pub id: i64,
    pub from_addr: String,
    pub to_addr: String,
    pub body: String,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<WireRef>,
    pub sent_at: i64,
    #[serde(default)]
    pub wake: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ResultStatus {
    Accepted,
    Rejected,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct WireResult {
    pub id: i64,
    pub status: ResultStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ExchangeRequest {
    /// Wire protocol version; must be 1.
    pub proto: u32,
    /// The calling hub's fleet id.
    pub fleet_id: String,
    /// Messages for this hub's sessions, at most 50.
    #[serde(default)]
    pub send: Vec<WireMessage>,
    /// Highest id of yours the caller has stored.
    #[serde(default)]
    pub after: i64,
    /// The caller's rejections of your earlier messages.
    #[serde(default)]
    pub results: Vec<WireResult>,
    /// Long-poll budget, ms; clamped to 25000.
    #[serde(default)]
    pub wait_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExchangeResponse {
    pub proto: u32,
    pub fleet_id: String,
    pub results: Vec<WireResult>,
    pub messages: Vec<WireMessage>,
    pub more: bool,
}

impl WireResult {
    pub fn accepted(id: i64) -> Self {
        Self {
            id,
            status: ResultStatus::Accepted,
            code: None,
            message: None,
        }
    }
    pub fn rejected(id: i64, code: &str, message: impl Into<String>) -> Self {
        Self {
            id,
            status: ResultStatus::Rejected,
            code: Some(code.into()),
            message: Some(message.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_without_optional_fields_parses() {
        let r: ExchangeRequest =
            serde_json::from_str(r#"{"proto":1,"fleet_id":"fleet-a"}"#).unwrap();
        assert!(r.send.is_empty() && r.results.is_empty());
        assert_eq!((r.after, r.wait_ms), (0, 0));
    }

    fn big(id: i64) -> WireMessage {
        WireMessage {
            id,
            from_addr: "fleet-a/session/h/a1".into(),
            to_addr: "fleet-b/session/h/b1".into(),
            // Control characters escape to six bytes each in JSON: the
            // worst growth a 32 KiB body can have.
            body: "\u{1}".repeat(PEER_BODY_MAX),
            kind: "message".into(),
            reply_to: None,
            sent_at: 0,
            wake: false,
        }
    }

    /// I4: fifty worst-case items are split across pages by size, and each
    /// page stays under the cap.
    #[test]
    fn a_page_of_large_items_is_split_by_size() {
        let items: Vec<WireMessage> = (1..=PEER_BATCH_MAX as i64).map(big).collect();
        let (page, cut) = cap_page(items);
        assert!(cut, "fifty large items are more than one page");
        assert!(!page.is_empty() && page.len() < PEER_BATCH_MAX);
        let bytes: usize = page
            .iter()
            .map(|m| serde_json::to_string(m).unwrap().len())
            .sum();
        assert!(bytes <= PEER_PAGE_MAX_BYTES, "{bytes}");
        assert_eq!(page[0].id, 1, "oldest first");
    }

    /// I4: an item over the cap on its own still goes, alone.
    #[test]
    fn a_single_item_over_the_cap_still_goes() {
        let mut huge = big(1);
        huge.body = "\u{1}".repeat(PEER_PAGE_MAX_BYTES);
        let (page, cut) = cap_page(vec![huge, big(2)]);
        assert_eq!(page.iter().map(|m| m.id).collect::<Vec<_>>(), vec![1]);
        assert!(cut);
        let (small, cut) = cap_page(vec![big(3)]);
        assert_eq!((small.len(), cut), (1, false));
    }
}
