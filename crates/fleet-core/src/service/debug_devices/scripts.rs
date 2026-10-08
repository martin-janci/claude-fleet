//! The bash a host runs for debug devices, and the parsers for what it
//! prints. Pure: every function here builds a string or reads one, so the
//! whole host side is testable without a phone (`tests` below run the
//! scripts against stub `adb` / `xcrun` binaries).
//!
//! Every script runs under `bash -lc` (a login shell, so a chatty profile
//! may print first) and anchors its answer on [`OUT_MARKER`]. Every value
//! interpolated into a script is quoted with [`quote`].

use super::Platform;
use crate::ipc_error::{codes, IpcError};
use crate::service::move_session::carry::{payload, OUT_MARKER};
use crate::shell::quote;
use crate::store::SeenDevice;

/// Finds the host's tools. `adb` and `emulator` are found on PATH or in the
/// usual SDK folders (a login shell often lacks `platform-tools` on PATH);
/// `xcrun` only on macOS.
const PREAMBLE: &str = r#"set +e
cf_sdk_tool() {
  if command -v "$1" >/dev/null 2>&1; then command -v "$1"; return 0; fi
  for d in "${ANDROID_HOME:-}" "${ANDROID_SDK_ROOT:-}" "$HOME/Library/Android/sdk" "$HOME/Android/Sdk" "$HOME/Android/sdk"; do
    if [ -n "$d" ] && [ -x "$d/$2" ]; then printf '%s\n' "$d/$2"; return 0; fi
  done
  return 1
}
ADB=$(cf_sdk_tool adb platform-tools/adb)
EMU=$(cf_sdk_tool emulator emulator/emulator)
XC=
if [ "$(uname -s)" = Darwin ] && command -v xcrun >/dev/null 2>&1; then XC=xcrun; fi
cf_fail() { printf '__CF_DEV_FAILED__ %s\n' "$1" >&2; exit 5; }
"#;

/// A script's own refusal, on stderr: `__CF_DEV_FAILED__ <why>`.
pub const FAILED: &str = "__CF_DEV_FAILED__";

/// Inventory the host: attached Android devices and running emulators
/// (`adb devices -l`, with each online one's Android version and an
/// emulator's AVD name), the AVDs that could be started, and on macOS the
/// iOS simulators and paired iOS devices. Sections start with `##`.
pub fn scan_script() -> String {
    let body = r#"printf '\n__CF_OUT__\n'
printf '##TOOLS adb=%s emulator=%s xcrun=%s\n' "${ADB:+1}" "${EMU:+1}" "${XC:+1}"
if [ -n "$ADB" ]; then
  printf '##ADB\n'
  "$ADB" devices -l 2>/dev/null | tr -d '\r'
  for s in $("$ADB" devices 2>/dev/null | tr -d '\r' | awk 'NR>1 && $2=="device" {print $1}'); do
    v=$("$ADB" -s "$s" shell getprop ro.build.version.release 2>/dev/null | tr -d '\r' | head -1)
    printf '##PROP %s version %s\n' "$s" "$v"
    case "$s" in
      emulator-*)
        n=$("$ADB" -s "$s" emu avd name 2>/dev/null | tr -d '\r' | head -1)
        printf '##PROP %s avd %s\n' "$s" "$n" ;;
    esac
  done
fi
if [ -n "$EMU" ]; then
  printf '##AVDS\n'
  "$EMU" -list-avds 2>/dev/null | tr -d '\r' | grep -v '^INFO'
fi
if [ -n "$XC" ]; then
  printf '##SIMCTL\n'
  xcrun simctl list devices available --json 2>/dev/null
  printf '\n'
  f=$(mktemp -t cfdev.XXXXXX)
  if xcrun devicectl list devices --quiet --json-output "$f" >/dev/null 2>&1; then
    printf '##DEVICECTL\n'
    cat "$f"
    printf '\n'
  fi
  rm -f "$f"
fi
printf '##END\n'
"#;
    format!("# cf-devices:scan\n{PREAMBLE}{body}")
}

/// A device fleet can start but that is not running: an Android Virtual
/// Device, or a shut-down iOS simulator. `key` is the `dev_key` it is
/// inventoried under once it runs.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Bootable {
    pub key: String,
    pub platform: String,
    pub kind: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub os_version: Option<String>,
    /// The simulator's UDID; absent for an AVD (started by name).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub udid: Option<String>,
}

/// What one scan of a host found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScanOutput {
    /// Attached or running: inventoried.
    pub running: Vec<SeenDevice>,
    /// Could be started.
    pub bootable: Vec<Bootable>,
    /// Which tools the host has (`adb`, `emulator`, `xcrun`).
    pub tools: Vec<String>,
}

