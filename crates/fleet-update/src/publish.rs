//! The publisher's side (slice S2): what CI writes, built from the same types
//! every reader parses, so a manifest or channel document the release job
//! writes is by construction one the fleet can read.
//!
//! Pure functions over their inputs; `bin/fleet-release.rs` is the CLI CI
//! calls, and signing is `minisign` itself (the reference implementation),
//! never this crate.

use std::collections::BTreeMap;

use crate::channel_doc::{ChannelDoc, Mandatory, ReleaseRef, Withdrawn, CHANNEL_SCHEMA};
use crate::manifest::{
    AgentProtoCompat, Artifact, Compatibility, ComponentRelease, ContractCompat, PeerProtoCompat,
    ReleaseInfo, ReleaseManifest, StoreCompat, MANIFEST_SCHEMA,
};
use crate::model::{Track, Window};
use crate::time::format_rfc3339;
use crate::Version;

/// What `fleet-hub compat --json` prints: the hub build's own windows.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct HubCompat {
    pub contract: HubContract,
    pub agent_proto: AgentProtoCompat,
    pub peer_proto: PeerProtoCompat,
    pub store: StoreCompat,
    pub update_proto: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct HubContract {
    pub hub_serves: u32,
}

/// One asset on the release: its file name, bytes' sha256 and size.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asset {
    pub name: String,
    pub sha256: String,
    pub size: u64,
}

/// A published hub image, as `hub-image.yml` records it on the release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HubImage {
    pub image: String,
    pub digest: String,
}

pub struct ManifestInput<'a> {
    pub version: &'a Version,
    pub commit: &'a str,
    pub build_id: &'a str,
    pub published_at: i64,
    /// `https://github.com/<repo>/releases/download/<tag>/`
    pub assets_base: &'a str,
    pub notes_url: &'a str,
    pub hub: &'a HubCompat,
    pub desktop_accepts: Window,
    pub assets: &'a [Asset],
    pub hub_image: Option<&'a HubImage>,
    /// The desktop's updater bundles (`.app.tar.gz`, `.AppImage`,
    /// `-setup.exe`) by asset name → `tauri-plugin-updater`'s `signature`:
    /// the base64 of a `minisign -S` signature by the release key, made by
    /// `scripts/release-manifest.sh` (S7). A bundle without one is offered as
    /// a download only.
    pub tauri_sigs: &'a BTreeMap<String, String>,
}

/// The track a version belongs to (U9): a pre-release is `beta` (`-rc.N`) or
/// `nightly` (`-dev.N…`); anything else is `stable`.
pub fn track_of(v: &Version) -> Track {
    if v.pre.is_empty() {
        Track::Stable
    } else if v.pre.as_str().starts_with("dev") {
        Track::Nightly
    } else {
        Track::Beta
    }
}

