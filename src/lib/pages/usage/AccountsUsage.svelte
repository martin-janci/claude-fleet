<script lang="ts">
  // An `account_usage` item on a page (declarative pages L8; Usage → Claude
  // accounts): every known account, one after another, each in `view`
  // (a page takes `block` only). No screen hands it a context, so it builds
  // each account's from the live stores the app already keeps current
  // (`list_account_usage` + `account_usage:updated`), ticks its own clock,
  // and refreshes through the same floor the Hosts view respects.
  import { onDestroy } from 'svelte';
  import AccountUsageItem from './AccountUsageItem.svelte';
  import { accounts, accountLabel } from '../../accounts';
  import { accountUsage, refreshAccountUsage } from '../../account_usage_store';
  import { refreshCountdown } from '../../account_usage';
  import { hosts } from '../../hosts';
  import { hubBlock, hubStatus } from '../../hub';
  import { pushError } from '../../toasts';
  import type { UsageView } from '../pages';

  let { view }: { view: UsageView } = $props();

  let now = $state(Math.floor(Date.now() / 1000));
  const timer = setInterval(() => (now = Math.floor(Date.now() / 1000)), 15_000);
  onDestroy(() => clearInterval(timer));

  const refreshBlocked = $derived(hubBlock('refresh_account_usage', $hubStatus));
  const sorted = $derived([...$accounts].sort((a, b) => accountLabel(a).localeCompare(accountLabel(b))));

  async function refresh(uuid: string) {
    const snap = $accountUsage[uuid];
    if (snap && refreshCountdown(snap.next_try_at, now) !== null) return;
    const r = await refreshAccountUsage(uuid);
    if (!r.ok) pushError(r.error, 'Usage refresh failed');
  }
</script>

{#if sorted.length === 0}
  <p class="empty" data-testid="accounts-usage-empty">No Claude accounts yet: a host logged in to Claude adds one.</p>
{:else}
  <div class="accounts" data-testid="accounts-usage">
    {#each sorted as a (a.uuid)}
      <div class="account" data-testid="accounts-usage-account" data-uuid={a.uuid}>
        <AccountUsageItem
          {view}
          ctx={{
            now,
            account: a,
            snapshot: $accountUsage[a.uuid] ?? null,
            sharedWith: $hosts.filter((h) => h.account_uuid === a.uuid).map((h) => h.alias),
            onrefresh: () => void refresh(a.uuid),
            refreshBlocked,
          }}
        />
      </div>
    {/each}
  </div>
{/if}

<style>
  .accounts {
    display: flex;
    flex-direction: column;
    gap: 0.9rem;
    flex: 1 1 100%;
  }
  .account + .account {
    border-top: 1px solid var(--border);
    padding-top: 0.7rem;
  }
  .empty {
    font-size: var(--text-2xs);
    color: var(--fg-muted);
    margin: 0;
  }
</style>
