//! `fleet-release`: what the release workflows call to write the signed update
//! documents (update-channel design §4–§5, slice S2). It writes; `minisign`
//! signs. Every document is built from the types the fleet reads, through
//! `fleet_update::publish`.
//!
//! ```text
//! fleet-release manifest --version V --commit SHA --build-id ID --assets DIR
//!     --assets-base URL --notes-url URL --compat compat.json --desktop-accepts MIN,MAX
//!     [--hub-image IMAGE@sha256:…] [--now UNIX] --out release-manifest.json
//! fleet-release channel-add --track T [--channel FILE] --manifest FILE --manifest-url URL
//!     [--now UNIX] [--expires-days N] [--keep N] --out FILE
//! fleet-release channel-edit --track T --channel FILE --op OP [--version V]
//!     [--component C] [--reason R] [--deadline RFC3339] [--now UNIX] [--expires-days N] --out FILE
//!     OP: resign | withdraw | recommend | rollback | clear-rollback | minimum
//!         | clear-minimum | mandatory
//! fleet-release verify --keys-file FILE --kind manifest|channel [--track T]
//!     --file FILE --sig FILE.minisig
//! ```
//!
//! `verify` checks a signature with the fleet's own verifier
//! (`fleet_update::verify`), so what CI publishes is proven readable by the
//! code that will read it, not only by `minisign`.

use std::collections::HashMap;
use std::path::Path;
use std::process::ExitCode;

use fleet_update::channel_doc::{ChannelDoc, Mandatory, ReleaseRef};
use fleet_update::publish::{
    build_manifest, channel_add, channel_edit, to_bytes, Asset, Edit, HubCompat, HubImage,
    ManifestInput, DEFAULT_EXPIRES_DAYS, DEFAULT_KEEP,
};
use fleet_update::verify::sha256_hex;
use fleet_update::{Track, Version, Window};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(msg) => {
            eprintln!("fleet-release: {msg}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("fleet-release: {e}");
            ExitCode::from(1)
        }
    }
}

struct Opts(HashMap<String, String>);

impl Opts {
    fn parse(args: &[String]) -> Result<Self, String> {
        let mut m = HashMap::new();
        let mut it = args.iter();
        while let Some(k) = it.next() {
            let key = k
                .strip_prefix("--")
                .ok_or_else(|| format!("unexpected argument {k:?}"))?;
            let val = it.next().ok_or_else(|| format!("--{key} needs a value"))?;
            if m.insert(key.to_string(), val.clone()).is_some() {
                return Err(format!("--{key} given twice"));
            }
        }
        Ok(Opts(m))
    }
    fn req(&self, k: &str) -> Result<&str, String> {
        self.0
            .get(k)
            .map(String::as_str)
            .ok_or_else(|| format!("--{k} is required"))
    }
    fn opt(&self, k: &str) -> Option<&str> {
        self.0.get(k).map(String::as_str)
    }
    fn num(&self, k: &str, default: i64) -> Result<i64, String> {
        self.opt(k)
            .map(|s| {
                s.parse()
                    .map_err(|_| format!("--{k} must be a number, got {s:?}"))
            })
            .transpose()
            .map(|v| v.unwrap_or(default))
    }
    fn version(&self, k: &str) -> Result<Version, String> {
        let s = self.req(k)?;
        Version::parse(s).map_err(|e| format!("--{k} {s:?}: {e}"))
    }
    fn track(&self) -> Result<Track, String> {
        self.req("track")?
            .parse()
            .map_err(|e| format!("--track: {e}"))
    }
    fn now(&self) -> Result<i64, String> {
        let wall = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        self.num("now", wall)
    }
}

