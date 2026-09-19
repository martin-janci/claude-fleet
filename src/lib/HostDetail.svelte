<script lang="ts">
  // The Hosts view's detail pane (spec: "The Hosts view" → Detail sections).
  // In order: header, usage, sessions, integration, danger. ("Today" is left
  // out: the frontend has per-session lifetime totals only, no per-day figure,
  // and this view adds no new data path.) Action safety per the spec:
  // reversible actions are plain buttons (hide shows an Undo toast);
  // `Rotate token…` and `Remove host…` sit at the bottom, have no keyboard
  // shortcut, and confirm with Cancel focused, stating the consequence.
  import type { HostRow } from './hosts';
  import { deleteHost } from './hosts';
  import type { AccountRow } from './accounts';
  import type { AccountUsageSnapshot } from './account_usage_store';
  import type { HostTokenInfo, TokenMode } from './mcp';
  import { restoreHostSessions, type RestorePlanEntry, type SessionRow } from './sessions';
  import { selectSession } from './selection';
  import { claudeStatusLabel, stuckKindLabel } from './attention';
  import { formatAge, hookHealthLabel, type HookHealth } from './hook_health';
  import { hideHostWithUndo, rotateToken, setTokenMode, showHost } from './host_actions';
  import { pushError, push } from './toasts';
  import { removeHostMessage, rotateTokenMessage, type HostAttention } from './hosts_view';
  import AccountNickname from './AccountNickname.svelte';
  import ConfirmDialog from './ConfirmDialog.svelte';
  import UsageBlock from './UsageBlock.svelte';

  let {
    host,
    account,
    snapshot,
    sharedWith,
    hostSessions,
    token,
    tokensLoaded,
    hook,
    attention,
    now,
    locale,
    timeZone,
    suppressUnavailable = false,
    editingNickname,
    probing = false,
    detailEl = $bindable(),
    oneditstart,
    oneditdone,
    onreprobe,
    onrefreshusage,
  }: {
    host: HostRow;
    account: AccountRow | null;
    snapshot: AccountUsageSnapshot | null;
    sharedWith: string[];
    hostSessions: SessionRow[];
    token: HostTokenInfo | null;
    tokensLoaded: boolean;
    hook: HookHealth;
    attention: HostAttention | null;
    now: number;
    locale?: string;
    timeZone?: string;
    suppressUnavailable?: boolean;
    editingNickname: boolean;
    probing?: boolean;
    detailEl?: HTMLElement;
    oneditstart: () => void;
    oneditdone: () => void;
    onreprobe: () => void;
    onrefreshusage: () => void;
  } = $props();

  let confirm = $state<'rotate' | 'remove' | 'restore' | null>(null);
  let busy = $state(false);

  const isLocal = $derived(host.alias === 'local');

  // Sessions the backend marked lost (host reboot / tmux server restart) that
  // still carry a Claude conversation to resume. `bg`/`external` rows have no
  // fleet-managed tmux pane to restore into.
  const restorable = $derived(
    hostSessions.filter((s) => s.lost_at !== null && s.claude_session_id && s.kind !== 'bg' && s.kind !== 'external'),
  );

  let restorePlan = $state<RestorePlanEntry[] | null>(null);
  let restoreError = $state<string | null>(null);
  let restoreSummary = $state<{ ok: number; total: number; failures: { name: string; error: string }[] } | null>(
    null,
  );

  function sessionName(s: SessionRow): string {
    return s.friendly_name?.trim() || s.tmux_name;
  }

  function sessionState(s: SessionRow): string {
    if (s.stuck_kind) return `stuck: ${stuckKindLabel(s.stuck_kind)}`;
    if (s.status === 'ghost') return 'ghost';
    return claudeStatusLabel(s.claude_status) || s.status;
  }

  async function onTokenMode(mode: TokenMode) {
    busy = true;
    const r = await setTokenMode(host.alias, mode);
    busy = false;
    if (!r.ok) pushError(r.error, 'Token mode not changed');
  }

  async function onHideToggle() {
    busy = true;
    if (host.hidden) await showHost(host.alias);
    else await hideHostWithUndo(host.alias);
    busy = false;
  }

  async function confirmRotate() {
    const alias = host.alias;
    busy = true;
    const r = await rotateToken(alias);
    busy = false;
    confirm = null;
    if (r.ok) push({ kind: 'success', message: `${alias} has a new control-API token.` });
    else pushError(r.error, `Rotate token for ${alias} failed`);
  }

  async function confirmRemove() {
    const alias = host.alias;
    busy = true;
    const r = await deleteHost(alias);
    busy = false;
    confirm = null;
    if (r.ok) push({ kind: 'info', message: `${alias} removed.` });
    else pushError(r.error, `Remove ${alias} failed`);
  }

  async function onRestoreClick() {
    restoreError = null;
    restoreSummary = null;
    busy = true;
    const r = await restoreHostSessions(host.alias, { dryRun: true });
    busy = false;
    if (!r.ok) {
      restoreError = r.error.message;
      return;
    }
    restorePlan = r.value.plan;
    confirm = 'restore';
  }

  function cancelRestore() {
    confirm = null;
    restorePlan = null;
  }

  async function confirmRestore() {
    const alias = host.alias;
    const ids = (restorePlan ?? []).filter((e) => e.action === 'restore').map((e) => e.session_id);
    busy = true;
    const r = await restoreHostSessions(alias, { sessionIds: ids });
    busy = false;
    confirm = null;
    restorePlan = null;
    if (r.ok) {
      const results = r.value.results;
      restoreSummary = {
        ok: results.filter((x) => x.ok).length,
        total: results.length,
        failures: results
          .filter((x) => !x.ok)
          .map((x) => ({ name: x.tmux_name, error: x.error ?? 'unknown error' })),
      };
    } else {
      restoreError = r.error.message;
    }
  }