/// Read [`scan_script`]'s answer. A section that cannot be read (a newer
/// `simctl` JSON shape) is skipped, never fatal: the rest still counts.
pub fn parse_scan(stdout: &[u8]) -> Result<ScanOutput, IpcError> {
    let body = payload(stdout).ok_or_else(|| {
        IpcError::new(codes::E_PARSE, "device scan printed no answer".to_string())
    })?;
    let text = String::from_utf8_lossy(body);
    let mut out = ScanOutput::default();
    let mut section = "";
    let mut adb_lines: Vec<&str> = Vec::new();
    let mut avds: Vec<&str> = Vec::new();
    let mut simctl = String::new();
    let mut devicectl = String::new();
    let mut props: Vec<(&str, &str, &str)> = Vec::new();
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("##") {
            let mut words = rest.splitn(2, ' ');
            let tag = words.next().unwrap_or("");
            match tag {
                "TOOLS" => {
                    for kv in words.next().unwrap_or("").split_whitespace() {
                        if let Some((k, v)) = kv.split_once('=') {
                            if v == "1" {
                                out.tools.push(k.to_string());
                            }
                        }
                    }
                    section = "";
                }
                "PROP" => {
                    let rest = words.next().unwrap_or("");
                    let mut p = rest.splitn(3, ' ');
                    if let (Some(s), Some(k)) = (p.next(), p.next()) {
                        props.push((s, k, p.next().unwrap_or("").trim()));
                    }
                }
                "ADB" | "AVDS" | "SIMCTL" | "DEVICECTL" | "END" => section = tag,
                _ => {}
            }
            continue;
        }
        match section {
            "ADB" => adb_lines.push(line),
            "AVDS" => avds.push(line),
            "SIMCTL" => {
                simctl.push_str(line);
                simctl.push('\n');
            }
            "DEVICECTL" => {
                devicectl.push_str(line);
                devicectl.push('\n');
            }
            _ => {}
        }
    }
    let prop = |serial: &str, key: &str| -> Option<String> {
        props
            .iter()
            .find(|(s, k, _)| *s == serial && *k == key)
            .map(|(_, _, v)| v.to_string())
            .filter(|v| !v.is_empty())
    };
    let mut running_avds: Vec<String> = Vec::new();
    for line in adb_lines {
        let Some(d) = parse_adb_line(line) else {
            continue;
        };
        let mut d = d;
        d.os_version =
            prop(d.serial.as_deref().unwrap_or(""), "version").map(|v| format!("Android {v}"));
        if d.kind == "emulator" {
            if let Some(avd) = prop(d.serial.as_deref().unwrap_or(""), "avd") {
                d.dev_key = format!("avd:{avd}");
                d.name = avd.replace('_', " ");
                running_avds.push(avd);
            }
        }
        out.running.push(d);
    }
    for avd in avds.iter().map(|a| a.trim()).filter(|a| !a.is_empty()) {
        if running_avds.iter().any(|r| r == avd) || avd.contains(' ') {
            continue;
        }
        out.bootable.push(Bootable {
            key: format!("avd:{avd}"),
            platform: Platform::Android.as_str().into(),
            kind: "emulator".into(),
            name: avd.replace('_', " "),
            os_version: None,
            udid: None,
        });
    }
    if !simctl.trim().is_empty() {
        parse_simctl(&simctl, &mut out);
    }
    if !devicectl.trim().is_empty() {
        parse_devicectl(&devicectl, &mut out);
    }
    Ok(out)
}

/// One line of `adb devices -l`: `<serial> <state> key:value…`.
fn parse_adb_line(line: &str) -> Option<SeenDevice> {
    let line = line.trim();
    if line.is_empty() || line.starts_with("List of devices") || line.starts_with('*') {
        return None;
    }
    let mut words = line.split_whitespace();
    let serial = words.next()?.to_string();
    let raw_state = words.next()?;
    let mut model = None;
    let mut product = None;
    for kv in words {
        match kv.split_once(':') {
            Some(("model", v)) => model = Some(v.to_string()),
            Some(("product", v)) => product = Some(v.to_string()),
            _ => {}
        }
    }
    let state = match raw_state {
        "device" => "online",
        "offline" => "offline",
        "unauthorized" => "unauthorized",
        "no" => "unauthorized", // "no permissions" (udev)
        other => other,
    };
    let emulator = serial.starts_with("emulator-");
    let name = model
        .clone()
        .or(product)
        .map(|m| m.replace('_', " "))
        .unwrap_or_else(|| serial.clone());
    Some(SeenDevice {
        dev_key: serial.clone(),
        platform: Platform::Android.as_str().into(),
        kind: if emulator { "emulator" } else { "physical" }.into(),
        serial: Some(serial),
        name,
        model: model.map(|m| m.replace('_', " ")),
        os_version: None,
        state: state.to_string(),
    })
}

/// `com.apple.CoreSimulator.SimRuntime.iOS-17-5` → `iOS 17.5`; `None` for a
/// runtime that is not iOS (watchOS, tvOS, visionOS are not inventoried).
fn ios_runtime(id: &str) -> Option<String> {
    let tail = id.rsplit('.').next()?;
    let ver = tail.strip_prefix("iOS-")?;
    Some(format!("iOS {}", ver.replace('-', ".")))
}

/// `xcrun simctl list devices available --json`: a booted simulator is
/// inventoried, a shut-down one is bootable.
fn parse_simctl(json: &str, out: &mut ScanOutput) {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(json) else {
        return;
    };
    let Some(runtimes) = v.get("devices").and_then(|d| d.as_object()) else {
        return;
    };
    for (runtime, devices) in runtimes {
        let Some(os) = ios_runtime(runtime) else {
            continue;
        };
        for d in devices.as_array().into_iter().flatten() {
            let (Some(udid), Some(name)) = (
                d.get("udid").and_then(|x| x.as_str()),
                d.get("name").and_then(|x| x.as_str()),
            ) else {
                continue;
            };
            if d.get("isAvailable").and_then(|x| x.as_bool()) == Some(false) {
                continue;
            }
            match d.get("state").and_then(|x| x.as_str()).unwrap_or("") {
                "Booted" => out.running.push(SeenDevice {
                    dev_key: udid.to_string(),
                    platform: Platform::Ios.as_str().into(),
                    kind: "simulator".into(),
                    serial: Some(udid.to_string()),
                    name: name.to_string(),
                    model: None,
                    os_version: Some(os.clone()),
                    state: "booted".into(),
                }),
                _ => out.bootable.push(Bootable {
                    key: udid.to_string(),
                    platform: Platform::Ios.as_str().into(),
                    kind: "simulator".into(),
                    name: name.to_string(),
                    os_version: Some(os.clone()),
                    udid: Some(udid.to_string()),
                }),
            }
        }
    }
}

