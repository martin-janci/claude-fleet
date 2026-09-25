//! The e2e fake-tracker override (work graph M10.2): refused by every build
//! that may not honour it, and — where it is honoured — confined to a direct
//! Jira Cloud tracker, with every real tracker's fences intact.

use super::*;

#[test]
fn only_a_debug_build_with_the_e2e_feature_honours_the_override() {
    // Unset or blank: nothing to refuse, in any build.
    for raw in [None, Some(""), Some("  ")] {
        for (feature, debug) in [(false, false), (false, true), (true, false), (true, true)] {
            assert_eq!(e2e_loopback_port_in(raw, feature, debug), Ok(None));
        }
    }
    // Set: a normal build (no feature) and a release build (no debug
    // assertions) refuse it, whatever the value.
    for (feature, debug) in [(false, false), (false, true), (true, false)] {
        let e = e2e_loopback_port_in(Some("4567"), feature, debug).unwrap_err();
        assert!(e.contains(E2E_TRACKER_PORT_ENV), "{e}");
    }
    assert!(e2e_loopback_port_in(Some("4567"), false, true)
        .unwrap_err()
        .contains("e2e"));
    assert!(e2e_loopback_port_in(Some("4567"), true, false)
        .unwrap_err()
        .contains("release"));
    assert_eq!(
        e2e_loopback_port_in(Some("4567"), true, true),
        Ok(Some(4567))
    );
    for bad in ["0", "65536", "-1", "http://127.0.0.1:1", "1 2"] {
        assert!(
            e2e_loopback_port_in(Some(bad), true, true).is_err(),
            "{bad}"
        );
    }
}

/// What `cargo test --workspace` and the CI hub build run: no `e2e`
/// feature, so the override is refused outright and the network is the
/// real one.
#[cfg(not(feature = "e2e"))]
#[test]
fn a_normal_build_refuses_the_override() {
    assert!(e2e_loopback_port(Some("4567")).is_err());
    assert!(TrackerNet::real(None)
        .with_e2e_override(Some("4567"))
        .is_err());
    assert!(TrackerNet::real(None).with_e2e_override(None).is_ok());
}

/// A release build refuses it even with the feature.
#[cfg(not(debug_assertions))]
#[test]
fn a_release_build_refuses_the_override() {
    assert!(e2e_loopback_port(Some("4567")).is_err());
}

/// With the feature: only a direct Jira Cloud tracker goes to the loopback
/// port; Data Center still refuses a loopback site (the SSRF fence).
#[cfg(all(feature = "e2e", debug_assertions))]
#[tokio::test]
async fn the_override_reaches_only_jira_cloud_and_keeps_the_fences() {
    use crate::net::https::{Request, TransportError};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((mut c, _)) = l.accept().await {
            let mut buf = vec![0u8; 4096];
            let _ = c.read(&mut buf).await;
            let _ = c
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}")
                .await;
            let _ = c.shutdown().await;
        }
    });
    let net = TrackerNet::real(None)
        .with_e2e_override(Some(&port.to_string()))
        .unwrap();
    let s = crate::store::Store::open_in_memory().unwrap();

    let cloud = s
        .add_tracker("jira", "E2E", "https://e2e.atlassian.net")
        .unwrap();
    let t = net.transport_for(&cloud).unwrap();
    let r = t
        .send(Request::get("https://e2e.atlassian.net/rest/api/3/myself"))
        .await
        .unwrap();
    assert_eq!((r.status, r.text()), (200, "{}".to_string()));
    // Its host policy still holds.
    let e = t
        .send(Request::get("https://evil.example.com/rest/api/3/myself"))
        .await
        .unwrap_err();
    assert!(matches!(e, TransportError::Refused(_)), "{e}");

    // Data Center on a loopback site: refused exactly as without it.
    let dc = s
        .add_tracker("jira_dc", "Loop", "https://127.0.0.1")
        .unwrap();
    let e = net
        .transport_for(&dc)
        .unwrap()
        .send(Request::get(format!(
            "https://127.0.0.1:{port}/rest/api/2/myself"
        )))
        .await
        .unwrap_err();
    assert!(
        matches!(&e, TransportError::Refused(m) if m.contains("allow_private_network")),
        "{e}"
    );
}
