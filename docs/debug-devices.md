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

Settings › Organisations › **Debug devices** opens with the **device
board**: every device as a card, grouped by host, with its state (and what
a device that is not ready needs), who holds it and for how long, and who
waits in its line (your own place too). Filter by state or platform and
search by name, host or model; the board reloads every 30 seconds while it
is on screen and the window has focus, and **Rescan all** scans every host now. On a card:
**Screenshot** (a thumbnail; click it for the full size), **Claim** or
**Wait in line** (with an optional note), **Leave line**, **Release**,
**Mirror** / **Stop mirror**, **Logs**, and **Start** / **Stop** for an
emulator or simulator. On a paired desktop your own claims are held under
your device's name, which the board cannot tell from another person's, so
it offers them as anyone's (Release asks first).

Under the board, the generated **Debug devices** page lists every device,
newest scan first, and edits it.
Opening it rescans any host whose last scan is more than five minutes old in
the background; **Rescan host** on a device scans its host now. On a device
you can:

- give it a **label** (what sessions call it: `{ device: "bench pixel" }`);
- turn on **Shared**, so sessions on the other hosts of its host's org may use
  it (off by default: only its own host's sessions do);
- **Claim** it while you test by hand, with an optional note on what for
  (sessions then get `E_CONFLICT` until the claim lapses or is released);
  with **Wait in line if someone holds it** on, a held device puts you in
  its queue instead (see *Queue*), and **Waiting** counts who is in line;
- **Install app…**: an `.apk`, `.app` or `.ipa` by its path on the device's
  own host, or on another host you pick (copied across first); the install's
  output is shown;
- **Logs**: its last 200 log lines, shown on the page (cut when longer);
- **Screenshot**: the screen as it is now, shown on the page;
- **Mirror** / **Stop mirror**: open or close a live scrcpy window of an
  Android device (see *Mirror*);
- **Release claim** when a session holds it and should not (the next in
  line gets it);
- **Start** / **Stop** an emulator or simulator;
- **Forget** it.

On a desktop paired with a hub the page shows the hub's devices: the hub
scans its hosts with its own SSH, and every change goes through the hub's
`debug_devices` tool.

## Mirror

**Mirror** opens a native `scrcpy` window for an Android phone or emulator,
with its input, on the machine you are sitting at. iOS is not supported: on a
physical iPhone the button shows and is refused (`E_UNSUPPORTED`), and
**Stop mirror** shows on every Android and physical row.

`scrcpy` and `adb` must be installed on THIS machine
(`brew install scrcpy android-platform-tools`). They are looked up in
`FLEET_SCRCPY` / `FLEET_ADB`, then `PATH`, then `/opt/homebrew/bin` and
`/usr/local/bin`, and for adb also `ANDROID_HOME` / `ANDROID_SDK_ROOT`,
`~/Library/Android/sdk`, `~/Android/Sdk` and `~/Android/sdk`. Neither needs to exist on the
device's host beyond its adb server.

Two routes, chosen from where the device is:

- **local**: the device is on host `local` and this desktop is standalone.
  scrcpy talks to this machine's own adb server.
- **ssh**: the device is on another host that this machine can reach with
  `ssh <alias>` from its own `~/.ssh/config` (in hub-client mode the alias
  comes from the hub). scrcpy talks to the host's adb server and to the
  device's forward port through two loopback relays, each an
  `ssh -W 127.0.0.1:<port> <alias>`.

In hub-client mode a device on host `local` (the hub's own machine) answers
`E_NO_ROUTE`: that needs the hub relay, which is not built yet. So do
devices on an agent host or a WSL host, which this machine has no ssh route
to; they wait for the hub relay too.

Mirror waits about 5 s (scrcpy's start-up grace) before it reports success.

While a mirror runs it claims the device with the note `scrcpy mirror`
(30 minutes, renewed every 10). The claim is released when the window closes
or on **Stop mirror**. On a standalone desktop, a claim you already held
when you pressed Mirror is kept exactly as it was (expiry and note) and is
not released when the mirror ends. On a paired desktop the mirror takes the
claim and releases it when it ends, until the hub can tell a client its own
claims. If the mirror's claim lapses (the laptop slept) and someone else claims
the device, the mirror leaves their claim alone. Mirroring a device that is
already mirrored is refused (`E_EXISTS`) and leaves the running mirror's
claim alone. If the app quits, scrcpy is killed and the claim lapses on its
own.

Before it starts, the mirror checks the host's adb server: it asks it
`host:version` and compares the answer with this machine's `adb version`. A
difference is refused (`E_ADB_VERSION_MISMATCH`, naming both versions),
because an adb client of another version kills the server it finds and
restarts its own, which would drop the host's other adb users. When the
server does not answer, the mirror runs `adb start-server` on the host over
ssh; if that fails too the result is `E_NO_ROUTE`.

scrcpy needs a forward on the host. The mirror takes the first free port in
27183-27199 of that host (from `host:list-forward`, minus this machine's own
live mirrors), and removes the forward (`killforward`) when the mirror ends.
With all 17 taken the answer is `E_LIMIT`.

**Security.** The relays bind `127.0.0.1` only and exist only while a mirror
runs. adb has no authentication, so another local user of this machine could
connect to them during that window and drive the device. A relay only ever
reaches `127.0.0.1` on the allowlisted ports of the device's own host.

| Code | Meaning |
|---|---|
| `E_TOOL_MISSING` | `scrcpy` or `adb` not found on this machine |
| `E_ADB_VERSION_MISMATCH` | the host's adb server and this machine's adb differ |
| `E_NO_ROUTE` | no way to reach the device: hub-local device, agent or WSL host, host adb server down and not startable |
| `E_MIRROR_FAILED` | scrcpy exited non-zero within the 5 s start-up grace; the message ends with its output |
| `E_UNSUPPORTED` | an iOS device |
| `E_INVALID_STATE` | the device is not ready, or has no adb serial yet |
| `E_EXISTS` | it is already mirrored |
| `E_CONFLICT` | someone else holds its claim |
| `E_LIMIT` | all forward ports on the host are taken |
| `E_NOTFOUND` | an unknown device id, or **Stop mirror** on a device that is not mirrored |

## From a session

The control API's `debug_devices` tool (see `docs/control-api.md`), by
`action`:

| Action | What it does |
|---|---|
| `list { refresh? }` | The devices you may use, and each host's last scan. `refresh` scans first and waits |
| `scan { host? }` | Scan now; also answers the emulators and simulators that could be started (`bootable`) |
| `claim { device, claim_s?, note?, queue? }` / `release { device }` | Keep other sessions off a device while you test; with `queue`, wait in line for a held one (see *Queue*) |
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

## In the chat

A session's work with devices shows in its Conversation view, desktop and
phone:

- **Each `debug_devices` call** is a row naming what it did
  (`screenshot · bench pixel`, `wait in line · #3`). Expanded, it shows the
  device as the call left it (state, holder, line), a list as a list, a
  command's output as output, and the screenshot it took as an image
  (`session_tool_detail` answers a result's images: at most 2, of 4 MiB
  base64 each; any tool's images show the same way).
- **The device strip**, under the conversation's header, lists the devices
  the session holds (state, time left, its note) and those it waits for
  (its place, who holds them), with **Screenshot**, **Mirror** and
  **Release…** on a held one. Every strip on screen shares one read of the
  list, every 90 seconds while the window has focus and again when a strip
  opens or releases a device; it shows nothing when the session holds and
  waits for nothing.
- **A `device` card** an agent writes in its reply
  (`{"spec": "fleet.ui/1", "kind": "device", "device": "bench pixel"}`,
  `docs/chat-blocks.md`) draws that device live from your own list, with
  **Screenshot** and **Mirror**.

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
  note. A person may still read a held device, `screenshot` and `logs`,
  without touching the claim, so the chat can show what a session tests on;
  another session may not. Each use by the holder extends it. A session that proves its tmux
  pane holds the claim as itself (`host:<alias>#<session>`), so two sessions
  on one host do not share a claim. Only the holder or a person releases
  one.

## Queue

Two sessions that both want the bench phone do not have to poll `claim`
until one wins. `claim { device, queue: true }` on a device someone else
holds puts the caller in the device's line and answers the device with:

- `queue`: who waits, first in line first (`holder`, `note`, `queued_at`);
- `waiting`: how many;
- `queue_place`: the caller's own place (1: next).

Asking again keeps the place (and updates its note). On a free device
`queue` changes nothing: the caller gets the claim at once.

Whenever the device is read (a list, a resolve, any action), a claim that
was released or has lapsed passes to the first in line, with that entry's
note, for 10 minutes. The new holder's first use extends it as any claim's
use does (to at least 30 minutes ahead), and a `claim` sets the length it
asks for. So a waiter whose session went away holds the rest of the line up
for at most those 10 minutes. A session that wants to know when its turn
came reads `list` or `claim { queue: true }` again: `claimed_by` is then
itself.

`release` from the holder (or a person) gives the claim up, and the next in
line gets it. `release` from someone waiting in the line of a device another
holds only leaves the line, a person's too: the claim stays its holder's (a
person who means to take it away releases again, once out of line). A place
lapses after a day; a forgotten device takes its line with it.

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
  on B reaches A's devices. The mirror's loopback relay prepares this; today
  a session uses the devices through the tool only.
- **Mirror through the hub and agents.** A device on the hub's own machine
  (hub-client mode) needs a `/relay` WebSocket on the hub, and a host
  reached through `fleet-agent` needs TCP stream frames in `fleet-proto`.
  Until then those answer `E_NO_ROUTE`.
- Input beyond scrcpy's window: taps and typing from a session stay
  `adb shell input` and `simctl io`.
- The phone app (fleet-mobile) has its own device sheet and the same chat
  cards, over the same `debug_devices` tool; it does not mirror.
- No push when a turn comes: a waiting session reads the device again to
  find it holds the claim.
