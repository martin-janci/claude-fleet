<script lang="ts">
  // The usage detail block for one Claude account (spec: "Showing usage" and
  // "Staleness and failure"). Header, the 5-hour and weekly rows, the
  // per-model disclosure, every status message from `statusMessage`, and the
  // `via <host> · checked N min ago` footer with a floor-respecting refresh.
  import type { AccountUsageSnapshot } from './account_usage_store';
  import { accountLabel, type AccountRow } from './accounts';
  import { copyText } from './clipboard';
  import UsageRow from './UsageRow.svelte';
  import {
    bindingModelBucket,
    checkedAgo,
    modelBuckets,
    notableBuckets,
    refreshCountdown,
    statusMessage,
    type UsageWindowKind,
  } from './account_usage';

  let {
    account,
    snapshot,
    sharedWith = [],
    now,
    onRefresh,
    locale,
    timeZone,
    suppressUnavailable = false,
    refreshBlocked = null,
  }: {
    account: AccountRow | null;
    snapshot: AccountUsageSnapshot | null;
    /** The other hosts logged in to this account. */
    sharedWith?: string[];
    /** Unix seconds; the owner ticks it. */
    now: number;
    onRefresh?: () => void;
    locale?: string;
    timeZone?: string;
    /** The owner shows ONE endpoint-unavailable banner for every account, so
     *  this block drops its own unavailable line and Copy details. */
    suppressUnavailable?: boolean;
    /** `hubBlock('refresh_account_usage', …)` from the owner — a refresh
     *  SSHes to the host from here, and a hub client has no such connection. */
    refreshBlocked?: string | null;
  } = $props();

  const msg = $derived(statusMessage(snapshot, account, sharedWith, now, locale, timeZone));
  const usage = $derived(snapshot?.usage ?? null);
  const fetchedAt = $derived(snapshot?.fetched_at ?? null);
  const hasExtra = $derived(account?.has_extra_usage ?? false);
  const buckets = $derived(modelBuckets(usage));
  const autoOpen = $derived(notableBuckets(usage).length > 0);
  // `null` until the user toggles: follow the auto-open rule until then.
  let toggled = $state<boolean | null>(null);
  const perModelOpen = $derived(toggled ?? autoOpen);
  const countdown = $derived(snapshot ? refreshCountdown(snapshot.next_try_at, now) : null);
  const generalLines = $derived(
    msg.lines.filter((l) => !l.window && !(suppressUnavailable && l.kind === 'unavailable')),
  );
  const showCopy = $derived(
    msg.copyDetail !== null &&
      snapshot?.status !== 'ok' &&
      !(suppressUnavailable && snapshot?.status === 'unavailable'),
  );
  let copied = $state(false);

  function noteFor(w: UsageWindowKind): string | null {
    const lines = msg.lines.filter((l) => l.window === w);
    return lines.length ? lines.map((l) => l.text).join(' ') : null;
  }

  async function copyDetails() {
    if (msg.copyDetail) copied = await copyText(msg.copyDetail);
  }
</script>

