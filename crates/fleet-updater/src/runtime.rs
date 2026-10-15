//! What the update loop (`engine`) does to the place the hub runs, so one
//! state machine drives every backend: a compose container through the
//! Docker Engine API (`docker::ContainerRuntime`) and a StatefulSet through
//! the Kubernetes API (`kube::KubeRuntime`).
//!
//! The loop owns the phases, the reports, the bad list, the backup and the
//! restore; a runtime only answers "what runs", runs a command in the hub,
//! and swaps what runs.

use std::path::PathBuf;

use async_trait::async_trait;
use serde_json::Value;

use crate::state::Previous;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecOutput {
    pub exit_code: i64,
    pub stdout: String,
    pub stderr: String,
}

/// What a runtime sees of the hub right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seen {
    /// What the gates compare with `Build::image_id`: Docker's local image
    /// id; on Kubernetes the digest the kubelet reports in `imageID`.
    pub image_id: Option<String>,
    /// The registry digest it runs, for the reports.
    pub digest: Option<String>,
    pub run: Run,
    /// Restarts since it was created (a pod: its containers' and init
    /// containers' together).
    pub restarts: i64,
    /// `healthy`, `starting`, `unhealthy`; `None` when nothing checks it
    /// (an image without a `HEALTHCHECK`).
    pub health: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Run {
    Running,
    /// Still coming up (being created, the image pulling, init containers
    /// running): not yet, bounded by the ready timeout.
    Starting(String),
    /// Exited, or stuck on something that will not fix itself.
    Down(String),
}

/// How to run what runs now again, taken just before an install.
#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    /// What [`Seen::image_id`] says of it.
    pub image_id: String,
    /// What brings it back (`Previous::spec`): a `containers/create` body, or
    /// a StatefulSet's images and replicas.
    pub spec: Value,
    /// The hub's data directory as the hub sees it (where `fleet-hub backup`
    /// writes).
    pub data_dir: PathBuf,
}

#[async_trait]
pub trait Runtime: Send + Sync {
    /// For the log: `hub-fleet-hub-1`, `papayapos-fleet/statefulset/fleet-hub`.
    fn describe(&self) -> String;

    /// The hub now; `None` when there is none (removed, scaled to zero).
    async fn inspect(&self) -> Result<Option<Seen>, String>;

    /// Run `cmd` in the running hub and wait for it.
    async fn exec(&self, cmd: &[String]) -> Result<ExecOutput, String>;

    /// `downloading`: get `image@digest` to where the hub runs.
    async fn fetch(&self, image: &str, digest: &str) -> Result<(), String>;

    /// `verifying`: what [`Seen::image_id`] will say once the hub runs
    /// `image@digest`; an error when the fetched image does not carry the
    /// digest.
    async fn verify(&self, image: &str, digest: &str) -> Result<String, String>;

    /// `ready`: how to run what runs now again. An error when it is not
    /// running, or not in a state an install may start from.
    async fn snapshot(&self) -> Result<Snapshot, String>;

    /// `installing`: run `image` (`repo@digest`) the way `spec` ran the
    /// previous build, in its place. Returns once the new build was started.
    async fn install(&self, spec: &Value, image: &str) -> Result<(), String>;

    /// Rolling back: run the previous build again, exactly as it ran.
    async fn reinstate(&self, prev: &Previous) -> Result<(), String>;

    /// Stop the hub, so nothing writes the database. `Ok` only once it is
    /// stopped.
    async fn stop(&self) -> Result<(), String>;

    /// The hub's last `tail` log lines.
    async fn logs(&self, tail: u32) -> Result<String, String>;
}
