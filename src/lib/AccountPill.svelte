<!--
  An account pill (redesign step 4.3): the session's account with the % left
  of its tighter window, amber from 80% used, red at the limit. A click opens
  the account on the Accounts page and does not select the row under it.
-->
<script lang="ts">
  import { accountByUuid } from './accounts';
  import { accountUsage } from './account_usage_store';
  import { accountPill, openAccount } from './account_pill';

  let {
    uuid,
    clock = () => Math.floor(Date.now() / 1000),
    testid = 'account-pill',
    onopen,
  }: {
    uuid: string;
    clock?: () => number;
    testid?: string;
    /** After the click opened the account (the palette closes itself). */
    onopen?: () => void;
  } = $props();

  const pill = $derived(accountPill(uuid, $accountByUuid.get(uuid), $accountUsage[uuid], clock()));
</script>

<button
  type="button"
  class="pill level-{pill.level}"
  data-testid={testid}
  data-level={pill.level}
  title={pill.title}
  aria-label={pill.title}
  onclick={(e) => {
    e.stopPropagation();
    openAccount(uuid);
    onopen?.();
  }}
  onmousedown={(e) => e.stopPropagation()}
>{pill.text}</button>

<style>
  .pill {
    flex: none;
    max-width: 16ch;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font: inherit;
    font-size: var(--text-2xs);
    line-height: 14px;
    padding: 0 6px;
    border-radius: var(--radius-md);
    border: 1px solid var(--border);
    background: transparent;
    color: var(--fg-muted);
    cursor: pointer;
  }
  .pill:hover {
    background: var(--bg-hover);
    color: var(--fg);
  }
  .level-warn {
    color: var(--usage-warn);
    border-color: color-mix(in srgb, var(--usage-warn) 45%, transparent);
  }
  .level-limit {
    color: var(--usage-crit);
    border-color: color-mix(in srgb, var(--usage-crit) 45%, transparent);
    font-weight: 600;
  }
  .pill:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 1px;
  }
</style>
