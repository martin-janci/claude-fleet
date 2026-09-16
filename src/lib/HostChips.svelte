<script lang="ts">
  // Host picker chips with NewSessionDialog's rule: every visible host, and
  // an unreachable one disabled unless it is `local`. `lockedReason` lets the
  // owner disable more (with the reason as the chip's title).
  //
  // With `showUsage` (the New-session dialog) each chip also shows its
  // account's headroom — % left AND the reset time, `~`+◷ when stale, `? left`
  // when expired, `no account` / `offline` — and the selected chip gets a full
  // line below the row plus a warning when its account is low. Picking is
  // always the user's: nothing here ever changes the chosen host.
  import { hosts } from './hosts';
  import { accounts } from './accounts';
  import { accountUsage } from './account_usage_store';
  import { hostChipUsage, lowHeadroomWarning, selectedUsageLine } from './usage_glance';

  let {
    active,
    labelId,
    disabled = false,
    lockedReason = () => null,
    onpick,
    showUsage = false,
    now = Math.floor(Date.now() / 1000),
    locale,
    timeZone,
  }: {
    active: string;
    /** DOM id for the "Host" caption that names the group. */
    labelId: string;
    /** Disable every chip (e.g. while a request is in flight). */
    disabled?: boolean;
    lockedReason?: (alias: string) => string | null;
    onpick: (alias: string) => void;
    /** Show each host's account usage (New-session dialog). */
    showUsage?: boolean;
    /** Unix seconds for the usage wording. */
    now?: number;
    locale?: string;
    timeZone?: string;
  } = $props();

  const visible = $derived($hosts.filter((h) => !h.hidden));
  const accountOf = (uuid: string | null) => (uuid ? ($accounts.find((a) => a.uuid === uuid) ?? null) : null);
  const snapshotOf = (uuid: string | null) => (uuid ? ($accountUsage[uuid] ?? null) : null);

  const selected = $derived(showUsage ? (visible.find((h) => h.alias === active) ?? null) : null);
  const selectedLine = $derived(
    selected
      ? selectedUsageLine(selected, accountOf(selected.account_uuid), snapshotOf(selected.account_uuid), now, locale, timeZone)
      : null,
  );
  const warning = $derived(
    selected
      ? lowHeadroomWarning(
          selected,
          $hosts,
          accountOf(selected.account_uuid),
          snapshotOf(selected.account_uuid),
          now,
          locale,
          timeZone,
        )
      : null,
  );
</script>

<span class="label" id={labelId}>Host</span>
<div class="host-row" class:with-usage={showUsage} role="group" aria-labelledby={labelId}>
  {#each visible as h (h.alias)}
    {@const locked = lockedReason(h.alias)}
    <button
      type="button"
      class="host-pick"
      class:active={active === h.alias}
      aria-pressed={active === h.alias}
      data-alias={h.alias}
      disabled={disabled || (!h.reachable && h.alias !== 'local') || locked !== null}
      title={locked ?? undefined}
      onclick={() => onpick(h.alias)}
    >
      <span class="alias">{h.alias}</span>
      {#if showUsage}
        <span class="usage" data-testid="chip-usage"
          >{hostChipUsage(h, accountOf(h.account_uuid), snapshotOf(h.account_uuid), now, locale, timeZone)}</span
        >
      {/if}
    </button>
  {/each}
</div>
{#if selectedLine}
  <p class="selected-line" data-testid="host-usage-line">{selectedLine}</p>
{/if}
{#if warning}
  <p class="warning" role="status" data-testid="host-usage-warning">{warning}</p>
{/if}

<style>
  .label { font-size: 0.7rem; color: var(--fg-muted); text-transform: uppercase; }
  .host-row {
    display: flex;
    gap: 0.3rem;
    flex-wrap: wrap;
    max-height: 5.2rem;
    overflow-y: auto;
  }
  .host-row.with-usage { max-height: 8.4rem; }
  .host-pick {
    font-size: 0.75rem;
    padding: 0.2rem 0.6rem;
    border: 1px solid var(--border);
    background: transparent;
    color: var(--fg-muted);
    border-radius: 999px;
    cursor: pointer;
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
  }
  .with-usage .host-pick {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 0.05rem;
    border-radius: 6px;
    text-align: left;
  }
  .usage {
    font-family: system-ui, sans-serif;
    font-size: 0.68rem;
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
  }
  .host-pick.active { color: var(--fg); border-color: var(--accent); }
  .host-pick:disabled { opacity: 0.4; cursor: not-allowed; }
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
</style>