</script>

<section
  bind:this={detailEl}
  class="host-detail"
  tabindex="-1"
  aria-label="{host.alias} details"
  data-testid="host-detail"
  data-alias={host.alias}
>
  <!-- 1. Header -->
  <header class="head">
    <div class="title-row">
      <h2 data-testid="detail-alias">{host.alias}</h2>
      <span class="status" class:off={!host.reachable} data-testid="detail-status"
        >{host.reachable ? '● online' : '○ offline'}</span
      >
      {#if host.hidden}<span class="muted">hidden</span>{/if}
      <button type="button" class="small" onclick={onreprobe} disabled={probing} data-testid="detail-reprobe"
        >{#if probing}probing…{:else}<kbd>r</kbd> Re-probe{/if}</button
      >
    </div>
    <dl class="facts">
      {#if host.ssh_alias}
        <dt>ssh</dt><dd data-testid="detail-ssh">{host.ssh_alias}</dd>
      {/if}
      <dt>last ping</dt>
      <dd data-testid="detail-ping">
        {host.last_pinged_at ? `${formatAge(now - host.last_pinged_at)} ago` : 'never'}
      </dd>
      <dt>claude</dt><dd>{host.claude_version ?? '—'}</dd>
      <dt>tmux</dt><dd>{host.tmux_version ?? '—'}</dd>
    </dl>
    {#if attention}
      <p class="attention" data-testid="detail-attention">{attention.glyph} {attention.title}</p>
    {/if}
  </header>

  <!-- 2. Usage -->
  <section class="block">
    {#if account}
      <div class="account-line" data-testid="detail-account">
        <span class="label">Account</span>
        <AccountNickname
          {account}
          editing={editingNickname}
          onedit={oneditstart}
          ondone={oneditdone}
          testid="detail-nickname"
        />
        {#if account.email}<span class="muted">{account.email}</span>{/if}
      </div>
    {/if}
    <UsageBlock
      {account}
      {snapshot}
      {sharedWith}
      {now}
      {locale}
      {timeZone}
      {suppressUnavailable}
      onRefresh={account ? onrefreshusage : undefined}
    />
  </section>

  <!-- 3. Sessions -->
  <section class="block" aria-label="Sessions on {host.alias}">
    <div class="section-head">
      <h3>Sessions <span class="muted">{hostSessions.length}</span></h3>
      {#if restorable.length > 0}
        <button
          type="button"
          class="small"
          disabled={busy}
          data-testid="restore-lost"
          onclick={onRestoreClick}
          >Restore {restorable.length} lost session{restorable.length === 1 ? '' : 's'}…</button
        >
      {/if}
    </div>
    {#if restoreError}
      <p class="error" data-testid="restore-error">{restoreError}</p>
    {/if}
    {#if restoreSummary}
      <p data-testid="restore-summary">
        Restored {restoreSummary.ok} of {restoreSummary.total}
        {#each restoreSummary.failures as f (f.name)}<br />{f.name}: {f.error}{/each}
      </p>
    {/if}
    {#if hostSessions.length === 0}
      <p class="muted">No sessions on this host. Press <kbd>n</kbd> to start one.</p>
    {:else}
      <ul class="sessions">
        {#each hostSessions as s (s.id)}
          <li>
            <button
              type="button"
              class="session"
              data-nav-row
              data-testid="detail-session"
              onclick={() => selectSession(s)}
            >
              <span class="s-name">{sessionName(s)}</span>
              <span class="muted">{sessionState(s)}</span>
            </button>
          </li>
        {/each}
      </ul>
    {/if}
  </section>

  <!-- 5. Integration -->
  <section class="block" aria-label="Integration">
    <h3>Integration</h3>
    <div class="kv">
      <span class="label" title="Control-API token: full = every tool, readonly = observe only">Token</span>
      {#if token}
        <select
          value={token.mode}
          disabled={busy}
          aria-label="Token mode"
          data-testid="detail-token-mode"
          onchange={(e) => onTokenMode((e.currentTarget as HTMLSelectElement).value as TokenMode)}
        >
          <option value="full">full</option>
          <option value="readonly">readonly</option>
        </select>
      {:else}
        <span class="muted">{tokensLoaded ? 'none — provision hosts to mint one' : '…'}</span>
      {/if}
    </div>
    <div class="kv">
      <span class="label" title="Installed with the host's token; last event = newest Stop hook from a session on this host">Hooks</span>
      <span data-testid="detail-hooks" data-state={hook.state}>{hookHealthLabel(hook, now)}</span>
    </div>
    {#if token}
      <button type="button" class="action" disabled={busy} data-testid="detail-rotate" onclick={() => (confirm = 'rotate')}
        >Rotate token…</button
      >
    {/if}
  </section>

  <!-- 6. Danger -->
  <section class="block danger-zone" aria-label="Danger">
    <h3>Danger</h3>
    {#if isLocal}
      <p class="muted">The local host can't be hidden or removed.</p>
    {:else}
      <div class="actions">
        <button type="button" class="action" disabled={busy} data-testid="detail-hide" onclick={onHideToggle}
          >{host.hidden ? 'Show host' : 'Hide host'}</button
        >
        <button type="button" class="action danger" disabled={busy} data-testid="detail-remove" onclick={() => (confirm = 'remove')}
          >Remove host…</button
        >
      </div>
    {/if}
  </section>
</section>

{#if confirm === 'rotate'}
  <ConfirmDialog
    title="Rotate the token for {host.alias}?"
    message={rotateTokenMessage(host.alias)}
    confirmLabel="Rotate token"
    danger
    {busy}
    confirmTestId="confirm-rotate"
    onconfirm={confirmRotate}
    oncancel={() => (confirm = null)}
  />
{:else if confirm === 'remove'}
  <ConfirmDialog
    title="Remove {host.alias}?"
    message={removeHostMessage(host.alias, hostSessions.length)}
    confirmLabel="Remove host"
    danger
    {busy}
    confirmTestId="confirm-remove"
    onconfirm={confirmRemove}
    oncancel={() => (confirm = null)}
  />
{:else if confirm === 'restore'}
  <ConfirmDialog
    title="Restore lost sessions on {host.alias}?"
    confirmLabel="Restore"
    {busy}
    confirmTestId="confirm-restore"
    onconfirm={confirmRestore}
    oncancel={cancelRestore}
  >
    <ul class="restore-plan">
      {#each restorePlan ?? [] as entry (entry.session_id)}
        <li>
          <span class="name">{entry.friendly_name ?? entry.tmux_name}</span>
          {#if entry.cwd}<span class="muted">{entry.cwd}</span>{/if}
          {#if entry.action === 'skip'}<span class="skip">skipped — {entry.reason}</span>{/if}
        </li>
      {/each}
    </ul>
    <p class="note">Each session resumes its Claude conversation. Any first-run prompt waits for you.</p>
  </ConfirmDialog>
{/if}

<style>
  .host-detail {
    display: flex;
    flex-direction: column;
    gap: 1rem;
    padding: 0.8rem 1rem 2rem;
    overflow-y: auto;
    min-height: 0;
    height: 100%;
    box-sizing: border-box;
    outline: none;
    font-size: 0.8rem;
  }
  .title-row { display: flex; align-items: baseline; gap: 0.6rem; flex-wrap: wrap; }
  h2 { margin: 0; font-size: 1.1rem; }
  h3 {
    margin: 0 0 0.35rem;
    font-size: 0.7rem;
    font-weight: 600;
    letter-spacing: 0.04em;
    text-transform: uppercase;
    color: var(--fg-muted);
  }
  .status.off { color: var(--usage-warn); }
  .muted { color: var(--fg-muted); }
  .facts {
    display: grid;
    grid-template-columns: max-content 1fr;
    column-gap: 0.6rem;
    row-gap: 0.1rem;
    margin: 0.4rem 0 0;
  }
  .facts dt { color: var(--fg-muted); }
  .facts dd { margin: 0; font-variant-numeric: tabular-nums; }
  .attention { margin: 0.4rem 0 0; color: var(--usage-warn); }
  .block { border-top: 1px solid var(--border); padding-top: 0.6rem; }
  .account-line { display: flex; align-items: baseline; gap: 0.5rem; margin-bottom: 0.4rem; min-width: 0; }
  .label { color: var(--fg-muted); min-width: 3.5rem; }
  .section-head { display: flex; align-items: baseline; justify-content: space-between; gap: 0.6rem; flex-wrap: wrap; }
  .section-head h3 { margin: 0; }
  .error { color: var(--usage-crit); margin: 0.4rem 0; }
  .restore-plan { list-style: none; margin: 0.4rem 0; padding: 0; display: flex; flex-direction: column; gap: 0.3rem; }
  .restore-plan li { display: flex; flex-wrap: wrap; gap: 0.4rem; }
  .restore-plan .skip { color: var(--usage-warn); }
  .note { margin: 0.4rem 0 0; color: var(--fg-muted); }
  .sessions { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; }
  .session {
    display: flex;
    gap: 0.6rem;
    width: 100%;
    padding: 0.2rem 0.3rem;
    border: 1px solid transparent;
    border-radius: 4px;
    background: transparent;
    color: var(--fg);
    font: inherit;
    text-align: left;
    cursor: pointer;
  }
  .session:hover,
  .session:focus-visible { border-color: var(--border); outline: none; background: color-mix(in srgb, var(--fg) 5%, transparent); }
  .s-name { flex: 1; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .kv { display: flex; align-items: center; gap: 0.5rem; margin-bottom: 0.3rem; }
  select {
    background: transparent;
    border: 1px solid var(--border);
    color: var(--fg);
    border-radius: 4px;
    font-size: 0.75rem;
  }
  .actions { display: flex; gap: 0.5rem; flex-wrap: wrap; }
  .action,
  .small {
    font-size: 0.75rem;
    padding: 0.2rem 0.6rem;
    border: 1px solid var(--border);
    border-radius: 4px;
    background: transparent;
    color: var(--fg);
    cursor: pointer;
  }
  .action:disabled,
  .small:disabled { opacity: 0.55; cursor: default; }
  .action.danger { color: var(--usage-crit); border-color: var(--usage-crit); }
  kbd {
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    font-size: 0.65rem;
    padding: 0 0.2rem;
    border: 1px solid var(--border);
    border-radius: 3px;
  }
</style>
