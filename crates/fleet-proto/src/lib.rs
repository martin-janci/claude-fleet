//! The JSON frame protocol spoken between a claude-fleet hub and a
//! `fleet-agent` over one WebSocket the agent opened to `wss://<hub>/agent`.
//!
//! This crate exists so the two sides compile the *same* types. `fleet-core`
//! (the hub) and `fleet-agent` both depend on it; `fleet-agent` never depends
//! on `fleet-core`, because an agent installs on a host that has no business
//! carrying the hub's tree. Nothing but the wire belongs here — no transport,
//! no registry, no tokio.
//!
//! The frame table in `docs/superpowers/specs/2026-09-18-host-agent-design.md`
//! is normative; `tests/frames.rs` pins it.
//!
//! Compatibility rules, in one place:
//!
//! - One request, one response, matched by `id`.
//! - An unknown `kind` is an **error**. There is no catch-all variant: a frame
//!   nobody understands must fail the call, not be quietly dropped.
//! - An unknown *field* is **ignored**, so a newer hub can add one without
//!   taking an older agent's connection down.
//! - Bytes that are not text (`stdin` excepted) travel base64 in `*_b64`
//!   fields, because JSON strings cannot hold arbitrary bytes.
//! - A frame past [`MAX_FRAME_BYTES`] is rejected at both ends. Truncation is
//!   the *executor's* job, reported by `Result.truncated`; the codec never
//!   shortens anything silently.

use base64::Engine as _;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Ceiling on one encoded frame, in bytes, in both directions.
///
/// Sized for `upload`, the only frame that carries a file: base64 costs 4/3,
/// so this admits a payload of roughly 12 MiB — far above the hooks, settings
/// and MCP entries provisioning writes, and well below anything that would
/// make a WebSocket peer buffer dangerously. It bounds `exec` output too; the
/// hub sets the per-call `cap_bytes` under it.
pub const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;

/// The standard, padded base64 alphabet — the one `base64(1)` on a remote host
/// already produces, so hub-side code that shells out and agent-side code that
/// encodes in process agree.
const B64: base64::engine::general_purpose::GeneralPurpose =
    base64::engine::general_purpose::STANDARD;

/// Hub → agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HubFrame {
    /// Run this argv. The agent execs it directly — there is no shell unless
    /// the argv itself is one (`["bash", "-lc", <script>]`, as the SSH path
    /// already does), so nothing here needs shell quoting.
    ///
    /// **For whoever writes `AgentTransport` (Task 3):** this is *not* the
    /// same shape as `SshExec`'s `args: &[&str]`. Those are space-joined and
    /// re-tokenised by the remote login shell, which is why callers already
    /// `shell::quote` a multi-word script. Handing them straight over as
    /// `argv` would exec the quoting literally. Reproduce today's semantics by
    /// sending `["bash", "-lc", args.join(" ")]`.
    Exec {
        id: String,
        argv: Vec<String>,
        /// Written to the child's stdin and closed. Absent means no stdin.
        /// Text only: `SshExec` has no stdin parameter today, so nothing sends
        /// this yet. If a caller ever needs non-UTF-8 stdin, this has to
        /// become a `stdin_b64` — it cannot be widened in place.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stdin: Option<String>,
        /// Wall clock the agent gives the child before killing it. The hub
        /// applies its own bound as well, so a wedged agent cannot hold a
        /// call open longer than a wedged SSH host could.
        timeout_ms: u64,
        /// Cap on each captured stream. Absent means the agent's own ceiling.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cap_bytes: Option<u64>,
    },
    /// Write a file, creating parent directories, with this mode.
    Upload {
        id: String,
        path: String,
        /// Unix permission bits, e.g. `0o600`. `SshExec::upload_file` has no
        /// mode argument — it pipes into `cat > path` and takes whatever the
        /// remote umask gives — so Task 3 has to choose what to send here.
        mode: u32,
        bytes_b64: String,
    },
    /// Kill the child of an in-flight request. `id` is that request's id.
    Cancel { id: String },
    /// Liveness. Answered with [`AgentFrame::Pong`] carrying the same `id`.
    Ping { id: String },
}

/// Agent → hub.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AgentFrame {
    /// The first frame after the upgrade. The hub records it in the registry.
    Hello {
        agent_version: String,
        host_name: String,
        os: String,
    },
    /// One per request, carrying that request's `id`.
    Result {
        id: String,
        /// The child's exit status; negative when it was killed by a signal
        /// or never started, matching what the SSH path already reports.
        exit_code: i32,
        stdout_b64: String,
        stderr_b64: String,
        /// Set when either stream hit the cap, so the caller knows the output
        /// is short rather than the command being quiet.
        truncated: bool,
    },
    Pong {
        id: String,
    },
}

/// What can go wrong turning bytes into a frame, or back.
///
/// Deliberately not an `IpcError`: this crate must not depend on `fleet-core`.
/// The hub maps these onto `E_AGENT_PROTOCOL` at its own boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProtoError {
    /// The frame is longer than [`MAX_FRAME_BYTES`]. Rejected whole — never
    /// truncated, because half a frame is not a frame.
    TooLarge { size: usize, cap: usize },
    /// Not JSON, not a frame of this direction, an unknown `kind`, a missing
    /// or mistyped field, or an undecodable base64 body.
    Malformed(String),
}

impl fmt::Display for ProtoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLarge { size, cap } => {
                write!(f, "frame is {size} bytes, over the {cap} byte cap")
            }
            Self::Malformed(why) => write!(f, "malformed frame: {why}"),
        }
    }
}

impl std::error::Error for ProtoError {}

/// Encode a hub frame for the wire.
pub fn encode_hub_frame(frame: &HubFrame) -> Result<String, ProtoError> {
    encode(frame)
}

/// Encode an agent frame for the wire.
pub fn encode_agent_frame(frame: &AgentFrame) -> Result<String, ProtoError> {
    encode(frame)
}

/// Decode a hub frame. An agent frame, an unknown `kind` and junk are all
/// errors; nothing here panics on hostile input.
pub fn decode_hub_frame(text: &str) -> Result<HubFrame, ProtoError> {
    decode(text)
}

/// Decode an agent frame. See [`decode_hub_frame`].
pub fn decode_agent_frame(text: &str) -> Result<AgentFrame, ProtoError> {
    decode(text)
}

fn encode<T: Serialize>(frame: &T) -> Result<String, ProtoError> {
    let text = serde_json::to_string(frame).map_err(|e| ProtoError::Malformed(e.to_string()))?;
    check_size(text.len())?;
    Ok(text)
}

fn decode<T: DeserializeOwned>(text: &str) -> Result<T, ProtoError> {
    // Size first: a hostile peer must not get serde to walk 100 MB before the
    // cap is consulted.
    check_size(text.len())?;
    serde_json::from_str(text).map_err(|e| ProtoError::Malformed(e.to_string()))
}

fn check_size(size: usize) -> Result<(), ProtoError> {
    if size > MAX_FRAME_BYTES {
        return Err(ProtoError::TooLarge {
            size,
            cap: MAX_FRAME_BYTES,
        });
    }
    Ok(())
}

/// Encode bytes for a `*_b64` field.
pub fn encode_b64(bytes: &[u8]) -> String {
    B64.encode(bytes)
}

/// Decode a `*_b64` field back to bytes.
pub fn decode_b64(text: &str) -> Result<Vec<u8>, ProtoError> {
    B64.decode(text)
        .map_err(|e| ProtoError::Malformed(format!("base64: {e}")))
}