fn run(args: &[String]) -> Result<String, String> {
    let (cmd, rest) = args
        .split_first()
        .ok_or("usage: fleet-release manifest | channel-add | channel-edit …")?;
    let o = Opts::parse(rest)?;
    match cmd.as_str() {
        "manifest" => manifest(&o),
        "channel-add" => add(&o),
        "channel-edit" => edit(&o),
        "verify" => verify(&o),
        other => Err(format!("unknown command {other:?}")),
    }
}

fn read(path: &str) -> Result<Vec<u8>, String> {
    std::fs::read(path).map_err(|e| format!("read {path}: {e}"))
}

fn write(path: &str, bytes: &[u8]) -> Result<(), String> {
    std::fs::write(path, bytes).map_err(|e| format!("write {path}: {e}"))
}

fn window(s: &str) -> Result<Window, String> {
    let (a, b) = s
        .split_once(',')
        .ok_or_else(|| format!("window {s:?} must be MIN,MAX"))?;
    let (a, b) = (a.trim().parse::<u32>(), b.trim().parse::<u32>());
    match (a, b) {
        (Ok(min), Ok(max)) if min <= max => Ok(Window::new(min, max)),
        _ => Err(format!("window {s:?} must be MIN,MAX with MIN <= MAX")),
    }
}

fn manifest(o: &Opts) -> Result<String, String> {
    let version = o.version("version")?;
    let compat: HubCompat = serde_json::from_slice(&read(o.req("compat")?)?)
        .map_err(|e| format!("--compat is not `fleet-hub compat --json` output: {e}"))?;
    let hub_image = o
        .opt("hub-image")
        .filter(|s| !s.is_empty())
        .map(|s| {
            s.split_once('@')
                .map(|(image, digest)| HubImage {
                    image: image.into(),
                    digest: digest.into(),
                })
                .ok_or_else(|| format!("--hub-image {s:?} must be IMAGE@sha256:…"))
        })
        .transpose()?;
    let dir = Path::new(o.req("assets")?);
    let mut assets = Vec::new();
    let entries = std::fs::read_dir(dir).map_err(|e| format!("read {}: {e}", dir.display()))?;
    for e in entries {
        let e = e.map_err(|e| e.to_string())?;
        if !e.file_type().map_err(|e| e.to_string())?.is_file() {
            continue;
        }
        let name = e.file_name().to_string_lossy().into_owned();
        let bytes = std::fs::read(e.path()).map_err(|err| format!("read {name}: {err}"))?;
        assets.push(Asset {
            sha256: sha256_hex(&bytes),
            size: bytes.len() as u64,
            name,
        });
    }
    assets.sort_by(|a, b| a.name.cmp(&b.name));
    let m = build_manifest(&ManifestInput {
        version: &version,
        commit: o.req("commit")?,
        build_id: o.req("build-id")?,
        published_at: o.now()?,
        assets_base: o.req("assets-base")?,
        notes_url: o.opt("notes-url").unwrap_or(""),
        hub: &compat,
        desktop_accepts: window(o.req("desktop-accepts")?)?,
        assets: &assets,
        hub_image: hub_image.as_ref(),
    })?;
    let out = o.req("out")?;
    write(out, &to_bytes(&m))?;
    let n: usize = m.components.values().map(|c| c.artifacts.len()).sum();
    Ok(format!(
        "wrote {out}: {version} ({}), {n} artifacts",
        m.release.track.as_str()
    ))
}

fn load_channel(path: Option<&str>) -> Result<Option<ChannelDoc>, String> {
    match path {
        Some(p) if Path::new(p).exists() => serde_json::from_slice(&read(p)?)
            .map(Some)
            .map_err(|e| format!("{p} is not a channel document: {e}")),
        _ => Ok(None),
    }
}

