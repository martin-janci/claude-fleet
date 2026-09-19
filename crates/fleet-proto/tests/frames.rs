//! The wire contract between a claude-fleet hub and a `fleet-agent`.
//!
//! These tests are the protocol. The table in
//! `docs/superpowers/specs/2026-09-18-host-agent-design.md` is normative, so
//! the tag and field names below are asserted literally: renaming a field in
//! `lib.rs` without renaming it here is a wire break, and that is the point.

use fleet_proto::{
    base64_len, decode_agent_frame, decode_agent_frame_lenient_within, decode_b64,
    decode_hub_frame, decode_hub_frame_lenient, decode_hub_frame_within, encode_agent_frame,
    encode_agent_frame_within, encode_b64, encode_hub_frame, encode_hub_frame_within, judge_proto,
    AgentFrame, Decoded, HubFrame, ProtoError, ProtoVerdict, MAX_FRAME_BYTES, MAX_PAYLOAD_BYTES,
    MIN_SUPPORTED_PROTO, PROTO_VERSION, VERSION_REFUSED_CLOSE_CODE,
};
use serde_json::{json, Value};

fn exec() -> HubFrame {
    HubFrame::Exec {
        id: "01J0".into(),
        argv: vec!["bash".into(), "-lc".into(), "echo hi".into()],
        stdin: None,
        timeout_ms: 30_000,
        cap_bytes: None,
    }
}

fn hub_frames() -> Vec<HubFrame> {
    vec![
        exec(),
        HubFrame::Exec {
            id: "01J1".into(),
            argv: vec!["cat".into()],
            stdin: Some("a prompt on stdin".into()),
            timeout_ms: 1,
            cap_bytes: Some(4096),
        },
        HubFrame::Upload {
            id: "01J2".into(),
            path: "/home/dev/.claude/settings.json".into(),
            mode: 0o600,
            bytes_b64: encode_b64(b"{\"hooks\":{}}"),
        },
        HubFrame::Cancel { id: "01J3".into() },
        HubFrame::Ping { id: "01J4".into() },
        HubFrame::Welcome {
            hub_version: "0.9.0".into(),
            proto: PROTO_VERSION,
        },
    ]
}

fn agent_frames() -> Vec<AgentFrame> {
    vec![
        AgentFrame::Hello {
            agent_version: "0.1.0".into(),
            host_name: "laptop".into(),
            os: "linux".into(),
            proto: PROTO_VERSION,
        },
        AgentFrame::Result {
            id: "01J0".into(),
            exit_code: 0,
            stdout_b64: encode_b64(b"hi\n"),
            stderr_b64: String::new(),
            truncated: false,
        },
        AgentFrame::Result {
            id: "01J1".into(),
            exit_code: -1,
            stdout_b64: encode_b64(b"partial"),
            stderr_b64: encode_b64(b"boom"),
            truncated: true,
        },
        AgentFrame::Pong { id: "01J4".into() },
    ]
}

// --- round trips ------------------------------------------------------------

#[test]
fn every_hub_frame_round_trips() {
    for frame in hub_frames() {
        let text = encode_hub_frame(&frame).expect("encodes");
        assert_eq!(decode_hub_frame(&text).expect("decodes"), frame);
    }
}

#[test]
fn every_agent_frame_round_trips() {
    for frame in agent_frames() {
        let text = encode_agent_frame(&frame).expect("encodes");
        assert_eq!(decode_agent_frame(&text).expect("decodes"), frame);
    }
}

// --- the names on the wire --------------------------------------------------

/// Pins tag and field names against the spec's table. A hub and an agent are
/// two separately-deployed binaries; only these strings keep them talking.
#[test]
fn hub_frames_use_the_spec_s_tag_and_field_names() {
    let expected = [
        json!({
            "kind": "exec",
            "id": "01J0",
            "argv": ["bash", "-lc", "echo hi"],
            "timeout_ms": 30000
        }),
        json!({
            "kind": "exec",
            "id": "01J1",
            "argv": ["cat"],
            "stdin": "a prompt on stdin",
            "timeout_ms": 1,
            "cap_bytes": 4096
        }),
        json!({
            "kind": "upload",
            "id": "01J2",
            "path": "/home/dev/.claude/settings.json",
            "mode": 384,
            "bytes_b64": "eyJob29rcyI6e319"
        }),
        json!({ "kind": "cancel", "id": "01J3" }),
        json!({ "kind": "ping", "id": "01J4" }),
        json!({ "kind": "welcome", "hub_version": "0.9.0", "proto": PROTO_VERSION }),
    ];
    for (frame, want) in hub_frames().into_iter().zip(expected) {
        let text = encode_hub_frame(&frame).expect("encodes");
        let got: Value = serde_json::from_str(&text).expect("valid json");
        assert_eq!(got, want, "frame {frame:?}");
    }
}

