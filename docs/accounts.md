# Claude accounts and login profiles

Which Claude login a session bills, and how to run sessions on one host under
more than one login. Design background:
the multi-account brainstorm (2026-10-07) and
`docs/superpowers/specs/2026-09-30-multi-user-gap-analysis.md` §2.5.

## What Claude Code uses

A `claude` process picks its credential at start, in this order:

1. a cloud provider (`CLAUDE_CODE_USE_BEDROCK` / `_VERTEX` / `_FOUNDRY`),
2. `ANTHROPIC_AUTH_TOKEN`,
3. `ANTHROPIC_API_KEY`,
4. an `apiKeyHelper`,
5. `CLAUDE_CODE_OAUTH_TOKEN` (a `claude setup-token` token),
6. `ANTHROPIC_PROFILE`,
7. the `/login` stored in its config dir: `$CLAUDE_CONFIG_DIR`, else
   `~/.claude` (with `~/.claude.json`).

A running `claude` never changes its credential. Switching a session to
another login means stopping it and resuming the conversation under the other
one, which is what fleet does.

## The host's login

Fleet reads each host's login from `$CLAUDE_CONFIG_DIR/.claude.json` (else
`~/.claude.json`) and shows it in Hosts. When a variable from rungs 1–6 is
set in the host's shell or tmux environment, Hosts flags the host (⚿): new
sessions there bill that credential, not the login. Fleet records the
variable's name only, never its value.

## Login profiles

A login profile is a named Claude config dir on a host,
`~/.claude-profiles/<name>`, with its own `/login`. A session started under
a profile runs `claude` with `CLAUDE_CONFIG_DIR` set to that dir, so it bills
that login. Everything else is shared with `~/.claude`: each visible entry
(`projects/`, `settings.json` with fleet's hooks, skills, commands, …) is
symlinked into the profile on every launch, unless the profile has its own.
The dotfiles are not linked, so `.credentials.json` and the profile's
`.claude.json` (its login and onboarding state) stay its own. On macOS the
Keychain entry is per config dir too.

Sharing `projects/` is what lets a conversation move between logins: the
transcript is in the same place under every profile, so `--resume` finds it.

A profile name is letters, digits, `_` and `-`, starting with a letter or
digit, at most 32 characters (`validate::claude_profile`).

### Start a session under a profile

- Desktop: New session → **Login profile**, type the name (empty = the host's
  login).
- MCP: `new_session { …, profile: "work" }`.

A profile that does not exist yet is created on first use, and the session
asks for `/login` in its pane. Log in there once; later sessions under the
same name on that host reuse it.

### Switch a running session

`restart_session { session_id, profile: "work" }` stores the new profile and
restarts the session, resuming its conversation under that login.
`profile: ""` switches back to the host's login; leaving `profile` out keeps
the current one. Restart, recreate, repair, rewind and move all relaunch
with the stored profile (`sessions.claude_profile`, migration 112).

A moved session keeps its profile name; on the target host that name is a
separate profile, which asks for its own `/login` the first time.

### Accounts and usage

A session under a profile does not take the host's account: its account link
stays empty until fleet learns the profile's login. Reading each profile's
account and usage is not built yet (see `docs/status.md`).