/// Build the release manifest from the release's actual assets. Names that
/// are not an installable artifact (SHA256SUMS, the manifest itself, an
/// updater bundle nobody signed) are left out.
pub fn build_manifest(i: &ManifestInput) -> Result<ReleaseManifest, String> {
    let v = i.version.to_string();
    let mut components: BTreeMap<String, ComponentRelease> = BTreeMap::new();
    let mut push = |c: &str, a: Artifact| {
        components
            .entry(c.to_string())
            .or_insert_with(|| ComponentRelease {
                version: i.version.clone(),
                artifacts: Vec::new(),
            })
            .artifacts
            .push(a);
    };
    if let Some(img) = i.hub_image {
        if !img.digest.starts_with("sha256:") || img.digest.len() != 7 + 64 {
            return Err(format!(
                "hub image digest {:?} is not sha256:<64 hex>",
                img.digest
            ));
        }
        push(
            "hub",
            Artifact::Oci {
                image: img.image.clone(),
                digest: img.digest.clone(),
                platforms: BTreeMap::new(),
            },
        );
    }
    let desktop = [
        (
            format!("claude-fleet_{v}_aarch64.dmg"),
            "macos-aarch64",
            "dmg",
        ),
        (format!("claude-fleet_{v}_x64.dmg"), "macos-x86_64", "dmg"),
        (format!("claude-fleet_{v}_amd64.deb"), "linux-x86_64", "deb"),
        (
            format!("claude-fleet_{v}_amd64.AppImage"),
            "linux-x86_64",
            "appimage",
        ),
        (
            format!("claude-fleet_{v}_x64-setup.exe"),
            "windows-x86_64",
            "nsis",
        ),
    ];
    // What tauri-plugin-updater installs in place, once signed.
    let updatable = [
        (
            format!("claude-fleet_{v}_aarch64.app.tar.gz"),
            "macos-aarch64",
            None,
        ),
        (
            format!("claude-fleet_{v}_x64.app.tar.gz"),
            "macos-x86_64",
            None,
        ),
        (
            format!("claude-fleet_{v}_amd64.AppImage"),
            "linux-x86_64",
            Some("appimage"),
        ),
        (
            format!("claude-fleet_{v}_x64-setup.exe"),
            "windows-x86_64",
            Some("nsis"),
        ),
    ];
    for a in i.assets {
        if let (Some((_, platform, variant)), Some(sig)) = (
            updatable.iter().find(|(n, _, _)| *n == a.name),
            i.tauri_sigs.get(&a.name),
        ) {
            push(
                "desktop",
                Artifact::Tauri {
                    platform: (*platform).into(),
                    variant: variant.map(String::from),
                    name: a.name.clone(),
                    sha256: a.sha256.clone(),
                    size: a.size,
                    tauri_signature: sig.clone(),
                },
            );
            continue;
        }
        if let Some(target) = a
            .name
            .strip_prefix(&format!("fleet-hub-{v}-"))
            .and_then(|r| r.strip_suffix(".tar.gz"))
        {
            push("hub", tarball(target, a));
        } else if let Some(target) = a
            .name
            .strip_prefix(&format!("fleet-agent-{v}-"))
            .and_then(|r| r.strip_suffix(".tar.gz"))
        {
            push("agent", tarball(target, a));
        } else if let Some((_, platform, variant)) = desktop.iter().find(|(n, _, _)| *n == a.name) {
            push(
                "desktop",
                Artifact::Download {
                    platform: (*platform).into(),
                    variant: Some((*variant).into()),
                    name: a.name.clone(),
                    sha256: a.sha256.clone(),
                    size: a.size,
                },
            );
        }
    }
    if !components.contains_key("hub") {
        return Err(
            "no hub artifact among the assets (no image digest and no fleet-hub tarball)".into(),
        );
    }
    let h = i.hub;
    Ok(ReleaseManifest {
        schema: MANIFEST_SCHEMA,
        release: ReleaseInfo {
            version: i.version.clone(),
            track: track_of(i.version),
            commit: i.commit.into(),
            build_id: i.build_id.into(),
            published_at: format_rfc3339(i.published_at),
            assets_base: i.assets_base.into(),
            notes_url: i.notes_url.into(),
        },
        compatibility: Compatibility {
            contract: ContractCompat {
                hub_serves: h.contract.hub_serves,
                desktop_accepts: i.desktop_accepts,
                mobile_accepts: None,
            },
            agent_proto: h.agent_proto,
            peer_proto: Some(h.peer_proto),
            store: Some(h.store),
            update_proto: h.update_proto,
        },
        components,
    })
}

fn tarball(target: &str, a: &Asset) -> Artifact {
    Artifact::Tarball {
        target: target.into(),
        name: a.name.clone(),
        sha256: a.sha256.clone(),
        size: a.size,
    }
}

/// How long a freshly signed channel document stays fresh.
pub const DEFAULT_EXPIRES_DAYS: i64 = 14;
/// Releases a channel keeps listed (the recommended and rollback ones are
/// always kept).
pub const DEFAULT_KEEP: usize = 30;