/// `xcrun devicectl list devices --json-output`: physical iOS / iPadOS
/// devices this Mac is paired with, connected or not.
fn parse_devicectl(json: &str, out: &mut ScanOutput) {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(json) else {
        return;
    };
    let devices = v
        .pointer("/result/devices")
        .and_then(|d| d.as_array())
        .cloned()
        .unwrap_or_default();
    for d in devices {
        let s = |p: &str| d.pointer(p).and_then(|x| x.as_str()).map(str::to_string);
        if s("/hardwareProperties/reality").as_deref() != Some("physical") {
            continue;
        }
        let platform = s("/hardwareProperties/platform").unwrap_or_default();
        if !(platform.eq_ignore_ascii_case("iOS") || platform.eq_ignore_ascii_case("iPadOS")) {
            continue;
        }
        let Some(udid) = s("/hardwareProperties/udid").or_else(|| s("/identifier")) else {
            continue;
        };
        let paired = s("/connectionProperties/pairingState").as_deref() == Some("paired");
        let state = match s("/connectionProperties/tunnelState").as_deref() {
            _ if !paired => "unauthorized",
            Some("connected") => "online",
            _ => "offline",
        };
        let model = s("/hardwareProperties/marketingName");
        out.running.push(SeenDevice {
            dev_key: udid.clone(),
            platform: Platform::Ios.as_str().into(),
            kind: "physical".into(),
            serial: Some(udid),
            name: s("/deviceProperties/name")
                .or_else(|| model.clone())
                .unwrap_or_else(|| "iOS device".into()),
            model,
            os_version: s("/deviceProperties/osVersionNumber").map(|v| format!("iOS {v}")),
            state: state.into(),
        });
    }
}

/// How a script addresses one device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// `"$ADB" -s <serial>`.
    Adb { serial: String },
    /// `xcrun simctl <verb> <udid>`.
    Simulator { udid: String },
    /// `xcrun devicectl device <verb…> --device <udid>`.
    IosDevice { udid: String },
}

fn needs(target: &Target) -> &'static str {
    match target {
        Target::Adb { .. } => {
            r#"[ -n "$ADB" ] || cf_fail 'adb is not installed on this host'
"#
        }
        Target::Simulator { .. } | Target::IosDevice { .. } => {
            r#"[ -n "$XC" ] || cf_fail 'xcrun is not available on this host (macOS with Xcode)'
"#
        }
    }
}

/// `adb` subcommands `run` may start. Each runs against the device; none
/// reads or writes a path on the host (`push`/`pull`/`install` do, and have
/// their own actions), and none streams forever (`logcat` is `logs`).
pub const ADB_VERBS: &[&str] = &[
    "shell",
    "uninstall",
    "reboot",
    "forward",
    "reverse",
    "get-state",
    "get-serialno",
    "get-devpath",
    "emu",
    "root",
    "unroot",
    "wait-for-device",
];

/// `xcrun simctl` verbs `run` may start, each taking the simulator's UDID
/// first. Not `spawn` (any process on the Mac), not `push`/`addmedia`
/// (a path on the Mac): `install` has its own action.
pub const SIMCTL_VERBS: &[&str] = &[
    "launch",
    "terminate",
    "openurl",
    "uninstall",
    "listapps",
    "appinfo",
    "get_app_container",
    "privacy",
    "ui",
    "status_bar",
    "location",
    "erase",
];

/// `xcrun devicectl device` verbs `run` may start.
pub const DEVICECTL_VERBS: &[&str] = &["info", "process", "uninstall", "reboot"];

/// Arguments for one command: at most this many, each at most this long.
pub const MAX_ARGS: usize = 64;
pub const MAX_ARG_BYTES: usize = 4096;
/// Output `run` keeps; the rest is cut and `truncated` set.
pub const RUN_OUTPUT_BYTES: usize = 256 * 1024;

/// Check `args` for `run` on `target`: a known verb first, bounded sizes.
pub fn check_run_args(target: &Target, args: &[String]) -> Result<(), IpcError> {
    let bad = |m: String| Err(IpcError::new(codes::E_INVALID, m));
    let Some(verb) = args.first() else {
        return bad("run needs args, its first the command (e.g. [\"shell\", \"pm\", \"list\", \"packages\"])".into());
    };
    if args.len() > MAX_ARGS {
        return bad(format!("at most {MAX_ARGS} args"));
    }
    if args
        .iter()
        .any(|a| a.len() > MAX_ARG_BYTES || a.contains('\0'))
    {
        return bad(format!(
            "an arg is over {MAX_ARG_BYTES} bytes or holds a NUL"
        ));
    }
    let (allowed, tool) = match target {
        Target::Adb { .. } => (ADB_VERBS, "adb"),
        Target::Simulator { .. } => (SIMCTL_VERBS, "simctl"),
        Target::IosDevice { .. } => (DEVICECTL_VERBS, "devicectl device"),
    };
    if !allowed.contains(&verb.as_str()) {
        return bad(format!(
            "{tool} {verb} is not allowed here; allowed: {}. Use install, logs or screenshot for those",
            allowed.join(", ")
        ));
    }
    Ok(())
}

fn quoted(args: &[String]) -> String {
    args.iter().map(|a| quote(a)).collect::<Vec<_>>().join(" ")
}

