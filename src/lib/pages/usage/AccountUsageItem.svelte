<script lang="ts">
  // The catalog's `account_usage` item (declarative pages L8): one account's
  // plan headroom drawn as `view`. The wording, staleness and severity rules
  // are the pure model's (`account_usage.ts`, `usage_glance.ts`,
  // `hosts_view.ts`, binding spec: the hosts-view design's "Showing usage",
  // "Staleness and failure" and "Glanceable surfaces"); this only draws them.
  import UsageBlock from './UsageBlock.svelte';
  import UsageBar from './UsageBar.svelte';
  import { compactWindow, freshnessMark } from '../../hosts_view';
  import { footerUsage, hostChipUsage, lowHeadroomWarning, selectedUsageLine } from '../../usage_glance';
  import type { UsageView } from '../pages';
  import type { UsageContext } from './context';

  let { view, ctx }: { view: UsageView; ctx: UsageContext } = $props();

  const MINI = [
    { kind: '5h' as const, short: '5h' },
    { kind: 'weekly' as const, short: 'wk' },
  ];

  const snap = $derived(ctx.snapshot ?? null);
  const account = $derived(ctx.account ?? null);
  const footer = $derived(
    view === 'footer' ? footerUsage(ctx.hosts ?? [], ctx.accounts ?? [], ctx.snapshots ?? {}, ctx.now) : null,
  );
  const line = $derived(
    view === 'line' && ctx.host
      ? selectedUsageLine(ctx.host, account, snap, ctx.now, ctx.locale, ctx.timeZone)
      : null,
  );
  const warning = $derived(
    view === 'warning' && ctx.host
      ? lowHeadroomWarning(ctx.host, ctx.hosts ?? [], account, snap, ctx.now, ctx.locale, ctx.timeZone)
      : null,
  );
</script>

{#if view === 'block'}
  <UsageBlock
    {account}
    snapshot={snap}
    sharedWith={ctx.sharedWith ?? []}
    now={ctx.now}
    locale={ctx.locale}
    timeZone={ctx.timeZone}
    suppressUnavailable={ctx.suppressUnavailable ?? false}
    onRefresh={account ? ctx.onrefresh : undefined}
    refreshBlocked={ctx.refreshBlocked ?? null}
    refreshKey={ctx.refreshKey ?? null}
  />
{:else if view === 'freshness'}
  {@const tier = snap?.subscription ?? account?.seat_tier ?? null}
  {@const fm = freshnessMark(snap, ctx.now)}
  {#if tier}<span class="tier" data-testid="group-tier">{tier}</span>{/if}
  <span class="fresh fresh-{fm.state}" title={fm.title} data-testid="group-freshness">{fm.mark}</span>
{:else if view === 'bars'}
  {#each MINI as { kind, short } (kind)}
    {@const cw = compactWindow(kind, snap, ctx.now, ctx.locale, ctx.timeZone)}
    {@const win = snap?.usage ? (kind === '5h' ? snap.usage.five_hour : snap.usage.seven_day) : null}
    <div class="mini" data-testid="group-usage-{kind}">
      <span class="mini-name">{short}</span>
      <UsageBar
        window={kind}
        {win}
        fetchedAt={snap?.fetched_at ?? null}
        now={ctx.now}
        hasExtraUsage={account?.has_extra_usage ?? false}
        compact
      />
      <span class="mini-left" class:muted={cw.freshness !== 'fresh'}>{cw.left}</span>
      {#if cw.reset}<span class="mini-reset">· {cw.reset}</span>{/if}
    </div>
  {/each}
{:else if view === 'chip'}
  {#if ctx.host}
    <span class="chip-usage" data-testid="chip-usage"
      >{hostChipUsage(ctx.host, account, snap, ctx.now, ctx.locale, ctx.timeZone)}</span
    >
  {/if}
{:else if view === 'line'}
  {#if line}<p class="selected-line" data-testid="host-usage-line">{line}</p>{/if}
{:else if view === 'warning'}
  {#if warning}<p class="warning" role="status" data-testid="host-usage-warning">{warning}</p>{/if}
{:else if view === 'footer'}
  {#if footer}
    <button
      type="button"
      class="usage-seg tone-{footer.tone}"
      data-testid="footer-usage"
      data-state={footer.state}
      aria-label={footer.ariaLabel}
      title={footer.ariaLabel}
      onclick={() => ctx.onopenhost?.(footer.host)}>{footer.text}</button
    >
  {/if}
{/if}

<style>
  .tier { color: var(--fg-muted); font-size: 0.7rem; }
  .fresh {
    margin-left: auto;
    color: var(--fg-muted);
    font-size: 0.7rem;
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
  }
  .fresh-expired { color: var(--usage-warn); }
  .mini {
    display: flex;
    align-items: center;
    gap: 0.35rem;
    font-size: 0.7rem;
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
    min-width: 0;
  }
  .mini-name { width: 1.3rem; color: var(--fg-muted); }
  .mini-left { font-weight: 600; }
  .mini-left.muted { color: var(--fg-muted); font-weight: 400; }
  .mini-reset { color: var(--fg-muted); overflow: hidden; text-overflow: ellipsis; }
  .chip-usage {
    font-family: system-ui, sans-serif;
    font-size: 0.68rem;
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
  }
  .selected-line {
    margin: 0;
    font-size: 0.72rem;
    color: var(--fg-muted);
    font-variant-numeric: tabular-nums;
  }
  .warning {
    margin: 0;
    font-size: 0.75rem;
    color: var(--usage-crit);
  }
  .usage-seg {
    margin-left: auto;
    background: transparent;
    border: none;
    padding: 0 0.3rem;
    font: inherit;
    font-variant-numeric: tabular-nums;
    color: var(--fg);
    cursor: pointer;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .usage-seg:hover { text-decoration: underline; }
  .usage-seg.tone-muted { color: var(--fg-muted); }
  .usage-seg.tone-warn { color: var(--usage-warn); }
  .usage-seg.tone-alarm { color: var(--usage-crit); }
</style>
