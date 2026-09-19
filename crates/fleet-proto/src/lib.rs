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
//! - An unknown `kind` is an **error** during the handshake — the hub's
//!   `first_hello`, and whatever the agent treats as its own handshake
//!   window — because a peer that cannot even be registered must not be
//!   trusted with anything. AFTER the handshake, an unknown `kind` is
//!   **ignorable**: [`decode_hub_frame_lenient`] and
//!   [`decode_agent_frame_lenient_within`] report it as [`Decoded::Unknown`]
//!   rather than an error, so a newer peer can add a frame kind an older one
//!   may skip without dropping the connection. A `kind` the receiver DOES
//!   know, but whose body will not parse, is corruption either way and stays
//!   a hard error — see [`decode_lenient`]'s doc.
//! - An unknown *field* is **ignored**, so a newer hub can add one without
//!   taking an older agent's connection down.
//! - Bytes that are not text (`stdin` excepted) travel base64 in `*_b64`
//!   fields, because JSON strings cannot hold arbitrary bytes.
//! - A frame past [`MAX_FRAME_BYTES`] is rejected at both ends. Truncation is
//!   the *executor's* job, reported by `Result.truncated`; the codec never
//!   shortens anything silently.
//! - **Protocol versioning.** [`PROTO_VERSION`] and [`MIN_SUPPORTED_PROTO`]
//!   are this build's own window; [`judge_proto`] judges a peer's number
//!   against it and [`ProtoVerdict::refusal_reason`] renders the close
//!   reason. The agent's `hello` carries `proto`; the hub's
//!   [`HubFrame::Welcome`] — the first frame down every accepted connection —
//!   carries the hub's own, so each side can refuse the other. Both use
//!   [`VERSION_REFUSED_CLOSE_CODE`] to close, so the reason for the close
//!   never has to be guessed from prose.

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

/// The wire protocol's own version — independent of `agent_version`, which
/// is just a human-readable build string nothing here parses.
///
/// **Bump this, and only this, when:** a change adds a frame `kind` the
/// OTHER side must understand and act on for a call to complete (not one it
/// may safely skip — a receiver ignores an unknown `kind` after the
/// handshake regardless, see the crate doc), or changes the MEANING of an
/// existing field. **Do not bump it for:** a new optional field (an older
/// peer already ignores a field it does not know), or a new frame `kind`
/// whose absence the sender can tolerate the receiver not acting on.
pub const PROTO_VERSION: u32 = 1;

/// The oldest `proto` a hub still accepts from an agent's `hello`.
///
/// **Why 1, not 0.** A `hello` with no `proto` field deserialises as `0`
/// (`proto` is `#[serde(default)]`) — what every agent built before this
/// field existed sends. This same version bump adds [`HubFrame::Welcome`],
/// which the hub sends as the FIRST frame down every accepted connection: a
/// pre-versioning agent has no idea what `welcome` is, and its (equally
/// pre-versioning) decoder hard-errors on any `kind` it does not recognise,
/// unconditionally — the exact reconnect loop this task exists to end.
/// Admitting proto 0 here would not avoid that loop; the hub would walk an
/// agent straight into it the moment it sent `welcome`. Refusing proto 0
/// with a clear reason ("update fleet-agent") is strictly better than a
/// decode error with none, so `MIN_SUPPORTED_PROTO` starts at
/// [`PROTO_VERSION`] itself. Once a proto-1 `fleet-agent` has actually
/// shipped, lowering this is a live compatibility decision for whoever ships
/// the next bump, not a default to carry forward blindly.
pub const MIN_SUPPORTED_PROTO: u32 = 1;

// Enforced at compile time, not just in a test: `judge_proto` assumes this
// ordering (a `their_proto` under `MIN_SUPPORTED_PROTO` and over
// `PROTO_VERSION` at once would be unreachable and its `PeerBehind`/
// `PeerAhead` split would stop meaning anything).
const _: () = assert!(MIN_SUPPORTED_PROTO <= PROTO_VERSION);

/// WebSocket close code for "your protocol version is out of range" —
/// distinct from an ordinary close so the receiving side can tell a version
/// refusal apart from any other reason without parsing prose out of the
/// close reason. In the 4000-4999 range RFC 6455 reserves for private use
/// between two implementations that agree on it, which the hub and
/// `fleet-agent` do, here.
pub const VERSION_REFUSED_CLOSE_CODE: u16 = 4001;

