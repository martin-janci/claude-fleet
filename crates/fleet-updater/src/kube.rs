//! The hub as a Kubernetes StatefulSet (`FLEET_UPDATER_BACKEND=kubernetes`):
//! the same update loop as the compose container (`engine`), with the
//! Kubernetes API in place of the Docker socket.
//!
//! ```text
//! downloading  nothing to pull here: the kubelet pulls when the pod starts
//! verifying    the signed digest is well formed
//! ready        `fleet-hub backup` in the hub container (pods/exec)
//! installing   strategic-merge patch of the StatefulSet: every selected
//!              container and init container → image@digest; then wait for
//!              the controller to replace <sts>-0 on the new revision
//! validating   the pod's imageID carries that digest, it is Running and
//!              Ready with no restart, and `fleet-hub healthcheck --ready
//!              --json` (pods/exec) names the manifest's build; the soak
//! rolling back scale to 0 (and wait for the pod to go), restore the
//!              backup when the candidate may have migrated, patch the
//!              images back to exactly what they were and the replicas
//!              back to 1 (a fresh pod, so the StatefulSet's "stuck
//!              rollout" — a bad pod it waits on forever — cannot happen)
//! rollback_failed  scale to 0 and leave everything for an operator
//! ```
//!
//! The API is spoken directly over HTTPS with the pod's service-account
//! token and CA (`ApiServer`, on `http::Client`), exec over a WebSocket
//! (`ws`). No kubectl, no `kube`/`k8s-openapi`: the updater reads a dozen
//! fields of two objects, and a typed client of the whole API would be most
//! of this crate's build.
//!
//! The database restore needs the hub's files: the updater mounts the hub's
//! volume claim too (`deploy/kubernetes/`), on the same node (a
//! ReadWriteOnce volume is per node, not per pod), at `FLEET_UPDATER_HUB_DATA`.

use std::path::{Path, PathBuf};
use std::time::Duration;

use async_trait::async_trait;
use fleet_updater::http;
use serde_json::{json, Map, Value};
use tokio::time::Instant;

use crate::docker::q;
use crate::engine::{log, HUB_DATA_DIR};
use crate::runtime::{ExecOutput, Run, Runtime, Seen, Snapshot};
use crate::state::Previous;

/// Where a pod finds its service account's token, CA and namespace.
pub const SA_DIR: &str = "/var/run/secrets/kubernetes.io/serviceaccount";

/// The patch type that merges containers by name.
pub const STRATEGIC: &str = "application/strategic-merge-patch+json";

/// The label a StatefulSet puts on each pod: the revision it was made from.
const REVISION_LABEL: &str = "controller-revision-hash";

const SMALL: u64 = 16 << 20;

/// The handful of API calls the runtime makes, behind a trait so the loop
/// is tested against a simulated cluster (`kube::tests`).
#[async_trait]
pub trait KubeApi: Send + Sync {
    /// A JSON request; `body` is (content type, object). `Ok(None)` for a 404.
    async fn call(
        &self,
        method: &str,
        path: &str,
        body: Option<(&str, &Value)>,
    ) -> Result<Option<Value>, String>;
    /// A GET answered with text (pod logs).
    async fn text(&self, path: &str) -> Result<String, String>;
    /// `GET <path>` upgraded to the exec WebSocket (`ws::exec`).
    async fn exec(&self, path: &str) -> Result<ExecOutput, String>;
}

enum Token {
    /// Re-read every call: a projected service-account token rotates.
    File(PathBuf),
    #[cfg_attr(not(test), allow(dead_code))]
    Fixed(String),
}

/// The API server, from inside a pod.
pub struct ApiServer {
    base: String,
    client: http::Client,
    token: Token,
    exec_timeout: Duration,
}

