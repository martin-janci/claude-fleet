# Debug devices

Phones, emulators and simulators for testing apps, set up once on the host
they are attached to and used from every session that may see them. A
session on `build-box` can install an APK on a Pixel plugged into your Mac,
take a screenshot and read its log, without either host reaching the other:
every command runs on the device's own host, over the same SSH (or local
spawn) fleet runs everything else with.

## What is found

Each host is scanned with the tools it already has:

| Device | Found with | Shows as |
|---|---|---|
| Android phone or tablet (USB or `adb connect`) | `adb devices -l`, then each one's Android version | `android` · `physical` |
| Running Android emulator | the same, plus `adb emu avd name` | `android` · `emulator`, keyed by its AVD so it keeps its row across restarts |
| Stopped Android Virtual Device | `emulator -list-avds` | `bootable` in a scan, a row once it has run |
| Booted iOS simulator | `xcrun simctl list devices available --json` (macOS) | `ios` · `simulator` |
| Shut-down iOS simulator | the same | `bootable`, or `shutdown` once it has a row |
| iPhone or iPad paired with the Mac | `xcrun devicectl list devices` (Xcode 15+) | `ios` · `physical` |

`adb` and `emulator` are found on `PATH` or in the usual SDK folders
(`$ANDROID_HOME`, `$ANDROID_SDK_ROOT`, `~/Library/Android/sdk`,
`~/Android/Sdk`), since a login shell often lacks `platform-tools`.

A device that is unplugged stays in the list as **missing**, with its label
and sharing, until you forget it. A host's devices go with the host when it
is removed. Watch, TV and Vision simulators are not listed.

## The Debug devices page

The desktop's **Debug devices** page lists every device, newest scan first.
Opening it rescans any host whose last scan is more than five minutes old in
the background; **Rescan host** on a device scans its host now. On a device
you can:

- give it a **label** (what sessions call it: `{ device: "bench pixel" }`);
- turn on **Shared**, so sessions on the other hosts of its host's org may use
  it (off by default: only its own host's sessions do);
- **Release claim** when a session holds it and should not;
- **Start** / **Stop** an emulator or simulator;
- **Forget** it.

On a desktop paired with a hub the page shows the hub's devices: the hub
scans its hosts with its own SSH, and every change goes through the hub's
`debug_devices` tool.

## From a session

The control API's `debug_devices` tool (see `docs/control-api.md`), by
`action`:

| Action | What it does |
|---|---|
| `list { refresh? }` | The devices you may use, and each host's last scan. `refresh` scans first and waits |
| `scan { host? }` | Scan now; also answers the emulators and simulators that could be started (`bootable`) |
| `claim { device, claim_s?, note? }` / `release { device }` | Keep other sessions off a device while you test |
| `run { device, args, timeout_s? }` | One `adb`, `xcrun simctl` or `xcrun devicectl device` command |
| `install { device, path, host? }` | Install an app: `.apk`, or a simulator's `.app`, or `.app` / `.ipa` on an iPhone |
| `logs { device, lines?, filter?, contains?, since_s? }` | Recent log lines: `logcat -d`, or a simulator's `log show` |
| `screenshot { device }` | The screen, as an image the model can see |
| `boot { device }` or `boot { host, name }` | Start an emulator or simulator |
| `shutdown { device }` | Stop one |
| `configure { device, label?, shared? }`, `forget { device }` | A person's: from a paired device or the master token |

A device is named by its id, its label, its name, its serial or its key,
optionally prefixed with a host (`mac-mini/Pixel 8`). An ambiguous name
answers `E_AMBIGUOUS` with the candidates.

`run` takes a closed set of verbs, each argument quoted on the host, so a
call can drive the device but not the machine it is attached to:

- `adb`: `shell`, `uninstall`, `reboot`, `forward`, `reverse`, `get-state`,
  `get-serialno`, `get-devpath`, `emu`, `root`, `unroot`, `wait-for-device`.
  `push`, `pull` and `install` read or write paths on the host and are not
  allowed; `install` has its own action. `logcat` streams forever; `logs`
  takes a dump.
- `simctl`: `launch`, `terminate`, `openurl`, `uninstall`, `listapps`,
  `appinfo`, `get_app_container`, `privacy`, `ui`, `status_bar`,
  `location`, `erase`. Not `spawn`, which runs any process on the Mac.
- `devicectl device`: `info`, `process`, `uninstall`, `reboot`.

Output is capped at 256 KiB (`truncated` says so); a command runs at most 600 s.

`install` copies the app from the caller's host when it is not the device's
host: it is packed with `tar` there, relayed through the machine that runs
fleet in 8 MiB chunks, unpacked under `~/.cache/claude-fleet/devices/` on the
device's host, installed, and both staging folders are removed. A per-host
token installs from its own host only; up to 2 GiB packed. A host reached
through `fleet-agent` cannot receive a copy (the agent does not pipe stdin
yet), so build on the device's host or install from it.

## Who may use what

- **A person** (the desktop, the master token, a paired phone or desktop)
  sees the devices on every host its org scope sees, and is the only one who
  labels, shares and forgets them. A device bound to an org sees that org's
  hosts' devices (and unassigned hosts' while the org's
  `bound_sees_unassigned` is on).
- **A host's Claude** (a per-host token) sees its own host's devices, and
  devices a person marked **Shared** on other hosts with the same org (both
  without an org counts as the same). It scans only its own host, installs
  from its own host only, and cannot share a device to itself.
- **Claims** are advisory leases: while one holds (30 minutes by default, up
  to a day), every other caller gets `E_CONFLICT` naming the holder and its
  note. Each use by the holder extends it. A session that proves its tmux
  pane holds the claim as itself (`host:<alias>#<session>`), so two sessions
  on one host do not share a claim. Only the holder or a person releases
  one.

Sharing a simulator lets other hosts' sessions run apps on that Mac, since a
simulator app is a Mac process. That is what the Shared toggle's
confirmation says.

## Choices made in this first slice

- Inventory is per host and cache-first: no background tick scans hosts that
  have no devices; a list rescans hosts older than five minutes.
- Physical iOS logs are not read (use Console.app); a physical iPhone's
  screenshot needs `idevicescreenshot` (libimobiledevice) on its Mac.
- An emulator started by fleet runs without a window on a Linux host with no
  display.

## Not built yet

- **An adb bridge for native tools.** `gradle installDebug` or Android
  Studio on host B talking to the adb server on host A, through an `ssh -L`
  from fleet's machine to A chained to an `ssh -R` onto B (the way the MCP
  tunnel reaches hosts, `service/tunnel.rs`), so `ANDROID_ADB_SERVER_PORT`
  on B reaches A's devices. Today a session uses the devices through the
  tool only.
- Live screen streaming and input (taps, typing) beyond `adb shell input`
  and `simctl io`.
- The phone app (fleet-mobile) shows no device page yet; it reads the same
  `debug_devices` tool.
