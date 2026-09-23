# SSH self-diagnosis — design

Date: 2026-09-23
Status: approved in brainstorming, awaiting spec review

## Problem

On 2026-09-23 the terminal for a session on host `mac` printed
`Host key verification failed.` and the pane went into the reconnect loop.
The cause: the `mac` alias resolves to `mac.rlt.sk` (192.168.13.123), and
`~/.ssh/known_hosts` held that machine's key only under `localhost`. Every
fleet ssh call runs with `BatchMode=yes`, so ssh could not ask and failed.
Meanwhile `list_hosts` reported `mac` as reachable.

What the code does today:

- **No shared stderr classifier.** `service/account_usage.rs:969`
  (`connection_never_established`) is the only code that recognises
  host-key/auth/DNS/connect failures, and only to decide usage-polling
  fallback. `hosts.rs:405` wraps raw stderr in `E_PROBE`; the lenient probe
  (`:434`) discards it. `ssh.rs:1536` `is_mux_failure` only drives a mux reset.
- **Terminal attach failures are invisible to the UI.** ssh stderr is pane
  bytes; on EOF `pty.rs:572-595` sets `exited`, and `TerminalView.svelte:669`
  calls `scheduleAutoReconnect`, which repeats the same failure until the
  "Connection lost. [Reconnect]" banner.
- **Probe and attach diverge.** `probe_host` uses `cm-<host>.sock`; the
  attach uses `cm-<host>-tty.sock` (`ssh.rs:259-285`). A live, already
  authenticated probe master hides the fact that a fresh connection fails.
- **No known_hosts management** and **no "always allow" storage** anywhere.
- **No in-process LLM call.** Claude work runs through Claude Code.

## Goals

1. When an SSH/connectivity failure happens, say what it is and why in the
   UI, next to where it happened, instead of an endless retry.
2. Offer a typed, previewable fix where one is safe. The user can authorize a
   fix kind for a host once, and later occurrences fix themselves.
3. For failures the rules do not recognise, an AI explanation on request.
4. Prevent the 2026-09-23 shape: surface "probe OK, fresh connection fails"
   before the user opens a terminal.

## Non-goals (v1)

- Session-level states (`stuck_kind`), non-SSH `E_*` errors.
- Applying fixes on the hub from a paired client (hub v1 diagnoses only).
- Agent-transport hosts (no SSH route), editing `~/.ssh/config`, managing
  ssh-agent keys.
- Learning new rules from AI output.

## Decisions

| Question | Decision |
|---|---|
| Scope | SSH / connectivity only |
| AI role | Deterministic rules first; AI (`claude -p`) only for `Unknown` |
| Grant granularity | fix kind × host alias |
| Changed host key | Never covered by a grant; always an explicit dialog |
| Grant binding | alias only (a changed `HostName` behind the alias is TOFU, not "changed") |
| Hub mode | Hub runs read-only diagnosis of its own ssh; no hub fix path in v1. Fixes to this desktop's ssh (terminal attach) work in both modes |

## Architecture

**Rule: the process whose `ssh` failed diagnoses and fixes, because only its
`~/.ssh` is the relevant one.**

- The terminal attach (`pty_open`) is `SameInBoth` (`backend/verdicts.rs:926`).
  It is always this desktop's own ssh, so it is diagnosed and fixed locally
  in both modes.
- Probes and other ssh operations run on the hub in hub-client mode, so they
  are diagnosed there.

```
crates/fleet-core/src/ssh_diag/
  mod.rs
  classify.rs   (stderr, exit status) -> Option<SshFailure>
  probe.rs      SshFailure -> Diagnosis (read-only checks)
  fix.rs        enum Fix + apply(); the ONLY writer of ~/.ssh files
  bundle.rs     redacted diagnostic bundle for the AI
crates/fleet-core/src/store/fix_grants.rs + migrations/NNN_fix_grants.sql
crates/fleet-core/src/service/ssh_diag.rs
  diagnose_host(alias) -> Diagnosis                  (probe-side failure)
  diagnose_attach_failure(alias, failure) -> Diagnosis (this machine's attach)
  apply_fix(alias, fix_id, remember: bool) -> FixOutcome
  list_fix_grants() / revoke_fix_grant(alias, fix_kind)
src-tauri/src/commands/ssh_diag.rs (thin handlers)
src/lib/SshDiagnosisCard.svelte, src/lib/sshDiag.ts
```

### Classifier (`classify.rs`)

`SshFailure { kind: SshFailureKind, host_alias, raw_tail: String }` where
`SshFailureKind` is `HostKeyUnknown | HostKeyChanged | AuthDenied | DnsFail |
Refused | Timeout | MuxBroken | Unknown`.

