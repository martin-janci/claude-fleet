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

### See a host's profiles

Every reconcile pass reads `~/.claude-profiles/*` on each host and the login
in each (`hosts.claude_profiles`, migration 114; `list_hosts` returns it).
Host details lists them as **Login profiles**, with the account each is
logged into, or "not logged in". Each logged-in profile's account appears in
Accounts like a host's login.

### Start a session under a profile

- Desktop: New session → **Login profile**: pick one of the host's profiles
  or type a new name (empty = the host's login).
- MCP: `new_session { …, profile: "work" }`.

A profile that does not exist yet is created on first use, and the session
asks for `/login` in its pane. Log in there once; later sessions under the
same name on that host reuse it.

### Add an account

Accounts → **+ Add account** (M15 step G2.9; MCP: `add_account`) makes a
new profile on a host, signed in one of two ways. Bedrock and Vertex are
not offered yet.

**Claude subscription.** Fleet opens a login pane on the host: a tmux
session of its own, `fleet-login--<name>`, running `claude /login` with
`CLAUDE_CONFIG_DIR` at the new profile (its `.claude.json` marks onboarding
done, so the pane opens on the login choice). Sessions lists never show
it. The dialog shows the pane's last lines, the sign-in link it printed
(**Open sign-in page**, **Copy link**), its numbered choices and Enter / ↑ /
↓ as buttons, and a field for the code the sign-in page hands out, which
is pasted into the pane from stdin. It reads the host's profiles every two
seconds and turns **Done** on once the host reports the profile logged in;
Done and Cancel close the pane. **Run it there instead** gives the command
for a terminal on the host,
`CLAUDE_CONFIG_DIR=~/.claude-profiles/<name> claude /login`, which works
whatever the CLI's own flow becomes. MCP: `start_login`, `login_status`
(`logged_in`, `pane`, `sign_in_url`, `command`), `login_key { key }` (1–9,
Enter, Up, Down, Escape, Tab), `login_code { code }`, `end_login`.

**API key.** Fleet asks Anthropic whether the key works (`GET /v1/models`
from this machine, to `api.anthropic.com` only); a refusal shows the
provider's own 401 or 403 on the key field. The key then goes to the host
on stdin, never in an argv, into `~/.claude-profiles/<name>/.fleet-api-key`
(mode 600). Beside it, `.fleet-account.json` names the account the profile
is listed as: `apikey-` and 16 hex of the key's SHA-256, so the same key on
two hosts is one account. Fleet keeps no copy of the key, writes it to no
log, audit line or reply, and changing it means adding it again (the next
session then asks once, in its pane, whether to use the new key). A
session under the profile exports the key as `ANTHROPIC_API_KEY`
(`tmux::PROFILE_API_KEY`, read with the shell's own `read`). A name that is
already a `/login` profile on that host is refused. The usage poll skips
API-key accounts: they have spend, not usage windows.

**Daily limit.** An API-key account can carry a daily limit in USD (the
form's field; MCP: `add_account { action: "daily_limit", daily_limit_usd }`,
0 clears it), kept in the `accounts.daily_limits` setting. When the
account's spend today (UTC, the Accounts page's per-account roll-up)
reaches it, a start or relaunch under any login on that account is refused
with `E_ACCOUNT_LIMIT`, whatever `over_limit_ok` says, and automation leaves
the login alone. A session already running is not stopped, and spend is
booked as transcripts are read, so the limit can be passed by what is
running when it is reached.

### Switch a running session

Desktop: session details → **Login**, pick the login, **Switch**, confirm.

MCP: `restart_session { session_id, profile: "work" }` restarts the session,
resuming its conversation under that login, and stores the new profile once
the relaunch succeeded (a failed one leaves the row as it was).
`profile: ""` switches back to the host's login; leaving `profile` out keeps
the current one. Restart, recreate, repair, rewind and move all relaunch
with the stored profile (`sessions.claude_profile`, migration 113).

A moved session keeps its profile name; on the target host that name is a
separate profile, which asks for its own `/login` the first time.

### Accounts and usage

A session under a profile is attributed to its profile's account, never the
host's; until the host reports the profile logged in, its account stays
empty. Switching drops the old link and the next pass sets the new one.
A session on the host's own login keeps its account while it runs, and a
relaunch (restart, recreate, repair, rewind) drops it, so the next pass
takes the account the host is logged into now.

The usage poll asks a profile for its account's usage like a host: the
script runs with `CLAUDE_CONFIG_DIR` at the profile and reads that
profile's `.credentials.json`. The source shows as `<host> (<profile>)`. A
host's own login is asked before its profiles. On macOS, where Claude Code
keeps the token in the Keychain, fleet cannot read it, as for a host login.
