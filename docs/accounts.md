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

Because the profile's `.claude.json` is its own, so are its user-scope MCP
servers. Provisioning a host (not the `content_only` refresh) merges fleet's
`claude-fleet` entry into every profile's `.claude.json`; a profile added
since then has no fleet MCP server until the host is provisioned again, and
a session picks the entry up only when it starts.

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

### Switch automatically

Off by default. The owner turns it on for the fleet in Settings › Limits
(`accounts.auto_switch`); from then on, a session that shows **Paused ·
limit** (a live Claude session, not working, not waiting on a dialog or a
form and not stuck, whose account has its 5-hour or weekly window fully
used) is resumed under the login on its host with the most headroom, without anyone pressing Switch account. It is the same
restart Switch account makes (`restart_session { profile }`), so the
conversation carries on and the row keeps the new login.

Each person decides for their own sessions: the Accounts page's **Switch my
sessions automatically when their account runs out** (MCP:
`auto_switch_account { enabled }`) is on by default and turns it off for
every session that person owns. The owner's answer covers the owner's
sessions and every session nobody owns.

**Switch before the limit** (`accounts.auto_switch_early`, off by default,
shown once the fleet switch is on) also moves an idle session once its
account passes `accounts.pause_at`, so its next turn does not stall at the
limit. A working session is never moved, early or not.

Below it, **then tell it to continue where it stopped** (MCP:
`auto_switch_account { continue_after: true }`, off by default): when the
limit cut the session's last turn off (its newest turn end is a
`stop_failure` of class `rate_limit`), a switch that went through queues
"Continue where you left off." as a deferred prompt. It goes in no sooner
than 30 seconds after the switch, once the resumed session is idle, and is
held if the new login is over `accounts.pause_at` too. A session that had
finished its turn is only resumed.

With it on, a paused row says where the session will go ("Moves to work
(ada@…) automatically", with **Switch now**), or that no other login on its
host has room, offering only Wait.

Which login: another account on the same host (a second login on the same
account shares its limit), not signed out, with a usage reading under
`accounts.pause_at` taken in the last hour, not at a limit and not past its
daily spend limit, the least used first. An account takes one session per
wave: one per run, and the next only after a usage reading newer than that
move, so a dozen sessions on a spent account spread out instead of all
landing on the same spare login and spending it too. With none, nothing moves and the row waits for a person. With
`decide.jev.limit_switch` on, Jev is asked the same question and its
answer is recorded beside the rule's (docs/decisions.md); the rule's pick
is the one used.

Bounds: the pass runs on the reconcile tick at most once a minute, switches
at most five sessions a run, leaves a session it switched or tried alone
for 30 minutes, skips the fleet controller, Control (whose own failover
moves it and tells it) and sessions on a hidden or unreachable host, stands still while Pause all is on, and runs only on the
hub or a standalone desktop, never on a desktop paired to a hub. Every
attempt leaves `account_switched` or `account_switch_failed` on the
session's timeline (logins by profile, accounts by uuid); the desktop shows
it in the conversation and as a toast, one toast for all when several
sessions moved at once.

A session Claude Code parked at its limit with a quota notification
(`quota_auto_resume_disabled`, or `quota_auto_resume_stale` waiting for
Enter) reads as blocked, but it is no dialog: it is switched like an idle
one. A permission prompt or a question raised after it is the newer word,
and that session waits for its person.

### Where new work runs

`service::placement` answers where new work should run: every host and
login that can take it, best first, and why the others cannot
(`suggest_placement` over MCP). A host must answer, have claude, be under
the recovery thresholds (low memory, memory pressure, CPU saturated) and,
when a project is named and some host has run it, have run it too; a login
must be on a known account, signed in, under `accounts.pause_at` and within
its daily limit. The rest rank by the account's unused share, then the
host's idle CPU and free memory; the host the work ran on before wins a
near tie. Control is told to call it before it starts anything, a mission's
run takes the best place inside its grant (its hosts, its login when it
names one), a start that names no host takes the best host that has run the
project and the login placement ranked first there (a start rule's or
placement rule's login, or `profile` on `work_link { start }`, wins), and
the over-limit refusal names a login on another host when none on its own
has room. `dispatch_task { new_worker }` takes `profile` too, so Control can
start the worker on the login a refusal named.

A resume (`work_link { resume }`, Resume in the desktop) runs under the
login `profile` names (`""` = the host's own); without one, under the past
session's login when its row is still known and the resume lands on the
same host, else the host's own login, or the login on the host with
headroom when the host's own is past `accounts.pause_at`. Neither a ticket
start nor a resume refuses an over-limit login: they are how Control
recovers work, so a spent login is the session's to report.

Automation leaves a login alone that is past `accounts.pause_at` or that
fleet knows is signed out: a routine's fire goes to its fallback host, else
skips and says why (with the reset time, or the sign-in command); a
mission's run waits. A `restart_session { profile }` onto another login is
refused for a signed-out login, and for one past the line unless
`over_limit_ok` says the person chose it.

### Signed-out logins

A login whose sign-in is gone (the usage check found it expired or its
token rejected, or a headless `claude -p` run was refused with "Login
expired · Run /login") is kept on its host row
(`hosts.signed_out_logins`) with the day fleet first saw it, until fleet
sees it signed in again; one signed-out login is asked again at most once
an hour. Placement leaves it out, Control's chat shows one card per login
("Claude login expired on htz (since Aug 9) · ssh htz then claude /login";
for a profile, `CLAUDE_CONFIG_DIR=~/.claude-profiles/<name> claude
/login`), and Control's feed says it once. Only a person can sign it in.

### Two copies of one login

Every OAuth refresh hands out a new refresh token and revokes the old one.
Two copies of the same login on one host refresh on their own schedules, so
the first to refresh leaves the other with a revoked token, and its next
refresh fails with "Login expired · Please run /login". Signing in again
only starts the next round. Fleet flags such a host in Hosts (⚿) and in
`fleet_health` (`hosts[].login_conflicts`, `service::login_conflicts`):

- **Keychain and file** (macOS). Claude Code keeps the default login in the
  login Keychain, but a `claude` started over SSH, as fleet's tmux sessions
  are, finds the Keychain locked and falls back to
  `~/.claude/.credentials.json`. The desktop app or a local terminal then
  refreshes one copy and fleet's sessions the other. The health probe asks
  whether both exist (`security find-generic-password` without `-w`, so
  never the secret; migration 203, `hosts.login_split`).
- **Shared account.** The host's login and a login profile, or two
  profiles, signed into the same account.

Fix it by keeping one login per account on a host: run fleet's sessions on
that host under a profile on another account, or sign the desktop app (or
the duplicate profile) into another account.

### Control's own login

`operator_status` reports the login Control runs under, its account and
usage, and "Paused · limit" or "signed out". A Control born on a host takes
the login it had there, else the best signed-in login with headroom on it.
With `control.switch_at_limit` on (the default), a Control at its limit or
signed out, and not mid-turn, is resumed under another login with headroom
on its own host (the same conversation: profiles share `projects/`), else
started on another host under the best login there (the host-down move: its
conversation stays behind). It gets a `[fleet]` note either way, and the
switch is on its timeline. Tried at most every 30 minutes.

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