/// The verdict on a peer's `proto`, judged against this binary's own
/// compiled [`PROTO_VERSION`]/[`MIN_SUPPORTED_PROTO`] window. See
/// [`judge_proto`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtoVerdict {
    /// In `MIN_SUPPORTED_PROTO..=PROTO_VERSION`. Proceed.
    Compatible,
    /// Below this binary's window: the PEER needs updating.
    PeerBehind { their: u32, min_supported: u32 },
    /// Above this binary's window: THIS binary needs updating.
    PeerAhead { their: u32, max_supported: u32 },
}

/// Judge a peer's `proto` against this binary's own compiled
/// [`PROTO_VERSION`]/[`MIN_SUPPORTED_PROTO`].
///
/// Both ends call this the same way, each with the OTHER side's number — the
/// hub with the agent's `hello.proto`, the agent with the hub's
/// `welcome.proto` — so the same two constants settle both directions
/// without either binary knowing what the other one is.
pub fn judge_proto(their_proto: u32) -> ProtoVerdict {
    if their_proto < MIN_SUPPORTED_PROTO {
        ProtoVerdict::PeerBehind {
            their: their_proto,
            min_supported: MIN_SUPPORTED_PROTO,
        }
    } else if their_proto > PROTO_VERSION {
        ProtoVerdict::PeerAhead {
            their: their_proto,
            max_supported: PROTO_VERSION,
        }
    } else {
        ProtoVerdict::Compatible
    }
}

impl ProtoVerdict {
    /// A close-frame reason naming both versions and which side to update —
    /// `None` when [`ProtoVerdict::Compatible`].
    ///
    /// `self_name`/`peer_name` name this call's own binary and the far side —
    /// e.g. `("the hub", "fleet-agent")` from the hub judging an agent's
    /// `hello`, or the reverse from the agent judging a hub's `welcome`.
    /// Whichever binary [`judge_proto`] found out of range is the one this
    /// renders as needing the update.
    pub fn refusal_reason(&self, self_name: &str, peer_name: &str) -> Option<String> {
        match *self {
            Self::Compatible => None,
            Self::PeerBehind {
                their,
                min_supported,
            } => Some(format!(
                "{peer_name} speaks protocol v{their}; {self_name} needs at least v{min_supported} \
                 — update {peer_name}"
            )),
            Self::PeerAhead {
                their,
                max_supported,
            } => Some(format!(
                "{peer_name} speaks protocol v{their}; {self_name} understands only up to \
                 v{max_supported} — update {self_name}"
            )),
        }
    }
}

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
    /// The first frame down every connection the hub accepts — sent right
    /// after it registers the agent's `hello`, never before: an agent whose
    /// `proto` is out of range is refused (see [`MIN_SUPPORTED_PROTO`]) and
    /// never receives one. Lets the agent refuse a hub whose OWN protocol is
    /// out of the agent's range, the way `hello.proto` lets the hub refuse
    /// the agent.
    ///
    /// Added by the same change that introduced `proto`: nothing released
    /// before it exists to stay compatible with, so adding a frame kind here
    /// costs nothing this time. A later addition would not get to assume
    /// that — see the crate doc's unknown-kind rule.
    Welcome {
        /// The hub's own build string — informational, not itself a
        /// compatibility signal; `proto` is that.
        hub_version: String,
        /// This hub's [`PROTO_VERSION`].
        proto: u32,
    },
}

/// Agent → hub.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AgentFrame {
    /// The first frame after the upgrade. The hub records it in the registry
    /// once `proto` clears [`judge_proto`] — see [`MIN_SUPPORTED_PROTO`].
    Hello {
        agent_version: String,
        host_name: String,
        os: String,
        /// This agent's [`PROTO_VERSION`]. Absent — an agent built before
        /// this field existed — deserialises as `0`, serde's default for
        /// `u32`, which is exactly the number [`MIN_SUPPORTED_PROTO`] is set
        /// to refuse: see its doc for why that refusal, not silent
        /// acceptance, is the safer default here.
        #[serde(default)]
        proto: u32,
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

/// A frame decoded once the handshake is behind us, where an unknown `kind`
/// is no longer fatal — see the crate doc's unknown-kind rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decoded<T> {
    /// Decoded into a real frame.
    Frame(T),
    /// The envelope parsed; its `kind` is not one this build knows. The
    /// receiver's job: log it once per KIND, at warn, and otherwise ignore
    /// it — never once per frame, or a peer that sends many floods the log.
    Unknown { kind: String },
}

/// [`decode_hub_frame`], except a `kind` this build does not recognise comes
/// back as [`Decoded::Unknown`] instead of an error. Only for frames read
/// AFTER the handshake — see the crate doc; the handshake itself keeps using
/// the strict decoders.
pub fn decode_hub_frame_lenient(text: &str) -> Result<Decoded<HubFrame>, ProtoError> {
    decode_lenient(text, MAX_FRAME_BYTES, known_hub_kind)
}