/// A new, empty channel for `track`, at sequence 0 (every write bumps it).
pub fn empty_channel(track: Track, first: &Version) -> ChannelDoc {
    ChannelDoc {
        schema: CHANNEL_SCHEMA,
        track,
        sequence: 0,
        generated_at: String::new(),
        expires_at: String::new(),
        current: first.clone(),
        recommended: first.clone(),
        minimum_supported: BTreeMap::new(),
        mandatory: Vec::new(),
        rollback: None,
        withdrawn: Vec::new(),
        releases: Vec::new(),
        next_keys: Vec::new(),
    }
}

/// Every write: a higher sequence and a fresh expiry.
fn stamp(doc: &mut ChannelDoc, now: i64, expires_days: i64) {
    doc.sequence += 1;
    doc.generated_at = format_rfc3339(now);
    doc.expires_at = format_rfc3339(now + expires_days * 86_400);
}

/// List a published release on a track: the entry is replaced if the
/// version is already there (a re-run), `current` moves up, and so does
/// `recommended` on a release's first listing — a publisher holds a release
/// back with [`Edit::Recommend`] afterwards, never by default, and a re-run
/// keeps that hold.
pub fn channel_add(
    doc: Option<ChannelDoc>,
    track: Track,
    release: ReleaseRef,
    now: i64,
    expires_days: i64,
    keep: usize,
) -> Result<ChannelDoc, String> {
    let mut doc = doc.unwrap_or_else(|| empty_channel(track, &release.version));
    if doc.track != track {
        return Err(format!(
            "the document is the {} channel, not {}",
            doc.track.as_str(),
            track.as_str()
        ));
    }
    let v = release.version.clone();
    // A re-run (CI retrying a publish) must not undo the publisher's later
    // `Recommend` hold-back, nor make a withdrawn release recommended.
    let first_listing = doc.release(&v).is_none() && !doc.is_withdrawn(&v);
    doc.releases.retain(|r| r.version != v);
    doc.releases.push(release);
    doc.releases.sort_by(|a, b| b.version.cmp(&a.version));
    if doc.releases.len() == 1 || v > doc.current {
        doc.current = v.clone();
    }
    if doc.releases.len() == 1 || (first_listing && v > doc.recommended) {
        doc.recommended = v.clone();
    }
    trim(&mut doc, keep);
    stamp(&mut doc, now, expires_days);
    Ok(doc)
}

fn trim(doc: &mut ChannelDoc, keep: usize) {
    let protect: Vec<Version> = [Some(doc.recommended.clone()), doc.rollback.clone()]
        .into_iter()
        .flatten()
        .collect();
    let mut kept = 0;
    doc.releases.retain(|r| {
        kept += 1;
        kept <= keep || protect.contains(&r.version)
    });
}

/// A publisher's after-the-fact change (the `update-channels.yml` dispatch).
#[derive(Debug, Clone, PartialEq)]
pub enum Edit {
    /// Only re-sign: a new sequence and expiry, nothing else changes.
    Resign,
    Withdraw {
        version: Version,
        reason: String,
    },
    Recommend(Version),
    Rollback(Option<Version>),
    Minimum {
        component: String,
        version: Option<Version>,
    },
    Mandatory(Mandatory),
}