- Input: exit status 255 plus the stderr tail. Anything else is not an SSH
  failure (`None`).
- `HostKeyChanged` is detected from the
  `REMOTE HOST IDENTIFICATION HAS CHANGED` block. `HostKeyUnknown` is
  `Host key verification failed.` without it (and
  `No ED25519 host key is known for`).
- `connection_never_established` in `account_usage.rs` and the probe's
  `E_PROBE` path are rewired onto it, so there is one implementation.
  `is_mux_failure` becomes the `MuxBroken` arm.
- The kind vocabulary lives in the enum (serde `snake_case`); MCP
  descriptions derive from it, the same way `claude_status` does.

### Diagnosis (`probe.rs`) and fixes (`fix.rs`)

All checks are read-only. Commands are built with `shq`, with bounded
timeouts (5 s each).

| kind | facts gathered | fix offered | grantable |
|---|---|---|---|
| `HostKeyUnknown` | `ssh -G` hostname/IP/port; `ssh-keyscan` fingerprint; same key already in known_hosts under another name (key-blob compare, works with hashed lines); IP is a local interface AND key equals `/etc/ssh/ssh_host_*.pub` → `verified_local: true` | `AddHostKey { names: [hostname, ip], key_type, key_b64 }` | yes |
| `HostKeyChanged` | old fingerprint + line, new fingerprint; `verified_local` check | `ReplaceHostKey { … }` | **never** |
| `AuthDenied` | `IdentityFile` existence (never contents), `ssh-add -l` count, identities offered (`ssh -v`) | none; explanation | — |
| `DnsFail` | resolver answer for the hostname | none; explanation | — |
| `Refused` / `Timeout` | 3 s TCP connect to the port | `Retry` | — |
| `MuxBroken` | socket state | `ResetMux` (existing reset) | yes |
| `Unknown` | → AI bundle | AI advice | **never** |

`Fix` rules:

- Each variant has a stable `kind()` string (the grant key), a
  human description, an exact **preview** (file, lines to be written, and an
  equivalent shell command for reference), and a `grantable()` that is
  hard-coded per variant, not data.
- `apply()` is Rust. It first writes a backup `known_hosts.bak-<unix-ts>`,
  then does an atomic write (temp file + rename, same permissions). It is
  idempotent: an entry already present is a no-op. `ReplaceHostKey` removes
  the old entries with `ssh-keygen -R <name> -f <file>` (via `shq`) and then
  appends.
- After every apply, a **fresh-connection verify** runs:
  `ssh -o ControlPath=none -o BatchMode=yes -o ConnectTimeout=5 -- <alias> true`.
  `FixOutcome` is `Fixed` or `StillFailing(Diagnosis)`.
- Every apply (manual or automatic) is recorded in the host's history: who,
  which fix, fingerprint.

### Grants (`fix_grants`)

```sql
CREATE TABLE fix_grants (
  host_alias TEXT NOT NULL,
  fix_kind   TEXT NOT NULL,
  granted_at INTEGER NOT NULL,
  PRIMARY KEY (host_alias, fix_kind)
);
```

- `apply_fix(..., remember: true)` stores a grant only if the fix is
  `grantable()`. Otherwise it returns `E_NOT_GRANTABLE`, enforced in the
  service, not just hidden in the UI.
- When a diagnosis yields a grantable fix with a grant, the service applies
  it without asking, verifies, and emits an `ssh_diag:auto_fixed` event. The
  UI shows a one-line notice with a Revoke link.
- `ai_diagnose` is also a grant kind. It means only "may send the bundle
  without asking".
- Revocable from Settings → Hosts.

### AI fallback (`bundle.rs` + desktop-side runner)

- Runs only for `Unknown`, only on the desktop, only on request (or with an
  `ai_diagnose` grant).
- The bundle contains:
  - the last ~40 stderr lines;
  - `ssh -G` filtered to `hostname port user proxyjump stricthostkeychecking
    userknownhostsfile controlpath`;
  - the `ssh -v` handshake with identity/key-path lines removed;
  - OS, ssh version, and the classifier kind.

  It never contains key material, `IdentityFile` paths, env, or tokens.
  Redaction is a pure function with fixture tests.
- Invocation: `claude -p --output-format json --max-turns 1
  --disallowedTools '*' --append-system-prompt <fixed prompt>`, with the
  bundle on stdin, a 60 s timeout, and cancellation through `cancel.rs`. The
  prompt demands JSON `{cause, confidence, explanation, steps[],
  suggested_command?}`. Output that fails to parse is shown raw.
- Consent: the first send shows the exact bundle, with
  `[Send] [Send and always allow for <host>] [Cancel]`.
