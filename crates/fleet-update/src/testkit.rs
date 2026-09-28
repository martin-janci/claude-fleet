//! A test-only minisign signer and a small release world to sign.
//!
//! minisign's default (prehashed, algorithm `ED`) signature is Ed25519 over
//! BLAKE2b-512 of the file, plus a global signature over
//! `signature || trusted_comment`. Producing it here lets the tests exercise
//! the exact bytes CI's `minisign -S` will produce, through the production
//! verifier.

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use blake2::{Blake2b512, Digest};
use ring::signature::{Ed25519KeyPair, KeyPair};

pub struct TestKey {
    pair: Ed25519KeyPair,
    key_id: [u8; 8],
}

impl TestKey {
    pub fn new(seed: u8) -> Self {
        let pair = Ed25519KeyPair::from_seed_unchecked(&[seed; 32]).unwrap();
        TestKey {
            pair,
            key_id: [seed, 1, 2, 3, 4, 5, 6, 7],
        }
    }

    /// The base64 public key, as `minisign.pub`'s second line.
    pub fn public(&self) -> String {
        let mut bin = Vec::with_capacity(42);
        bin.extend_from_slice(b"Ed");
        bin.extend_from_slice(&self.key_id);
        bin.extend_from_slice(self.pair.public_key().as_ref());
        B64.encode(bin)
    }

    /// A `.minisig` file for `data`.
    pub fn sign(&self, data: &[u8]) -> String {
        let digest = Blake2b512::digest(data);
        let sig = self.pair.sign(&digest);
        let mut bin1 = Vec::with_capacity(74);
        bin1.extend_from_slice(b"ED");
        bin1.extend_from_slice(&self.key_id);
        bin1.extend_from_slice(sig.as_ref());
        let trusted = "timestamp:1790763120\tfile:test";
        let mut global = sig.as_ref().to_vec();
        global.extend_from_slice(trusted.as_bytes());
        let gsig = self.pair.sign(&global);
        format!(
            "untrusted comment: signature from test key\n{}\ntrusted comment: {}\n{}\n",
            B64.encode(bin1),
            trusted,
            B64.encode(gsig.as_ref())
        )
    }
}

/// A release manifest for `version` with the given protocol windows and one
/// artifact per platform the tests use.
pub fn manifest_json(
    version: &str,
    contract: u32,
    desktop: [u32; 2],
    agent_speaks: u32,
    hub_accepts: [u32; 2],
) -> String {
    serde_json::json!({
        "schema": 1,
        "release": {
            "version": version, "track": "stable", "commit": format!("c{version}"),
            "build_id": "test", "published_at": "2026-09-30T00:00:00Z",
            "assets_base": format!("https://example.test/releases/download/v{version}/")
        },
        "compatibility": {
            "contract": { "hub_serves": contract, "desktop_accepts": desktop, "mobile_accepts": [0, contract] },
            "agent_proto": { "hub_accepts": hub_accepts, "agent_speaks": agent_speaks },
            "update_proto": 1
        },
        "components": {
            "hub": { "version": version, "artifacts": [
                { "kind": "oci", "image": "ghcr.io/test/fleet-hub", "digest": format!("sha256:hub-{version}"),
                  "platforms": { "linux/amd64": format!("sha256:hub-{version}-amd64") } } ] },
            "agent": { "version": version, "artifacts": [
                { "kind": "tarball", "target": "x86_64-unknown-linux-gnu", "name": format!("fleet-agent-{version}.tar.gz"),
                  "sha256": format!("agent-{version}"), "size": 1 } ] },
            "desktop": { "version": version, "artifacts": [
                { "kind": "tauri", "platform": "macos-aarch64", "name": format!("claude-fleet_{version}_aarch64.app.tar.gz"),
                  "sha256": format!("mac-{version}"), "size": 1, "tauri_signature": "sig" },
                { "kind": "tauri", "platform": "linux-x86_64", "variant": "appimage",
                  "name": format!("claude-fleet_{version}_amd64.AppImage"),
                  "sha256": format!("appimage-{version}"), "size": 1, "tauri_signature": "sig" } ] },
            "android": { "version": version, "artifacts": [
                { "kind": "apk", "url": format!("https://example.test/fleet-mobile-{version}.apk"),
                  "sha256": format!("apk-{version}"), "size": 1, "version_code": 1, "signer_sha256": "cert" } ] }
        }
    })
    .to_string()
}
