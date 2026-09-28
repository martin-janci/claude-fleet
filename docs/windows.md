# claude-fleet on Windows

On Windows, claude-fleet is a **desktop client** for a fleet of Linux and macOS
hosts. Windows itself is not a fleet host, because it has no tmux. A WSL
distribution on the same machine can be one, though (see
[WSL distributions as hosts](#wsl-distributions-as-hosts)).

| On Windows | |
|---|---|
| The app, its database, Settings, the work graph | yes |
| The Control API (MCP) | yes |
| SSH to Linux / macOS hosts, sessions in their tmux | yes |
| The terminal (ConPTY running `ssh.exe -tt … tmux attach`) | yes |
| Hub-client mode (a window onto a `fleet-hub`) | yes, and the recommended setup |
| WSL distributions on this machine as hosts (`wsl-<name>`) | yes, no sshd needed, on a **standalone** desktop only (not while paired with a hub, see below) |
| The `local` host | no: switched off, as on a hub with `hub.local_host=false` |
| `fleet-agent`, `fleet-hub` | no: both run on Linux (see [hub.md](hub.md)) |

The implementation plan, and what is still open, is
[`superpowers/plans/2026-09-27-windows-desktop.md`](superpowers/plans/2026-09-27-windows-desktop.md).

## Install

Download `claude-fleet_<version>_x64-setup.exe` from the
[releases page](https://github.com/martin-janci/claude-fleet/releases). It
installs for the current user only and needs no administrator rights.

The installer is **not code-signed** yet, so Microsoft Defender SmartScreen
stops the first run with *"Windows protected your PC"*. Click **More info →
Run anyway**. Check the download against `SHA256SUMS` first (see the
README's [Verify what you downloaded](../README.md#verify-what-you-downloaded)):

```powershell
Get-FileHash .\claude-fleet_*_x64-setup.exe -Algorithm SHA256
```

The app renders through WebView2, which ships with Windows 11 and current
Windows 10. The installer downloads it if it is missing.

## Prerequisites

- **The OpenSSH Client.** It is built into Windows 10 (1809 and later) and
  Windows 11. Check with `ssh -V` in PowerShell. If it is missing, turn it on
  in **Settings → System → Optional features → OpenSSH Client**. The app
  says so in the host probe when it cannot find `ssh`.
- **Key-based SSH to every host**, with no password prompt: the app runs
  `ssh` with `BatchMode=yes`. For a key with a passphrase, start the agent
  once (an administrator PowerShell), then add the key:

  ```powershell
  Get-Service ssh-agent | Set-Service -StartupType Automatic
  Start-Service ssh-agent
  ssh-add $env:USERPROFILE\.ssh\id_ed25519
  ```

- **Hosts in `%USERPROFILE%\.ssh\config`.** That is the file Windows'
  `ssh` reads, and the one the app discovers hosts from.

On each host, `claude` and `tmux` are needed as on any other platform (see
[getting-started.md](getting-started.md)).

## WSL distributions as hosts

If WSL is installed, each of its distributions shows up in **Add a host** as
`wsl-<name>`, for example `wsl-ubuntu-22.04`. This is a **standalone-desktop
feature**: a desktop paired with a hub lists the hub's hosts, and the hub
(on Linux) cannot reach a distribution on your Windows machine. See
[WSL and hub-client mode](#wsl-and-hub-client-mode). Once the distribution has
`tmux` and Claude Code installed, it works like any other host: sessions,
the terminal, move and transfer. There are two differences:

- **No SSH.** fleet reaches the distribution through
  `wsl.exe --distribution <name>`, so there is nothing to set up: no sshd,
  no keys, no `~/.ssh/config` entry.
- **Hooks depend on WSL networking.** Claude Code's hooks inside the
  distribution report back to fleet on `127.0.0.1`. That address is this
  Windows machine only under WSL1, or under WSL2 with mirrored networking
  (`networkingMode=mirrored` in `%USERPROFILE%\.wslconfig`, Windows 11 22H2
  or later). Under WSL2's default NAT networking, sessions still run, but
  turn events and the Control API's per-host features do not arrive.
  **Provision hosts** (Settings → Control API) checks this from inside the
  distribution: when the hooks cannot reach the desktop, the distribution's
  row says so ("hooks can't reach the desktop … enable WSL mirrored
  networking"). After changing `.wslconfig`, run `wsl --shutdown` and
  provision again.

The distributions are detected when the app starts, in the background: the
first `wsl.exe` after a reboot starts the WSL service, which can take several
seconds, and the window does not wait for it. A command for a `wsl-` host
that comes in before detection has finished waits for it, up to 20 seconds.
If that first detection timed out, or a `wsl-` host is not in the table (a
distribution installed or renamed since), the next command for it detects
again and waits for that: at most every 10 seconds after a detection that
timed out, every minute after one that answered. **Add a host** lists what
the last detection found; restart the app to list a new distribution there
at once.

Commands start in the distribution user's home directory (`wsl.exe --cd
~`), as they would over SSH. When `wsl.exe` leaves `$SHELL` unset, the
toolchain probe asks the passwd database for your login shell, so the PATH
your `.bashrc` / `.zshrc` sets up (`~/.local/bin`, where Claude Code
installs) is found.

### WSL and hub-client mode

A desktop paired with a hub is a window onto that hub: its host list, its
sessions, its commands all come from the hub, and host discovery on the
desktop is switched off. The `wsl-` hosts exist only on the desktop that
found them, so a paired desktop shows none. To run sessions in WSL, either
use the desktop standalone (unpaired), or install tmux, Claude Code and an
sshd in the distribution and add it to the hub like any other Linux host.

Each distribution needs `bash`, `tmux` and Claude Code. Alpine's default
image has no `bash`: `apk add bash tmux` first.

An alias you already defined in `~/.ssh/config` (for example a `wsl-ubuntu`
that reaches an sshd inside WSL) stays an SSH host; fleet does not shadow it.
Docker Desktop's internal distributions are never offered.

## Git Bash, MSYS2 and Cygwin

A machine with Git for Windows, MSYS2 or Cygwin usually has more than one
`ssh.exe`. Each reads a different home directory and talks to a different
ssh-agent, so the first `ssh` on `PATH` is not a safe choice.

- **The default is the Windows OpenSSH,**
  `%SystemRoot%\System32\OpenSSH\ssh.exe`, whenever it is installed. It
  reads `%USERPROFILE%\.ssh` and uses the Windows `ssh-agent` service. The
  terminal, every probe and the tunnels all use this same program.
- **To use another build,** set `CLAUDE_FLEET_SSH` to its full path, for
  example `C:\cygwin64\bin\ssh.exe` or `C:\Program
  Files\Git\usr\bin\ssh.exe`, and restart the app. Quotes around the
  value, as Explorer's *Copy as path* adds them, are fine. If the path does
  not exist, the app logs a warning at startup. With a non-system `ssh`,
  fleet's host discovery also reads `%HOME%\.ssh\config` **when the app
  itself sees a `HOME` environment variable holding a Windows path** (for
  example a user variable `HOME=C:\cygwin64\home\<you>`). An app started
  from the Start menu normally has no `HOME`, so by default only
  `%USERPROFILE%\.ssh\config` is read. The `ssh` you chose still reads its
  own config when it connects; only the **Add a host** list is affected.
  Set `HOME` as a user environment variable, or copy the `Host` entries into
  `%USERPROFILE%\.ssh\config`, to see them there.

Host discovery reads your ssh config the way `ssh` does. Every alias on a
`Host a b` line is offered. `Include` files are followed, relative to
`.ssh`, with `*` globs. For a host defined more than once, the first value
wins. `Match` blocks give their values to no host.

Whichever `ssh` you use must be able to log in without a prompt
(`BatchMode=yes`), with its own agent or with an unencrypted key.

## SSH without multiplexing

On macOS and Linux, the app keeps one SSH connection per host open
(`ControlMaster`) and runs every probe, listing and tmux command through it.
Windows' OpenSSH cannot do that: it has no ControlMaster support. On Windows
the app therefore opens a **new SSH connection for every command**. Everything
works, but each refresh pays a full SSH handshake per host, so the sidebar
updates more slowly with many hosts or a slow link.

The fix is **hub-client mode**. Run `fleet-hub` on a Linux machine (see
[hub.md](hub.md)) and pair the desktop with it in **Settings → Hub**. The
hub does all the SSH work from Linux, with multiplexing. The Windows app
then opens SSH only for the terminal you are looking at.

## The terminal

The terminal pane runs `ssh.exe` (or `wsl.exe`) in a Windows pseudo console
(ConPTY). The installer ships Microsoft's current ConPTY, `conpty.dll` and
`OpenConsole.exe` from the Windows Terminal project (MIT), next to
`claude-fleet.exe`. The app uses that copy instead of the one built into
Windows. The built-in one, on Windows 10 especially, redraws the screen
itself and drops tmux's bracketed-paste and mouse modes. Without those, a
multi-line paste into Claude submits at its first line and the mouse wheel
does nothing in the pane.

When you build from source, `pnpm tauri dev` uses the built-in ConPTY. For
the shipped behaviour, run `bash scripts/fetch-conpty.sh` once, then
`pnpm tauri dev --config src-tauri/tauri.conpty.conf.json`. portable-pty
loads `conpty.dll` from the directory of the running `.exe`
(`src-tauri\target\debug\` for a dev build), and Windows looks for
`OpenConsole.exe` next to that DLL: if either is missing there, the
built-in ConPTY is used without an error. The first terminal logs which
one loaded (`[pty]` in the log).

## Where things are kept

| What | Where |
|---|---|
| Database, logs | `%LOCALAPPDATA%\rlt\claude-fleet\data` |
| SSH scratch (cache) | `%LOCALAPPDATA%\claude-fleet` |
| Hub client token | Windows Credential Manager, generic credential `claude-fleet/hub-client-token` |

Builds before this one kept the database in the Roaming profile
(`%APPDATA%\rlt\claude-fleet\data`). It holds tracker credentials and
per-host tokens, and a Roaming profile is copied to a server at sign-out on
a domain machine, so the first start of this build moves it (with its
`-wal` / `-shm`, the logs and anything else there) to the Local profile. If
the move fails, nothing is moved, the app keeps using the old place, and
the log says why.

The hub client token never goes into the database. Disconnect in Settings →
Hub removes it; you can also remove it from **Control Panel → Credential
Manager → Windows Credentials**.

## Known gaps

- The installer is unsigned (SmartScreen warns, see above).
- The terminal on ConPTY has had no long manual test yet: resizing, pasting
  large blocks and fast session switching are the places to watch. Please
  report what you see.
- There is no auto-update on any platform yet; install a new release over
  the old one.