#[test]
fn agent_frames_use_the_spec_s_tag_and_field_names() {
    let expected = [
        json!({
            "kind": "hello",
            "agent_version": "0.1.0",
            "host_name": "laptop",
            "os": "linux",
            "proto": PROTO_VERSION
        }),
        json!({
            "kind": "result",
            "id": "01J0",
            "exit_code": 0,
            "stdout_b64": "aGkK",
            "stderr_b64": "",
            "truncated": false
        }),
        json!({
            "kind": "result",
            "id": "01J1",
            "exit_code": -1,
            "stdout_b64": "cGFydGlhbA==",
            "stderr_b64": "Ym9vbQ==",
            "truncated": true
        }),
        json!({ "kind": "pong", "id": "01J4" }),
    ];
    for (frame, want) in agent_frames().into_iter().zip(expected) {
        let text = encode_agent_frame(&frame).expect("encodes");
        let got: Value = serde_json::from_str(&text).expect("valid json");
        assert_eq!(got, want, "frame {frame:?}");
    }
}

#[test]
fn absent_optional_exec_fields_are_omitted_and_default_back_to_none() {
    let text = encode_hub_frame(&exec()).expect("encodes");
    assert!(!text.contains("stdin"), "{text}");
    assert!(!text.contains("cap_bytes"), "{text}");

    let decoded = decode_hub_frame(r#"{"kind":"exec","id":"01J0","argv":["true"],"timeout_ms":5}"#)
        .expect("decodes without the optional fields");
    assert_eq!(
        decoded,
        HubFrame::Exec {
            id: "01J0".into(),
            argv: vec!["true".into()],
            stdin: None,
            timeout_ms: 5,
            cap_bytes: None,
        }
    );
}

/// Forward compatibility: a newer hub adding a field must not take an older
/// agent's connection down, so an unknown *field* is ignored (an unknown
/// *kind* is not — see below).
#[test]
fn an_unknown_field_is_ignored() {
    let decoded = decode_hub_frame(r#"{"kind":"ping","id":"01J4","nonce":7}"#).expect("decodes");
    assert_eq!(decoded, HubFrame::Ping { id: "01J4".into() });
}

// --- unknown kinds and junk -------------------------------------------------

#[test]
fn an_unknown_kind_is_an_error_not_a_panic() {
    let err = decode_hub_frame(r#"{"kind":"selfdestruct","id":"01J0"}"#).expect_err("rejected");
    assert!(matches!(err, ProtoError::Malformed(_)), "{err:?}");
    let err = decode_agent_frame(r#"{"kind":"selfdestruct","id":"01J0"}"#).expect_err("rejected");
    assert!(matches!(err, ProtoError::Malformed(_)), "{err:?}");
}

#[test]
fn junk_is_always_an_error_in_both_directions() {
    for text in [
        "",
        "null",
        "[]",
        "{}",
        "not json at all",
        r#"{"kind":null}"#,
        r#"{"kind":42}"#,
        r#"{"kind":"exec"}"#,
        r#"{"kind":"exec","id":"1","argv":"not a list","timeout_ms":1}"#,
        r#"{"kind":"exec","id":"1","argv":[],"timeout_ms":-1}"#,
        r#"{"kind":"result","id":"1","exit_code":0,"stdout_b64":"","stderr_b64":""}"#,
        r#"{"kind":"upload","id":"1","path":"/tmp/x"}"#,
        "\u{0}",
    ] {
        assert!(decode_hub_frame(text).is_err(), "hub accepted {text:?}");
        assert!(decode_agent_frame(text).is_err(), "agent accepted {text:?}");
    }
}

/// The two directions are separate vocabularies: an agent must not be able to
/// make a hub act on a frame only the hub is allowed to send, or vice versa.
#[test]
fn the_two_directions_do_not_cross_decode() {
    let pong = encode_agent_frame(&AgentFrame::Pong { id: "01J4".into() }).expect("encodes");
    assert!(decode_hub_frame(&pong).is_err(), "{pong}");

    let ping = encode_hub_frame(&HubFrame::Ping { id: "01J4".into() }).expect("encodes");
    assert!(decode_agent_frame(&ping).is_err(), "{ping}");
}

// --- the size cap -----------------------------------------------------------

/// A small stand-in ceiling. The cap mechanism is what these tests are about,
/// and driving it at [`MAX_FRAME_BYTES`] — now sized for a 200 MiB transcript
/// — would allocate a quarter of a gigabyte per test, several at once, for no
/// extra coverage. `the_public_codec_uses_the_real_cap` pins the number.
const SMALL_CAP: usize = 4096;

#[test]
fn a_payload_past_the_cap_is_rejected_on_decode_not_truncated() {
    let body = "a".repeat(SMALL_CAP);
    let text =
        format!(r#"{{"kind":"upload","id":"1","path":"/tmp/x","mode":384,"bytes_b64":"{body}"}}"#);
    assert!(text.len() > SMALL_CAP);

    match decode_hub_frame_within(&text, SMALL_CAP) {
        Err(ProtoError::TooLarge { size, cap }) => {
            assert_eq!(size, text.len());
            assert_eq!(cap, SMALL_CAP);
        }
        other => panic!("expected TooLarge, got {other:?}"),
    }
}

#[test]
fn a_payload_past_the_cap_is_rejected_on_encode_so_a_peer_never_sees_it() {
    let frame = AgentFrame::Result {
        id: "01J0".into(),
        exit_code: 0,
        stdout_b64: "A".repeat(SMALL_CAP),
        stderr_b64: String::new(),
        truncated: false,
    };
    match encode_agent_frame_within(&frame, SMALL_CAP) {
        Err(ProtoError::TooLarge { cap, .. }) => assert_eq!(cap, SMALL_CAP),
        other => panic!("expected TooLarge, got {:?}", other.map(|t| t.len())),
    }
}

/// The cap is a ceiling, not a target: a frame that just fits still goes.
#[test]
fn a_frame_that_just_fits_the_cap_is_accepted() {
    let skeleton = encode_hub_frame(&HubFrame::Upload {
        id: "1".into(),
        path: "/tmp/x".into(),
        mode: 0o600,
        bytes_b64: String::new(),
    })
    .expect("encodes");
    let frame = HubFrame::Upload {
        id: "1".into(),
        path: "/tmp/x".into(),
        mode: 0o600,
        bytes_b64: "A".repeat(SMALL_CAP - skeleton.len()),
    };
    let text = encode_hub_frame_within(&frame, SMALL_CAP).expect("a frame at the cap encodes");
    assert_eq!(text.len(), SMALL_CAP);
    assert_eq!(
        decode_hub_frame_within(&text, SMALL_CAP).expect("and decodes"),
        frame
    );
}

/// The ceiling the public functions apply, and where it comes from. A 16 MiB
/// cap admitted ~12 MiB of file, which silently broke `move_session` — whose
/// own default cap is 200 MiB — in both directions. The number is now derived
/// from that payload, so this test is the derivation, not a restatement.
#[test]
fn the_public_codec_uses_the_real_cap() {
    assert_eq!(MAX_PAYLOAD_BYTES, 200 * 1024 * 1024);
    assert!(
        MAX_FRAME_BYTES > base64_len(MAX_PAYLOAD_BYTES),
        "the cap must fit a full payload after base64"
    );
    assert!(
        MAX_FRAME_BYTES - base64_len(MAX_PAYLOAD_BYTES) >= 64 * 1024,
        "…with room for the JSON around it"
    );

    // The public entry points really do apply it: one byte over is refused.
    let text = "a".repeat(MAX_FRAME_BYTES + 1);
    match decode_hub_frame(&text) {
        Err(ProtoError::TooLarge { cap, .. }) => assert_eq!(cap, MAX_FRAME_BYTES),
        other => panic!("expected TooLarge, got {other:?}"),
    }
}

/// base64 is 4 bytes per 3, rounded up to a 4-byte group — the arithmetic the
/// transport uses to refuse an oversize file from its size alone, without
/// reading or encoding it.
#[test]
fn base64_len_matches_what_the_encoder_produces() {
    for n in [0usize, 1, 2, 3, 4, 5, 6, 100, 4095, 4096] {
        let bytes = vec![0u8; n];
        assert_eq!(base64_len(n), encode_b64(&bytes).len(), "n={n}");
    }
}

// --- base64 fields ----------------------------------------------------------

#[test]
fn base64_fields_carry_arbitrary_bytes_through_json() {
    // Not valid UTF-8 — the reason these fields are base64 and not strings.
    let raw: &[u8] = &[0x00, 0x9f, 0x92, 0x96, 0xff, b'h', b'i'];
    let frame = HubFrame::Upload {
        id: "01J2".into(),
        path: "/tmp/blob".into(),
        mode: 0o644,
        bytes_b64: encode_b64(raw),
    };
    let decoded = decode_hub_frame(&encode_hub_frame(&frame).expect("encodes")).expect("decodes");
    let HubFrame::Upload { bytes_b64, .. } = decoded else {
        panic!("expected an upload");
    };
    assert_eq!(decode_b64(&bytes_b64).expect("base64 decodes"), raw);
}

#[test]
fn base64_round_trips_the_empty_payload() {
    assert_eq!(encode_b64(b""), "");
    assert_eq!(decode_b64("").expect("decodes"), Vec::<u8>::new());
}

#[test]
fn a_malformed_base64_field_is_an_error_not_a_panic() {
    for text in ["not base64!!", "A", "====", "aGk*"] {
        assert!(
            matches!(decode_b64(text), Err(ProtoError::Malformed(_))),
            "accepted {text:?}"
        );
    }
}

// --- the error type ---------------------------------------------------------

#[test]
fn proto_errors_say_what_went_wrong() {
    let too_large = ProtoError::TooLarge {
        size: 20,
        cap: MAX_FRAME_BYTES,
    };
    let rendered = too_large.to_string();
    assert!(rendered.contains("20"), "{rendered}");
    assert!(
        rendered.contains(&MAX_FRAME_BYTES.to_string()),
        "{rendered}"
    );

    // Usable as a `std::error::Error`, so callers can box it.
    let boxed: Box<dyn std::error::Error> = Box::new(too_large);
    assert!(!boxed.to_string().is_empty());
}

// ── what an `exec`'s answer may cost, shared by both ends ───────────────────
//
// The hub decodes a `result` against `result_budget(cap_bytes)`; the agent
// sizes its streams with `result_stream_limits(cap_bytes)`. They are two
// halves of one contract, so the test is that the worst answer the agent's
// limits allow always fits the hub's budget.

use fleet_proto::{result_budget, result_stream_limits, HEARTBEAT};

/// The largest `result` the limits permit for `cap_bytes`, built for real.
fn worst_result(cap_bytes: Option<u64>) -> AgentFrame {
    let limits = result_stream_limits(cap_bytes);
    let stdout = vec![0xffu8; limits.per_stream];
    let stderr = vec![0xffu8; limits.combined - limits.per_stream];
    AgentFrame::Result {
        // A uuid, as `AgentTransport` sends.
        id: "0b9e6b7e-5a55-4c1e-9d33-3f7a1c2e8f00".into(),
        exit_code: i32::MIN,
        stdout_b64: encode_b64(&stdout),
        stderr_b64: encode_b64(&stderr),
        truncated: true,
    }
}

#[test]
fn the_worst_answer_the_agent_may_send_fits_the_budget_the_hub_decodes_with() {
    for cap in [0u64, 1, 2, 3, 4, 1000, 4096, 65_536, 1_000_000] {
        let frame = worst_result(Some(cap));
        let budget = result_budget(Some(cap));
        let text = encode_agent_frame_within(&frame, budget)
            .unwrap_or_else(|e| panic!("cap {cap}: the agent's worst answer is refused: {e}"));
        assert!(text.len() <= budget);
    }
}

#[test]
fn each_stream_may_reach_the_cap_the_hub_asked_for() {
    let limits = result_stream_limits(Some(1000));
    assert_eq!(limits.per_stream, 1000);
    assert_eq!(limits.combined, 2000, "both streams, each at the cap");
}

/// Past half the payload limit the two streams SHARE it: the hub's budget
/// stops at the frame ceiling, which leaves one envelope around ONE payload.
/// Checked by arithmetic, because building it would allocate ~0.5 GiB.
#[test]
fn large_and_absent_caps_share_one_payload_between_the_streams() {
    for cap in [None, Some(u64::MAX), Some(MAX_PAYLOAD_BYTES as u64)] {
        let limits = result_stream_limits(cap);
        assert_eq!(limits.per_stream, MAX_PAYLOAD_BYTES, "{cap:?}");
        assert_eq!(limits.combined, MAX_PAYLOAD_BYTES, "{cap:?}");
        assert_eq!(result_budget(cap), MAX_FRAME_BYTES, "{cap:?}");
        // base64 of two streams is at most 4 bytes past base64 of their sum,
        // and the JSON around them is far under the envelope.
        assert!(base64_len(limits.combined) + 4 + 1024 <= result_budget(cap));
    }
}

#[test]
fn the_budget_follows_the_cap_and_a_hostile_cap_cannot_overflow_it() {
    assert!(result_budget(Some(1_000_000)) > base64_len(2_000_000));
    assert!(result_budget(Some(1_000_000)) < MAX_FRAME_BYTES / 10);
    assert!(result_budget(Some(1024)) < result_budget(Some(1024 * 1024)));
    assert_eq!(result_budget(Some(u64::MAX)), MAX_FRAME_BYTES);
    assert_eq!(result_budget(None), MAX_FRAME_BYTES);
}

#[test]
fn both_ends_agree_on_the_heartbeat() {
    assert_eq!(HEARTBEAT, std::time::Duration::from_secs(30));
}

// ── protocol versioning ──────────────────────────────────────────────────

/// A `hello` with no `proto` field — what every pre-versioning agent sends —
/// deserialises with `proto: 0`, serde's default for `u32`.
#[test]
fn a_hello_without_proto_decodes_as_zero() {
    let decoded = decode_agent_frame(
        r#"{"kind":"hello","agent_version":"0.1.0","host_name":"laptop","os":"linux"}"#,
    )
    .expect("decodes without the new field");
    assert_eq!(
        decoded,
        AgentFrame::Hello {
            agent_version: "0.1.0".into(),
            host_name: "laptop".into(),
            os: "linux".into(),
            proto: 0,
        }
    );
}

/// A `hello` that does carry `proto` round-trips it exactly — the ordinary
/// case, now that every current build sends one.
#[test]
fn a_hello_with_proto_round_trips_it() {
    let frame = AgentFrame::Hello {
        agent_version: "1.2.3".into(),
        host_name: "laptop".into(),
        os: "linux".into(),
        proto: 7,
    };
    let text = encode_agent_frame(&frame).expect("encodes");
    assert!(text.contains(r#""proto":7"#), "{text}");
    assert_eq!(decode_agent_frame(&text).expect("decodes"), frame);
}

#[test]
fn min_supported_proto_is_the_current_version() {
    // Task 3's own choice: see `MIN_SUPPORTED_PROTO`'s doc for why 1, not 0
    // — the same change that introduces the version field also introduces
    // `HubFrame::Welcome`, which a proto-0 (pre-versioning) agent cannot
    // parse, so admitting proto 0 would not avoid the reconnect loop this
    // task exists to end.
    assert_eq!(MIN_SUPPORTED_PROTO, PROTO_VERSION);
}

#[test]
fn judge_proto_accepts_exactly_the_supported_window() {
    assert_eq!(judge_proto(PROTO_VERSION), ProtoVerdict::Compatible);
    assert_eq!(judge_proto(MIN_SUPPORTED_PROTO), ProtoVerdict::Compatible);
}

#[test]
fn judge_proto_flags_a_peer_below_the_window() {
    let verdict = judge_proto(0);
    assert_eq!(
        verdict,
        ProtoVerdict::PeerBehind {
            their: 0,
            min_supported: MIN_SUPPORTED_PROTO,
        }
    );
    let reason = verdict
        .refusal_reason("the hub", "fleet-agent")
        .expect("out of range");
    assert!(reason.contains('0'), "{reason}");
    assert!(
        reason.contains(&MIN_SUPPORTED_PROTO.to_string()),
        "{reason}"
    );
    assert!(reason.contains("update fleet-agent"), "{reason}");
}

#[test]
fn judge_proto_flags_a_peer_above_the_window() {
    let too_new = PROTO_VERSION + 1;
    let verdict = judge_proto(too_new);
    assert_eq!(
        verdict,
        ProtoVerdict::PeerAhead {
            their: too_new,
            max_supported: PROTO_VERSION,
        }
    );
    let reason = verdict
        .refusal_reason("fleet-agent", "the hub")
        .expect("out of range");
    assert!(reason.contains(&too_new.to_string()), "{reason}");
    assert!(reason.contains(&PROTO_VERSION.to_string()), "{reason}");
    assert!(reason.contains("update fleet-agent"), "{reason}");
}

#[test]
fn a_compatible_verdict_has_no_refusal_reason() {
    assert_eq!(
        ProtoVerdict::Compatible.refusal_reason("the hub", "fleet-agent"),
        None
    );
}

#[test]
fn the_version_refused_close_code_is_in_the_private_use_range() {
    assert!((4000..=4999).contains(&VERSION_REFUSED_CLOSE_CODE));
}

// ── lenient decoding, past the handshake ─────────────────────────────────

#[test]
fn a_known_kind_lenient_decodes_the_same_as_the_strict_decoder() {
    let frame = HubFrame::Ping { id: "01J4".into() };
    let text = encode_hub_frame(&frame).expect("encodes");
    assert_eq!(
        decode_hub_frame_lenient(&text).expect("decodes"),
        Decoded::Frame(frame)
    );
}

#[test]
fn an_unknown_kind_is_skippable_once_past_the_handshake() {
    let text = r#"{"kind":"selfdestruct","id":"01J0"}"#;
    assert_eq!(
        decode_hub_frame_lenient(text).expect("not an error"),
        Decoded::Unknown {
            kind: "selfdestruct".into()
        }
    );
    assert_eq!(
        decode_agent_frame_lenient_within(text, MAX_FRAME_BYTES).expect("not an error"),
        Decoded::Unknown {
            kind: "selfdestruct".into()
        }
    );
}

/// A `kind` the receiver DOES know, but whose body will not parse, is
/// corruption, not evolution — it stays a hard error even leniently.
#[test]
fn a_known_kind_that_will_not_parse_is_still_an_error_leniently() {
    let text = r#"{"kind":"exec","id":"1","argv":"not a list","timeout_ms":1}"#;
    let err = decode_hub_frame_lenient(text).expect_err("rejected");
    assert!(matches!(err, ProtoError::Malformed(_)), "{err:?}");
}

/// Junk with no recognisable `kind` at all is still a hard error leniently,
/// not `Unknown` — there is nothing here to skip.
#[test]
fn junk_with_no_kind_is_still_an_error_leniently() {
    for text in [
        "",
        "null",
        "[]",
        "{}",
        "not json at all",
        r#"{"kind":null}"#,
    ] {
        assert!(decode_hub_frame_lenient(text).is_err(), "accepted {text:?}");
        assert!(
            decode_agent_frame_lenient_within(text, MAX_FRAME_BYTES).is_err(),
            "accepted {text:?}"
        );
    }
}

#[test]
fn the_lenient_decoder_still_enforces_the_size_cap() {
    let body = "a".repeat(SMALL_CAP);
    let text =
        format!(r#"{{"kind":"upload","id":"1","path":"/tmp/x","mode":384,"bytes_b64":"{body}"}}"#);
    match decode_agent_frame_lenient_within(&text, SMALL_CAP) {
        Err(ProtoError::TooLarge { cap, .. }) => assert_eq!(cap, SMALL_CAP),
        other => panic!("expected TooLarge, got {other:?}"),
    }
}
