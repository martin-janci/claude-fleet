# Getting started with claude-fleet

claude-fleet is a Tauri 2 desktop app for managing long-lived Claude Code sessions running in tmux across multiple machines over SSH. This guide walks you from a fresh install to your first active session.

For background on the core concepts (hosts, sessions, projects, the Control API), see [concepts.md](concepts.md).

---

## Prerequisites

### Local machine

- **`claude` CLI** installed and on your `PATH` (run `claude --version` to verify).
- **`tmux`** installed and on your `PATH`.
- A **projects directory** laid out as `~/projects/github.com/<owner>/<repo>`. This is the default; override it by setting the environment variable `CLAUDE_FLEET_PROJECTS_BASE` to any absolute path before launching the app.

### Remote hosts

- Each remote machine must be reachable by **key-based SSH** (no password prompt).
- The host must have an entry in `~/.ssh/config` — that is how the app discovers it.
- `claude` and `tmux` must be installed on the remote machine (the app probes for them when you add the host).

---

## Install & launch

Clone the repository, then:

```bash
pnpm install
pnpm tauri dev
```

Packaged binaries will be available in a future release; for now, run from source.

**First launch on an empty fleet:** The app shows the "Welcome to claude-fleet" dialog.

- Click **"Let's set up →"** to open the guided setup checklist in the sidebar.
- Click **"Skip for now"** to close the dialog and leave the checklist available in the sidebar whenever you are ready.

The dialog only appears once, on first launch, when no hosts have been added yet.

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

Scans the projects path (default `~/projects/github.com`) and registers every `<owner>/<repo>` directory it finds. Click the row to re-scan after adding repositories. The sublabel shows how many projects were found.

### Enable Control API (optional)

Starts a localhost-only MCP server that lets an AI assistant drive the fleet. It is **off by default**. Click the row to enable it.

Once enabled, the checklist shows the port and a masked bearer token with a **Copy config** button. The default port is **4180** and the endpoint is `http://127.0.0.1:4180/mcp`. You can change the port and regenerate the token in Settings → **Control API (MCP)**.

See [control-api.md](control-api.md) for a full reference.

### Create first session

The finish line. Click the row to open the new-session picker, choose a host and project, and start your first Claude Code session running in tmux.

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
- **Send a prompt** — type in the prompt bar to send text to the active session. To send the same prompt to several sessions at once, use the broadcast feature.
- **Background sessions (⚡)** — sessions marked with ⚡ run without an attached terminal. They continue working while you watch other sessions.
- **Files, diffs, commit graph, branches** — the sidebar panels give you a read-only view of the repository state on the host where the session is running.
- **Filter** — use the host picker and recency filter in the sidebar to narrow the session list when you manage many machines.

For a deeper explanation of how hosts, sessions, projects, and the event bus fit together, see [concepts.md](concepts.md). If something is not working as expected, see [troubleshooting.md](troubleshooting.md).
