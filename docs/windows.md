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
| WSL distributions on this machine as hosts (`wsl-<name>`) | yes, no sshd needed |
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
`wsl-<name>`, for example `wsl-ubuntu-22.04`. Once the distribution has
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

The distributions are detected when the app starts. Restart the app after
installing a new one.

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
  Files\Git\usr\bin\ssh.exe`, and restart the app. fleet then also reads
  the `~/.ssh/config` under that environment's `HOME` (Cygwin's
  `C:\cygwin64\home\<you>`), besides the one in your Windows profile.

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
`pnpm tauri dev --config src-tauri/tauri.conpty.conf.json`.

## Where things are kept

| What | Where |
|---|---|
| Database, logs | `%APPDATA%\rlt\claude-fleet\data` |
| SSH scratch (cache) | `%LOCALAPPDATA%\claude-fleet` |
| Hub client token | Windows Credential Manager, generic credential `claude-fleet/hub-client-token` |

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
