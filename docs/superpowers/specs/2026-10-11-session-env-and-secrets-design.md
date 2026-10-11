# Session environment variables and secrets — design

Date: 2026-10-11 · Status: design approved, spec under review

## Problem

Fleet has no way to give a session environment variables. A session working
in a repository that needs `DATABASE_URL`, `GITHUB_TOKEN` or a test API key
gets them only if someone put them on the host by hand. The secret handling
that exists is narrow: chat-form `secret` fields (`docs/forms.md` →
*Secrets*), tracker and decision credentials, catalog secrets for asset sync,
and a profile's `ANTHROPIC_API_KEY`.

## Goal

One mechanism that sets environment variables for every session fleet
launches, scoped `global` < `host` < `project` < `session` (narrower wins).
An entry is either a plain value or a secret. Secrets never appear in an
argv, a tmux environment, a log, the MCP audit or a listing.

## Non-goals (v1)

- Encryption at rest. `env_vars` stores values like `tracker_secrets`,
  `decision_secrets` and `catalog_secrets` do (plaintext in `state.db`).
  Encrypting at rest is one later change across all four tables.
- `op://` (1Password) references, resolved on the host.
- A per-worktree scope.
- Changing a running session's environment. Edits take effect on the
  session's next launch (create, restart, recreate, move); the UI says so.

## What exists and is reused

- **The pane command** is built by `tmux.rs` `pane_command_with` with
  `ClaudeLaunch { model, effort, profile }`. `profile_prefix()`
  (`tmux.rs:1857`) exports `CLAUDE_CONFIG_DIR` inline, not via tmux `-e`,
  because `respawn-pane` takes no environment.
- **`PROFILE_API_KEY`** (`tmux.rs:1789`) reads a 0600 file into a variable,
  so the key is never in an argv. This design generalises that pattern.
- **`provision::write_host_file_secret`** (`provision.rs:1268`) writes a
  0600 file locally, over SSH (`upload_file`, content on stdin) and over
  fleet-agent (Upload frame). No value appears in an argv on any path.
- **`credential_ref`** (`env:NAME`, `file:/abs`) as tracker and decision
  secrets use it, resolved by the controlling process.
- **`forms::sweep_secret_dirs`** (`forms.rs:607`) as the model for removing
  leftover files from the tick.
- **`logging::redact_secrets`** with a `*_secret_literals()` feed, as
  `Store::decision_secret_literals` (`decisions.rs:898`) does.

Why not the alternatives:

- **tmux `-e` for plain values**: `respawn-pane` (restart) cannot pass an
  environment. It also leaves values in `tmux show-environment`.
- **Inline `export NAME=value` in the pane command**: the pane command is in
  argv on every path. The `sh -c '<pane_cmd>'` parent of `claude` stays in
  `ps` for the session's whole life.
- **fleet-agent resolving variables on the host**: covers agent hosts only;
  most hosts are plain SSH.

## Design

### 1. Data — migration `NNN_env_vars.sql`

Take `NNN` from `origin/main` when the migration is written (172 is the
highest at the time of writing).

```sql
CREATE TABLE env_vars (
  id             INTEGER PRIMARY KEY,
  scope          TEXT NOT NULL CHECK (scope IN ('global','host','project','session')),
  scope_key      TEXT NOT NULL,          -- '' | host alias | 'owner/repo' | session id
  name           TEXT NOT NULL,
  value          TEXT,
  credential_ref TEXT,                    -- 'env:NAME' | 'file:/abs'
  secret         INTEGER NOT NULL DEFAULT 0,
  updated_at     INTEGER NOT NULL,
  UNIQUE (scope, scope_key, name),
  CHECK ((value IS NULL) <> (credential_ref IS NULL)),
  CHECK (scope <> 'global' OR scope_key = '')
);
```

- `project` keys on `owner/repo`, matching `projects UNIQUE(owner, repo)`.
  A project variable therefore applies on every host.
- Session-scope rows are deleted with their session (trigger on `sessions`
  delete).
- A reference entry counts as secret whatever its `secret` flag: what it
  resolves to is never listed.

### 2. Validation (service layer, `E_INVALID_INPUT`)

- Name: `^[A-Za-z_][A-Za-z0-9_]{0,127}$`.
- Reserved names are refused, because fleet sets them or the session depends
  on them: `PATH`, `HOME`, `SHELL`, `USER`, `TERM`, `COLORTERM`, `LANG`,
  `LC_*`, `TMUX`, `TMUX_*`, `CLAUDE_CONFIG_DIR`, `ANTHROPIC_API_KEY`,
  `CLAUDE_FLEET_*`. The list is one constant shared by validation and its
  tests.
- Value: ≤ 32 KiB. The resolved set for one session: ≤ 256 KiB. A NUL byte
  is refused.
- `credential_ref`: only `env:` and `file:/` (absolute path).

### 3. Store — `crates/fleet-core/src/store/env_vars.rs`

Every statement that reads `env_vars` lives here. A test pins the count of
statements naming the table, as the `tracker_secrets` test does.

- `set_env_var(scope, scope_key, name, Value | Ref, secret)` — upsert.
- `unset_env_var(scope, scope_key, name)`.
- `list_env_vars(scope?, scope_key?) -> Vec<EnvVarRow>`. Each row holds
  name, scope, scope_key, `secret`, `is_ref`, `updated_at`, and `value`
  **only** when the entry is neither secret nor a reference. Secrets are
  write-only.
- `env_for_launch(host, owner_repo?, session_id) -> Vec<(String, EnvSource)>`
  merges the four scopes, narrower winning. `EnvSource` holds a value as the
  redacting `Secret` newtype, which has no `Serialize`.
