//! The per-release manifest (design §4): immutable, one per version,
//! published as the release asset `release-manifest.json` + `.minisig`.
//!
//! Forward compatibility is the rule for every type here: unknown fields are
//! ignored, an unknown artifact `kind` reads as [`Artifact::Unknown`], and
//! components are keyed by string so a future `worker` does not break an
//! older reader.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::model::{Component, Platform, Track, Window};
use crate::Version;

/// The only manifest schema this crate reads. A higher one is refused by
/// [`crate::verify::verify_manifest`] rather than half-understood.
pub const MANIFEST_SCHEMA: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReleaseManifest {
    pub schema: u32,
    pub release: ReleaseInfo,
    pub compatibility: Compatibility,
    #[serde(default)]
    pub components: BTreeMap<String, ComponentRelease>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReleaseInfo {
    pub version: Version,
    pub track: Track,
    pub commit: String,
    #[serde(default)]
    pub build_id: String,
    pub published_at: String,
    /// Where `name`d artifacts resolve, e.g.
    /// `https://github.com/martin-janci/claude-fleet/releases/download/v0.3.4/`.
    pub assets_base: String,
    #[serde(default)]
    pub notes_url: String,
}

/// The build's own protocol windows (U3), emitted by the binaries
/// (`fleet-hub compat --json`), never typed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Compatibility {
    pub contract: ContractCompat,
    pub agent_proto: AgentProtoCompat,
    #[serde(default)]
    pub peer_proto: Option<PeerProtoCompat>,
    #[serde(default)]
    pub store: Option<StoreCompat>,
    pub update_proto: u32,
}

/// Hub↔client wire contract (`CONTRACT_REVISION` and the clients'
/// `MIN_HUB_CONTRACT` / `MAX_HUB_CONTRACT`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ContractCompat {
    pub hub_serves: u32,
    pub desktop_accepts: Window,
    /// The phone's window. Absent until fleet-mobile's release amends the
    /// manifest (design §4); a release without it is never offered to a
    /// phone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mobile_accepts: Option<Window>,
}

/// Hub↔agent frame protocol (`PROTO_VERSION`, `MIN_SUPPORTED_PROTO`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct AgentProtoCompat {
    pub hub_accepts: Window,
    pub agent_speaks: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PeerProtoCompat {
    pub speaks: u32,
    pub accepts: Window,
}

/// The store's schema: the migration this build brings the database to, and
/// the oldest it will open. Used for the rollback analysis (§8.3).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct StoreCompat {
    pub schema_to: i64,
    pub opens_down_to: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ComponentRelease {
    pub version: Version,
    #[serde(default)]
    pub artifacts: Vec<Artifact>,
}

/// One installable thing. `kind` decides which platform layer installs it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Artifact {
    /// A container image, identified by digest only (F5). `platforms` maps
    /// `linux/amd64` to the per-arch manifest digest under the index `digest`.
    Oci {
        image: String,
        digest: String,
        #[serde(default)]
        platforms: BTreeMap<String, String>,
    },
    /// A `fleet-hub` / `fleet-agent` release tarball.
    Tarball {
        target: String,
        name: String,
        sha256: String,
        size: u64,
    },
    /// A bundle `tauri-plugin-updater` installs (NSIS, AppImage,
    /// `.app.tar.gz`); `tauri_signature` is its minisign `.sig`.
    Tauri {
        platform: String,
        #[serde(default)]
        variant: Option<String>,
        name: String,
        sha256: String,
        size: u64,
        tauri_signature: String,
    },
    /// Something only a person can install (the `.deb`): the decision offers
    /// the download, never an install.
    Download {
        platform: String,
        #[serde(default)]
        variant: Option<String>,
        name: String,
        sha256: String,
        size: u64,
    },
    /// An Android APK, from fleet-mobile's release.
    Apk {
        url: String,
        sha256: String,
        size: u64,
        version_code: u64,
        signer_sha256: String,
    },
    /// No installable artifact (iOS until there is a store listing): the
    /// decision is shown, `url` opened if present.
    Notify {
        #[serde(default)]
        url: Option<String>,
    },
    /// A kind this build does not know. Never matches a platform.
    #[serde(other)]
    Unknown,
}

