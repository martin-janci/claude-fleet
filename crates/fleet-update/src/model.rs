//! The small vocabulary every other module speaks.

use serde::{Deserialize, Serialize};

/// Which part of the fleet a check or an artifact is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Component {
    Hub,
    Agent,
    Desktop,
    Android,
    Ios,
}

impl Component {
    /// The key under `components` in a release manifest and under
    /// `minimum_supported` in a channel document.
    pub fn as_str(self) -> &'static str {
        match self {
            Component::Hub => "hub",
            Component::Agent => "agent",
            Component::Desktop => "desktop",
            Component::Android => "android",
            Component::Ios => "ios",
        }
    }

    /// Whether the platform can install an older build over a newer one.
    /// Docker (the hub) and the agent's versioned install directory can; a
    /// desktop bundle, an APK and an App Store app never self-downgrade.
    pub fn can_downgrade(self) -> bool {
        matches!(self, Component::Hub | Component::Agent)
    }

    pub const ALL: [Component; 5] = [
        Component::Hub,
        Component::Agent,
        Component::Desktop,
        Component::Android,
        Component::Ios,
    ];
}

impl std::str::FromStr for Component {
    type Err = String;

    /// The inverse of [`Component::as_str`] (and of the serde name).
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Component::ALL
            .into_iter()
            .find(|c| c.as_str() == s)
            .ok_or_else(|| format!("unknown component {s:?}"))
    }
}

/// Where a decision came from (F7: the source, not the track).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Git,
    Hub,
}

/// The release track (U9): `stable` = `vX.Y.Z` tags, `beta` = `-rc.N` tags,
/// `dev` = every green `main` push (`-dev.N…`, the desktop too), `nightly` =
/// those of them at most one every two hours. A client older than `dev`
/// cannot read a decision on it: put only current builds on `dev`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Track {
    Stable,
    Beta,
    Nightly,
    Dev,
}

impl Track {
    pub fn as_str(self) -> &'static str {
        match self {
            Track::Stable => "stable",
            Track::Beta => "beta",
            Track::Nightly => "nightly",
            Track::Dev => "dev",
        }
    }
}

/// The policy mode for one component (design §7.3). "Mandatory" is not a
/// mode: it is the floor (`Policy::minimum`) or the publisher's `mandatory`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// Never offered on its own; the operator pins versions.
    Manual,
    /// Offered, installed when a person says so.
    #[default]
    Notify,
    /// Installed at the next quiet point without asking.
    Automatic,
}

/// What a caller runs on. Matching against artifacts is in
/// [`crate::manifest::Artifact::matches`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Platform {
    /// `linux`, `macos`, `windows`, `android`, `ios`.
    pub os: String,
    /// `x86_64`, `aarch64`.
    pub arch: String,
    /// The install flavour where one OS/arch has several: `oci`, `tarball`,
    /// `appimage`, `deb`, `nsis`, `tauri`, `apk`. Empty when there is only one.
    #[serde(default)]
    pub variant: String,
}

impl Platform {
    pub fn new(os: &str, arch: &str, variant: &str) -> Self {
        Platform {
            os: os.into(),
            arch: arch.into(),
            variant: variant.into(),
        }
    }

    /// `<os>-<arch>`, the key desktop artifacts carry.
    pub fn os_arch(&self) -> String {
        format!("{}-{}", self.os, self.arch)
    }

    /// Docker's spelling of the architecture (`linux/amd64`).
    pub fn docker_platform(&self) -> String {
        let arch = match self.arch.as_str() {
            "x86_64" => "amd64",
            "aarch64" => "arm64",
            other => other,
        };
        format!("{}/{}", self.os, arch)
    }
}

/// An inclusive protocol window, `[min, max]` on the wire — the shape the
/// code's own `MIN_*` / `MAX_*` constants already have.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "[u32; 2]", into = "[u32; 2]")]
pub struct Window {
    pub min: u32,
    pub max: u32,
}

impl Window {
    pub const fn new(min: u32, max: u32) -> Self {
        Window { min, max }
    }

    pub fn contains(&self, v: u32) -> bool {
        self.min <= v && v <= self.max
    }
}

impl From<[u32; 2]> for Window {
    fn from([min, max]: [u32; 2]) -> Self {
        Window { min, max }
    }
}

impl From<Window> for [u32; 2] {
    fn from(w: Window) -> Self {
        [w.min, w.max]
    }
}

#[cfg(test)]
mod tests {
    use super::Component;

    #[test]
    fn component_parses_what_it_prints_and_what_serde_prints() {
        for c in Component::ALL {
            assert_eq!(c.as_str().parse::<Component>(), Ok(c));
            assert_eq!(
                serde_json::to_value(c).unwrap(),
                serde_json::Value::String(c.as_str().into())
            );
        }
        assert!("fridge".parse::<Component>().is_err());
    }
}
