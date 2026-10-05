<script lang="ts">
  import Modal from './Modal.svelte';
  import Badge from './Badge.svelte';
  import { opTone, outcomeTone } from './assets_visual';
  import { applySync, isDestructive, planSync, syncProgress, type SyncPlan, type SyncRunSummary } from './assets';

  /** A host plan skipped because the host has no layers assigned — the
   *  fleet-core detail always starts with this (see `UNLAYERED_DETAIL` in
   *  `sync/mod.rs`); matched by prefix since the rest of the sentence is
   *  free text. */
  const UNLAYERED_PREFIX = 'no layers assigned';

  /** Ops whose reason is a caution (a host edit could be lost or could not be
   *  ruled out); every other non-blocked reason is information (a private
   *  asset withheld, a removal, a held catalog) and reads muted. */
  const CAUTION_OPS = new Set(['update', 'overwrite', 'plugin_update']);

  let {
    plan,
    filter = {},
    onclose,
    onapplied,
    onopensecrets,
    onapplying,
    onreplanned,
  }: {
    plan: SyncPlan;
    /** The filter that produced `plan`, re-sent (with `allowUnlayered: true`
     *  added) when the user clicks "Plan anyway" on a skipped-unlayered
     *  host. */
    filter?: { hostAlias?: string; kind?: string; name?: string };
    onclose: () => void;
    onapplied: (summary: SyncRunSummary) => void;
    /** Optional: a blocked-on-missing-secrets row links here so the caller
     *  can open the SecretsPanel. No-op if omitted. */
    onopensecrets?: () => void;
    /** Optional: fires true when an apply starts and false when it settles,
     *  so the caller (AssetsPanel) can disable its own Sync button for the
     *  duration. */
    onapplying?: (applying: boolean) => void;
    /** Optional: fires with the freshly computed plan after "Plan anyway"
     *  succeeds, so the caller can keep its own copy (e.g. `lastPlan`) in
     *  sync. No-op if omitted. */
    onreplanned?: (plan: SyncPlan) => void;
  } = $props();

  let applying = $state(false);
  let forcePartial = $state(false);
  let error = $state<string | null>(null);
  // Set on an E_SECRET_MISSING apply failure so the checkbox stays visible
  // even for a plan that (unexpectedly) had no blocked row at plan time.
  let sawSecretMissing = $state(false);
  let summary = $state<SyncRunSummary | null>(null);
  let controller: AbortController | null = null;
  let replanning = $state(false);
  let replanError = $state<string | null>(null);

  async function planAnyway(hostAlias: string) {
    replanning = true;
    replanError = null;
    const r = await planSync({ ...filter, hostAlias, allowUnlayered: true });
    replanning = false;
    if (!r.ok) { replanError = r.error.message; return; }
    onreplanned?.(r.value);
  }

  const destructive = $derived(isDestructive(plan));
  const hasBlocked = $derived(plan.hosts.some((h) => h.actions.some((a) => a.op === 'blocked')));
  const showForcePartial = $derived(hasBlocked || sawSecretMissing);
  const applicableCount = $derived(
    plan.hosts.reduce((n, h) => n + h.actions.filter((a) => a.op !== 'noop' && a.op !== 'blocked').length, 0),
  );
  const countsEntries = $derived(Object.entries(plan.counts).filter(([, n]) => n > 0));
  const progress = $derived(applying && $syncProgress && $syncProgress.plan_id === plan.id ? $syncProgress : null);

  function outcomeFor(hostAlias: string, harness: string, kind: string, name: string): string | null {
    const host = summary?.hosts.find((h) => h.host_alias === hostAlias && h.harness === harness);
    return host?.actions.find((a) => a.kind === kind && a.name === name)?.outcome ?? null;
  }

  async function apply() {
    applying = true;
    onapplying?.(true);
    error = null;
    controller = new AbortController();
    const r = await applySync(plan.id, forcePartial, controller.signal);
    applying = false;
    onapplying?.(false);
    controller = null;
    if (!r.ok) {
      error = r.error.message;
      if (r.error.code === 'E_SECRET_MISSING') sawSecretMissing = true;
      return;
    }
    summary = r.value;
    onapplied(summary);
  }

  function cancelApply() {
    controller?.abort();
  }
</script>

