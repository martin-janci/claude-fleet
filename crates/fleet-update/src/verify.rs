//! Signature verification, the replay / freeze guard, and the one target check
//! every install goes through (U1, U2, U6).
//!
//! The release key is the authority for *content*. Nothing a hub (or a CDN,
//! or GitHub) says can make a caller install bytes the key did not sign: a
//! decision is only ever a *selection* among signed releases, and
//! [`verify_target`] re-derives the selection's legitimacy from the signed
//! documents alone.

use std::fmt;

use minisign_verify::{PublicKey, Signature};

use crate::channel_doc::{ChannelDoc, CHANNEL_SCHEMA};
use crate::manifest::{Artifact, ReleaseManifest, MANIFEST_SCHEMA};
use crate::model::{Component, Platform, Track};
use crate::time::parse_rfc3339;
use crate::wire::{Decision, Evidence};
use crate::Version;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifyError {
    /// A key or signature that does not decode.
    Malformed(String),
    /// No trusted key produced this signature.
    BadSignature,
    /// The JSON does not parse as the document it claims to be.
    Decode(String),
    UnsupportedSchema(u32),
    WrongTrack {
        expected: Track,
        got: Track,
    },
    /// A sequence lower than one already seen: a replayed document.
    Replayed {
        seen: u64,
        got: u64,
    },
    /// The decision names a target but carries no evidence for it.
    MissingEvidence,
    /// The manifest's bytes are not the ones the channel lists for the target.
    ManifestNotInChannel,
    /// The decision's target and the signed manifest disagree.
    TargetMismatch(String),
    /// The target is withdrawn or below the publisher's signed floor (U6).
    NotPermitted(Version),
    /// The decision's artifact is not one the signed manifest carries for
    /// this component and platform.
    ArtifactMismatch,
}

impl fmt::Display for VerifyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            VerifyError::Malformed(m) => write!(f, "malformed key or signature: {m}"),
            VerifyError::BadSignature => write!(f, "no trusted release key signed this document"),
            VerifyError::Decode(m) => write!(f, "unreadable document: {m}"),
            VerifyError::UnsupportedSchema(s) => {
                write!(f, "document schema {s} is newer than this build reads")
            }
            VerifyError::WrongTrack { expected, got } => {
                write!(
                    f,
                    "expected the {} channel, got {}",
                    expected.as_str(),
                    got.as_str()
                )
            }
            VerifyError::Replayed { seen, got } => {
                write!(
                    f,
                    "channel sequence {got} is older than {seen}, already seen (replay)"
                )
            }
            VerifyError::MissingEvidence => {
                write!(f, "the decision carries no signed evidence for its target")
            }
            VerifyError::ManifestNotInChannel => {
                write!(
                    f,
                    "the manifest is not the one the signed channel lists for this release"
                )
            }
            VerifyError::TargetMismatch(m) => {
                write!(f, "the target disagrees with its signed manifest: {m}")
            }
            VerifyError::NotPermitted(v) => {
                write!(
                    f,
                    "{v} is withdrawn or below the publisher's signed minimum"
                )
            }
            VerifyError::ArtifactMismatch => {
                write!(
                    f,
                    "the artifact is not one the signed manifest carries for this platform"
                )
            }
        }
    }
}

impl std::error::Error for VerifyError {}

/// The release keys a build trusts: compiled in, plus any a verified channel
/// document rotated in ([`TrustedKeys::adopt_rotation`]).
#[derive(Debug, Clone)]
pub struct TrustedKeys {
    keys: Vec<(String, PublicKey)>,
}

