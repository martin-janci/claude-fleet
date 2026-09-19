//! The agent's half of the protocol, compiled from the agent crate.
//!
//! The point of this file is the dependency edge, not the assertions: it fails
//! to build if `fleet-agent` ever loses its `fleet-proto` dependency, or if
//! someone "shares" the frame types by giving the agent a `fleet-core`
//! dependency instead. The wire itself is pinned in `fleet-proto/tests`.

use fleet_proto::{decode_hub_frame, encode_agent_frame, encode_b64, AgentFrame, HubFrame};

#[test]
fn the_agent_decodes_a_hub_frame_and_answers_with_a_result() {
    let request =
        decode_hub_frame(r#"{"kind":"exec","id":"01J0","argv":["echo","hi"],"timeout_ms":30000}"#)
            .expect("a hub exec frame decodes on the agent side");
    let HubFrame::Exec { id, argv, .. } = request else {
        panic!("expected an exec");
    };
    assert_eq!(argv, ["echo", "hi"]);

    let reply = AgentFrame::Result {
        id,
        exit_code: 0,
        stdout_b64: encode_b64(b"hi\n"),
        stderr_b64: String::new(),
        truncated: false,
    };
    assert!(encode_agent_frame(&reply)
        .expect("encodes")
        .contains("01J0"));
}
