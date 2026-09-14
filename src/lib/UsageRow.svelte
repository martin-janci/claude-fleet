<script lang="ts">
  // One line of the usage detail block: name, bar, `% left`, muted `% used`,
  // the severity word + glyph, and the reset line (with pace on weekly).
  // A number is printed only while it is valid: `~` when stale, `? left` when
  // expired, `—` when the window is missing, `checking…` before first load.
  import type { UsageWindow } from './account_usage_store';
  import UsageBar from './UsageBar.svelte';
  import {
    formatReset,
    formatResetShort,
    freshness,
    leftPct,
    paceFraction,
    paceLabel,
    severity,
    severityBadge,
    type ModelBucket,
    type UsageWindowKind,
  } from './account_usage';

  let {
    name,
    window,
    win,
    fetchedAt,
    now,
    hasExtraUsage = false,
    checking = false,
    bindingBucket = null,
    showPace = false,
    note = null,
    locale,
    timeZone,
    testid,
  }: {
    name: string;
    window: UsageWindowKind;
    win: UsageWindow | null;
    fetchedAt: number | null;
    now: number;
    hasExtraUsage?: boolean;
    checking?: boolean;
    /** Named on the weekly line when a model bucket binds. */
    bindingBucket?: ModelBucket | null;
    showPace?: boolean;
    /** A line about this window (e.g. its reset came after the last check). */
    note?: string | null;
    locale?: string;
    timeZone?: string;
    testid: string;
  } = $props();

  const fresh = $derived(win ? freshness(window, fetchedAt, win.resets_at, now) : 'expired');
  const known = $derived(win !== null && fresh !== 'expired');
  const left = $derived(win && known ? leftPct(win) : null);
  const level = $derived(
    win && left !== null ? severity(window, left, win.resets_at, now, hasExtraUsage) : null,
  );
  const badge = $derived(level ? severityBadge(level, hasExtraUsage) : null);
  const leftText = $derived(
    checking && !win
      ? 'checking…'
      : !win
        ? '—'
        : left === null
          ? '? left'
          : `${fresh === 'stale' ? '~' : ''}${left}% left`,
  );
  const bucketText = $derived.by(() => {
    if (!bindingBucket || left === null) return null;
    const b = freshness(window, fetchedAt, bindingBucket.window.resets_at, now);
    if (b === 'expired') return null;
    return `${bindingBucket.model} ${b === 'stale' ? '~' : ''}${bindingBucket.left}% left ▲`;
  });
  const resetText = $derived.by(() => {
    if (!win || level === 'limit') return null;
    if (win.resets_at !== null && win.resets_at <= now) return null;
    const reset = formatReset(window, win.resets_at, now, locale, timeZone);
    const pace = showPace && left !== null
      ? paceLabel((100 - left) / 100, paceFraction(win.resets_at, now))
      : null;
    return pace ? `${reset} · ${pace}` : reset;
  });
</script>

<div class="usage-row" data-testid={testid}>
  <span class="name">{name}</span>
  <UsageBar {window} {win} {fetchedAt} {now} {hasExtraUsage} />
  <span class="nums">
    <span class="left" class:stale={fresh === 'stale' && known} data-testid="usage-left">{leftText}</span>
    {#if bucketText}
      <span class="bucket" data-testid="usage-binding-bucket">· {bucketText}</span>
    {/if}
    {#if left !== null}
      <span class="used" data-testid="usage-used">{100 - left}% used</span>
    {/if}
    {#if badge && win}
      <span class="sev sev-{level}" data-testid="usage-severity"
        >{badge.glyph} {badge.word}{level === 'limit'
          ? ` · ${formatResetShort(window, win.resets_at, now, locale, timeZone)}`
          : ''}</span
      >
    {/if}
  </span>
  {#if resetText}
    <span class="reset" data-testid="usage-reset">{resetText}</span>
  {/if}
  {#if note}
    <span class="note" data-testid="usage-note">{note}</span>
  {/if}
</div>

<style>
  .usage-row {
    display: grid;
    grid-template-columns: 4rem 7rem 1fr;
    column-gap: 0.5rem;
    row-gap: 0.1rem;
    align-items: center;
    font-size: 0.75rem;
  }
  .name { color: var(--fg-muted); }
  .nums {
    display: flex;
    flex-wrap: wrap;
    gap: 0.4rem;
    align-items: baseline;
    font-variant-numeric: tabular-nums;
  }
  .left { color: var(--fg); font-weight: 600; }
  .left.stale { color: var(--fg-muted); }
  .used,
  .bucket { color: var(--fg-muted); }
  .sev { font-weight: 600; font-size: 0.7rem; }
  .sev-caution { color: var(--usage-warn); }
  .sev-low,
  .sev-limit { color: var(--usage-crit); }
  .reset,
  .note {
    grid-column: 3;
    color: var(--fg-muted);
    font-size: 0.7rem;
  }
</style>