impl TrustedKeys {
    /// From base64 public keys (`minisign.pub`'s second line).
    pub fn from_base64<'a>(keys: impl IntoIterator<Item = &'a str>) -> Result<Self, VerifyError> {
        let mut out = TrustedKeys { keys: Vec::new() };
        for k in keys {
            out.push(k)?;
        }
        Ok(out)
    }

    fn push(&mut self, b64: &str) -> Result<(), VerifyError> {
        let b64 = b64.trim();
        if self.keys.iter().any(|(k, _)| k == b64) {
            return Ok(());
        }
        let pk = PublicKey::from_base64(b64).map_err(|e| VerifyError::Malformed(e.to_string()))?;
        self.keys.push((b64.to_string(), pk));
        Ok(())
    }

    /// The base64 form of every trusted key, for persisting a rotation.
    pub fn to_base64(&self) -> Vec<String> {
        self.keys.iter().map(|(k, _)| k.clone()).collect()
    }

    /// Trust the keys a *verified* channel document vouches for. The caller
    /// persists the result; the document was signed by a key already trusted,
    /// so this can only ever extend trust along a chain the publisher signed.
    pub fn adopt_rotation(&mut self, verified: &VerifiedChannel) -> Result<(), VerifyError> {
        for k in &verified.doc.next_keys {
            self.push(k)?;
        }
        Ok(())
    }

    /// Verify a detached minisign signature over `data`.
    pub fn verify(&self, data: &[u8], minisig: &str) -> Result<(), VerifyError> {
        let sig = Signature::decode(minisig).map_err(|e| VerifyError::Malformed(e.to_string()))?;
        // Prehashed signatures only: `minisign -S` has produced them by default
        // since 0.8, and CI signs with nothing else.
        if self
            .keys
            .iter()
            .any(|(_, pk)| pk.verify(data, &sig, false).is_ok())
        {
            Ok(())
        } else {
            Err(VerifyError::BadSignature)
        }
    }
}

/// A channel document whose signature, schema, track and sequence checked out.
#[derive(Debug, Clone, PartialEq)]
pub struct VerifiedChannel {
    pub doc: ChannelDoc,
    /// `now < expires_at`. A stale document still says what exists; it never
    /// offers or requires an update (decide rule 1).
    pub fresh: bool,
}

pub fn verify_manifest(
    bytes: &[u8],
    minisig: &str,
    keys: &TrustedKeys,
) -> Result<ReleaseManifest, VerifyError> {
    keys.verify(bytes, minisig)?;
    let schema = peek_schema(bytes)?;
    if schema != MANIFEST_SCHEMA {
        return Err(VerifyError::UnsupportedSchema(schema));
    }
    serde_json::from_slice(bytes).map_err(|e| VerifyError::Decode(e.to_string()))
}

/// Verify a channel document for `track`. `seen` is the highest sequence the
/// caller has stored for the track (0 for none); the caller stores
/// `doc.sequence` after this succeeds.
pub fn verify_channel(
    bytes: &[u8],
    minisig: &str,
    keys: &TrustedKeys,
    track: Track,
    seen: u64,
    now: i64,
) -> Result<VerifiedChannel, VerifyError> {
    keys.verify(bytes, minisig)?;
    let schema = peek_schema(bytes)?;
    if schema != CHANNEL_SCHEMA {
        return Err(VerifyError::UnsupportedSchema(schema));
    }
    let doc: ChannelDoc =
        serde_json::from_slice(bytes).map_err(|e| VerifyError::Decode(e.to_string()))?;
    if doc.track != track {
        return Err(VerifyError::WrongTrack {
            expected: track,
            got: doc.track,
        });
    }
    if doc.sequence < seen {
        return Err(VerifyError::Replayed {
            seen,
            got: doc.sequence,
        });
    }
    // An unreadable expiry is treated as already expired: fail stale, not open.
    let fresh = parse_rfc3339(&doc.expires_at).is_some_and(|exp| now < exp);
    Ok(VerifiedChannel { doc, fresh })
}

fn peek_schema(bytes: &[u8]) -> Result<u32, VerifyError> {
    #[derive(serde::Deserialize)]
    struct Peek {
        schema: u32,
    }
    serde_json::from_slice::<Peek>(bytes)
        .map(|p| p.schema)
        .map_err(|e| VerifyError::Decode(e.to_string()))
}

