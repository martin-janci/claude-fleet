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
`claude-fleet` entry into every profile's `.claude.json`, and with it every
user-scope MCP server of the host's own `~/.claude.json` (Jira, a company's
servers…; the host's entry wins over a profile's of the same name, a server
only the profile has is kept). A mission's grant does the same for the
logins its runs will use, once per grant (the access check, below). A
login added through + Add account gets them once the host reports it
signed in (`login_status`); a profile made by hand has neither until one of
those runs, and a session picks the servers up only when it starts.

An OAuth server's sign-in is not shared: Claude Code keeps it with the
login (`.credentials.json`, or the Keychain on macOS), so such a server asks
to be signed in once under each login
(`CLAUDE_CONFIG_DIR=~/.claude-profiles/<name> claude`, then `/mcp`).

**The access check.** When a mission's grant lets the loop act, the loop
looks once where the grant's runs will go (its hosts, else placement's pick
for each repo) and under which login, on provisioned hosts: it shares the
host's MCP servers into that login, names a server signed in on the host's
login but not on this one, asks each repo's `origin` for its `HEAD`, and
compares the hosts' organisation with the tasks'. The mission's log gets one
`access_check` event; anything a person must do is one ask card.

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

**Setup token.** For a host without a browser, or a login that keeps
expiring: run `claude setup-token` on any computer signed in to the
subscription and paste the `sk-ant-oat…` token it prints (MCP: `add_account
{ action: "setup_token", setup_token, account? }`). Fleet asks Anthropic
whether it works (`GET /api/oauth/profile`, and when the token's scopes do
not reach it, `GET /v1/models`, from this machine to `api.anthropic.com`
only); a refusal is the provider's own 401 or 403 on the token field. The
token then goes to the host on stdin, never in an argv, into
`~/.claude-profiles/<name>/.fleet-oauth-token` (mode 600), and a session
under the profile exports it as `CLAUDE_CODE_OAUTH_TOKEN` (rung 5 above,
ahead of the profile's `/login`; an `ANTHROPIC_API_KEY` or
`ANTHROPIC_AUTH_TOKEN` from the host's shell is dropped for that
`claude`). Fleet keeps no copy and writes it to no log, audit line or reply.

The profile is listed as the token's account: the one Anthropic names when
the token may read its profile, else the one picked under **Same account
as** (a subscription fleet knows from a login, so the token shares its
limits and usage readings), else the profile's existing `/login`, else an
account of its own (`setuptoken-` and 16 hex of the token's SHA-256).
`.fleet-account.json` carries it, with `"fleetCredential": "setup_token"`,
and `list_hosts` shows the profile with `credential: "setup_token"`.

- **Replace** a token (they last a year): the same call with the same name.
  Running sessions keep the old one until they restart.
- **On a `/login` profile** (say one that is signed out): the token goes in
  front of that login and the profile keeps its account; its signed-out
  note is cleared.
- **Never on an API-key profile**: refused (`E_EXISTS`).
- **Usage**: a setup token is inference only, so its profile is never a
  usage source; the account's 5-hour and weekly readings come from a
  `/login` of the same account elsewhere, if there is one. Nor is a token
  beside a `/login` of the same account a login conflict: it has no refresh
  token to revoke.
- **Signed out** (the token expired or was revoked): the login card, the
  refusal of automation and the Accounts page say "make a new token with
  `claude setup-token`" instead of `claude /login`, which does not touch the
  exported token; the Accounts page copies `claude setup-token` to run. The
  hourly recheck of signed-out logins skips the profile (a usage fetch
  cannot read a token), so the note goes when a new token replaces it. A
  headless run's refusal, which has no host row to tell, names both fixes.

Desktop: Accounts or Host details → **+ Add account** → *Sign in with*
**Setup token**; Host details → Login profiles → **Setup token…** opens it
preset.

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
the current one. Restart, recreate, repair (and the attach path that
recreates a dead pane) and rewind all relaunch with the stored profile
(`sessions.claude_profile`, migration 113), and all are refused past that
account's daily limit (`run_login::refuse_relaunch`). A restore skips such a
session with the reason, and auto-restore defers it (`daily_limit`) without
spending an attempt, until the limit resets.

A move keeps the login by account, not by name. On the target it takes the
same profile name when that profile is signed in to the same account there;
else the target's signed-in login on that account (the host's own login
first); else the same name when the target has a signed-in profile of that
name on another account; else the target's own login, or, when that one is
signed out, forbidden or spent, the signed-in, allowed login there with the
most headroom. A signed-out login, one on an account the session's project
forbids and one whose daily limit is spent are skipped at every step, and a
move never lands on an empty profile that asks for `/login`; with no login
left it is refused (`E_ACCOUNT_FORBIDDEN` naming `suggested_profile`,
`E_ACCOUNT_LIMIT`, or the sign-in command) before anything is carried, and
again when a deferred move runs. A session already running on an account
its project forbade later, whose account the target has a login on, is
refused rather than moved onto another account. When fleet does not know the session's account,
the profile name is kept as it was, held to the same refusals.