- Output:
  - `suggested_command` is copy-only. The app never executes AI text, grant
    or no grant.
  - If `cause` maps to a known kind, the card offers "re-run as `<kind>`",
    which runs the deterministic diagnosis and leads to a typed fix.
  - "Copy as issue" copies the bundle plus the answer.
- If no `claude` binary is found, the button is disabled with the reason.
- In hub-client mode the hub's `diagnose_host` returns the already-redacted
  bundle and the AI runs on the desktop.

### Prevention

- **Fresh-connection check in `probe_host`.** A `ControlPath=none` connect
  runs only when:
  - `ssh-keygen -F <resolved hostname>` finds no key, or
  - the last fresh check is older than 24 h.

  A failure sets `ssh_failure` on the host row even though the mux probe
  succeeded. That turns "reachable but the terminal fails" into a visible
  warning before the terminal opens.
- **TOFU on Add Host.** If the new host's key is unknown, the add flow shows
  the `HostKeyUnknown` card (fingerprint + `verified_local`) before saving.

## UI

- **Terminal (`TerminalView.svelte`).** On EOF the backend emits the
  classified `ssh_failure` along with `exited`. With a recognised failure,
  `scheduleAutoReconnect` is **not** called. Instead of the banner, the pane
  shows `SshDiagnosisCard` inline (no toast, per consolidation-01 D5):
  - a title;
  - facts, e.g. fingerprint and "✓ verified: this Mac";
  - the fix with a [Preview];
  - `[Fix] [Fix and always allow for <host>] [Cancel]`.

  A successful fix plus verify then reconnects automatically. With a grant,
  only the one-line "Auto-fixed … · Revoke" notice shows and the reconnect
  proceeds.
- **HostDetail.** The same card when the host row carries `ssh_failure`. In
  hub-client mode the fix buttons are disabled with the "fix it on the hub"
  reason (see Hub / routing).
- **Settings → Hosts.** A grants list (host · kind · since · Revoke).

## Hub / routing

- New MCP tool `diagnose_host` (read-only) calls `service::ssh_diag`.
  Regenerate `docs/control-api-reference.md` (`REGEN_DOCS=1`) and keep the
  description within the tool-description budget.
- Verdict rows. Two diagnosis commands, because two different machines'
  `~/.ssh` are involved:
  - `diagnose_host` (a failure seen by probes/operations) routes to the hub.
  - `diagnose_attach_failure` (a failure seen by this desktop's `pty_open`)
    is `SameInBoth`: it acts on the local ssh, like `pty_open` itself.
  - `apply_fix`, `list_fix_grants` and `revoke_fix_grant` are `SameInBoth`.
    They always act on THIS machine's `~/.ssh` and store, so a terminal-card
    fix works in hub-client mode too.
  - Hub v1 has no fix path at all (no hub `apply_fix` tool). In hub-client
    mode, HostDetail's card built from a hub `diagnose_host` result shows its
    fix buttons disabled, with the reason "this failure is on the hub's
    ssh; fix it on the hub (`docker exec fleet-hub …`)".
- Regenerate the verdict outputs (`REGEN_HUB_VERDICTS=1`).
- A new wire field (`ssh_failure` on host rows) uses `#[serde(default)]` and
  gets a contract golden regen (`REGEN_HUB_CONTRACT=1`).

## Testing

- **Classifier:** a fixture table of real OpenSSH 8.x/9.x stderr for every
  kind, including kex and mux.
- **Fixes:** a temp `HOME` with a fake known_hosts. Tests cover the backup,
  the atomic write, idempotence, permissions preserved, and that
  `ReplaceHostKey` is never auto-applied even when a grant row exists.
- **Grants:** store round-trip, and `remember` on a non-grantable fix →
  `E_NOT_GRANTABLE`.
- **Bundle redaction:** fixtures containing key paths, `-----BEGIN`, and
  token-shaped strings that must not survive.
- **Frontend:** `SshDiagnosisCard` states; `TerminalView` does not
  auto-reconnect on a classified failure.
- **E2E:** extend `scripts/hub-e2e.sh` with an empty-known_hosts scenario if
  its sshd setup allows it (to be checked in the plan).

## Delivery (one PR each, in order)

1. **Classifier.** `ssh_diag/classify.rs`, rewire `account_usage` and
   `probe_host`. Better error text, no UI change.
2. **Diagnosis + fixes + grants.** `probe.rs`, `fix.rs`, migration,
   commands, `diagnose_host` hub tool, verdicts.
3. **UI.** `SshDiagnosisCard`, terminal + HostDetail integration, Settings
   grants list.
4. **Prevention.** Fresh-connection check in `probe_host`, TOFU on Add Host.
5. **AI fallback.** Bundle, runner, consent, card.