pub fn channel_edit(
    mut doc: ChannelDoc,
    edit: Edit,
    now: i64,
    expires_days: i64,
) -> Result<ChannelDoc, String> {
    let listed = |doc: &ChannelDoc, v: &Version| {
        doc.release(v)
            .map(|_| ())
            .ok_or_else(|| format!("{v} is not listed on the {} channel", doc.track.as_str()))
    };
    match edit {
        Edit::Resign => {}
        Edit::Withdraw { version, reason } => {
            listed(&doc, &version)?;
            if version == doc.recommended {
                return Err(format!(
                    "{version} is recommended; recommend another release before withdrawing it"
                ));
            }
            doc.withdrawn.retain(|w| w.version != version);
            doc.withdrawn.push(Withdrawn { version, reason });
        }
        Edit::Recommend(v) => {
            listed(&doc, &v)?;
            if doc.is_withdrawn(&v) {
                return Err(format!("{v} is withdrawn"));
            }
            doc.recommended = v;
        }
        Edit::Rollback(v) => {
            if let Some(v) = &v {
                listed(&doc, v)?;
            }
            doc.rollback = v;
        }
        Edit::Minimum { component, version } => {
            const COMPONENTS: [&str; 5] = ["hub", "agent", "desktop", "android", "ios"];
            if !COMPONENTS.contains(&component.as_str()) {
                return Err(format!(
                    "component must be one of {}",
                    COMPONENTS.join(" | ")
                ));
            }
            match version {
                Some(v) => {
                    doc.minimum_supported.insert(component, v);
                }
                None => {
                    doc.minimum_supported.remove(&component);
                }
            }
        }
        Edit::Mandatory(m) => {
            listed(&doc, &m.version)?;
            if let Some(d) = &m.deadline {
                crate::time::parse_rfc3339(d)
                    .ok_or_else(|| format!("deadline {d:?} is not RFC 3339"))?;
            }
            doc.mandatory.retain(|x| x.version != m.version);
            doc.mandatory.push(m);
        }
    }
    stamp(&mut doc, now, expires_days);
    Ok(doc)
}