/// The command line that runs `args` against `target`.
fn command_line(target: &Target, args: &[String]) -> String {
    match target {
        Target::Adb { serial } => format!(r#""$ADB" -s {} {}"#, quote(serial), quoted(args)),
        Target::Simulator { udid } => {
            let (verb, rest) = args.split_first().expect("checked non-empty");
            format!(
                "xcrun simctl {} {} {}",
                quote(verb),
                quote(udid),
                quoted(rest)
            )
        }
        Target::IosDevice { udid } => format!(
            "xcrun devicectl device {} --device {}",
            quoted(args),
            quote(udid)
        ),
    }
}

/// Run `args` (already [`check_run_args`]-checked) and print, after
/// [`OUT_MARKER`], the first [`RUN_OUTPUT_BYTES`] of its stdout and stderr,
/// then `__CF_RC__ <exit status>` on a line of its own.
pub fn run_script(target: &Target, args: &[String]) -> String {
    format!(
        "# cf-devices:run\n{PREAMBLE}{needs}printf '\\n{OUT_MARKER}\\n'\n\
         {cmd} </dev/null 2>&1 | head -c {cap}\nrc=${{PIPESTATUS[0]}}\n\
         printf '\\n__CF_RC__ %s\\n' \"$rc\"\n",
        needs = needs(target),
        cmd = command_line(target, args),
        cap = RUN_OUTPUT_BYTES,
    )
}

/// What a command printed, and how it ended.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct RunOutput {
    pub exit_code: i32,
    pub output: String,
    /// The output was longer than [`RUN_OUTPUT_BYTES`] and was cut.
    pub truncated: bool,
}

pub fn parse_run(stdout: &[u8]) -> Result<RunOutput, IpcError> {
    let body = payload(stdout).ok_or_else(|| {
        IpcError::new(codes::E_PARSE, "the command printed no answer".to_string())
    })?;
    let text = String::from_utf8_lossy(body);
    let Some(idx) = text.rfind("\n__CF_RC__ ") else {
        return Err(IpcError::new(
            codes::E_PARSE,
            "the command's exit status is missing".to_string(),
        ));
    };
    let output = text[..idx]
        .strip_suffix('\n')
        .unwrap_or(&text[..idx])
        .to_string();
    let exit_code = text[idx + "\n__CF_RC__ ".len()..]
        .trim()
        .parse::<i32>()
        .unwrap_or(-1);
    let truncated = output.len() >= RUN_OUTPUT_BYTES;
    Ok(RunOutput {
        exit_code,
        output,
        truncated,
    })
}

/// Log lines `logs` returns at most.
pub const MAX_LOG_LINES: u32 = 2000;

/// Recent log lines, newest last, through `run`'s output shape. Android:
/// `logcat -d` (a dump, never a stream) with an optional filterspec
/// (`MyApp:D *:S`); a simulator: `log show` over the last `since_secs`
/// with an optional predicate. `contains` keeps lines holding it (case
/// insensitive) before the last `lines` are taken.
pub fn logs_script(
    target: &Target,
    lines: u32,
    since_secs: u32,
    filter: Option<&str>,
    contains: Option<&str>,
) -> Result<String, IpcError> {
    let lines = lines.clamp(1, MAX_LOG_LINES);
    let source = match target {
        Target::Adb { serial } => {
            let spec = filter
                .map(|f| {
                    f.split_whitespace()
                        .map(quote)
                        .collect::<Vec<_>>()
                        .join(" ")
                })
                .unwrap_or_default();
            format!(
                r#""$ADB" -s {} logcat -d -v threadtime -t 20000 {spec}"#,
                quote(serial)
            )
        }
        Target::Simulator { udid } => {
            let predicate = filter
                .map(|p| format!("--predicate {}", quote(p)))
                .unwrap_or_default();
            format!(
                "xcrun simctl spawn {} log show --style compact --last {}s {predicate}",
                quote(udid),
                since_secs.clamp(1, 86_400)
            )
        }
        Target::IosDevice { .. } => {
            return Err(IpcError::new(
                codes::E_UNSUPPORTED,
                "logs from a physical iOS device are not read by fleet yet; use Console.app \
                 on its Mac, or run the app in a simulator"
                    .to_string(),
            ))
        }
    };
    let grep = contains
        .map(|c| format!("| grep -F -i -- {} ", quote(c)))
        .unwrap_or_default();
    Ok(format!(
        "# cf-devices:logs\n{PREAMBLE}{needs}printf '\\n{OUT_MARKER}\\n'\n\
         {source} </dev/null 2>&1 {grep}| tail -n {lines} | head -c {cap}\n\
         rc=${{PIPESTATUS[0]}}\nprintf '\\n__CF_RC__ %s\\n' \"$rc\"\n",
        needs = needs(target),
        cap = RUN_OUTPUT_BYTES,
    ))
}

/// The screen as PNG bytes after [`OUT_MARKER`].
pub fn screenshot_script(target: &Target) -> String {
    let take = match target {
        Target::Adb { serial } => format!(
            r#"printf '\n{OUT_MARKER}\n'
"$ADB" -s {} exec-out screencap -p
"#,
            quote(serial)
        ),
        Target::Simulator { udid } => format!(
            r#"f=$(mktemp -t cfshot.XXXXXX) || cf_fail 'mktemp'
xcrun simctl io {} screenshot --type=png "$f" >/dev/null 2>&1 || {{ rm -f "$f"; cf_fail 'simctl io screenshot failed (is the simulator booted?)'; }}
printf '\n{OUT_MARKER}\n'
cat "$f"; rm -f "$f"
"#,
            quote(udid)
        ),
        Target::IosDevice { udid } => format!(
            r#"command -v idevicescreenshot >/dev/null 2>&1 || cf_fail 'a physical iOS screenshot needs libimobiledevice (idevicescreenshot) on the Mac'
f=$(mktemp -t cfshot.XXXXXX) || cf_fail 'mktemp'
idevicescreenshot -u {} "$f" >/dev/null 2>&1 || {{ rm -f "$f"; cf_fail 'idevicescreenshot failed'; }}
printf '\n{OUT_MARKER}\n'
cat "$f"; rm -f "$f"
"#,
            quote(udid)
        ),
    };
    format!("# cf-devices:screenshot\n{PREAMBLE}{}{take}", needs(target))
}