impl ApiServer {
    /// `KUBERNETES_SERVICE_HOST` / `_PORT`, the service account's token and
    /// CA bundle (what client-go's in-cluster config reads).
    pub fn in_cluster() -> Result<ApiServer, String> {
        let host = std::env::var("KUBERNETES_SERVICE_HOST")
            .ok()
            .filter(|h| !h.trim().is_empty())
            .ok_or("KUBERNETES_SERVICE_HOST is not set: not in a pod")?;
        let port = std::env::var("KUBERNETES_SERVICE_PORT")
            .ok()
            .filter(|p| !p.trim().is_empty())
            .unwrap_or_else(|| "443".into());
        let sa = PathBuf::from(SA_DIR);
        let ca = sa.join("ca.crt");
        let host = if host.contains(':') {
            format!("[{host}]")
        } else {
            host
        };
        Ok(ApiServer {
            base: format!("https://{host}:{port}"),
            client: http::Client::with_ca(true, Duration::from_secs(60), Some(&ca))?,
            token: Token::File(sa.join("token")),
            exec_timeout: Duration::from_secs(600),
        })
    }

    /// A server at `base` (`http://` in the wire tests) with a fixed token.
    #[cfg(test)]
    pub fn at(base: &str, token: &str) -> ApiServer {
        ApiServer {
            base: base.trim_end_matches('/').to_string(),
            client: http::Client::new(false, Duration::from_secs(10)).unwrap(),
            token: Token::Fixed(token.into()),
            exec_timeout: Duration::from_secs(10),
        }
    }

    fn bearer(&self) -> Result<String, String> {
        let t = match &self.token {
            Token::Fixed(t) => t.clone(),
            Token::File(p) => std::fs::read_to_string(p)
                .map_err(|e| format!("the service-account token {}: {e}", p.display()))?,
        };
        Ok(format!("Bearer {}", t.trim()))
    }
}

#[async_trait]
impl KubeApi for ApiServer {
    async fn call(
        &self,
        method: &str,
        path: &str,
        body: Option<(&str, &Value)>,
    ) -> Result<Option<Value>, String> {
        let auth = self.bearer()?;
        let bytes = body.map(|(_, v)| serde_json::to_vec(v).expect("a Value serialises"));
        let mut headers = vec![
            ("Authorization", auth.as_str()),
            ("Accept", "application/json"),
        ];
        if let Some((ct, _)) = body {
            headers.push(("Content-Type", ct));
        }
        let r = self
            .client
            .send(
                method,
                &format!("{}{path}", self.base),
                &headers,
                bytes.as_deref(),
                SMALL,
            )
            .await?;
        if r.status == 404 {
            return Ok(None);
        }
        if !r.ok() {
            return Err(format!(
                "{method} {path}: HTTP {}: {}",
                r.status,
                crate::ws::status_message(&r.body)
            ));
        }
        if r.body.is_empty() {
            return Ok(Some(Value::Null));
        }
        serde_json::from_slice(&r.body)
            .map(Some)
            .map_err(|e| format!("{method} {path}: {e}"))
    }

    async fn text(&self, path: &str) -> Result<String, String> {
        let auth = self.bearer()?;
        let r = self
            .client
            .send(
                "GET",
                &format!("{}{path}", self.base),
                &[("Authorization", auth.as_str())],
                None,
                SMALL,
            )
            .await?;
        if !r.ok() {
            return Err(format!(
                "GET {path}: HTTP {}: {}",
                r.status,
                crate::ws::status_message(&r.body)
            ));
        }
        Ok(r.text())
    }

    async fn exec(&self, path: &str) -> Result<ExecOutput, String> {
        let auth = self.bearer()?;
        let u = http::Url::parse(&format!("{}{path}", self.base))?;
        let host = self.client.host_for(&u);
        let fut = async {
            let stream = self.client.connect(&u).await?;
            crate::ws::exec(stream, &host, &u.path, &[("Authorization", &auth)], SMALL).await
        };
        tokio::time::timeout(self.exec_timeout, fut)
            .await
            .map_err(|_| format!("exec {path}: timed out"))?
    }
}

/// Which StatefulSet, and how long to wait for it.
#[derive(Debug, Clone)]
pub struct KubeConfig {
    pub namespace: String,
    pub statefulset: String,
    /// The hub container: exec, logs and the digest gate.
    pub container: String,
    /// The containers (and init containers) whose image is switched; `None`:
    /// every one whose image repository is the hub container's.
    pub containers: Option<Vec<String>>,
    pub poll: Duration,
    /// For the controller to replace the pod after a patch.
    pub rollout_timeout: Duration,
    /// For the pod to go after a scale to zero (then it is deleted).
    pub stop_timeout: Duration,
}