### Switch automatically

Off by default. The owner turns it on for the fleet in Settings › Limits
(`accounts.auto_switch`); from then on, a session that shows **Paused ·
limit** (a live Claude session, not working, not waiting on a dialog or a
form and not stuck, whose account has its 5-hour or weekly window fully
used) is resumed under the login on its host with the most headroom, without anyone pressing Switch account. It is the same
restart Switch account makes (`restart_session { profile }`), so the
conversation carries on and the row keeps the new login.

The same goes for a session whose own login is **signed out** (the
"no credentials" rows, *Signed-out logins* below): it is resumed under
another login on its host, chosen as below except that another login of
the same account counts too (a setup token beside an expired `/login`,
say), and never the signed-out one. Fleet learns a login is signed out from
the usage check, a refused headless run, the session's pane, or a
StopFailure hook whose error is a 401 / `authentication_error`. The
timeline event's detail says `"reason": "signed_out"` (else `"limit"`).

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
`stop_failure` of class `rate_limit`, or of class `auth` for a signed-out
login), a switch that went through queues
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
daily spend limit, the least used first. For an account no session in the
fleet runs on, a reading up to a day old counts: nothing under such a login
refreshes its access token, so after a few idle hours the usage check reads
"access token expired" and its number stops moving, as its use does. The
first session sent there refreshes the token. An account takes one session per
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
same host, else the host's own login; either gives way to the login on the
host with headroom when it is past `accounts.pause_at`. A ticket start on a
named host that names no login does the same with the host's own
(`account_limits::login_with_headroom_for`). Neither a ticket start nor a resume
refuses an over-limit login, and neither moves off a login the caller or a
rule named: they are how Control recovers work, so a spent login is the
session's to report.

Automation leaves a login alone that is past `accounts.pause_at` or that
fleet knows is signed out: a routine that names no login, whose host's own
login is past the line, runs under the login on that host with the most
headroom (as a ticket start does; the run's note names it); otherwise a
routine's fire goes to its fallback host, else skips and says why (with the reset time, or the sign-in command). On the
fallback a routine that names its login runs under a login there on the
same account (the same name first), never an empty profile or another
account, and one on its host's own login takes the fallback's own login or
the profile there with the most headroom; either is held to the project's
forbidden accounts, the daily limit, the sign-in and the pause line there,
and so is the fallback a failed start goes to. A ticket start or a resume
that names no login steps off a login fleet knows is signed out onto the
signed-in, allowed one with the most headroom on the host; a
mission's run waits. A `restart_session { profile }` onto another login is
refused for a signed-out login, and for one past the line unless
`over_limit_ok` says the person chose it. A switch from a person's paired
device (Switch account on a hub-paired desktop or the phone, org-bound or
not) is the person's choice; the operator, a per-host token and the master
token are held.

### Keep an account off a project

A person can forbid an account for some projects: on the Accounts page,
the account's **Not for these projects** list (the phone's account sheet has
the same list). MCP: `set_account_projects { uuid, forbidden_project_ids }`,
which replaces the list; the account's row carries it as
`forbidden_project_ids` (`account_project_blocks`, migration 217). Only the
hub owner changes it, as with Add account: the master or a trusted full
device. Control cannot, so it cannot lift the block that refused its start.

A whole org can be forbidden too (**Forbid for a whole org…** under the same
list; the phone ticks "All of <org>"): every project of the org, those added
later included, is treated as if it were on the list.
`set_account_projects { uuid, forbidden_org_ids }` replaces that list
(`forbidden_org_ids` on the row, `account_org_blocks`, migration 218); each
of the two lists is replaced only when it is sent. A project's org is its own
rule, the most specific org rule on its owner, repo or path
(`Store::project_org`), and, for work on a given host, also the org a
session there is filed under: a host-qualified rule or the host's own org
(`Store::project_orgs_on`). The start, switch, move and relaunch checks know
the host and count both; a pick made before a host is chosen sees only the
project's own. The refusal then says "(all of <org>)". Removing the org removes its blocks.

No work in a forbidden project runs on a login on that account
(`service::project_accounts`):