/// A screenshot larger than this is refused rather than relayed.
pub const MAX_SCREENSHOT_BYTES: usize = 16 * 1024 * 1024;

/// The image's bytes and MIME type (PNG, or JPEG from some
/// `idevicescreenshot` builds).
pub fn parse_screenshot(stdout: &[u8]) -> Result<(Vec<u8>, &'static str), IpcError> {
    let body = payload(stdout).ok_or_else(|| {
        IpcError::new(codes::E_PARSE, "the screenshot printed nothing".to_string())
    })?;
    let mime = if body.starts_with(b"\x89PNG\r\n\x1a\n") {
        "image/png"
    } else if body.starts_with(&[0xff, 0xd8, 0xff]) {
        "image/jpeg"
    } else {
        let head = String::from_utf8_lossy(&body[..body.len().min(200)]).into_owned();
        return Err(IpcError::new(
            codes::E_PARSE,
            format!("the screenshot is not an image: {}", head.trim()),
        ));
    };
    if body.len() > MAX_SCREENSHOT_BYTES {
        return Err(IpcError::new(
            codes::E_LIMIT,
            "the screenshot is over 16 MiB".to_string(),
        ));
    }
    Ok((body.to_vec(), mime))
}

/// Install the app at `path` on the device's host. Android: an `.apk`
/// (`install -r`, `-d` with `downgrade`); a simulator: a `.app` folder
/// (or a `.ipa`); a device: `.app` or `.ipa` through `devicectl`.
pub fn install_script(target: &Target, path: &str, downgrade: bool) -> String {
    let cmd = match target {
        Target::Adb { serial } => format!(
            r#""$ADB" -s {} install -r {}"$p""#,
            quote(serial),
            if downgrade { "-d " } else { "" }
        ),
        Target::Simulator { udid } => format!(r#"xcrun simctl install {} "$p""#, quote(udid)),
        Target::IosDevice { udid } => {
            format!(
                r#"xcrun devicectl device install app --device {} "$p""#,
                quote(udid)
            )
        }
    };
    format!(
        "# cf-devices:install\n{PREAMBLE}{needs}p={p}\ncase \"$p\" in '~/'*) p=\"$HOME/${{p#\\~/}}\";; esac\n\
         [ -e \"$p\" ] || cf_fail \"no such file: $p\"\nprintf '\\n{OUT_MARKER}\\n'\n\
         {cmd} </dev/null 2>&1 | head -c {cap}\nrc=${{PIPESTATUS[0]}}\n\
         printf '\\n__CF_RC__ %s\\n' \"$rc\"\n",
        needs = needs(target),
        p = quote(path),
        cap = RUN_OUTPUT_BYTES,
    )
}

/// Start a stopped device. An AVD is started detached (no window on a
/// host with no display) and answers at once: it shows as online on the
/// next scan, typically within a minute. A simulator boots in place.
pub fn boot_script(boot: &BootTarget) -> String {
    let body = match boot {
        BootTarget::Avd { name } => format!(
            r#"[ -n "$EMU" ] || cf_fail 'the Android emulator is not installed on this host'
extra=
if [ "$(uname -s)" != Darwin ] && [ -z "${{DISPLAY:-}}" ] && [ -z "${{WAYLAND_DISPLAY:-}}" ]; then extra=-no-window; fi
nohup "$EMU" -avd {} $extra >/dev/null 2>&1 </dev/null &
printf '\n{OUT_MARKER}\nstarting\n'
"#,
            quote(name)
        ),
        BootTarget::Simulator { udid } => format!(
            r#"[ -n "$XC" ] || cf_fail 'xcrun is not available on this host (macOS with Xcode)'
e=$(xcrun simctl boot {} 2>&1) || case "$e" in *"current state: Booted"*) ;; *) cf_fail "$(printf '%s' "$e" | tr '\n' ' ' | cut -c1-300)";; esac
printf '\n{OUT_MARKER}\nbooted\n'
"#,
            quote(udid)
        ),
    };
    format!("# cf-devices:boot\n{PREAMBLE}{body}")
}

/// What `boot` starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BootTarget {
    Avd { name: String },
    Simulator { udid: String },
}

/// Stop a running emulator or simulator.
pub fn shutdown_script(target: &Target) -> Result<String, IpcError> {
    let body = match target {
        Target::Adb { serial } if serial.starts_with("emulator-") => {
            format!(
                r#""$ADB" -s {} emu kill >/dev/null 2>&1 || cf_fail 'adb emu kill failed'"#,
                quote(serial)
            )
        }
        Target::Simulator { udid } => format!(
            r#"e=$(xcrun simctl shutdown {} 2>&1) || case "$e" in *"current state: Shutdown"*) ;; *) cf_fail "$(printf '%s' "$e" | tr '\n' ' ' | cut -c1-300)";; esac"#,
            quote(udid)
        ),
        _ => {
            return Err(IpcError::new(
                codes::E_UNSUPPORTED,
                "only an emulator or a simulator is shut down; reboot a phone with run".to_string(),
            ))
        }
    };
    Ok(format!(
        "# cf-devices:shutdown\n{PREAMBLE}{}{body}\nprintf '\\n{OUT_MARKER}\\nstopped\\n'\n",
        needs(target)
    ))
}