impl KubeConfig {
    pub fn new(namespace: &str, statefulset: &str) -> KubeConfig {
        KubeConfig {
            namespace: namespace.into(),
            statefulset: statefulset.into(),
            container: "fleet-hub".into(),
            containers: None,
            poll: Duration::from_secs(3),
            rollout_timeout: Duration::from_secs(300),
            stop_timeout: Duration::from_secs(120),
        }
    }

    /// Every name goes into an API path: DNS-1123, nothing else.
    pub fn validate(&self) -> Result<(), String> {
        let mut names = vec![&self.namespace, &self.statefulset, &self.container];
        names.extend(self.containers.iter().flatten());
        for n in names {
            if !dns_name(n) {
                return Err(format!("{n:?} is not a Kubernetes object name"));
            }
        }
        Ok(())
    }
}

fn dns_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 253
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'.')
        && s.as_bytes()[0].is_ascii_alphanumeric()
        && s.as_bytes()[s.len() - 1].is_ascii_alphanumeric()
}

pub struct KubeRuntime<K: KubeApi> {
    pub api: K,
    pub cfg: KubeConfig,
}

impl<K: KubeApi> KubeRuntime<K> {
    pub fn new(api: K, cfg: KubeConfig) -> Result<Self, String> {
        cfg.validate()?;
        Ok(KubeRuntime { api, cfg })
    }

    fn sts_path(&self) -> String {
        format!(
            "/apis/apps/v1/namespaces/{}/statefulsets/{}",
            self.cfg.namespace, self.cfg.statefulset
        )
    }

    /// One replica: always ordinal 0.
    fn pod_name(&self) -> String {
        format!("{}-0", self.cfg.statefulset)
    }

    fn pod_path(&self) -> String {
        format!(
            "/api/v1/namespaces/{}/pods/{}",
            self.cfg.namespace,
            self.pod_name()
        )
    }

    async fn sts(&self) -> Result<Value, String> {
        self.api
            .call("GET", &self.sts_path(), None)
            .await?
            .ok_or_else(|| format!("no StatefulSet {}", self.describe()))
    }

    async fn pod(&self) -> Result<Option<Value>, String> {
        self.api.call("GET", &self.pod_path(), None).await
    }

    async fn patch(&self, body: &Value) -> Result<Value, String> {
        self.api
            .call("PATCH", &self.sts_path(), Some((STRATEGIC, body)))
            .await?
            .ok_or_else(|| format!("no StatefulSet {}", self.describe()))
    }

    /// Patch the recorded containers to `image_for(<recorded image>)` and the
    /// replicas to the recorded count, then wait for the controller to put a
    /// pod of that revision in place.
    async fn apply(&self, spec: &Value, image_for: impl Fn(&str) -> String) -> Result<(), String> {
        let list = |key: &str| -> Vec<Value> {
            spec.get(key)
                .and_then(Value::as_object)
                .map(|m| {
                    m.iter()
                        .map(|(name, img)| {
                            json!({ "name": name, "image": image_for(img.as_str().unwrap_or_default()) })
                        })
                        .collect()
                })
                .unwrap_or_default()
        };
        let containers = list("containers");
        let inits = list("initContainers");
        if containers.is_empty() && inits.is_empty() {
            return Err("the recorded StatefulSet spec names no container".into());
        }
        let mut pod_spec = Map::new();
        if !containers.is_empty() {
            pod_spec.insert("containers".into(), Value::Array(containers));
        }
        if !inits.is_empty() {
            pod_spec.insert("initContainers".into(), Value::Array(inits));
        }
        let replicas = spec.get("replicas").and_then(Value::as_i64).unwrap_or(1);
        let body = json!({ "spec": { "replicas": replicas, "template": { "spec": pod_spec } } });
        let sts = self.patch(&body).await?;
        let generation = sts
            .pointer("/metadata/generation")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        self.wait_rollout(generation).await
    }