/// [`decode_agent_frame_within`], except a `kind` this build does not
/// recognise comes back as [`Decoded::Unknown`] instead of an error. Only for
/// frames read AFTER the handshake — see the crate doc.
pub fn decode_agent_frame_lenient_within(
    text: &str,
    cap: usize,
) -> Result<Decoded<AgentFrame>, ProtoError> {
    decode_lenient(text, cap, known_agent_kind)
}

/// The `kind` field alone, for [`decode_lenient`]'s second pass. Never used
/// to build a real frame — only to explain why the first pass failed.
#[derive(Deserialize)]
struct KindOnly {
    kind: String,
}

/// `kind` strings [`HubFrame`] actually has a variant for. Kept in sync with
/// the enum by `fleet-proto`'s own `known_kind_lists_match_every_real_variant`
/// test, which fails to COMPILE — not just to pass — the moment a variant is
/// added here without a matching arm there.
fn known_hub_kind(kind: &str) -> bool {
    matches!(kind, "exec" | "upload" | "cancel" | "ping" | "welcome")
}

/// [`known_hub_kind`], for [`AgentFrame`].
fn known_agent_kind(kind: &str) -> bool {
    matches!(kind, "hello" | "result" | "pong")
}

/// Decode leniently: a `kind` this build does not know is [`Decoded::Unknown`],
/// never an error. A `kind` it DOES know, but whose body will not parse — or
/// no `kind` at all, or invalid JSON — is corruption either way, and stays a
/// hard [`ProtoError::Malformed`]: the unknown-kind rule forgives evolution,
/// not damage.
///
/// The two-pass shape (real decode first, `KindOnly` only to explain a
/// failure) means the common case — a frame this build understands — costs
/// nothing extra; the second parse only runs once the first has already
/// failed.
fn decode_lenient<T: DeserializeOwned>(
    text: &str,
    cap: usize,
    known_kind: fn(&str) -> bool,
) -> Result<Decoded<T>, ProtoError> {
    check_size(text.len(), cap)?;
    match serde_json::from_str::<T>(text) {
        Ok(frame) => Ok(Decoded::Frame(frame)),
        Err(e) => match serde_json::from_str::<KindOnly>(text) {
            Ok(KindOnly { kind }) if !known_kind(&kind) => Ok(Decoded::Unknown { kind }),
            _ => Err(ProtoError::Malformed(e.to_string())),
        },
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn hub_kind(frame: &HubFrame) -> &'static str {
        // Exhaustive on purpose: adding a `HubFrame` variant without adding
        // an arm here fails to COMPILE, which is what actually keeps
        // `known_hub_kind` from calling a real variant "unknown".
        match frame {
            HubFrame::Exec { .. } => "exec",
            HubFrame::Upload { .. } => "upload",
            HubFrame::Cancel { .. } => "cancel",
            HubFrame::Ping { .. } => "ping",
            HubFrame::Welcome { .. } => "welcome",
        }
    }

    fn agent_kind(frame: &AgentFrame) -> &'static str {
        match frame {
            AgentFrame::Hello { .. } => "hello",
            AgentFrame::Result { .. } => "result",
            AgentFrame::Pong { .. } => "pong",
        }
    }

    #[test]
    fn known_kind_lists_match_every_real_variant() {
        let hub_frames = [
            HubFrame::Ping { id: "x".into() },
            HubFrame::Cancel { id: "x".into() },
            HubFrame::Exec {
                id: "x".into(),
                argv: vec![],
                stdin: None,
                timeout_ms: 1,
                cap_bytes: None,
            },
            HubFrame::Upload {
                id: "x".into(),
                path: "x".into(),
                mode: 0,
                bytes_b64: String::new(),
            },
            HubFrame::Welcome {
                hub_version: "x".into(),
                proto: 1,
            },
        ];
        for frame in &hub_frames {
            assert!(known_hub_kind(hub_kind(frame)), "{frame:?}");
        }

        let agent_frames = [
            AgentFrame::Hello {
                agent_version: "x".into(),
                host_name: "x".into(),
                os: "x".into(),
                proto: 1,
            },
            AgentFrame::Result {
                id: "x".into(),
                exit_code: 0,
                stdout_b64: String::new(),
                stderr_b64: String::new(),
                truncated: false,
            },
            AgentFrame::Pong { id: "x".into() },
        ];
        for frame in &agent_frames {
            assert!(known_agent_kind(agent_kind(frame)), "{frame:?}");
        }
    }
}