/// The exact bytes CI signs and publishes for a document: pretty JSON with a
/// trailing newline, so a diff of the channel branch reads line by line.
pub fn to_bytes<T: serde::Serialize>(doc: &T) -> Vec<u8> {
    let mut out = serde_json::to_vec_pretty(doc).expect("our own types serialize");
    out.push(b'\n');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Platform;
    use crate::testkit::TestKey;
    use crate::verify::{sha256_hex, verify_channel, verify_manifest, TrustedKeys};

    fn v(s: &str) -> Version {
        Version::parse(s).unwrap()
    }

    fn compat() -> HubCompat {
        HubCompat {
            contract: HubContract { hub_serves: 6 },
            agent_proto: AgentProtoCompat {
                hub_accepts: Window::new(1, 1),
                agent_speaks: 1,
            },
            peer_proto: PeerProtoCompat {
                speaks: 1,
                accepts: Window::new(1, 1),
            },
            store: StoreCompat {
                schema_to: 79,
                opens_down_to: 1,
            },
            update_proto: 1,
        }
    }

    fn asset(name: &str) -> Asset {
        Asset {
            name: name.into(),
            sha256: sha256_hex(name.as_bytes()),
            size: 42,
        }
    }

    fn manifest_for(version: &str, image: Option<&HubImage>) -> Result<ReleaseManifest, String> {
        let ver = v(version);
        let names = [
            format!("claude-fleet_{version}_aarch64.dmg"),
            format!("claude-fleet_{version}_aarch64.app.tar.gz"),
            format!("claude-fleet_{version}_amd64.AppImage"),
            format!("claude-fleet_{version}_x64-setup.exe"),
            format!("fleet-hub-{version}-x86_64-unknown-linux-gnu.tar.gz"),
            format!("fleet-agent-{version}-aarch64-unknown-linux-gnu.tar.gz"),
            "SHA256SUMS".to_string(),
        ];
        let assets: Vec<Asset> = names.iter().map(|n| asset(n)).collect();
        build_manifest(&ManifestInput {
            version: &ver,
            commit: "a83f19d0",
            build_id: "gh-run-1-1",
            published_at: 1_790_763_120,
            assets_base: "https://github.com/o/r/releases/download/v0.4.1/",
            notes_url: "https://github.com/o/r/releases/tag/v0.4.1",
            hub: &compat(),
            desktop_accepts: Window::new(6, 6),
            assets: &assets,
            hub_image: image,
            tauri_sigs: &BTreeMap::new(),
        })
    }

    #[test]
    fn a_signed_updater_bundle_is_a_tauri_artifact_and_an_unsigned_one_a_download() {
        let ver = v("0.5.5");
        let names = [
            "claude-fleet_0.5.5_aarch64.app.tar.gz",
            "claude-fleet_0.5.5_aarch64.dmg",
            "claude-fleet_0.5.5_amd64.AppImage",
            "claude-fleet_0.5.5_amd64.deb",
            "claude-fleet_0.5.5_x64-setup.exe",
            "fleet-hub-0.5.5-x86_64-unknown-linux-gnu.tar.gz",
        ];
        let assets: Vec<Asset> = names.iter().map(|n| asset(n)).collect();
        let sigs: BTreeMap<String, String> = [
            ("claude-fleet_0.5.5_aarch64.app.tar.gz", "c2lnLW1hYw=="),
            ("claude-fleet_0.5.5_amd64.AppImage", "c2lnLWFwcGltYWdl"),
        ]
        .into_iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect();
        let m = build_manifest(&ManifestInput {
            version: &ver,
            commit: "c",
            build_id: "b",
            published_at: 0,
            assets_base: "https://x/",
            notes_url: "",
            hub: &compat(),
            desktop_accepts: Window::new(6, 6),
            assets: &assets,
            hub_image: None,
            tauri_sigs: &sigs,
        })
        .unwrap();
        let d = crate::Component::Desktop;
        let mac = m
            .artifact_for(d, &Platform::new("macos", "aarch64", "tauri"))
            .unwrap();
        assert!(
            matches!(mac, Artifact::Tauri { tauri_signature, name, .. }
                if tauri_signature == "c2lnLW1hYw==" && name.ends_with(".app.tar.gz")),
            "{mac:?}"
        );
        let appimage = m
            .artifact_for(d, &Platform::new("linux", "x86_64", "appimage"))
            .unwrap();
        assert!(matches!(appimage, Artifact::Tauri { .. }), "{appimage:?}");
        // No signature: the installer is a download a person runs.
        let nsis = m
            .artifact_for(d, &Platform::new("windows", "x86_64", "nsis"))
            .unwrap();
        assert!(matches!(nsis, Artifact::Download { .. }), "{nsis:?}");
        assert!(matches!(
            m.artifact_for(d, &Platform::new("linux", "x86_64", "deb")),
            Some(Artifact::Download { .. })
        ));
    }

    #[test]
    fn a_manifest_from_the_release_assets_reads_back_and_matches_platforms() {
        let img = HubImage {
            image: "ghcr.io/o/fleet-hub".into(),
            digest: format!("sha256:{}", "a".repeat(64)),
        };
        let m = manifest_for("0.4.1", Some(&img)).unwrap();
        assert_eq!(m.release.track, Track::Stable);
        assert_eq!(m.release.published_at, "2026-09-30T10:12:00Z");
        assert_eq!(m.compatibility.contract.desktop_accepts, Window::new(6, 6));
        assert_eq!(m.compatibility.contract.mobile_accepts, None);
        // The Tauri updater bundle and SHA256SUMS are not artifacts.
        assert_eq!(
            m.component(crate::Component::Desktop)
                .unwrap()
                .artifacts
                .len(),
            3
        );
        assert!(m
            .artifact_for(
                crate::Component::Hub,
                &Platform::new("linux", "x86_64", "oci")
            )
            .is_some());
        assert!(m
            .artifact_for(
                crate::Component::Hub,
                &Platform::new("linux", "x86_64", "tarball")
            )
            .is_some());
        assert!(m
            .artifact_for(
                crate::Component::Agent,
                &Platform::new("linux", "aarch64", "tarball")
            )
            .is_some());
        assert!(m
            .artifact_for(
                crate::Component::Desktop,
                &Platform::new("linux", "x86_64", "appimage")
            )
            .is_some());

        // Signed, it verifies and parses as the same manifest.
        let key = TestKey::new(3);
        let bytes = to_bytes(&m);
        let back = verify_manifest(
            &bytes,
            &key.sign(&bytes),
            &TrustedKeys::from_base64([key.public().as_str()]).unwrap(),
        )
        .unwrap();
        assert_eq!(back, m);
    }

    #[test]
    fn a_manifest_needs_a_hub_artifact_and_a_real_digest() {
        let bad = HubImage {
            image: "i".into(),
            digest: "sha256:short".into(),
        };
        assert!(manifest_for("0.4.1", Some(&bad))
            .unwrap_err()
            .contains("digest"));
        // Without an image the hub tarball still carries it.
        assert!(manifest_for("0.4.1", None).is_ok());
        let none: Vec<Asset> = vec![];
        let e = build_manifest(&ManifestInput {
            version: &v("0.4.1"),
            commit: "c",
            build_id: "b",
            published_at: 0,
            assets_base: "x/",
            notes_url: "",
            hub: &compat(),
            desktop_accepts: Window::new(6, 6),
            assets: &none,
            hub_image: None,
            tauri_sigs: &BTreeMap::new(),
        })
        .unwrap_err();
        assert!(e.contains("no hub artifact"), "{e}");
    }

    #[test]
    fn tracks_follow_the_version() {
        assert_eq!(track_of(&v("0.4.1")), Track::Stable);
        assert_eq!(track_of(&v("0.4.1-rc.1")), Track::Beta);
        assert_eq!(track_of(&v("0.4.2-dev.17.ga83f19d")), Track::Nightly);
    }

    fn rref(ver: &str) -> ReleaseRef {
        ReleaseRef {
            version: v(ver),
            manifest: format!("https://x/v{ver}/release-manifest.json"),
            manifest_sha256: sha256_hex(ver.as_bytes()),
        }
    }

    #[test]
    fn channel_add_moves_up_bumps_the_sequence_and_is_rerunnable() {
        let now = 1_790_763_120;
        let d = channel_add(None, Track::Stable, rref("0.4.1"), now, 14, 30).unwrap();
        assert_eq!(
            (d.sequence, d.current.to_string(), d.recommended.to_string()),
            (1, "0.4.1".into(), "0.4.1".into())
        );
        assert_eq!(d.expires_at, "2026-10-14T10:12:00Z");
        let d = channel_add(Some(d), Track::Stable, rref("0.4.2"), now, 14, 30).unwrap();
        assert_eq!((d.sequence, d.current.to_string()), (2, "0.4.2".into()));
        // A re-run of the same release replaces its entry, still bumps.
        let d = channel_add(Some(d), Track::Stable, rref("0.4.2"), now, 14, 30).unwrap();
        assert_eq!((d.sequence, d.releases.len()), (3, 2));
        // An older release listed late never moves current or recommended back.
        let d = channel_add(Some(d), Track::Stable, rref("0.4.0"), now, 14, 30).unwrap();
        assert_eq!(
            (d.current.to_string(), d.recommended.to_string()),
            ("0.4.2".into(), "0.4.2".into())
        );
        assert_eq!(d.releases[0].version, v("0.4.2"));
        assert!(channel_add(Some(d), Track::Beta, rref("0.4.3"), now, 14, 30).is_err());
    }

    #[test]
    fn a_rerun_keeps_a_held_back_or_withdrawn_release_unrecommended() {
        let now = 1_790_763_120;
        let mut d = channel_add(None, Track::Stable, rref("0.4.1"), now, 14, 30).unwrap();
        d = channel_add(Some(d), Track::Stable, rref("0.4.2"), now, 14, 30).unwrap();
        d = channel_edit(d, Edit::Recommend(v("0.4.1")), now, 14).unwrap();

        // Held back: a re-run of 0.4.2's publish keeps the hold.
        let held = channel_add(Some(d.clone()), Track::Stable, rref("0.4.2"), now, 14, 30).unwrap();
        assert_eq!(held.recommended, v("0.4.1"));

        // Withdrawn: a re-run never recommends it again.
        d = channel_edit(
            d,
            Edit::Withdraw {
                version: v("0.4.2"),
                reason: "bad migration".into(),
            },
            now,
            14,
        )
        .unwrap();
        let d = channel_add(Some(d), Track::Stable, rref("0.4.2"), now, 14, 30).unwrap();
        assert_eq!(d.recommended, v("0.4.1"));

        // A genuinely new release still moves recommended up.
        let d = channel_add(Some(d), Track::Stable, rref("0.4.3"), now, 14, 30).unwrap();
        assert_eq!(d.recommended, v("0.4.3"));
    }

    #[test]
    fn trimming_keeps_the_recommended_and_rollback_releases() {
        let now = 0;
        let mut d = channel_add(None, Track::Stable, rref("0.1.0"), now, 14, 30).unwrap();
        d = channel_edit(d, Edit::Rollback(Some(v("0.1.0"))), now, 14).unwrap();
        for p in 1..=5 {
            d = channel_add(
                Some(d),
                Track::Stable,
                rref(&format!("0.1.{p}")),
                now,
                14,
                2,
            )
            .unwrap();
        }
        let listed: Vec<String> = d.releases.iter().map(|r| r.version.to_string()).collect();
        assert_eq!(listed, ["0.1.5", "0.1.4", "0.1.0"]);
    }

    #[test]
    fn edits_are_checked_and_signed_documents_verify() {
        let now = 1_790_763_120;
        let mut d = channel_add(None, Track::Stable, rref("0.4.1"), now, 14, 30).unwrap();
        d = channel_add(Some(d), Track::Stable, rref("0.4.2"), now, 14, 30).unwrap();
        assert!(channel_edit(
            d.clone(),
            Edit::Withdraw {
                version: v("0.4.2"),
                reason: "x".into()
            },
            now,
            14
        )
        .unwrap_err()
        .contains("recommended"));
        d = channel_edit(d, Edit::Recommend(v("0.4.1")), now, 14).unwrap();
        d = channel_edit(
            d,
            Edit::Withdraw {
                version: v("0.4.2"),
                reason: "bad migration".into(),
            },
            now,
            14,
        )
        .unwrap();
        assert!(channel_edit(d.clone(), Edit::Recommend(v("0.4.2")), now, 14).is_err());
        assert!(channel_edit(d.clone(), Edit::Recommend(v("9.9.9")), now, 14).is_err());
        d = channel_edit(
            d,
            Edit::Minimum {
                component: "hub".into(),
                version: Some(v("0.4.1")),
            },
            now,
            14,
        )
        .unwrap();
        assert!(channel_edit(
            d.clone(),
            Edit::Minimum {
                component: "fridge".into(),
                version: None
            },
            now,
            14
        )
        .is_err());
        d = channel_edit(
            d,
            Edit::Mandatory(Mandatory {
                version: v("0.4.1"),
                components: vec!["hub".into()],
                reason: "security".into(),
                deadline: Some("2026-10-03T00:00:00Z".into()),
            }),
            now,
            14,
        )
        .unwrap();
        let seq = d.sequence;
        d = channel_edit(d, Edit::Resign, now + 86_400, 14).unwrap();
        assert_eq!(d.sequence, seq + 1);

        let key = TestKey::new(4);
        let bytes = to_bytes(&d);
        let ok = verify_channel(
            &bytes,
            &key.sign(&bytes),
            &TrustedKeys::from_base64([key.public().as_str()]).unwrap(),
            Track::Stable,
            seq,
            now + 86_400,
        )
        .unwrap();
        assert!(ok.fresh);
        assert_eq!(ok.doc, d);
        assert!(ok.doc.is_withdrawn(&v("0.4.2")));
    }
}
