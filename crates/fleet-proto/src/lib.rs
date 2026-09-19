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

/// The largest raw payload one frame carries, before base64.
///
/// **Derived, not chosen.** The biggest thing the fleet moves across this
/// transport is a Claude Code transcript: `service::move_session`'s
/// `move.max_transcript_mb` setting defaults to 200 MiB, and the move checks
/// the transcript's real size against it *before* copying — so 200 MiB is the
/// application's own declared maximum, not a guess about what files are like.
/// It crosses the transport twice, read back as `Result.stdout_b64` and
/// written as `Upload.bytes_b64`, so a limit below it breaks "Move to host…"
/// for any real session on an agent host. fleet-core's
/// `the_frame_cap_covers_the_transcript_the_fleet_moves` pins the two
/// together so they cannot drift apart again.
///
/// An operator who raises `move.max_transcript_mb` past this gets a clear
/// `E_UPLOAD` naming the limit, not a silent failure — but the two numbers do
/// have to move together.
pub const MAX_PAYLOAD_BYTES: usize = 200 * 1024 * 1024;

/// Room for the JSON around one payload: `kind`, a uuid `id`, a path, a mode,
/// and the *second* `*_b64` field of a `result` (an agent that fills both
/// streams to the payload limit is refused — capping stderr is its job).
/// Generous on purpose; the cap exists to bound memory, not to be exact.
const ENVELOPE_BYTES: usize = 64 * 1024;

/// Ceiling on one encoded frame, in bytes, in both directions.
///
/// [`MAX_PAYLOAD_BYTES`] after base64 (4/3), plus [`ENVELOPE_BYTES`]. It
/// bounds `exec` output too; the hub sets the per-call `cap_bytes` under it.
///
/// **The memory this admits is real**, and a reader should size it: one frame
/// at the cap is ~267 MiB of `String` on each side, on top of the raw bytes
/// the hub already holds. That matches what the SSH path already costs for
/// the same move (`move_session` reads the whole transcript into memory before
/// writing it), but it means a peer can make this process allocate that much,
/// once per frame. Where a caller knows its own answer is small, it should say
/// so with [`decode_agent_frame_within`] rather than rely on this ceiling.
pub const MAX_FRAME_BYTES: usize = base64_len(MAX_PAYLOAD_BYTES) + ENVELOPE_BYTES;

/// Length of standard padded base64 for `n` bytes — the 4/3 blow-up every
/// size check here has to account for.
pub const fn base64_len(n: usize) -> usize {
    n.div_ceil(3) * 4
}

/// How often the hub pings an idle agent, and so the unit both ends count
/// silence in. The hub drops an agent after two beats with nothing heard; the
/// agent gives up on a hub after a few beats of silence and dials again. The
/// ping comes from the HUB — an agent answers it and never originates one.
pub const HEARTBEAT: std::time::Duration = std::time::Duration::from_secs(30);

/// How much output one `exec`'s `result` may carry, before base64.
///
/// The agent's half of [`result_budget`]: an agent that truncates to these
/// limits always produces a `result` the hub will decode. `tests/frames.rs`
/// pins the two together.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamLimits {
    /// The most either stream may carry: the hub's `cap_bytes`, or
    /// [`MAX_PAYLOAD_BYTES`] when it sent none.
    pub per_stream: usize,
    /// The most both streams may carry together. Twice `per_stream` for a
    /// small cap; one [`MAX_PAYLOAD_BYTES`] once that would overrun the frame
    /// ceiling, which leaves one envelope around ONE payload — so an agent
    /// with a full stdout has no room left for stderr.
    pub combined: usize,
}

/// What an `exec`'s `result` may cost on the wire: both streams at the cap,
/// base64'd, plus one envelope — or, with no cap, the whole ceiling, because
/// an uncapped `run` is what reads a 200 MiB transcript back.
///
/// The hub decodes every answer against this (`decode_agent_frame_within`),
/// so the number lives here, beside [`result_stream_limits`], rather than in
/// either end.
pub fn result_budget(cap_bytes: Option<u64>) -> usize {
    match cap_bytes {
        None => MAX_FRAME_BYTES,
        Some(cap) => {
            // Clamped before the arithmetic: `cap_bytes` is a u64 off the
            // wire, and no cap above the payload limit can buy more than the
            // ceiling anyway.
            let per_stream = usize::try_from(cap)
                .unwrap_or(usize::MAX)
                .min(MAX_PAYLOAD_BYTES);
            base64_len(per_stream.saturating_mul(2))
                .saturating_add(ENVELOPE_BYTES)
                .min(MAX_FRAME_BYTES)
        }
    }
}

