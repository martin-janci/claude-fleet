<script lang="ts">
  // One usage window as a meter. Fills with USED, like the context meter.
  // ok is neutral (the healthy state stays quiet), caution amber, low/limit
  // red with diagonal stripes. Stale: dimmed with a dotted outline. Unknown
  // (expired, never fetched, missing): a DASHED EMPTY track — never a solid
  // empty bar, which would read as 100% free.
  import type { UsageWindow } from './account_usage_store';
  import {
    freshness,
    leftPct,
    paceFraction,
    severity,
    type UsageWindowKind,
  } from './account_usage';

  let {
    window,
    win,
    fetchedAt,
    now,
    hasExtraUsage = false,
    compact = false,
  }: {
    window: UsageWindowKind;
    win: UsageWindow | null;
    fetchedAt: number | null;
    /** Unix seconds. */
    now: number;
    hasExtraUsage?: boolean;
    compact?: boolean;
  } = $props();

  const fresh = $derived(win ? freshness(window, fetchedAt, win.resets_at, now) : 'expired');
  const known = $derived(win !== null && fresh !== 'expired');
  const left = $derived(win && known ? leftPct(win) : null);
  const used = $derived(left === null ? null : 100 - left);
  const level = $derived(
    win && left !== null ? severity(window, left, win.resets_at, now, hasExtraUsage) : null,
  );
  const pace = $derived(
    window === 'weekly' && win && known ? paceFraction(win.resets_at, now) : null,
  );
  const label = $derived(
    left === null ? 'usage unknown' : fresh === 'stale' ? `about ${left}% left` : `${left}% left`,
  );
</script>

<span
  class="usage-bar"
  class:compact
  class:stale={known && fresh === 'stale'}
  class:unknown={!known}
  data-testid="usage-bar"
  data-window={window}
  data-level={level ?? 'unknown'}
  data-freshness={known ? fresh : 'unknown'}
  role="meter"
  aria-valuemin="0"
  aria-valuemax="100"
  aria-valuenow={used ?? undefined}
  aria-label={label}
>
  {#if used !== null}
    <span class="fill level-{level}" data-testid="usage-fill" style="width: {used}%;"></span>
  {/if}
  {#if pace !== null}
    <span class="pace" data-testid="usage-pace" style="left: {pace * 100}%;" aria-hidden="true"></span>
  {/if}
</span>

<style>
  .usage-bar {
    position: relative;
    display: inline-block;
    box-sizing: border-box;
    width: 7rem;
    height: 0.55rem;
    border-radius: 2px;
    border: 1px solid var(--border);
    background: color-mix(in srgb, var(--fg) 6%, transparent);
    overflow: hidden;
    vertical-align: middle;
    flex-shrink: 0;
  }
  .usage-bar.compact { width: 3.5rem; height: 0.4rem; }
  .usage-bar.stale {
    opacity: 0.55;
    outline: 1px dotted var(--fg-muted);
    outline-offset: 1px;
  }
  .usage-bar.unknown {
    background: transparent;
    border: 1px dashed var(--fg-muted);
  }
  .fill {
    position: absolute;
    left: 0;
    top: 0;
    bottom: 0;
  }
  .fill.level-ok { background: var(--fg-muted); }
  .fill.level-caution { background: var(--usage-warn); }
  .fill.level-low,
  .fill.level-limit {
    background: repeating-linear-gradient(
      -45deg,
      var(--usage-crit) 0 3px,
      color-mix(in srgb, var(--usage-crit) 45%, transparent) 3px 5px
    );
  }
  .pace {
    position: absolute;
    top: -1px;
    bottom: -1px;
    width: 1px;
    margin-left: -0.5px;
    background: var(--fg);
    opacity: 0.7;
  }
</style>