- a start under such a login is refused with `E_ACCOUNT_FORBIDDEN`, from
  every caller (the desktop, MCP and hub `new_session`, a ticket start, a
  resume, a routine or mission run, `dispatch_task`); the refusal names the
  allowed login on the host with the most headroom
  (`details.suggested_profile`, `""` for the host's own), and no flag
  passes it;
- Switch account (`restart_session { profile }`) onto such a login is
  refused; a plain restart of a live session keeps the login it already
  runs under, as long as that login is still on the account the session ran
  on. A restart, recreate, repair or restore that brings a lost session
  back, or relaunches it under a login signed in to another account since,
  is refused the same way;
- a review session and a moved session take an allowed login on the host
  or are refused; a background session, a fleet agent's run (both bill the
  host's own login) are refused or skipped; a mission's headless check
  moves to an allowed login or runs as a session instead;
- where fleet picks the login, it never picks one: placement leaves it out
  ("the project forbids its account"), a ticket start or a resume that names
  no login moves to the allowed login on the host with the most headroom,
  a routine on its host's own login moves to an allowed login under the
  pause line (and within its daily limit) there, else skips its run and
  says why, as one that names a forbidden login does (a fallback host
  takes a run its host cannot, not one the rule forbids), a move takes an allowed
  login on the target or is refused, the over-limit refusal and
  `check_account_headroom { project_id }` never suggest one, and the
  automatic switch never moves a session onto one;
- Claude work that does not go through `new_session` holds to the same rule,
  and to the daily limit and a known sign-out, through one guard
  (`service::run_login`): a review session runs under the reviewed
  session's login (stepped off a forbidden or spent one, and stored on the
  review's row) or is refused; a background session (the host's own login)
  is refused; a mission's headless review or test runs under the grant's
  login, else the implementer's, moved to an allowed one, or takes the
  session path, whose start refuses; the AI check on done and a brief draft
  do the same or are refused before anything runs; the mission planner and
  its release-note and triage drafts never pick such a login (with none
  left the planner refuses and records no limit wait); and a PR-reviewer,
  support or fleet-agent job is skipped while the rule host's own login is
  forbidden, and waits (deferred, its attempt given back) while it is
  signed out, over its daily limit or past `accounts.pause_at`.
  `run_login_tests` lists every launch path.

A profile that is not logged in yet has no account, so a start under it is
not refused; once the host reports its account, the next start, resume or
switch is. Forbidding never stops a session already running on the
account: it changes where work goes next.

### Signed-out logins

A login whose sign-in is gone (the usage check found it expired or its
token rejected, a headless `claude -p` run was refused with "Login
expired · Run /login", or a session's pane ended its last turn in Claude
Code's own error line for it) is kept on its host row
(`hosts.signed_out_logins`) with the day fleet first saw it, until fleet
sees it signed in again; one signed-out login is asked again at most once
an hour. Placement leaves it out, Control's chat shows one card per login
("Claude login expired on htz (since Aug 9) · ssh htz then claude /login";
for a profile, `CLAUDE_CONFIG_DIR=~/.claude-profiles/<name> claude
/login`), and Control's feed says it once. Only a person can sign it in.

**Sign in again** (on the card, on the session's row, and on the Accounts
page beside Copy sign-in command) opens + Add account's sign-in step over
the login that is there: the login pane runs `claude /login` under the
profile, or with no `CLAUDE_CONFIG_DIR` for the host's own login (pane
`fleet-login--_own`). It is done when the pane says "Login successful"
(the profile had its account all along, so that says nothing); the login's
signed-out note then goes at once, so its card clears and placement takes
it again. A setup-token profile's button is **New token…** (+ Add account
preset to a new token for it); an API-key profile has no `/login` to renew
and is refused. MCP: the login actions take `again: true` (`start_login`,
`login_status`, `login_key`, `login_code`, `end_login`), `profile: ""` for
the host's own login. Signing in on one host does not fix a copy of the
same login on another: each copy refreshes on its own (*Two copies of one
login*).

The pane reading (`pane_intel::detect_signed_out`, every reconcile pass) is
strict, since a session may be printing or discussing these words: only the
pane's last `⎿` line counts, and only when it opens with `API Error: 401`,
`Login expired`, `Invalid API key`, `Not logged in`, `OAuth token has
expired` / `has been revoked` or `Invalid bearer token`, with no answer
(`●`) below it. A `/login` run there afterwards ("⎿  Login successful") or a
later turn ends it. The login noted is the session's profile, else its
host's own; a session reconcile sees for the first time notes nothing. Its
sessions then read `no_credentials` (Needs you) while they are not working.

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
- **Copied login**, across hosts. One `.credentials.json` copied onto
  another host, or into another profile: a dotfiles or `~/.claude` sync,
  `rsync`, a copied profile dir. Both copies hold one refresh token, so the
  same race runs between hosts. The health probe reads each login's
  refresh token into a shell variable, hashes it with SHA-256 on the host
  and prints only its first 12 hex digits (`loginfp=`; migration 228,
  `hosts.login_fingerprints`). The store keeps each login's last three, and
  one login's newest fingerprint among another's marks both: the older ones
  keep the mark after the first copy refreshes, until the other is signed
  in on its own. `local`'s own login is left out, for the same reason as
  Keychain and file below; its profiles are not.

Fix it by keeping one copy per login on a host. For Keychain and file, run
fleet's sessions on that host under a login profile, then delete
`~/.claude/.credentials.json`: the probe only asks whether both copies
exist, so the mark stays until one is gone. `local` is never flagged, since
fleet's local tmux server starts in the login session and reads the
Keychain. For a shared account, sign the duplicate profile into another
account. For a copied login, sign each host in on its own (Sign in again)
and stop the copying: leave `.credentials.json` out of whatever syncs
`~/.claude` or the profiles. An API key under two profiles is not a
conflict: it has no refresh token to revoke.

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
