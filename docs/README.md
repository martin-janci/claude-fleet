# claude-fleet documentation

A Tauri 2 desktop app for managing long-lived Claude Code sessions in tmux across multiple machines over SSH.

- **[Getting Started](getting-started.md)** — install, add a host, first session.
- **[Concepts](concepts.md)** — sessions, hosts, projects, Control API, the terminal.
- **[Fleet v obrazoch](fleet-v-obrazoch.md)** — diagrams (in Slovak): standalone desktop, with a hub, with a phone, and how they compare.
- **[Conversation view](conversation-view.md)** — read a session's transcript and prompt it from the app, one of the Session tab's two views. **[Chat blocks](chat-blocks.md)**: the cards an agent's reply can hold (task reports, tutorials, guides, choices, forms).
- **[Work](work-graph.md)** — what each session is working on: tickets and trackers, detection, start and resume, handover, Today, tidy-up, orgs, and every `work.*` setting.
- **[Troubleshooting](troubleshooting.md)** — common problems and fixes.
- **[Windows](windows.md)** — the desktop as a client on Windows: what works, OpenSSH, hub-client mode.
- **[Control API](control-api.md)** — enable the MCP control server; **[reference](control-api-reference.md)** (generated).
- **[Voice relay](voice.md)** — talk to a remote session with `/voice` through the app's microphone.
- **[fleet-hub](hub.md)** — run the fleet headless as a daemon, without the desktop app.
- **[Hub acceptance](hub-acceptance.md)** — the live acceptance record for the hub, paired clients, the agent and hub-client mode.
- **[Releasing](RELEASING.md)** — versioning & changelog automation.
- **[Buildkite builder](buildkite.md)** — the persistent builder that runs an agent's full verification (`scripts/verify.sh remote`), and how to set it up.

**For contributors:** start with [CLAUDE.md](../CLAUDE.md) for repo orientation, [Architecture](architecture.md) for how each subsystem is put together and [Status](status.md) for what is landed or off, then browse [specs/](specs/) for per-iteration design documents and [plans/](plans/) for implementation plans.