/// A staging folder for a copied app on a host:
/// `$HOME/.cache/claude-fleet/devices/<id>`. Prints its path.
pub fn stage_dir_script(id: &str) -> String {
    format!(
        r#"# cf-devices:stage
set +e
[ -n "$HOME" ] || {{ printf '{FAILED} HOME\n' >&2; exit 5; }}
d="$HOME/.cache/claude-fleet/devices/"{id}
mkdir -p "$d" || {{ printf '{FAILED} mkdir\n' >&2; exit 5; }}
printf '\n{OUT_MARKER}\n%s\n' "$d"
"#,
        id = quote(id)
    )
}

/// Pack `path` (a file, or a folder such as a `.app`) on the source host
/// into `<stage>/app.tgz`. Prints the archive's size and the packed name.
pub fn pack_script(path: &str, stage: &str) -> String {
    format!(
        r#"# cf-devices:pack
set +e
p={p}
case "$p" in '~/'*) p="$HOME/${{p#\~/}}";; esac
[ -e "$p" ] || {{ printf '{FAILED} no such file: %s\n' "$p" >&2; exit 5; }}
s={s}
b=$(basename "$p")
tar -czf "$s/app.tgz" -C "$(dirname "$p")" -- "$b" 2>/dev/null || {{ printf '{FAILED} tar\n' >&2; exit 5; }}
n=$(wc -c < "$s/app.tgz" | tr -d ' ')
printf '\n{OUT_MARKER}\n%s\n%s\n' "$n" "$b"
"#,
        p = quote(path),
        s = quote(stage),
    )
}

/// Append stdin to `<stage>/app.tgz` on the device's host.
pub fn append_script(stage: &str) -> String {
    format!(
        "# cf-devices:append\nset +e\ncat >> {}/app.tgz || exit 5\nprintf '\\n{OUT_MARKER}\\nok\\n'\n",
        quote(stage)
    )
}

/// Unpack the copied archive in its staging folder; prints the app's path.
pub fn unpack_script(stage: &str, name: &str) -> String {
    format!(
        r#"# cf-devices:unpack
set +e
s={s}
tar -xzf "$s/app.tgz" -C "$s" 2>/dev/null || {{ printf '{FAILED} untar\n' >&2; exit 5; }}
rm -f "$s/app.tgz"
printf '\n{OUT_MARKER}\n%s\n' "$s/"{n}
"#,
        s = quote(stage),
        n = quote(name),
    )
}

/// Remove a staging folder (only one under fleet's own cache).
pub fn cleanup_script(stage: &str) -> String {
    format!(
        r#"# cf-devices:cleanup
s={s}
case "$s" in *..*) exit 0;; esac
case "$s" in "$HOME/.cache/claude-fleet/devices/"?*) rm -rf -- "$s";; esac
"#,
        s = quote(stage)
    )
}

/// The first line after [`OUT_MARKER`], trimmed.
pub fn first_line(stdout: &[u8]) -> Option<String> {
    let body = payload(stdout)?;
    let text = String::from_utf8_lossy(body);
    text.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(str::to_string)
}

