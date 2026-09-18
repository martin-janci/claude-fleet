//! The wire contract between a claude-fleet hub and a `fleet-agent`.
//!
//! These tests are the protocol. The table in
//! `docs/superpowers/specs/2026-09-18-host-agent-design.md` is normative, so
//! the tag and field names below are asserted literally: renaming a field in
//! `lib.rs` without renaming it here is a wire break, and that is the point.

use fleet_proto::{
    decode_agent_frame, decode_b64, decode_hub_frame, encode_agent_frame, encode_b64,
    encode_hub_frame, AgentFrame, HubFrame, ProtoError, MAX_FRAME_BYTES,
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
    ]
}

fn agent_frames() -> Vec<AgentFrame> {
    vec![
        AgentFrame::Hello {
            agent_version: "0.1.0".into(),
            host_name: "laptop".into(),
            os: "linux".into(),
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
            "os": "linux"
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

#[test]
fn a_payload_past_the_cap_is_rejected_on_decode_not_truncated() {
    let body = "a".repeat(MAX_FRAME_BYTES);
    let text =
        format!(r#"{{"kind":"upload","id":"1","path":"/tmp/x","mode":384,"bytes_b64":"{body}"}}"#);
    assert!(text.len() > MAX_FRAME_BYTES);

    match decode_hub_frame(&text) {
        Err(ProtoError::TooLarge { size, cap }) => {
            assert_eq!(size, text.len());
            assert_eq!(cap, MAX_FRAME_BYTES);
        }
        other => panic!("expected TooLarge, got {other:?}"),
    }
}

#[test]
fn a_payload_past_the_cap_is_rejected_on_encode_so_a_peer_never_sees_it() {
    let frame = AgentFrame::Result {
        id: "01J0".into(),
        exit_code: 0,
        stdout_b64: "A".repeat(MAX_FRAME_BYTES),
        stderr_b64: String::new(),
        truncated: false,
    };
    match encode_agent_frame(&frame) {
        Err(ProtoError::TooLarge { cap, .. }) => assert_eq!(cap, MAX_FRAME_BYTES),
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
        bytes_b64: "A".repeat(MAX_FRAME_BYTES - skeleton.len()),
    };
    let text = encode_hub_frame(&frame).expect("a frame at the cap encodes");
    assert_eq!(text.len(), MAX_FRAME_BYTES);
    assert_eq!(decode_hub_frame(&text).expect("and decodes"), frame);
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
