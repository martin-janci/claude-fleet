//! Re-creating the hub container on another image (design §8.2 step 5).
//!
//! `docker inspect` of a container reports its *effective* config: what the
//! operator (or compose) set, merged with the image's own defaults. Passing
//! that back verbatim would pin the old image's `ENV`, `CMD`, labels and
//! `HEALTHCHECK` onto the new one. So everything equal to the old image's own
//! default is dropped, and the new image supplies its own; what remains is
//! the container's: compose's labels, the env file, volumes, networks and
//! their aliases, the restart policy, the logging driver.

use serde_json::{Map, Value};

/// The `containers/create` body that runs `new_image` the way `container`
/// ran `old_image`.
pub fn create_body(container: &Value, old_image: &Value, new_image: &str) -> Value {
    let empty = Value::Object(Map::new());
    let img_cfg = old_image.get("Config").unwrap_or(&empty);
    let mut cfg = container
        .get("Config")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();

    // Docker sets these per container; the new one gets its own.
    cfg.remove("Hostname");
    cfg.remove("Image");

    // Env: drop the image's own entries (exact `K=V` matches).
    if let Some(env) = cfg.get("Env").and_then(Value::as_array) {
        let image_env: Vec<&Value> = img_cfg
            .get("Env")
            .and_then(Value::as_array)
            .map(|a| a.iter().collect())
            .unwrap_or_default();
        let kept: Vec<Value> = env
            .iter()
            .filter(|e| !image_env.contains(e))
            .cloned()
            .collect();
        cfg.insert("Env".into(), Value::Array(kept));
    }

    // Labels: drop the image's own (OCI version / revision labels above all).
    if let Some(labels) = cfg.get("Labels").and_then(Value::as_object) {
        let image_labels = img_cfg.get("Labels").and_then(Value::as_object);
        let kept: Map<String, Value> = labels
            .iter()
            .filter(|(k, v)| image_labels.and_then(|l| l.get(*k)) != Some(*v))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        cfg.insert("Labels".into(), Value::Object(kept));
    }

    // Whole fields that equal the image's default are the image's to set.
    for key in [
        "Cmd",
        "Entrypoint",
        "Healthcheck",
        "User",
        "WorkingDir",
        "StopSignal",
        "ExposedPorts",
        "Volumes",
        "Shell",
        "OnBuild",
    ] {
        if cfg.get(key).is_some() && cfg.get(key) == img_cfg.get(key) {
            cfg.remove(key);
        }
    }

    cfg.insert("Image".into(), Value::String(new_image.to_string()));
    if let Some(hc) = container.get("HostConfig") {
        cfg.insert("HostConfig".into(), hc.clone());
    }
    cfg.insert(
        "NetworkingConfig".into(),
        serde_json::json!({ "EndpointsConfig": endpoints(container) }),
    );
    Value::Object(cfg)
}

