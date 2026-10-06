<script lang="ts">
  import Badge from './Badge.svelte';
  import { opTone, outcomeTone } from './assets_visual';
  import { cardMayApply, cardOwns } from './assets_cards';
  import { onMount, tick, untrack } from 'svelte';
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
    mode = 'sync',
    owned = null,
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
    /** `review` is a Rollout card's read-only plan (R15): no Apply, only the
     *  card's own actions, and the ones it would hold marked. */
    mode?: 'sync' | 'review';
    /** The assets and catalogs a card owns; a review shows only these. */
    owned?: { assets: Set<string>; catalogs: Set<string> } | null;
    onclose: () => void;
    onapplied?: (summary: SyncRunSummary) => void;
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

  const review = $derived(mode === 'review');
  let backEl: HTMLButtonElement | undefined = $state();
  let rootEl: HTMLElement | undefined = $state();
  // The view replaces the list, so the keyboard must start inside it.
  onMount(() => backEl?.focus());
  /** Focus that fell out of the view (the clicked button was removed by a
   *  re-plan, or disabled by an apply) returns to Back once it is enabled,
   *  else to the view itself, so the keyboard is never stranded on `body`.
   *  (The workspace also takes Esc from `body` while a plan is open.) */
  async function refocus() {
    await tick();
    const a = document.activeElement as HTMLButtonElement | null;
    // The view itself held focus only while Back was disabled.
    if (a && a !== document.body && a.isConnected && !a.disabled && !(a === rootEl && backEl && !backEl.disabled)) return;
    (backEl && !backEl.disabled ? backEl : rootEl)?.focus();
  }

  // A new plan (a re-plan swaps the host's header, and the clicked button
  // with it) is a moment focus can fall out of the view.
  $effect(() => {
    void plan;
    untrack(() => void refocus());
  });

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
    if (!r.ok) { replanError = r.error.message; void refocus(); return; }
    onreplanned?.(r.value);
    void refocus();
  }

  /** What the view lists: in a review, only what the card owns. */
  const shownHosts = $derived(
    review && owned ? plan.hosts.map((h) => ({ ...h, actions: h.actions.filter((a) => cardOwns(a, owned.assets, owned.catalogs)) })) : plan.hosts,
  );
  const destructive = $derived(isDestructive(plan));
  const hasBlocked = $derived(plan.hosts.some((h) => h.actions.some((a) => a.op === 'blocked')));
  const showForcePartial = $derived(hasBlocked || sawSecretMissing);
  const applicableCount = $derived(
    plan.hosts.reduce((n, h) => n + h.actions.filter((a) => a.op !== 'noop' && a.op !== 'blocked').length, 0),
  );
  // A review counts what it shows (by op, noop left out, as the backend
  // counts), not the whole host plans' counts.
  const countsEntries = $derived.by((): [string, number][] => {
    if (!(review && owned)) return Object.entries(plan.counts).filter(([, n]) => n > 0);
    const m = new Map<string, number>();
    for (const h of shownHosts) for (const a of h.actions) if (a.op !== 'noop') m.set(a.op, (m.get(a.op) ?? 0) + 1);
    return [...m];
  });
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
    void refocus();
    const r = await applySync(plan.id, forcePartial, controller.signal);
    applying = false;
    onapplying?.(false);
    controller = null;
    if (!r.ok) {
      error = r.error.message;
      if (r.error.code === 'E_SECRET_MISSING') sawSecretMissing = true;
      void refocus();
      return;
    }
    summary = r.value;
    onapplied?.(summary);
    void refocus();
  }

  function cancelApply() {
    controller?.abort();
  }
</script>

<section class="plan-view" aria-label="Sync plan" data-testid="sync-plan-view" tabindex="-1" bind:this={rootEl}>
  <header class="line">
    <button type="button" class="btn btn--quiet" data-testid="plan-back" bind:this={backEl} disabled={applying} onclick={onclose}>← Back</button>
    <h2>{review ? 'Roll-out review' : 'Sync plan'}</h2>
  </header>

  {#if review}
    <p class="muted" data-testid="plan-review-note">Roll out applies only creates, adopts and updates of copies fleet wrote; a held copy waits for your own Sync of that host.</p>
  {/if}

  <div class="counts" data-testid="plan-counts">
    {#each countsEntries as [op, n] (op)}<Badge tone={opTone(op)} label={`${op}: ${n}`} />{/each}
    {#if countsEntries.length === 0}<span class="muted">Nothing to do.</span>{/if}
  </div>

  {#if error}<p class="error" role="alert" data-testid="plan-error">{error}</p>{/if}
  {#if replanError}<p class="error" role="alert" data-testid="plan-replan-error">{replanError}</p>{/if}

  <div class="hosts">
    {#each shownHosts as h (h.host_alias + '::' + h.harness)}
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
          {@const held = review && !!owned && !cardMayApply(a)}
          <div class="action-row" data-testid={`plan-${held ? 'held' : 'action'}-${h.host_alias}-${h.harness}-${a.kind}-${a.name}`}>
            <Badge tone={opTone(a.op)} label={a.op} />
            <span class="asset">{a.kind}/{a.name}</span>
            {#if held}<Badge tone="warn" glyph="◐" label="held — sync it yourself" />{/if}
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

  {#if summary && !review}
    {#each summary.hosts.filter((r) => r.restart_required) as r (r.host_alias)}
      <p class="restart" data-testid={`plan-restart-${r.host_alias}`}>restart Claude on {r.host_alias}</p>
    {/each}
  {/if}

  {#if showForcePartial && !review}
    <label class="force-partial">
      <input type="checkbox" bind:checked={forcePartial} disabled={applying} data-testid="plan-force-partial" />
      Apply anyway, skipping actions blocked on a missing secret
    </label>
  {/if}

  {#if progress && !review}
    <p class="progress" data-testid="plan-progress">
      {progress.done}/{progress.total}{progress.host_alias ? ` — ${progress.host_alias}/${progress.harness}` : ''}
    </p>
  {/if}

  {#if !review}
    <div class="actions">
      {#if applying}
        <button type="button" class="btn" onclick={cancelApply} data-testid="plan-cancel">Cancel</button>
      {/if}
      <button
        type="button"
        class="btn btn--primary"
        class:danger={destructive}
        onclick={apply}
        disabled={applying || applicableCount === 0 || summary !== null}
        data-testid="plan-apply"
      >{applying ? 'Applying…' : 'Apply'}</button>
    </div>
  {/if}
</section>

<style>
  .plan-view { outline: 0; display: flex; flex-direction: column; gap: 10px; padding: 10px 14px; min-height: 100%; box-sizing: border-box; }
  .line { display: flex; align-items: center; gap: 10px; }
  h2 { margin: 0; font-size: 14px; font-weight: 600; letter-spacing: -0.005em; }
  .counts { display: flex; gap: 6px; flex-wrap: wrap; font-size: 12px; }
  .hosts { display: flex; flex-direction: column; gap: 10px; }
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
  .actions button.danger { background: var(--usage-crit); border-color: var(--usage-crit); color: var(--accent-fg); }
  .muted { color: var(--fg-muted); font-size: 12px; }
  .error { color: var(--usage-crit); }
</style>