/// A decision's target, proven against the signed documents.
#[derive(Debug, Clone, PartialEq)]
pub struct VerifiedTarget {
    pub component: Component,
    pub version: Version,
    pub artifact: Artifact,
    pub url: Option<String>,
    pub manifest: ReleaseManifest,
    pub channel_sequence: u64,
}

/// The check between "a channel said so" and "install it", run on every
/// decision whichever channel produced it:
///
/// 1. the relayed channel document and manifest verify under `keys`;
/// 2. the manifest's exact bytes are what the channel lists for the target;
/// 3. the target is a release the publisher permits for this component (U6);
/// 4. the decision's artifact is one the manifest carries for `platform`.
///
/// `Ok(None)` when the decision names no target. `seen` is the caller's
/// stored sequence for the track; store `channel_sequence` afterwards.
pub fn verify_target(
    decision: &Decision,
    keys: &TrustedKeys,
    platform: &Platform,
    seen: u64,
    now: i64,
) -> Result<Option<VerifiedTarget>, VerifyError> {
    let Some(target) = &decision.target else {
        return Ok(None);
    };
    let Evidence {
        channel,
        channel_sig,
        manifest,
        manifest_sig,
    } = target
        .evidence
        .as_ref()
        .ok_or(VerifyError::MissingEvidence)?;
    let ch = verify_channel(
        channel.as_bytes(),
        channel_sig,
        keys,
        decision.track,
        seen,
        now,
    )?;
    let m = verify_manifest(manifest.as_bytes(), manifest_sig, keys)?;
    let listed = ch
        .doc
        .release(&target.version)
        .ok_or(VerifyError::ManifestNotInChannel)?;
    let digest = sha256_hex(manifest.as_bytes());
    if !listed.manifest_sha256.eq_ignore_ascii_case(&digest)
        || !target.manifest.sha256.eq_ignore_ascii_case(&digest)
    {
        return Err(VerifyError::ManifestNotInChannel);
    }
    if m.release.version != target.version {
        return Err(VerifyError::TargetMismatch(format!(
            "manifest is {}, target is {}",
            m.release.version, target.version
        )));
    }
    let component = decision.component;
    if !ch.doc.permits(component, &target.version) {
        return Err(VerifyError::NotPermitted(target.version.clone()));
    }
    let carried = m
        .component(component)
        .map(|c| {
            c.artifacts
                .iter()
                .any(|a| a == &target.artifact && a.matches(platform))
        })
        .unwrap_or(false);
    if !carried {
        return Err(VerifyError::ArtifactMismatch);
    }
    let url = target.artifact.url(&m.release.assets_base);
    Ok(Some(VerifiedTarget {
        component,
        version: target.version.clone(),
        artifact: target.artifact.clone(),
        url,
        manifest: m,
        channel_sequence: ch.doc.sequence,
    }))
}