/// The stream limits whose worst case fits [`result_budget`] for the same
/// `cap_bytes`.
pub fn result_stream_limits(cap_bytes: Option<u64>) -> StreamLimits {
    let per_stream = cap_bytes
        .map(|cap| usize::try_from(cap).unwrap_or(usize::MAX))
        .unwrap_or(MAX_PAYLOAD_BYTES)
        .min(MAX_PAYLOAD_BYTES);
    StreamLimits {
        per_stream,
        combined: per_stream.saturating_mul(2).min(MAX_PAYLOAD_BYTES),
    }
}

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
    /// **This is *not* the same shape as `SshExec`'s `args: &[&str]`.** Those
    /// are space-joined by ssh and re-tokenised by a shell on the far side,
    /// which is why callers already `shell::quote` a multi-word script.
    /// Handing them straight over as `argv` would exec the quoting literally.
    ///
    /// `AgentTransport` therefore sends **`["bash", "-c", args.join(" ")]`** —
    /// `-c`, not `-lc`. That is byte for byte what `fleet-core`'s own
    /// `LocalExec::command` already does for the same trait, and sshd runs a
    /// remote command as `$SHELL -c` without sourcing the login profile, so
    /// the login shell the fleet wants is the *inner* one its callers write.
    /// An outer `-l` would source the profile a second time and put anything
    /// it prints in front of output the service layer parses.
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
    encode(frame, MAX_FRAME_BYTES)
}

/// Encode an agent frame for the wire.
pub fn encode_agent_frame(frame: &AgentFrame) -> Result<String, ProtoError> {
    encode(frame, MAX_FRAME_BYTES)
}

/// Decode a hub frame. An agent frame, an unknown `kind` and junk are all
/// errors; nothing here panics on hostile input.
pub fn decode_hub_frame(text: &str) -> Result<HubFrame, ProtoError> {
    decode(text, MAX_FRAME_BYTES)
}

/// Decode an agent frame. See [`decode_hub_frame`].
pub fn decode_agent_frame(text: &str) -> Result<AgentFrame, ProtoError> {
    decode(text, MAX_FRAME_BYTES)
}

/// [`encode_hub_frame`] against an explicit ceiling.
pub fn encode_hub_frame_within(frame: &HubFrame, cap: usize) -> Result<String, ProtoError> {
    encode(frame, cap)
}

/// [`encode_agent_frame`] against an explicit ceiling.
pub fn encode_agent_frame_within(frame: &AgentFrame, cap: usize) -> Result<String, ProtoError> {
    encode(frame, cap)
}

/// [`decode_hub_frame`] against an explicit ceiling.
pub fn decode_hub_frame_within(text: &str, cap: usize) -> Result<HubFrame, ProtoError> {
    decode(text, cap)
}

/// [`decode_agent_frame`] against an explicit ceiling.
///
/// [`MAX_FRAME_BYTES`] has to admit the largest thing the fleet moves, which
/// is far larger than the answer to a typical `exec`. A reader that knows its
/// own budget — the hub's socket loop, once it knows the `cap_bytes` it asked
/// for — should pass that budget here instead of letting every peer spend the
/// whole ceiling.
pub fn decode_agent_frame_within(text: &str, cap: usize) -> Result<AgentFrame, ProtoError> {
    decode(text, cap)
}

fn encode<T: Serialize>(frame: &T, cap: usize) -> Result<String, ProtoError> {
    let text = serde_json::to_string(frame).map_err(|e| ProtoError::Malformed(e.to_string()))?;
    check_size(text.len(), cap)?;
    Ok(text)
}

fn decode<T: DeserializeOwned>(text: &str, cap: usize) -> Result<T, ProtoError> {
    // Size first: a hostile peer must not get serde to walk 100 MB before the
    // cap is consulted.
    check_size(text.len(), cap)?;
    serde_json::from_str(text).map_err(|e| ProtoError::Malformed(e.to_string()))
}

fn check_size(size: usize, cap: usize) -> Result<(), ProtoError> {
    if size > cap {
        return Err(ProtoError::TooLarge { size, cap });
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