<section class="usage-block" data-testid="usage-block" aria-label="Usage">
  <header>
    <span class="title">USAGE</span>
    {#if account}
      <span class="acct" title={account.email ?? undefined} data-testid="usage-account"
        >{accountLabel(account)}</span
      >
      {#if snapshot?.subscription}
        <span class="meta">· {snapshot.subscription}</span>
      {/if}
      {#if sharedWith.length > 0}
        <span class="meta" data-testid="usage-shared">· shared with {sharedWith.join(', ')}</span>
      {/if}
      {#if onRefresh}
        <button
          type="button"
          class="refresh"
          data-testid="usage-refresh"
          disabled={countdown !== null || refreshBlocked !== null}
          title={refreshBlocked ?? ''}
          onclick={() => onRefresh?.()}
        >
          {#if refreshBlocked}refresh{:else if countdown !== null}refresh available in {countdown}{:else}<kbd>u</kbd> refresh{/if}
        </button>
      {/if}
    {/if}
  </header>

  {#if generalLines.length > 0}
    <ul class="messages">
      {#each generalLines as line, i (i)}
        <li class="msg tone-{line.tone}" data-testid="usage-message" data-kind={line.kind}>
          {#if line.glyph}<span class="glyph" aria-hidden="true">{line.glyph}</span>{/if}
          {line.text}
        </li>
      {/each}
    </ul>
  {/if}
  {#if showCopy}
    <button type="button" class="link" data-testid="usage-copy-details" onclick={copyDetails}
      >{copied ? 'Copied' : 'Copy details'}</button
    >
  {/if}

  {#if account}
    <div class="rows">
      <UsageRow
        testid="usage-row-5h"
        name="5-hour"
        window="5h"
        win={usage?.five_hour ?? null}
        {fetchedAt}
        {now}
        hasExtraUsage={hasExtra}
        checking={msg.checking}
        note={noteFor('5h')}
        {locale}
        {timeZone}
      />
      <UsageRow
        testid="usage-row-weekly"
        name="Weekly"
        window="weekly"
        win={usage?.seven_day ?? null}
        {fetchedAt}
        {now}
        hasExtraUsage={hasExtra}
        checking={msg.checking}
        bindingBucket={bindingModelBucket(usage)}
        showPace
        note={noteFor('weekly')}
        {locale}
        {timeZone}
      />
    </div>

    {#if buckets.length > 0}
      <button
        type="button"
        class="link disclosure"
        data-testid="usage-per-model"
        aria-expanded={perModelOpen}
        onclick={() => (toggled = !perModelOpen)}>Per-model {perModelOpen ? '▾' : '▸'}</button
      >
      {#if perModelOpen}
        <div class="rows" data-testid="usage-per-model-rows">
          {#each buckets as b (b.model)}
            <UsageRow
              testid="usage-row-{b.model.toLowerCase()}"
              name={b.model}
              window="weekly"
              win={b.window}
              {fetchedAt}
              {now}
              hasExtraUsage={hasExtra}
              {locale}
              {timeZone}
            />
          {/each}
        </div>
      {/if}
    {/if}

    {#if fetchedAt !== null}
      <footer data-testid="usage-footer">
        {snapshot?.source_host ? `via ${snapshot.source_host} · ` : ''}{checkedAgo(fetchedAt, now)}
      </footer>
    {/if}
  {/if}
</section>

<style>
  .usage-block {
    display: flex;
    flex-direction: column;
    gap: 0.4rem;
    font-size: 0.75rem;
  }
  header {
    display: flex;
    flex-wrap: wrap;
    align-items: baseline;
    gap: 0.35rem;
  }
  .title {
    font-size: 0.7rem;
    letter-spacing: 0.04em;
    color: var(--fg-muted);
  }
  .acct { font-weight: 600; }
  .meta { color: var(--fg-muted); }
  .refresh {
    margin-left: auto;
    font-size: 0.7rem;
    padding: 0.1rem 0.45rem;
    border: 1px solid var(--border);
    border-radius: 4px;
    background: transparent;
    color: var(--fg);
    cursor: pointer;
    font-variant-numeric: tabular-nums;
  }
  .refresh:disabled { color: var(--fg-muted); cursor: default; }
  kbd {
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    font-size: 0.65rem;
    padding: 0 0.2rem;
    border: 1px solid var(--border);
    border-radius: 3px;
  }
  .messages {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 0.2rem;
  }
  .msg { line-height: 1.35; }
  .glyph { margin-right: 0.25rem; }
  .tone-muted { color: var(--fg-muted); }
  .tone-warn { color: var(--usage-warn); }
  .tone-alarm { color: var(--usage-crit); }
  .rows {
    display: flex;
    flex-direction: column;
    gap: 0.35rem;
  }
  .link {
    align-self: flex-start;
    border: none;
    background: none;
    padding: 0;
    font-size: 0.7rem;
    color: var(--accent);
    cursor: pointer;
  }
  .disclosure { color: var(--fg-muted); }
  footer { color: var(--fg-muted); font-size: 0.7rem; }
</style>