/// The container's networks and aliases, without what Docker derives per
/// container (its short id as an alias, addresses, the endpoint ids).
fn endpoints(container: &Value) -> Value {
    let id = container.get("Id").and_then(Value::as_str).unwrap_or("");
    let short = &id[..id.len().min(12)];
    let mut out = Map::new();
    let Some(nets) = container
        .pointer("/NetworkSettings/Networks")
        .and_then(Value::as_object)
    else {
        return Value::Object(out);
    };
    for (name, ep) in nets {
        let mut e = Map::new();
        if let Some(aliases) = ep.get("Aliases").and_then(Value::as_array) {
            let kept: Vec<Value> = aliases
                .iter()
                .filter(|a| a.as_str().is_some_and(|a| !short.is_empty() && a != short))
                .cloned()
                .collect();
            e.insert("Aliases".into(), Value::Array(kept));
        }
        // Addresses, MAC and endpoint ids are Docker's per container; only
        // what an operator configures is carried over.
        for key in ["IPAMConfig", "Links", "DriverOpts"] {
            if let Some(v) = ep.get(key).filter(|v| !v.is_null()) {
                e.insert(key.into(), v.clone());
            }
        }
        out.insert(name.clone(), Value::Object(e));
    }
    Value::Object(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn old_image() -> Value {
        json!({"Config": {
            "Env": ["PATH=/usr/bin", "FLEET_HUB_DATA_DIR=/var/lib/fleet-hub"],
            "Cmd": ["serve"], "Entrypoint": ["tini", "--", "fleet-hub"], "User": "fleet",
            "Labels": {"org.opencontainers.image.version": "0.5.3"},
            "Healthcheck": {"Test": ["CMD", "fleet-hub", "healthcheck"]},
            "ExposedPorts": {"4180/tcp": {}}
        }})
    }

    fn container() -> Value {
        json!({
            "Id": "0123456789abcdef0123",
            "Config": {
                "Hostname": "0123456789ab", "Image": "ghcr.io/x/fleet-hub:0.5.3",
                "Env": ["PATH=/usr/bin", "FLEET_HUB_DATA_DIR=/var/lib/fleet-hub", "FLEET_HUB_BIND=0.0.0.0", "FLEET_HUB_PUBLIC_URL=https://hub"],
                "Cmd": ["serve"], "Entrypoint": ["tini", "--", "fleet-hub"], "User": "fleet",
                "Labels": {"org.opencontainers.image.version": "0.5.3",
                           "com.docker.compose.project": "hub", "com.docker.compose.service": "fleet-hub"},
                "Healthcheck": {"Test": ["CMD", "fleet-hub", "healthcheck"]},
                "ExposedPorts": {"4180/tcp": {}}
            },
            "HostConfig": {"Binds": ["/srv/ssh:/home/fleet/.ssh:rw"], "RestartPolicy": {"Name": "unless-stopped"},
                           "Mounts": [{"Type": "volume", "Source": "hub_hub-data", "Target": "/var/lib/fleet-hub"}]},
            "NetworkSettings": {"Networks": {"hub_default": {
                "Aliases": ["hub-fleet-hub-1", "fleet-hub", "0123456789ab"], "IPAddress": "172.18.0.2",
                "EndpointID": "e", "MacAddress": "02:42:ac:12:00:02", "IPAMConfig": null}}}
        })
    }

    #[test]
    fn keeps_the_containers_own_config_and_drops_the_images() {
        let b = create_body(&container(), &old_image(), "ghcr.io/x/fleet-hub@sha256:new");
        assert_eq!(b["Image"], "ghcr.io/x/fleet-hub@sha256:new");
        assert!(b.get("Hostname").is_none());
        assert_eq!(
            b["Env"],
            json!(["FLEET_HUB_BIND=0.0.0.0", "FLEET_HUB_PUBLIC_URL=https://hub"])
        );
        assert_eq!(
            b["Labels"],
            json!({"com.docker.compose.project": "hub", "com.docker.compose.service": "fleet-hub"})
        );
        for k in ["Cmd", "Entrypoint", "User", "Healthcheck", "ExposedPorts"] {
            assert!(
                b.get(k).is_none(),
                "{k} is the image's default and must be dropped"
            );
        }
        assert_eq!(b["HostConfig"], container()["HostConfig"]);
        assert_eq!(
            b["NetworkingConfig"],
            json!({"EndpointsConfig": {"hub_default": {"Aliases": ["hub-fleet-hub-1", "fleet-hub"]}}})
        );
    }

    #[test]
    fn an_operators_override_survives() {
        let mut c = container();
        c["Config"]["Cmd"] = json!(["serve", "--port", "4181"]);
        c["Config"]["Env"] = json!(["FLEET_HUB_DATA_DIR=/data"]);
        let b = create_body(&c, &old_image(), "img@sha256:n");
        assert_eq!(b["Cmd"], json!(["serve", "--port", "4181"]));
        assert_eq!(b["Env"], json!(["FLEET_HUB_DATA_DIR=/data"]));
    }
}
