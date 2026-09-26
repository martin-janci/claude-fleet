# claude-fleet documentation

A Tauri 2 desktop app for managing long-lived Claude Code sessions in tmux across multiple machines over SSH.

- **[Getting Started](getting-started.md)** — install, add a host, first session.
- **[Concepts](concepts.md)** — sessions, hosts, projects, Control API, the terminal.
- **[Conversation view](conversation-view.md)** — read a session's transcript and prompt it from the app, one of the Session tab's two views.
- **[Work](work-graph.md)** — what each session is working on: tickets and trackers, detection, start and resume, handover, Today, tidy-up, orgs, and every `work.*` setting.
- **[Troubleshooting](troubleshooting.md)** — common problems and fixes.
- **[Control API](control-api.md)** — enable the MCP control server; **[reference](control-api-reference.md)** (generated).
- **[fleet-hub](hub.md)** — run the fleet headless as a daemon, without the desktop app.
- **[Releasing](RELEASING.md)** — versioning & changelog automation.

**For contributors:** start with [CLAUDE.md](../CLAUDE.md) for repo orientation, then browse [specs/](specs/) for per-iteration design documents and [plans/](plans/) for implementation plans.