pub fn sha256_hex(data: &[u8]) -> String {
    use sha2::Digest as _;
    sha2::Sha256::digest(data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::{manifest_json, TestKey};

    const NOW: i64 = 1_790_763_120; // 2026-09-30T10:12:00Z

    fn channel_json(seq: u64, expires: &str, next_keys: &[String]) -> String {
        serde_json::json!({
            "schema": 1, "track": "stable", "sequence": seq,
            "generated_at": "2026-09-30T00:00:00Z", "expires_at": expires,
            "current": "0.3.4", "recommended": "0.3.4",
            "releases": [], "next_keys": next_keys
        })
        .to_string()
    }

    #[test]
    fn a_signed_manifest_verifies_and_a_tampered_one_does_not() {
        let k = TestKey::new(1);
        let keys = TrustedKeys::from_base64([k.public().as_str()]).unwrap();
        let m = manifest_json("0.3.4", 5, [5, 5], 1, [1, 1]);
        let sig = k.sign(m.as_bytes());
        let got = verify_manifest(m.as_bytes(), &sig, &keys).unwrap();
        assert_eq!(got.release.version, Version::new(0, 3, 4));

        let tampered = m.replace("sha256:hub-0.3.4", "sha256:evil");
        assert_eq!(
            verify_manifest(tampered.as_bytes(), &sig, &keys),
            Err(VerifyError::BadSignature)
        );
    }

    #[test]
    fn a_foreign_key_is_refused() {
        let (k, other) = (TestKey::new(1), TestKey::new(2));
        let keys = TrustedKeys::from_base64([k.public().as_str()]).unwrap();
        let m = manifest_json("0.3.4", 5, [5, 5], 1, [1, 1]);
        assert_eq!(
            verify_manifest(m.as_bytes(), &other.sign(m.as_bytes()), &keys),
            Err(VerifyError::BadSignature)
        );
    }

    #[test]
    fn a_newer_schema_is_refused_not_half_read() {
        let k = TestKey::new(1);
        let keys = TrustedKeys::from_base64([k.public().as_str()]).unwrap();
        let m = manifest_json("0.3.4", 5, [5, 5], 1, [1, 1]).replacen(
            "\"schema\":1",
            "\"schema\":2",
            1,
        );
        assert_eq!(
            verify_manifest(m.as_bytes(), &k.sign(m.as_bytes()), &keys),
            Err(VerifyError::UnsupportedSchema(2))
        );
    }

    #[test]
    fn replay_wrong_track_and_expiry() {
        let k = TestKey::new(1);
        let keys = TrustedKeys::from_base64([k.public().as_str()]).unwrap();
        let doc = channel_json(118, "2026-10-14T00:00:00Z", &[]);
        let sig = k.sign(doc.as_bytes());

        let ok = verify_channel(doc.as_bytes(), &sig, &keys, Track::Stable, 118, NOW).unwrap();
        assert!(ok.fresh);
        assert_eq!(
            verify_channel(doc.as_bytes(), &sig, &keys, Track::Stable, 119, NOW),
            Err(VerifyError::Replayed {
                seen: 119,
                got: 118
            })
        );
        assert!(matches!(
            verify_channel(doc.as_bytes(), &sig, &keys, Track::Beta, 0, NOW),
            Err(VerifyError::WrongTrack { .. })
        ));
        let later = crate::time::parse_rfc3339("2026-10-14T00:00:00Z").unwrap();
        assert!(
            !verify_channel(doc.as_bytes(), &sig, &keys, Track::Stable, 0, later)
                .unwrap()
                .fresh
        );

        let garbled = channel_json(118, "soon", &[]);
        let g = verify_channel(
            garbled.as_bytes(),
            &k.sign(garbled.as_bytes()),
            &keys,
            Track::Stable,
            0,
            NOW,
        );
        assert!(!g.unwrap().fresh, "an unreadable expiry fails stale");
    }

    #[test]
    fn rotation_extends_trust_only_through_a_verified_document() {
        let (old, new) = (TestKey::new(1), TestKey::new(2));
        let mut keys = TrustedKeys::from_base64([old.public().as_str()]).unwrap();

        let by_new = channel_json(2, "2026-10-14T00:00:00Z", &[]);
        let sig_new = new.sign(by_new.as_bytes());
        assert_eq!(
            verify_channel(by_new.as_bytes(), &sig_new, &keys, Track::Stable, 0, NOW),
            Err(VerifyError::BadSignature)
        );

        let handover = channel_json(1, "2026-10-14T00:00:00Z", &[new.public()]);
        let v = verify_channel(
            handover.as_bytes(),
            &old.sign(handover.as_bytes()),
            &keys,
            Track::Stable,
            0,
            NOW,
        )
        .unwrap();
        keys.adopt_rotation(&v).unwrap();
        assert_eq!(keys.to_base64().len(), 2);
        verify_channel(by_new.as_bytes(), &sig_new, &keys, Track::Stable, 1, NOW).unwrap();
    }
}