<Modal title="Sync plan" onclose={applying ? undefined : onclose} width="640px" testid="sync-plan-dialog">
  <div class="counts" data-testid="plan-counts">
    {#each countsEntries as [op, n] (op)}<Badge tone={opTone(op)} label={`${op}: ${n}`} />{/each}
    {#if countsEntries.length === 0}<span class="muted">Nothing to do.</span>{/if}
  </div>

  {#if error}<p class="error" data-testid="plan-error">{error}</p>{/if}
  {#if replanError}<p class="error" data-testid="plan-replan-error">{replanError}</p>{/if}

  <div class="hosts">
    {#each plan.hosts as h (h.host_alias + '::' + h.harness)}
      <div class="host-section" data-testid={`plan-host-${h.host_alias}-${h.harness}`}>
        <div class="host-header">
          <strong>{h.host_alias}</strong>
          <span class="harness">{h.harness}</span>
          <span class="status">{h.status}</span>
          {#if h.detail?.startsWith(UNLAYERED_PREFIX)}
            <span class="detail warning" data-testid={`plan-unlayered-${h.host_alias}-${h.harness}`}>{h.detail}</span>
            <button
              class="link quiet"
              onclick={() => planAnyway(h.host_alias)}
              disabled={replanning}
              data-testid={`plan-anyway-${h.host_alias}-${h.harness}`}
            >{replanning ? 'Planning…' : 'Plan anyway'}</button>
          {:else if h.detail}
            <span class="detail">{h.detail}</span>
          {/if}
        </div>
        {#each h.actions as a (a.kind + '::' + a.name)}
          {@const outcome = outcomeFor(h.host_alias, h.harness, a.kind, a.name)}
          <div class="action-row" data-testid={`plan-action-${h.host_alias}-${h.harness}-${a.kind}-${a.name}`}>
            <Badge tone={opTone(a.op)} label={a.op} />
            <span class="asset">{a.kind}/{a.name}</span>
            {#if a.backup}<span class="backup" title="A backup will be made before writing">backup</span>{/if}
            {#if a.secrets.length}<span class="secrets">secrets: {a.secrets.join(', ')}</span>{/if}
            {#if a.op !== 'blocked' && a.reason}
              <span class="note" class:caution={CAUTION_OPS.has(a.op)} data-testid={`plan-note-${h.host_alias}-${h.harness}-${a.kind}-${a.name}`}>{a.reason}</span>
            {/if}
            {#if a.op === 'blocked'}
              {#if a.reason}<span class="reason">{a.reason}</span>{/if}
              {#if a.missing_secrets.length > 0}
                <button
                  class="link"
                  onclick={() => onopensecrets?.()}
                  data-testid={`plan-action-secrets-${h.host_alias}-${h.harness}-${a.kind}-${a.name}`}
                >Set secrets</button>
              {/if}
            {/if}
            {#if outcome}
              <Badge
                tone={outcomeTone(outcome)}
                label={outcome}
                testid={`plan-outcome-${h.host_alias}-${h.harness}-${a.kind}-${a.name}`}
              />
            {/if}
          </div>
        {/each}
        {#if h.actions.length === 0}<p class="muted">Nothing to do on this host.</p>{/if}
      </div>
    {/each}
  </div>

  {#if summary}
    {#each summary.hosts.filter((r) => r.restart_required) as r (r.host_alias)}
      <p class="restart" data-testid={`plan-restart-${r.host_alias}`}>restart Claude on {r.host_alias}</p>
    {/each}
  {/if}

  {#if showForcePartial}
    <label class="force-partial">
      <input type="checkbox" bind:checked={forcePartial} disabled={applying} data-testid="plan-force-partial" />
      Apply anyway, skipping actions blocked on a missing secret
    </label>
  {/if}

  {#if progress}
    <p class="progress" data-testid="plan-progress">
      {progress.done}/{progress.total}{progress.host_alias ? ` — ${progress.host_alias}/${progress.harness}` : ''}
    </p>
  {/if}

  <div class="actions">
    <button onclick={onclose} disabled={applying}>{summary ? 'Done' : 'Close'}</button>
    {#if applying}
      <button onclick={cancelApply} data-testid="plan-cancel">Cancel</button>
    {/if}
    <button
      class="primary"
      class:danger={destructive}
      onclick={apply}
      disabled={applying || applicableCount === 0 || summary !== null}
      data-testid="plan-apply"
    >{applying ? 'Applying…' : 'Apply'}</button>
  </div>
</Modal>

<style>
  .counts { display: flex; gap: 6px; flex-wrap: wrap; font-size: 12px; }
  .hosts { display: flex; flex-direction: column; gap: 10px; max-height: 50vh; overflow: auto; }
  .host-section { border: 1px solid var(--border); border-radius: 6px; padding: 6px 8px; }
  .host-header { display: flex; align-items: center; gap: 8px; font-size: 12px; margin-bottom: 4px; }
  .harness, .status, .detail { color: var(--fg-muted); }
  .detail.warning { color: var(--usage-warn); }
  .link.quiet { font-size: 11px; opacity: 0.75; }
  .action-row { display: flex; align-items: center; gap: 8px; font-size: 12px; padding: 2px 0; flex-wrap: wrap; }
  .asset { font-family: ui-monospace, monospace; }
  .backup { color: var(--usage-warn); } .secrets { color: var(--fg-muted); } .reason { color: var(--usage-crit); }
  /* A reason on a planned action: information (muted), or a caution (warn) for
     update/overwrite/plugin_update — a note, never an error. */
  .note { color: var(--fg-muted); }
  .note.caution { color: var(--usage-warn); }
  .link { background: none; border: 0; color: var(--accent); cursor: pointer; padding: 0; font-size: 12px; }
  .restart { color: var(--usage-warn); font-size: 12px; margin: 0; }
  .force-partial { display: flex; align-items: center; gap: 6px; font-size: 12px; }
  .progress { font-size: 12px; color: var(--fg-muted); margin: 0; }
  .actions { display: flex; gap: 8px; justify-content: flex-end; }
  .actions button { font-size: 0.85rem; padding: 0.3rem 0.8rem; border: 1px solid var(--border); background: transparent; color: var(--fg); border-radius: 4px; cursor: pointer; }
  .actions button:disabled { opacity: 0.5; cursor: not-allowed; }
  .actions button.primary { border-color: var(--accent); }
  .actions button.danger { color: var(--usage-crit); border-color: var(--usage-crit); }
  .muted { color: var(--fg-muted); font-size: 12px; }
  .error { color: var(--usage-crit); }
</style>