impl Artifact {
    /// Whether this artifact installs on `p`.
    pub fn matches(&self, p: &Platform) -> bool {
        let variant_ok = |v: &Option<String>| v.as_deref().is_none_or(|v| v == p.variant);
        match self {
            Artifact::Oci { platforms, .. } => {
                p.variant == "oci"
                    && (platforms.is_empty() || platforms.contains_key(&p.docker_platform()))
            }
            Artifact::Tarball { target, .. } => {
                p.os == "linux"
                    && p.variant == "tarball"
                    && target.starts_with(&format!("{}-", p.arch))
            }
            Artifact::Tauri {
                platform, variant, ..
            }
            | Artifact::Download {
                platform, variant, ..
            } => *platform == p.os_arch() && variant_ok(variant),
            Artifact::Apk { .. } => p.os == "android",
            Artifact::Notify { .. } => p.os == "ios",
            Artifact::Unknown => false,
        }
    }

    /// The URL to fetch, resolving a `name` against the release's
    /// `assets_base`. `None` for an image (pulled by digest) or `notify`
    /// without a link.
    pub fn url(&self, assets_base: &str) -> Option<String> {
        let join = |name: &str| {
            if assets_base.ends_with('/') {
                format!("{assets_base}{name}")
            } else {
                format!("{assets_base}/{name}")
            }
        };
        match self {
            Artifact::Tarball { name, .. }
            | Artifact::Tauri { name, .. }
            | Artifact::Download { name, .. } => Some(join(name)),
            Artifact::Apk { url, .. } => Some(url.clone()),
            Artifact::Notify { url } => url.clone(),
            Artifact::Oci { .. } | Artifact::Unknown => None,
        }
    }
}

impl ReleaseManifest {
    pub fn component(&self, c: Component) -> Option<&ComponentRelease> {
        self.components.get(c.as_str())
    }

    /// The first artifact of `c` that installs on `p`, in manifest order.
    pub fn artifact_for(&self, c: Component, p: &Platform) -> Option<&Artifact> {
        self.component(c)?.artifacts.iter().find(|a| a.matches(p))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_kind_and_fields_are_tolerated() {
        let a: Vec<Artifact> =
            serde_json::from_str(r#"[{"kind":"flatpak","ref":"x"},{"kind":"notify","extra":1}]"#)
                .unwrap();
        assert_eq!(a, vec![Artifact::Unknown, Artifact::Notify { url: None }]);
        assert!(!a[0].matches(&Platform::new("linux", "x86_64", "flatpak")));
    }

    #[test]
    fn platform_matching() {
        let oci = Artifact::Oci {
            image: "i".into(),
            digest: "sha256:x".into(),
            platforms: [("linux/amd64".to_string(), "sha256:a".to_string())].into(),
        };
        assert!(oci.matches(&Platform::new("linux", "x86_64", "oci")));
        assert!(!oci.matches(&Platform::new("linux", "aarch64", "oci")));
        assert!(!oci.matches(&Platform::new("linux", "x86_64", "tarball")));

        let tar = Artifact::Tarball {
            target: "aarch64-unknown-linux-gnu".into(),
            name: "n".into(),
            sha256: "s".into(),
            size: 1,
        };
        assert!(tar.matches(&Platform::new("linux", "aarch64", "tarball")));
        assert!(!tar.matches(&Platform::new("linux", "x86_64", "tarball")));

        let appimage = Artifact::Tauri {
            platform: "linux-x86_64".into(),
            variant: Some("appimage".into()),
            name: "n".into(),
            sha256: "s".into(),
            size: 1,
            tauri_signature: "sig".into(),
        };
        assert!(appimage.matches(&Platform::new("linux", "x86_64", "appimage")));
        assert!(!appimage.matches(&Platform::new("linux", "x86_64", "deb")));
        let mac = Artifact::Tauri {
            platform: "macos-aarch64".into(),
            variant: None,
            name: "n".into(),
            sha256: "s".into(),
            size: 1,
            tauri_signature: "sig".into(),
        };
        assert!(mac.matches(&Platform::new("macos", "aarch64", "tauri")));
        assert_eq!(
            mac.url("https://x/releases/download/v1"),
            Some("https://x/releases/download/v1/n".into())
        );
    }
}
