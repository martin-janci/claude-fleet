# Getting started with claude-fleet

claude-fleet is a Tauri 2 desktop app for managing long-lived Claude Code sessions running in tmux across multiple machines over SSH. This guide walks you from a fresh install to your first active session.

For background on the core concepts (hosts, sessions, projects, the Control API), see [concepts.md](concepts.md).

---

## Quickstart: one host

The shortest path to a working fleet is this machine plus one remote host, such as a VPS. Your repositories do not need to follow any particular layout.

### 1. Build and launch the app

No release has been published yet, so for now run claude-fleet from source. You need Node with pnpm, a stable Rust toolchain, and the Tauri system libraries. The [README](../README.md) lists the exact versions.

```bash
git clone https://github.com/martin-janci/claude-fleet.git
cd claude-fleet
pnpm install
pnpm tauri dev
```

The app drives the `claude` CLI and `tmux`, so both must be on your local `PATH` (`claude --version`, `tmux -V`).

Once releases are published (see [RELEASING.md](RELEASING.md)), you will be able to download a build from the [releases page](https://github.com/martin-janci/claude-fleet/releases) instead: `.dmg` or `.app.tar.gz` on macOS, `.AppImage` or `.deb` on Linux. Those builds are not code-signed:

- **macOS:** Gatekeeper blocks the first launch. Right-click the app and choose **Open**, or clear the quarantine flag with `xattr -d com.apple.quarantine /Applications/claude-fleet.app`.
- **Linux:** mark the AppImage executable (`chmod +x`) before running it, or install the `.deb` with `sudo apt install ./claude-fleet_*.deb`.

### 2. Add one host

The remote host needs:

- **key-based SSH** with no password prompt,
- an entry in `~/.ssh/config`, which is how the app discovers it,
- `claude` and `tmux` installed. The app checks for both when you add the host.

On first launch the app shows the "Welcome to claude-fleet" dialog. Click **"Let's set up →"** to open the **Get started** checklist in the sidebar, or **"Skip for now"** to open it later. The dialog appears once, and only while no hosts have been added.

In the checklist, click **Add a host**, pick the alias from your `~/.ssh/config`, and confirm with **Add** once the probe shows the `claude` and `tmux` versions.

### 3. Set the host's projects base

Open **Settings → Projects**. Pick a layout, then enter the directory that holds the repositories for each host:

| Layout | Where a project lives | Example base |
|---|---|---|
| `github` (default) | `<base>/<owner>/<repo>` | `~/projects/github.com` |
| `flat` | `<base>/<repo>` | `~/code`, for `~/code/my-app` |

- A path must be absolute or start with `~/`. The `~/` part is expanded against that host's home directory.
- The line under each field previews where a project ends up.
- Click **Save & rescan** to apply.

A host left blank uses the default: `~/projects/github.com` (`~/projects` with the `flat` layout). On this machine, the `CLAUDE_FLEET_PROJECTS_BASE` environment variable is checked before that default, so existing setups keep working unchanged.

Repositories on this machine are found by scanning the base. On a remote host, the app clones `git@github.com:<owner>/<repo>.git` into that host's base the first time you start a session there. The host therefore needs GitHub SSH access for any repository that is not already present.

### 4. Create a session

Click **Create first session** in the checklist, or use the new-session button in the sidebar. Choose the host and a project, then start. Claude Code launches in a tmux session on that host and the terminal attaches to it.

---

## Guided setup — the "Get started" checklist

The sidebar shows a **Get started** card with a progress counter (**{n} of {m} done**) and one row per step. Click a row to act on that step. When all required steps are complete, the card shows **"You're all set 🎉"** and a **Dismiss** button. You can re-open the checklist at any time via Settings → **Replay setup guide**.

### Local prerequisites

Checks that `claude`, `tmux`, and the projects path are all present and readable on your local machine. Click the row to re-run the check after fixing anything that is missing. A sublabel shows the detected versions once all three pass.

### Add a host

Opens the **Add SSH host** picker, which scans `~/.ssh/config` for candidate aliases. Click an alias to probe it — the app connects over SSH, checks `claude` and `tmux` versions, and reads the logged-in Claude account. Confirm with **Add** to register the host. Add your local machine as well if you want to manage local sessions.

### Provision & tunnels

Installs the fleet skills and MCP configuration on every registered host. Click the row to run provisioning.

The tunnel badge reflects the current state:

| Badge | Meaning |
|---|---|
| `tunnel: starts with Control API` | Host is provisioned, but the Control API is not enabled — the tunnel starts when you enable it. |
| `tunnel: up` | Control API is enabled and the SSH reverse tunnel is established. |
| `tunnel: down — retrying` | Control API is enabled but the tunnel has not connected yet; the app retries automatically. |

### Pick projects

Scans this machine's projects base and registers every repository it finds in the configured layout. The base is set in **Settings → Projects** and defaults to `~/projects/github.com`. Click the row to re-scan after adding repositories. The sublabel shows how many projects were found.

### Enable Control API (optional)

Starts a localhost-only MCP server that lets an AI assistant drive the fleet. It is **off by default**. Click the row to enable it.

Once enabled, the checklist shows the port and a masked bearer token with a **Copy config** button. The default port is **4180** and the endpoint is `http://127.0.0.1:4180/mcp`. You can change the port and regenerate the token in Settings → **Control API (MCP)**.

That token is the **master token**, for the assistant you configure yourself. Hosts use their own tokens instead. **Provision hosts** (Settings → Control API) creates a separate **per-host token** for each host, `local` included, and writes only that token into the host's `~/.claude.json` and hook config. A per-host token can only act as sessions on its own host, and it cannot run fleet-admin tools such as `add_host` or `provision_hosts`. That way a token copied from one machine cannot pose as another. You can set each host's token to `full` or `readonly` in the **Token** column of Settings → **Hosts**. After upgrading from a build without per-host tokens, re-provision every host. See [Per-host tokens](control-api.md#per-host-tokens).

See [control-api.md](control-api.md) for a full reference.

### Create first session

The finish line. Click the row to open the new-session picker, choose a host and project, and start your first Claude Code session running in tmux.

The dialog offers a generated name ("blue sirius") so you never have to invent one — press **🎲** or **Ctrl/⌘+R** to roll another, type over it to use your own, and **Enter** to create. The name becomes the session label, the branch/worktree slug (`blue-sirius`) and the tail of the tmux name (`dev-<owner>-<repo>--blue-sirius`); leave the tmux name empty and fleet picks one. The dialog remembers the host, worktree and type you last used per project.

---

## Feature hints

The first time you use certain features, a small bubble appears near the relevant UI element with a short explanation. Dismiss a hint with **Got it** or **✕**; it will not appear again.

Manage hints in Settings:

- **Show feature hints** — toggle the hint system on or off globally.
- **Reset hints** — marks all hints as unseen so they show again from the beginning.

---

## Everyday use

Once you have at least one session running:

- **Attach** — click a session row to open the live terminal view and watch the session in real time.
- **Quick switcher (⌘K / ⌘P on macOS, Ctrl+Shift+K / Ctrl+Shift+P on Linux and Windows)** — works even while the terminal has focus; plain Ctrl+K and Ctrl+P still go to the terminal (readline kill-line / previous history). Type any part of a session's name, project, host, branch or status; recently opened sessions come first. **Enter** attaches and reveals the session in the sidebar (its project is expanded and the row scrolled into view), **Ctrl/⌘+Enter** opens the new-session dialog with what you typed as the name, and the "New session in <project>" rows start one for that project.
- **Send a prompt** — type in the prompt bar to send text to the active session. To send the same prompt to several sessions at once, use the broadcast feature.
- **Background sessions (⚡)** — sessions marked with ⚡ run without an attached terminal. They continue working while you watch other sessions.
- **Files, diffs, commit graph, branches** — the sidebar panels give you a read-only view of the repository state on the host where the session is running.
- **Filter** — use the host picker and recency filter in the sidebar to narrow the session list when you manage many machines.

For a deeper explanation of how hosts, sessions, projects, and the event bus fit together, see [concepts.md](concepts.md). If something is not working as expected, see [troubleshooting.md](troubleshooting.md).