    /// Until the controller has seen `generation` and `<sts>-0` runs its
    /// update revision. Under `OnDelete` the controller never replaces a pod
    /// by itself, so the old one is deleted (once).
    async fn wait_rollout(&self, generation: i64) -> Result<(), String> {
        let deadline = Instant::now() + self.cfg.rollout_timeout;
        let mut deleted = false;
        let mut last = String::from("the controller has not seen the change yet");
        loop {
            let sts = self.sts().await?;
            let observed = sts
                .pointer("/status/observedGeneration")
                .and_then(Value::as_i64)
                .unwrap_or(0);
            let update = str_at(&sts, "/status/updateRevision").unwrap_or_default();
            if observed >= generation && !update.is_empty() {
                match self.pod().await? {
                    Some(p) => {
                        let rev = str_at(&p, &format!("/metadata/labels/{REVISION_LABEL}"))
                            .unwrap_or_default();
                        let going = p.pointer("/metadata/deletionTimestamp").is_some();
                        if rev == update && !going {
                            return Ok(());
                        }
                        last = format!("{} is on revision {rev:?}, not {update}", self.pod_name());
                        if rev != update && !going && strategy(&sts) == "OnDelete" && !deleted {
                            log!(
                                "the StatefulSet updates OnDelete: deleting {} so it comes back on {update}",
                                self.pod_name()
                            );
                            self.api.call("DELETE", &self.pod_path(), None).await?;
                            deleted = true;
                        }
                    }
                    None => last = format!("{} is not there yet", self.pod_name()),
                }
            }
            if Instant::now() >= deadline {
                return Err(format!(
                    "the StatefulSet did not roll {} within {} s: {last}",
                    self.pod_name(),
                    self.cfg.rollout_timeout.as_secs()
                ));
            }
            tokio::time::sleep(self.cfg.poll).await;
        }
    }

    async fn wait_gone(&self, within: Duration) -> Result<bool, String> {
        let deadline = Instant::now() + within;
        loop {
            if self.pod().await?.is_none() {
                return Ok(true);
            }
            if Instant::now() >= deadline {
                return Ok(false);
            }
            tokio::time::sleep(self.cfg.poll).await;
        }
    }
}

#[async_trait]
impl<K: KubeApi> Runtime for KubeRuntime<K> {
    fn describe(&self) -> String {
        format!(
            "{}/statefulset/{}",
            self.cfg.namespace, self.cfg.statefulset
        )
    }

    async fn inspect(&self) -> Result<Option<Seen>, String> {
        Ok(self.pod().await?.map(|p| seen_of(&p, &self.cfg.container)))
    }

    async fn exec(&self, cmd: &[String]) -> Result<ExecOutput, String> {
        let mut path = format!(
            "{}/exec?container={}&stdout=true&stderr=true",
            self.pod_path(),
            q(&self.cfg.container)
        );
        for c in cmd {
            path.push_str("&command=");
            path.push_str(&q(c));
        }
        self.api.exec(&path).await
    }

    async fn fetch(&self, image: &str, digest: &str) -> Result<(), String> {
        // Pulling is the kubelet's, with the pod's own pull secrets, once
        // the pod is created; a pull that fails ends the gates' wait.
        log!("{image}@{digest} is pulled by the kubelet when the new pod starts");
        Ok(())
    }

    async fn verify(&self, image: &str, digest: &str) -> Result<String, String> {
        let well_formed = digest.split_once(':').is_some_and(|(algo, hex)| {
            !algo.is_empty()
                && algo.bytes().all(|b| b.is_ascii_alphanumeric())
                && hex.len() >= 32
                && hex.bytes().all(|b| b.is_ascii_hexdigit())
        });
        if !well_formed {
            return Err(format!("{digest:?} is not an image digest"));
        }
        if image.is_empty() || image.contains(['@', ' ', '"']) {
            return Err(format!("{image:?} is not an image repository"));
        }
        // What the kubelet will report as the pod's imageID digest.
        Ok(digest.to_ascii_lowercase())
    }