- `env_secret_literals() -> Vec<String>` feeds `logging::redact_secrets`.
  It returns stored secret values only; resolved references are added by the
  resolver for the call that resolved them.

### 4. Launch

New `service/env.rs`:

1. `resolve(store, host, project, session) -> Result<EnvFile, IpcError>`
   reads `env_for_launch` and resolves each reference in the controlling
   process: the hub in client mode, otherwise the desktop. A failure refuses
   the launch with a new code, `E_ENV_UNRESOLVED`, whose message names the
   variable and the reference kind, never the value. An empty set gives
   `EnvFile::None`.
2. `EnvFile::render()` writes one `export NAME=<shell::quote(value)>` line
   per variable. Names are validated, values quoted by the canonical
   `shell::quote`, so newlines and quotes survive.
3. `write(ssh, host, session_id, &EnvFile)` calls `write_host_file_secret`
   with `~/.claude-fleet/env/<session_id>.sh` (directory 0700, file 0600).

`ClaudeLaunch` gains `env_file: Option<String>` (the remote path). When set,
every pane-command builder (claude `pane_command_with`, codex, agy and
`shell_pane_command`) puts this prefix after the profile export:

```sh
f=<quoted path>; if [ -r "$f" ]; then . "$f"; fi; rm -f "$f"; unset f;
```

The file is gone once the shell has read it. Nothing about the values is in
the argv; only the path is.

Call sites in `service/sessions/lifecycle.rs` resolve and write the file
just before `tmux.new_session` at create (`:1154`), restart (`:2141`),
`restart_unrepaired` (`:2325`) and recreate (`:2492`), and the same on
move's launch on the target host. A restart therefore picks up edits.

The tick sweep removes `~/.claude-fleet/env/*.sh` files older than 10
minutes on reachable hosts: a launch that wrote its file but never started.
It follows `sweep_secret_dirs`, gated by a cheap `ls` first.

### 5. Surfaces

**MCP**

- New tool `env_vars { action: "list" | "set" | "unset", scope, scope_key?,
  name?, value?, ref?, secret? }`. `value` is already in `guard.rs`
  `SKIP_KEYS`; add `ref` to `SKIP_KEYS` as well. Keep the description slim
  (the served-surface budget test); the prose goes in the control skill.
- `new_session` gains an optional `env: { NAME: value }`. These are
  session-scope plain values; secrets go through `env_vars set secret:true`
  or a form. Add `env` to `COUNT_KEYS` so only the field count is audited.
- An agent that must not see a secret uses `ask` with a `secret` field. The
  answer handler stores it with `set_env_var(..., secret: true)` when the
  form names an env target. This is an `env_target` on the field, validated
  against the same rules, so the value never reaches the agent.

**Desktop and hub**

- Tauri commands `env_vars_list`, `env_var_set`, `env_var_unset`, hub-routed
  (`Verdict::Routed`). In client mode the desktop sends values to the hub over
  the existing authenticated link, as `answer_form` does.
- Each new command arg struct gets `Serialize` and a non-default row in
  `tests_routing.rs`.
- Bump `CONTRACT_REVISION` (17 at the time of writing) together with
  `MIN_HUB_CONTRACT` / `MAX_HUB_CONTRACT`, then `REGEN_HUB_CONTRACT`.
- UI: Settings › Environment, a declarative page spec with tabs Global /
  Hosts / Projects. Sessions show and edit theirs in the session's detail
  panel. Secrets render as `••••` with Replace and Remove; never Reveal. A
  note under the table: "Changes apply when a session next starts or
  restarts."

**Generated docs to regenerate**: `REGEN_DOCS` (control-api-reference),
`REGEN_HUB_VERDICTS`, `REGEN_PAGE_DOCS`, `REGEN_FORM_DOCS` (the `env_target`
field), `REGEN_HUB_CONTRACT`.

## Error handling

| Case | Result |
|---|---|
| Invalid or reserved name, oversized value, NUL | `E_INVALID_INPUT`, nothing stored |
| Reference unresolved at launch | launch refused, `E_ENV_UNRESOLVED` naming the variable |
| Env file write fails (host unreachable, disk) | launch refused with the write's error; no tmux session created |
| File left behind (launch never ran) | removed by the tick sweep after 10 min |
| Hub older than the desktop's contract | existing contract-mismatch refusal |

## Testing

- **Store**: scope precedence (all four levels, ties), unique upsert,
  `value`/`credential_ref` exclusivity, session-delete trigger, listing never
  returns a secret or reference value, the statement-count pin.
- **Validation**: name regex, every reserved name, size caps, NUL.
- **Render**: property test that `render()` then `sh -c '. file; printf %s
  "$NAME"'` round-trips arbitrary values (quotes, newlines, `$`, backticks).
- **Pane command**: a golden test per adapter with `env_file` set. A
  negative test that no value from a fixture set appears in any built pane
  command or in the `tmux new-session` script.
- **Launch e2e** (fake host): create writes the file at 0600; the pane reads
  it and the file is gone; restart rewrites it with an edited value;
  unresolved reference refuses the launch and creates no tmux session.
- **Audit**: `redact_args` drops `value` and `ref` and counts `env`.
- **Redaction**: `env_secret_literals` masks a stored secret in a log line.
- **Routing**: a `tests_routing.rs` row per command; hub contract regen.
- **Sweep**: an old file removed, a fresh one kept.

## Rollout

One branch, landing in this order: migration and store → service and launch
→ MCP tool → Tauri commands and routing → UI page → docs (`docs/concepts.md`
gains an *Environment* section; `docs/status.md` an entry). The desktop and
its hub ship together (contract bump).