fn add(o: &Opts) -> Result<String, String> {
    let track = o.track()?;
    let bytes = read(o.req("manifest")?)?;
    let m: fleet_update::ReleaseManifest =
        serde_json::from_slice(&bytes).map_err(|e| format!("--manifest does not parse: {e}"))?;
    let release = ReleaseRef {
        version: m.release.version.clone(),
        manifest: o.req("manifest-url")?.into(),
        manifest_sha256: sha256_hex(&bytes),
    };
    let doc = channel_add(
        load_channel(o.opt("channel"))?,
        track,
        release,
        o.now()?,
        o.num("expires-days", DEFAULT_EXPIRES_DAYS)?,
        o.num("keep", DEFAULT_KEEP as i64)?.max(1) as usize,
    )?;
    let out = o.req("out")?;
    write(out, &to_bytes(&doc))?;
    Ok(format!(
        "wrote {out}: {} #{} lists {} (recommended {})",
        track.as_str(),
        doc.sequence,
        m.release.version,
        doc.recommended
    ))
}

fn edit(o: &Opts) -> Result<String, String> {
    let track = o.track()?;
    let doc = load_channel(Some(o.req("channel")?))?.ok_or("--channel does not exist")?;
    if doc.track != track {
        return Err(format!(
            "--channel is the {} channel, not {}",
            doc.track.as_str(),
            track.as_str()
        ));
    }
    let reason = || o.opt("reason").unwrap_or("").to_string();
    let op = o.req("op")?;
    let e = match op {
        "resign" => Edit::Resign,
        "withdraw" => Edit::Withdraw {
            version: o.version("version")?,
            reason: reason(),
        },
        "recommend" => Edit::Recommend(o.version("version")?),
        "rollback" => Edit::Rollback(Some(o.version("version")?)),
        "clear-rollback" => Edit::Rollback(None),
        "minimum" => Edit::Minimum {
            component: o.req("component")?.into(),
            version: Some(o.version("version")?),
        },
        "clear-minimum" => Edit::Minimum {
            component: o.req("component")?.into(),
            version: None,
        },
        "mandatory" => Edit::Mandatory(Mandatory {
            version: o.version("version")?,
            components: o
                .opt("component")
                .map(|c| {
                    c.split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect()
                })
                .unwrap_or_default(),
            reason: reason(),
            deadline: o
                .opt("deadline")
                .filter(|d| !d.is_empty())
                .map(String::from),
        }),
        other => return Err(format!("unknown --op {other:?}")),
    };
    let doc = channel_edit(
        doc,
        e,
        o.now()?,
        o.num("expires-days", DEFAULT_EXPIRES_DAYS)?,
    )?;
    let out = o.req("out")?;
    write(out, &to_bytes(&doc))?;
    Ok(format!(
        "wrote {out}: {} #{} after {op}",
        track.as_str(),
        doc.sequence
    ))
}

fn verify(o: &Opts) -> Result<String, String> {
    let keys_text = String::from_utf8(read(o.req("keys-file")?)?).map_err(|e| e.to_string())?;
    let keys = fleet_update::TrustedKeys::from_base64(
        keys_text.lines().map(str::trim).filter(|l| !l.is_empty()),
    )
    .map_err(|e| e.to_string())?;
    let file = o.req("file")?;
    let bytes = read(file)?;
    let sig = String::from_utf8(read(o.req("sig")?)?).map_err(|e| e.to_string())?;
    match o.req("kind")? {
        "manifest" => {
            let m = fleet_update::verify::verify_manifest(&bytes, &sig, &keys)
                .map_err(|e| e.to_string())?;
            Ok(format!(
                "{file}: a verified manifest for {}",
                m.release.version
            ))
        }
        "channel" => {
            let ch =
                fleet_update::verify::verify_channel(&bytes, &sig, &keys, o.track()?, 0, o.now()?)
                    .map_err(|e| e.to_string())?;
            Ok(format!(
                "{file}: a verified {} channel, #{}, {}",
                ch.doc.track.as_str(),
                ch.doc.sequence,
                if ch.fresh { "fresh" } else { "STALE" }
            ))
        }
        other => Err(format!("--kind must be manifest | channel, got {other:?}")),
    }
}