    async fn snapshot(&self) -> Result<Snapshot, String> {
        let sts = self.sts().await?;
        let replicas = sts
            .pointer("/spec/replicas")
            .and_then(Value::as_i64)
            .unwrap_or(1);
        if replicas != 1 {
            return Err(format!(
                "{} has {replicas} replicas; the updater runs a one-replica hub",
                self.describe()
            ));
        }
        let strategy = strategy(&sts);
        let partition = sts
            .pointer("/spec/updateStrategy/rollingUpdate/partition")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        if strategy == "RollingUpdate" && partition > 0 {
            return Err(format!(
                "{}'s rolling update has partition {partition}, which keeps {} on the old revision",
                self.describe(),
                self.pod_name()
            ));
        }
        let generation = sts
            .pointer("/metadata/generation")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        let observed = sts
            .pointer("/status/observedGeneration")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        let current = str_at(&sts, "/status/currentRevision").unwrap_or_default();
        let update = str_at(&sts, "/status/updateRevision").unwrap_or_default();
        // Only a rolling update ever moves currentRevision up to the update
        // revision; under OnDelete the pod's own revision (below) says it.
        let rolling = strategy == "RollingUpdate" && !current.is_empty() && current != update;
        if observed < generation || rolling {
            return Err(format!(
                "{} is mid-rollout (generation {generation}, observed {observed}, revision {current} → {update}); not starting another",
                self.describe()
            ));
        }

        let containers = template_images(&sts, "containers");
        let inits = template_images(&sts, "initContainers");
        let hub_image = containers
            .iter()
            .find(|(n, _)| n == &self.cfg.container)
            .map(|(_, i)| i.clone())
            .ok_or_else(|| {
                format!(
                    "{} has no container named {}",
                    self.describe(),
                    self.cfg.container
                )
            })?;
        let (main, init) = select(
            &containers,
            &inits,
            &hub_image,
            &self.cfg.container,
            self.cfg.containers.as_deref(),
        )?;

        let pod = self
            .pod()
            .await?
            .ok_or_else(|| format!("{} is not there", self.pod_name()))?;
        let seen = seen_of(&pod, &self.cfg.container);
        if seen.run != Run::Running {
            return Err("the hub is not running, so it cannot take a backup".into());
        }
        let rev = str_at(&pod, &format!("/metadata/labels/{REVISION_LABEL}")).unwrap_or_default();
        if !update.is_empty() && rev != update {
            return Err(format!(
                "{} runs revision {rev:?}, not the StatefulSet's {update}",
                self.pod_name()
            ));
        }
        let image_id = seen.image_id.ok_or(
            "the hub container reports no image digest (status.containerStatuses[].imageID)",
        )?;
        let data_dir = sts
            .pointer("/spec/template/spec/containers")
            .and_then(Value::as_array)
            .and_then(|cs| cs.iter().find(|c| c["name"] == self.cfg.container.as_str()))
            .and_then(|c| c["env"].as_array())
            .and_then(|env| {
                env.iter()
                    .find(|e| e["name"] == "FLEET_HUB_DATA_DIR")
                    .and_then(|e| e["value"].as_str())
            })
            .unwrap_or(HUB_DATA_DIR)
            .to_string();
        let to_map = |v: Vec<(String, String)>| -> Value {
            Value::Object(v.into_iter().map(|(n, i)| (n, Value::String(i))).collect())
        };
        let mut spec = json!({
            "kind": "StatefulSet",
            "namespace": self.cfg.namespace,
            "name": self.cfg.statefulset,
            "replicas": replicas,
            "strategy": strategy,
            "revision": update,
            "containers": to_map(main),
        });
        if !init.is_empty() {
            spec["initContainers"] = to_map(init);
        }
        Ok(Snapshot {
            image_id,
            spec,
            data_dir: PathBuf::from(data_dir),
        })
    }

    async fn install(&self, spec: &Value, image: &str) -> Result<(), String> {
        self.apply(spec, |_| image.to_string()).await
    }

    async fn reinstate(&self, prev: &Previous) -> Result<(), String> {
        // Exactly the images it ran with (a tag stays a tag), so the
        // template matches its old revision again.
        self.apply(&prev.spec, |old| old.to_string()).await
    }

    async fn stop(&self) -> Result<(), String> {
        self.patch(&json!({ "spec": { "replicas": 0 } })).await?;
        if self.wait_gone(self.cfg.stop_timeout).await? {
            return Ok(());
        }
        // Scaled to zero and still there: the controller is waiting on
        // something; take the pod away directly.
        log!(
            "{} did not go within {} s of the scale to 0; deleting it",
            self.pod_name(),
            self.cfg.stop_timeout.as_secs()
        );
        self.api.call("DELETE", &self.pod_path(), None).await?;
        if self.wait_gone(self.cfg.stop_timeout).await? {
            Ok(())
        } else {
            Err(format!("{} is still there after a delete", self.pod_name()))
        }
    }

