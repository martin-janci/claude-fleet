# claude-fleet — core concepts

claude-fleet manages long-lived Claude Code processes running in tmux across multiple machines, presenting them as a unified fleet from a single Tauri 2 desktop app.

## Sessions

A session is a tmux window on some host that runs a Claude Code process; claude-fleet attaches to it, streams its output, and can send prompts or keystrokes. Sessions that should run unattended are created as **background sessions** (`kind: 'bg'`): they are headless, supervised by the reconcile tick, and never attach a terminal. Work sessions and background sessions share the same lifecycle model — create, peek, kill — but only work sessions get an interactive terminal view.

## Hosts & accounts

Hosts are discovered from `~/.ssh/config` plus a built-in `local` entry for the machine running the app. SSH connections to each remote host are multiplexed through a per-host `ControlMaster` socket so subsequent commands reuse the same authenticated channel without re-negotiating. The app probes each host's `~/.claude.json` to surface the signed-in account (email, org, tier); it reads this file for display purposes only — no credentials are extracted or stored by claude-fleet.

## Projects & worktrees

Each host has a **projects base**, the directory that holds its repositories, set in Settings → Projects. The layout under that base is either `github` (`<base>/<owner>/<repo>`) or `flat` (`<base>/<repo>`). When a host has no base configured, the default is `~/projects/github.com` (`~/projects` for `flat`). On the local machine the `CLAUDE_FLEET_PROJECTS_BASE` environment variable is checked before that default. The app scans the local base for repositories and their git worktrees. For a remote host it derives the project directory from that host's base and clones the repository on first use. Sessions are associated with the project whose working directory they were started from, so the UI can group and filter sessions by repository and branch.

## Control API & tunnels

An embedded MCP server (disabled by default, bound to `localhost`, protected by a bearer token, default port 4180) exposes the full fleet API so an AI assistant can drive sessions programmatically — creating sessions, sending prompts, reading output. When the control API is enabled, **reverse SSH tunnels** (`ssh -R`) forward that localhost port to each remote host's localhost, allowing remote agents to call back to the central server. Each tunnel is supervised: if the `ssh` process exits it restarts with capped exponential backoff. See [control-api.md](control-api.md) for the full tool reference.

## The terminal

The in-app terminal is a hand-rolled ANSI screen-buffer renderer (`src/lib/ansi.ts` + `TerminalView.svelte`), not xterm.js. xterm.js was tried first but its renderer silently no-ops after the first write in the Tauri 2 + macOS WKWebView environment, producing a blank terminal. The custom renderer covers the escape-sequence surface area that tmux and Claude's TUI actually emit — SGR colors, cursor positioning, clear-screen/line, basic scrolling — and renders into a plain DOM node where repaint is reliable. The trade-off is fewer features: no mouse tracking, no application keypad, no scrollback beyond the visible window. Only one PTY is attached at a time.

---

Going deeper: see [../CLAUDE.md](../CLAUDE.md) for architecture details and conventions, and [specs/](specs/) for in-progress design documents.
