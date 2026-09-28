//! Runs every case of `decide_cases.json` through `decide()`.

use std::collections::{BTreeMap, BTreeSet};

use fleet_update::decide::{decide, DecideInput, HubSpeaks, Pin, Policy, Rollout};
use fleet_update::time::parse_rfc3339;
use fleet_update::wire::Speaks;
use fleet_update::{
    ChannelDoc, Component, Mode, Platform, ReleaseManifest, Source, VerifiedChannel, Version,
    Window,
};
use serde_json::{json, Value};

const FIXTURE: &str = include_str!("decide_cases.json");
const DEFAULT_NOW: &str = "2026-10-01T00:00:00Z";

fn v(s: &str) -> Version {
    Version::parse(s).unwrap()
}

/// Expands a compact release into a full manifest.
fn manifest(version: &str, r: &Value) -> ReleaseManifest {
    let m = json!({
        "schema": 1,
        "release": { "version": version, "track": "stable", "commit": "c", "published_at": "2026-09-30T00:00:00Z",
                     "assets_base": format!("https://example.test/v{version}/") },
        "compatibility": {
            "contract": { "hub_serves": r["contract"], "desktop_accepts": r["desktop"], "mobile_accepts": [0, r["contract"]] },
            "agent_proto": { "hub_accepts": r["hub_accepts"], "agent_speaks": r["agent"] },
            "update_proto": 1
        },
        "components": {
            "hub": { "version": version, "artifacts": [{ "kind": "oci", "image": "ghcr.io/t/fleet-hub",
                "digest": format!("sha256:{version}"), "platforms": { "linux/amd64": format!("sha256:{version}-amd64") } }] },
            "agent": { "version": version, "artifacts": [{ "kind": "tarball", "target": "x86_64-unknown-linux-gnu",
                "name": format!("fleet-agent-{version}.tar.gz"), "sha256": "a", "size": 1 }] },
            "desktop": { "version": version, "artifacts": [
                { "kind": "tauri", "platform": "macos-aarch64", "name": "d.app.tar.gz", "sha256": "m", "size": 1, "tauri_signature": "s" },
                { "kind": "tauri", "platform": "linux-x86_64", "variant": "appimage", "name": "d.AppImage", "sha256": "l", "size": 1, "tauri_signature": "s" }] },
            "android": { "version": version, "artifacts": [{ "kind": "apk", "url": "https://example.test/a.apk",
                "sha256": "p", "size": 1, "version_code": 1, "signer_sha256": "c" }] }
        }
    });
    serde_json::from_value(m).unwrap()
}

fn channel(base: &Value, patch: Option<&Value>) -> ChannelDoc {
    let mut ch = base.clone();
    let versions: Vec<String> = ch["releases"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_str().unwrap().to_string())
        .collect();
    ch["releases"] = versions
        .iter()
        .map(|v| json!({ "version": v, "manifest": format!("https://example.test/v{v}/release-manifest.json"), "manifest_sha256": "0" }))
        .collect();
    if let Some(Value::Object(p)) = patch {
        for (k, val) in p {
            ch[k] = val.clone();
        }
    }
    serde_json::from_value(ch).unwrap()
}

fn policy(p: Option<&Value>) -> Policy {
    let mut out = Policy::default();
    let Some(p) = p else { return out };
    if let Some(m) = p.get("mode") {
        out.mode = serde_json::from_value::<Mode>(m.clone()).unwrap();
    }
    if let Some(m) = p.get("minimum").and_then(Value::as_str) {
        out.minimum = Some(v(m));
    }
    if let Some(pin) = p.get("pin") {
        out.pin = Some(Pin {
            version: v(pin["version"].as_str().unwrap()),
            mandatory: pin
                .get("mandatory")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        });
    }
    out.outside_window = p
        .get("outside_window")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    out
}

#[test]
fn every_fixture_case() {
    let fx: Value = serde_json::from_str(FIXTURE).unwrap();
    let manifests: BTreeMap<Version, ReleaseManifest> = fx["releases"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(ver, r)| (v(ver), manifest(ver, r)))
        .collect();
    let mut failures = Vec::new();
    let mut statuses = BTreeSet::new();

    for case in fx["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let component: Component = serde_json::from_value(case["component"].clone()).unwrap();
        let p = case["platform"].as_array().unwrap();
        let platform = Platform::new(
            p[0].as_str().unwrap(),
            p[1].as_str().unwrap(),
            p[2].as_str().unwrap(),
        );
        let installed = v(case["installed"].as_str().unwrap());
        let speaks: Speaks = case
            .get("speaks")
            .map(|s| serde_json::from_value(s.clone()).unwrap())
            .unwrap_or_default();
        let hub = case.get("hub").filter(|h| !h.is_null()).map(|h| HubSpeaks {
            contract_serves: h["contract"].as_u64().unwrap() as u32,
            agent_proto_accepts: serde_json::from_value::<Window>(h["agent"].clone()).unwrap(),
        });
        let verified = VerifiedChannel {
            doc: channel(&fx["channel"], case.get("channel_patch")),
            fresh: case.get("fresh").and_then(Value::as_bool).unwrap_or(true),
        };
        let no_channel = case
            .get("no_channel")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let pol = policy(case.get("policy"));
        let rollout = case.get("rollout").map(|r| Rollout {
            version: v(r["version"].as_str().unwrap()),
            waves: serde_json::from_value(r["waves"].clone()).unwrap(),
            wave: r["wave"].as_u64().unwrap() as usize,
            paused: r.get("paused").and_then(Value::as_bool).unwrap_or(false),
        });
        let now = parse_rfc3339(
            case.get("now")
                .and_then(Value::as_str)
                .unwrap_or(DEFAULT_NOW),
        )
        .unwrap();

        let d = decide(&DecideInput {
            component,
            platform: &platform,
            installed: &installed,
            speaks: &speaks,
            hub,
            channel: (!no_channel).then_some(&verified),
            manifests: &manifests,
            policy: &pol,
            rollout: rollout.as_ref(),
            target_id: "client:1",
            source: Source::Hub,
            track: fleet_update::Track::Stable,
            now,
        });

        let got = serde_json::to_value(&d).unwrap();
        let exp = &case["expect"];
        statuses.insert(exp["status"].as_str().unwrap().to_string());
        let got_target = got["target"].get("version").cloned().unwrap_or(Value::Null);
        let mut bad = Vec::new();
        if got["status"] != exp["status"] {
            bad.push(format!("status {} (want {})", got["status"], exp["status"]));
        }
        if got["reason"]["code"] != exp["reason"] {
            bad.push(format!(
                "reason {} (want {})",
                got["reason"]["code"], exp["reason"]
            ));
        }
        if got_target != exp["target"] {
            bad.push(format!("target {got_target} (want {})", exp["target"]));
        }
        if let Some(m) = exp.get("mandatory") {
            if &got["target"]["mandatory"] != m {
                bad.push(format!(
                    "mandatory {} (want {m})",
                    got["target"]["mandatory"]
                ));
            }
        }
        if d.reason.text.is_empty() {
            bad.push("empty reason text".into());
        }
        if !bad.is_empty() {
            failures.push(format!("{name}: {}", bad.join(", ")));
        }
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));

    // The phone keeps a sentence per status; the fixture must exercise all of them.
    let all: BTreeSet<String> = [
        "up_to_date",
        "update_available",
        "update_required",
        "client_too_new",
        "rollback",
        "hold",
        "unknown",
    ]
    .into_iter()
    .map(String::from)
    .collect();
    assert_eq!(
        statuses, all,
        "every status needs at least one fixture case"
    );
}
