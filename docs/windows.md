# claude-fleet on Windows

On Windows, claude-fleet is a **desktop client** for a fleet of Linux and macOS
hosts. It is not a fleet host itself: Claude Code sessions live in tmux on the
hosts, and Windows has no tmux.

| On Windows | |
|---|---|
| The app, its database, Settings, the work graph | yes |
| The Control API (MCP) | yes |
| SSH to Linux / macOS hosts, sessions in their tmux | yes |
| The terminal (ConPTY running `ssh.exe -tt … tmux attach`) | yes |
| Hub-client mode (a window onto a `fleet-hub`) | yes, and the recommended setup |
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