    async fn logs(&self, tail: u32) -> Result<String, String> {
        let restarts = self.inspect().await?.map_or(0, |s| s.restarts);
        let base = format!(
            "{}/log?container={}&tailLines={tail}&timestamps=true",
            self.pod_path(),
            q(&self.cfg.container)
        );
        let mut out = String::new();
        if restarts > 0 {
            if let Ok(t) = self.api.text(&format!("{base}&previous=true")).await {
                out.push_str("--- the previous container ---\n");
                out.push_str(&t);
                out.push_str("--- the current container ---\n");
            }
        }
        match self.api.text(&base).await {
            Ok(t) => out.push_str(&t),
            Err(e) if out.is_empty() => return Err(e),
            Err(e) => out.push_str(&format!("({e})\n")),
        }
        Ok(out)
    }
}

fn str_at<'a>(v: &'a Value, pointer: &str) -> Option<&'a str> {
    v.pointer(pointer)?.as_str()
}

fn strategy(sts: &Value) -> &str {
    str_at(sts, "/spec/updateStrategy/type").unwrap_or("RollingUpdate")
}

/// `(container name, image)` pairs.
pub type Images = Vec<(String, String)>;

/// `(name, image)` of the pod template's `containers` / `initContainers`.
fn template_images(sts: &Value, key: &str) -> Images {
    sts.pointer(&format!("/spec/template/spec/{key}"))
        .and_then(Value::as_array)
        .map(|cs| {
            cs.iter()
                .filter_map(|c| {
                    Some((
                        c["name"].as_str()?.to_string(),
                        c["image"].as_str().unwrap_or_default().to_string(),
                    ))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The containers to switch: the named ones, or every container and init
/// container on the hub container's repository. The hub container is always
/// one of them.
pub fn select(
    containers: &[(String, String)],
    inits: &[(String, String)],
    hub_image: &str,
    hub: &str,
    named: Option<&[String]>,
) -> Result<(Images, Images), String> {
    let pick = |list: &[(String, String)]| -> Vec<(String, String)> {
        list.iter()
            .filter(|(n, i)| match named {
                Some(ns) => ns.iter().any(|x| x == n),
                None => repo_of(i) == repo_of(hub_image),
            })
            .cloned()
            .collect()
    };
    let (main, init) = (pick(containers), pick(inits));
    if let Some(ns) = named {
        for n in ns {
            if !main.iter().chain(&init).any(|(m, _)| m == n) {
                return Err(format!(
                    "FLEET_UPDATER_K8S_CONTAINERS names {n}, which the StatefulSet does not have"
                ));
            }
        }
    }
    if !main.iter().any(|(n, _)| n == hub) {
        return Err(format!(
            "the containers to switch do not include the hub container {hub}"
        ));
    }
    Ok((main, init))
}

/// `ghcr.io/x/fleet-hub` of `ghcr.io/x/fleet-hub:0.6.3` or `…@sha256:…`
/// (a registry's `:port` is not a tag).
pub fn repo_of(image: &str) -> &str {
    let no_digest = image.split_once('@').map_or(image, |(r, _)| r);
    match no_digest.rfind(':') {
        Some(i) if !no_digest[i..].contains('/') => &no_digest[..i],
        _ => no_digest,
    }
}

/// The digest in an `imageID`: `repo@sha256:…` (containerd, CRI-O),
/// `docker-pullable://repo@sha256:…` (dockershim); a bare `sha256:…` is
/// taken as it is.
pub fn digest_of(image_id: &str) -> Option<String> {
    let id = image_id.trim();
    if id.is_empty() {
        return None;
    }
    Some(
        id.rsplit_once('@')
            .map_or(id, |(_, d)| d)
            .to_ascii_lowercase(),
    )
}

/// Waiting reasons that do not fix themselves. Anything else (creating,
/// pulling, initialising) is "not yet", bounded by the ready timeout.
const STUCK: [&str; 8] = [
    "CrashLoopBackOff",
    "CreateContainerConfigError",
    "CreateContainerError",
    "InvalidImageName",
    "RunContainerError",
    "ErrImageNeverPull",
    "PreStartHookError",
    "PostStartHookError",
];

/// What a pod says about the hub container.
pub fn seen_of(pod: &Value, hub: &str) -> Seen {
    let statuses = pod
        .pointer("/status/containerStatuses")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let inits = pod
        .pointer("/status/initContainerStatuses")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let restarts = statuses
        .iter()
        .chain(&inits)
        .map(|s| s["restartCount"].as_i64().unwrap_or(0))
        .sum();
    let hub_status = statuses.iter().find(|s| s["name"] == hub);
    let digest = hub_status
        .and_then(|s| s["imageID"].as_str())
        .and_then(digest_of);
    let health = hub_status.map(|s| {
        if s["ready"].as_bool() == Some(true) {
            "healthy".to_string()
        } else {
            "starting".to_string()
        }
    });
    let waiting = |s: &Value| -> Option<(String, String)> {
        let w = s.pointer("/state/waiting")?;
        Some((
            w["reason"].as_str().unwrap_or("Waiting").to_string(),
            w["message"].as_str().unwrap_or_default().to_string(),
        ))
    };
    let run = (|| {
        if pod.pointer("/metadata/deletionTimestamp").is_some() {
            return Run::Down("the pod is being deleted".into());
        }
        for s in &inits {
            let name = s["name"].as_str().unwrap_or("?");
            if let Some((reason, msg)) = waiting(s) {
                if STUCK.contains(&reason.as_str()) {
                    return Run::Down(
                        format!("init container {name}: {reason} {msg}")
                            .trim()
                            .into(),
                    );
                }
            }
            if let Some(code) = s
                .pointer("/state/terminated/exitCode")
                .and_then(Value::as_i64)
            {
                if code != 0 {
                    return Run::Down(format!("init container {name} exited (code {code})"));
                }
            }
        }
        let Some(s) = hub_status else {
            let phase = str_at(pod, "/status/phase").unwrap_or("Pending");
            return Run::Starting(format!("the pod is {phase}"));
        };
        if s.pointer("/state/running").is_some() {
            // Every container started from the hub's image must run the
            // same digest as the hub container.
            let hub_image = pod
                .pointer("/spec/containers")
                .and_then(Value::as_array)
                .and_then(|cs| cs.iter().find(|c| c["name"] == hub))
                .and_then(|c| c["image"].as_str());
            let same_image: Vec<&str> = ["/spec/containers", "/spec/initContainers"]
                .iter()
                .filter_map(|p| pod.pointer(p).and_then(Value::as_array))
                .flatten()
                .filter(|c| hub_image.is_some() && c["image"].as_str() == hub_image)
                .filter_map(|c| c["name"].as_str())
                .collect();
            for other in statuses.iter().chain(&inits) {
                let name = other["name"].as_str().unwrap_or_default();
                if name == hub || !same_image.contains(&name) {
                    continue;
                }
                let theirs = other["imageID"].as_str().and_then(digest_of);
                if theirs.is_some() && theirs != digest {
                    return Run::Down(format!(
                        "container {name} runs {}, the hub container {}",
                        theirs.unwrap_or_default(),
                        digest.clone().unwrap_or_default()
                    ));
                }
            }
            return Run::Running;
        }
        if let Some((reason, msg)) = waiting(s) {
            let why = format!("{reason} {msg}").trim().to_string();
            return if STUCK.contains(&reason.as_str()) {
                Run::Down(why)
            } else {
                Run::Starting(why)
            };
        }
        if let Some(t) = s.pointer("/state/terminated") {
            return Run::Down(format!(
                "it exited (code {}): {}",
                t["exitCode"].as_i64().unwrap_or(-1),
                t["reason"].as_str().unwrap_or("Terminated")
            ));
        }
        Run::Starting("the container has no state yet".into())
    })();
    Seen {
        image_id: digest.clone(),
        digest,
        run,
        restarts,
        health,
    }
}

/// The namespace this pod runs in (the service account's mount).
pub fn own_namespace() -> Option<String> {
    std::fs::read_to_string(Path::new(SA_DIR).join("namespace"))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests;