/// The script's own reason for failing, if it gave one.
pub fn failure(stderr: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(stderr);
    text.lines()
        .find_map(|l| l.strip_prefix(FAILED))
        .map(|r| r.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const ADB_OUT: &str = "List of devices attached\n\
        R5CT1234ABC            device usb:1-1 product:o1sxeea model:SM_G991B device:o1s transport_id:2\n\
        emulator-5554          device product:sdk_gphone64_arm64 model:sdk_gphone64_arm64 device:emu64a transport_id:1\n\
        0123456789             unauthorized usb:1-2 transport_id:3\n\
        192.168.1.5:5555       offline\n";

    fn scan_answer() -> Vec<u8> {
        let simctl = r#"{"devices":{
          "com.apple.CoreSimulator.SimRuntime.iOS-17-5":[
            {"udid":"AAAA-1","name":"iPhone 15","state":"Booted","isAvailable":true},
            {"udid":"BBBB-2","name":"iPad Air","state":"Shutdown","isAvailable":true}],
          "com.apple.CoreSimulator.SimRuntime.watchOS-10-0":[
            {"udid":"CCCC-3","name":"Watch","state":"Booted","isAvailable":true}]}}"#;
        let devicectl = r#"{"result":{"devices":[
          {"identifier":"X-1","deviceProperties":{"name":"Martin's iPhone","osVersionNumber":"17.4"},
           "hardwareProperties":{"udid":"00008110-1","platform":"iOS","reality":"physical","marketingName":"iPhone 14"},
           "connectionProperties":{"tunnelState":"connected","pairingState":"paired"}},
          {"identifier":"X-2","deviceProperties":{"name":"Old iPad"},
           "hardwareProperties":{"udid":"00008110-2","platform":"iOS","reality":"physical"},
           "connectionProperties":{"tunnelState":"unavailable","pairingState":"paired"}},
          {"identifier":"X-3","hardwareProperties":{"platform":"watchOS","reality":"physical"}}]}}"#;
        format!(
            "Welcome to the login banner\n\n{OUT_MARKER}\n##TOOLS adb=1 emulator=1 xcrun=1\n##ADB\n{ADB_OUT}\
             ##PROP R5CT1234ABC version 14\n##PROP emulator-5554 version 15\n##PROP emulator-5554 avd Pixel_8_API_35\n\
             ##AVDS\nPixel_8_API_35\nTablet_API_34\n##SIMCTL\n{simctl}\n##DEVICECTL\n{devicectl}\n##END\n"
        )
        .into_bytes()
    }

    #[test]
    fn a_scan_reads_every_section() {
        let out = parse_scan(&scan_answer()).unwrap();
        assert_eq!(out.tools, ["adb", "emulator", "xcrun"]);
        let keys: Vec<(&str, &str, &str)> = out
            .running
            .iter()
            .map(|d| (d.dev_key.as_str(), d.kind.as_str(), d.state.as_str()))
            .collect();
        assert_eq!(
            keys,
            [
                ("R5CT1234ABC", "physical", "online"),
                ("avd:Pixel_8_API_35", "emulator", "online"),
                ("0123456789", "physical", "unauthorized"),
                ("192.168.1.5:5555", "physical", "offline"),
                ("AAAA-1", "simulator", "booted"),
                ("00008110-1", "physical", "online"),
                ("00008110-2", "physical", "offline"),
            ]
        );
        let phone = &out.running[0];
        assert_eq!(phone.name, "SM G991B");
        assert_eq!(phone.os_version.as_deref(), Some("Android 14"));
        let emu = &out.running[1];
        assert_eq!(emu.serial.as_deref(), Some("emulator-5554"));
        assert_eq!(emu.name, "Pixel 8 API 35");
        let iphone = &out.running[5];
        assert_eq!(iphone.name, "Martin's iPhone");
        assert_eq!(iphone.model.as_deref(), Some("iPhone 14"));
        assert_eq!(iphone.os_version.as_deref(), Some("iOS 17.4"));
        let boot: Vec<&str> = out.bootable.iter().map(|b| b.key.as_str()).collect();
        // The running AVD is not bootable; the watchOS runtime is skipped.
        assert_eq!(boot, ["avd:Tablet_API_34", "BBBB-2"]);
        assert_eq!(out.bootable[1].os_version.as_deref(), Some("iOS 17.5"));
    }

    #[test]
    fn a_scan_with_no_tools_is_empty_and_no_answer_is_an_error() {
        let out = parse_scan(
            format!("\n{OUT_MARKER}\n##TOOLS adb= emulator= xcrun=\n##END\n").as_bytes(),
        )
        .unwrap();
        assert_eq!(out, ScanOutput::default());
        assert!(parse_scan(b"bash: oops").is_err());
        // A garbled simctl section is skipped, not fatal.
        let out =
            parse_scan(format!("\n{OUT_MARKER}\n##SIMCTL\n{{nope\n##END\n").as_bytes()).unwrap();
        assert!(out.running.is_empty());
    }

    #[test]
    fn run_args_are_checked_against_each_tools_verbs() {
        let adb = Target::Adb { serial: "S".into() };
        let sim = Target::Simulator { udid: "U".into() };
        let ok = |t: &Target, a: &[&str]| {
            check_run_args(t, &a.iter().map(|s| s.to_string()).collect::<Vec<_>>())
        };
        assert!(ok(&adb, &["shell", "pm", "list", "packages"]).is_ok());
        assert!(ok(&adb, &["pull", "/sdcard/x", "/etc/passwd"]).is_err());
        assert!(ok(&adb, &["push", "~/.ssh/id_rsa", "/sdcard/"]).is_err());
        assert!(ok(&adb, &[]).is_err());
        assert!(ok(&sim, &["launch", "com.example"]).is_ok());
        assert!(ok(&sim, &["spawn", "/bin/sh"]).is_err());
        let long = "x".repeat(MAX_ARG_BYTES + 1);
        assert!(ok(&adb, &["shell", &long]).is_err());
    }

    #[test]
    fn every_value_in_a_command_is_quoted() {
        let t = Target::Adb {
            serial: "S; rm -rf /".into(),
        };
        let s = run_script(&t, &["shell".into(), "echo $(id)".into()]);
        assert!(
            s.contains(r#""$ADB" -s 'S; rm -rf /' 'shell' 'echo $(id)'"#),
            "{s}"
        );
        let sim = Target::Simulator { udid: "U".into() };
        let s = run_script(&sim, &["launch".into(), "com.x".into()]);
        assert!(s.contains("xcrun simctl 'launch' 'U' 'com.x'"), "{s}");
        let dev = Target::IosDevice { udid: "D".into() };
        let s = run_script(&dev, &["info".into(), "apps".into()]);
        assert!(
            s.contains("xcrun devicectl device 'info' 'apps' --device 'D'"),
            "{s}"
        );
    }

    #[test]
    fn a_run_answer_carries_output_and_status() {
        let out =
            parse_run(format!("banner\n\n{OUT_MARKER}\nhello\nworld\n__CF_RC__ 3\n").as_bytes())
                .unwrap();
        assert_eq!(out.exit_code, 3);
        assert_eq!(out.output, "hello\nworld");
        assert!(!out.truncated);
        assert!(parse_run(format!("\n{OUT_MARKER}\nhello").as_bytes()).is_err());
    }

    #[test]
    fn a_screenshot_must_be_an_image() {
        let mut png = format!("\n{OUT_MARKER}\n").into_bytes();
        png.extend_from_slice(b"\x89PNG\r\n\x1a\nrest");
        assert_eq!(parse_screenshot(&png).unwrap().1, "image/png");
        let err = parse_screenshot(format!("\n{OUT_MARKER}\nerror: device offline").as_bytes())
            .unwrap_err();
        assert!(err.message.contains("device offline"), "{err:?}");
    }

    #[test]
    fn logs_from_a_physical_ios_device_are_unsupported() {
        let e =
            logs_script(&Target::IosDevice { udid: "D".into() }, 10, 60, None, None).unwrap_err();
        assert_eq!(e.code, codes::E_UNSUPPORTED);
        let s = logs_script(
            &Target::Adb { serial: "S".into() },
            50,
            60,
            Some("MyApp:D *:S"),
            Some("crash"),
        )
        .unwrap();
        assert!(
            s.contains("logcat -d -v threadtime -t 20000 'MyApp:D' '*:S'"),
            "{s}"
        );
        assert!(s.contains("grep -F -i -- 'crash'"), "{s}");
        assert!(s.contains("tail -n 50"), "{s}");
    }

    #[test]
    fn the_failure_line_is_read_back() {
        assert_eq!(
            failure(format!("noise\n{FAILED} adb is not installed on this host\n").as_bytes())
                .as_deref(),
            Some("adb is not installed on this host")
        );
        assert_eq!(failure(b"plain error"), None);
    }

    /// The scripts against stub tools: a fake `adb` on PATH answers like
    /// the real one, so the bash itself (quoting, PIPESTATUS, the marker)
    /// is exercised, not only the parsers.
    #[cfg(unix)]
    #[test]
    fn the_scan_and_run_scripts_work_against_a_stub_adb() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        let adb = bin.join("adb");
        std::fs::write(
            &adb,
            "#!/bin/bash\n\
             if [ \"$1\" = devices ]; then printf 'List of devices attached\\nR5CT1   device usb:1 model:Pixel_7 transport_id:1\\n'; exit 0; fi\n\
             if [ \"$1\" = -s ] && [ \"$3\" = shell ] && [ \"$4\" = getprop ]; then echo 14; exit 0; fi\n\
             if [ \"$1\" = -s ] && [ \"$3\" = shell ]; then shift 3; echo \"ran: $*\"; exit 7; fi\n\
             exit 1\n",
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&adb, std::fs::Permissions::from_mode(0o755)).unwrap();
        let run = |script: &str| {
            crate::proc::std_command("bash")
                .arg("-c")
                .arg(script)
                .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
                .env("HOME", dir.path())
                .env_remove("ANDROID_HOME")
                .env_remove("ANDROID_SDK_ROOT")
                .output()
                .unwrap()
        };
        let out = run(&scan_script());
        let scan = parse_scan(&out.stdout).unwrap();
        assert_eq!(
            scan.running.len(),
            1,
            "{}",
            String::from_utf8_lossy(&out.stdout)
        );
        assert_eq!(scan.running[0].os_version.as_deref(), Some("Android 14"));
        assert_eq!(scan.tools, ["adb"]);

        let t = Target::Adb {
            serial: "R5CT1".into(),
        };
        let out = run(&run_script(
            &t,
            &["shell".into(), "echo".into(), "a b".into()],
        ));
        let r = parse_run(&out.stdout).unwrap();
        assert_eq!((r.exit_code, r.output.as_str()), (7, "ran: echo a b"));

        // No xcrun on Linux: a simulator command fails with the reason.
        let sim = Target::Simulator { udid: "U".into() };
        let out = run(&run_script(&sim, &["launch".into(), "x".into()]));
        if !cfg!(target_os = "macos") {
            assert_eq!(out.status.code(), Some(5));
            assert!(failure(&out.stderr).unwrap().contains("xcrun"));
        }
    }

    #[cfg(unix)]
    #[test]
    fn the_copy_scripts_round_trip_a_folder() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let app = home.join("build/My App.app");
        std::fs::create_dir_all(&app).unwrap();
        std::fs::write(app.join("Info.plist"), "plist").unwrap();
        let run = |script: &str, stdin: &[u8]| {
            use std::io::Write;
            let mut c = crate::proc::std_command("bash")
                .arg("-c")
                .arg(script)
                .env("HOME", home)
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .unwrap();
            c.stdin.take().unwrap().write_all(stdin).unwrap();
            c.wait_with_output().unwrap()
        };
        let src = first_line(&run(&stage_dir_script("src1"), b"").stdout).unwrap();
        let dst = first_line(&run(&stage_dir_script("dst1"), b"").stdout).unwrap();
        let packed = run(&pack_script("~/build/My App.app", &src), b"");
        let body = String::from_utf8_lossy(payload(&packed.stdout).unwrap()).into_owned();
        let mut lines = body.lines();
        let size: u64 = lines.next().unwrap().parse().unwrap();
        let name = lines.next().unwrap().to_string();
        assert_eq!(name, "My App.app");
        let bytes = std::fs::read(format!("{src}/app.tgz")).unwrap();
        assert_eq!(bytes.len() as u64, size);
        let (a, b) = bytes.split_at(bytes.len() / 2);
        run(&append_script(&dst), a);
        run(&append_script(&dst), b);
        let unpacked = first_line(&run(&unpack_script(&dst, &name), b"").stdout).unwrap();
        assert_eq!(
            std::fs::read_to_string(format!("{unpacked}/Info.plist")).unwrap(),
            "plist"
        );
        run(&cleanup_script(&dst), b"");
        run(&cleanup_script(&src), b"");
        assert!(!std::path::Path::new(&dst).exists());
        assert!(!std::path::Path::new(&src).exists());
        // Cleanup refuses anything outside fleet's own staging folder.
        run(
            &cleanup_script(&home.join("build").display().to_string()),
            b"",
        );
        assert!(app.exists());
    }
}
