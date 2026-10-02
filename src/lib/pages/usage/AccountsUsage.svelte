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
  import { pushError } from '../../toasts';
  import type { UsageView } from '../pages';

  let { view }: { view: UsageView } = $props();

  let now = $state(Math.floor(Date.now() / 1000));
  const timer = setInterval(() => (now = Math.floor(Date.now() / 1000)), 15_000);
  onDestroy(() => clearInterval(timer));

  // No `refreshBlocked`. `refresh_account_usage` IS `LocalOnly`, but this page
  // is a data item on a declarative page, and a paired desktop (`remote`) draws
  // no data items at all — so every path that reaches this component is a
  // standalone one, where `hubBlock` answers null. The derived, its import and
  // the ctx field were inert. If data items are ever shown in `remote` mode,
  // the reason belongs here, and it is the `LocalOnly` one.
  const sorted = $derived([...$accounts].sort((a, b) => accountLabel(a).localeCompare(accountLabel(b))));

  /** Hosts logged in to this account right now. */
  const hostsOf = (uuid: string) => $hosts.filter((h) => h.account_uuid === uuid).map((h) => h.alias);

  /**
   * An account no host is logged in to and that has never answered.
   *
   * `accounts` is every row the table ever held, so an account a host logged
   * out of stays in it — and nothing will ever fetch usage for one, because the
   * fetch goes over a host's SSH connection. Drawn as a full usage block it was
   * a permanent "no usage yet" card that no refresh could ever fill. One muted
   * line says what it is instead.
   */
  const retired = (uuid: string) => hostsOf(uuid).length === 0 && !$accountUsage[uuid]?.fetched_at;

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
        {#if retired(a.uuid)}
          <p class="retired" data-testid="accounts-usage-retired">
            {accountLabel(a)} — no host is logged in to this account, so there is no usage to read.
          </p>
        {:else}
        <AccountUsageItem
          {view}
          ctx={{
            now,
            account: a,
            snapshot: $accountUsage[a.uuid] ?? null,
            // `sharedWith` is contracted as "the OTHER hosts logged in to this
            // account" — a list relative to one host. This page is per
            // ACCOUNT and names no host, so there is no "other": handing it
            // every host made the line read "· shared with mefistos" beside
            // an account whose only host is mefistos. The hosts are named by
            // the view itself instead.
            sharedWith: [],
            onrefresh: () => void refresh(a.uuid),
          }}
        />
        {/if}
      </div>
    {/each}
  </div>
{/if}

<style>
  .retired {
    color: var(--fg-muted);
    margin: 0;
    padding: 6px 0;
  }
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
    font-size: 0.8rem;
    color: var(--fg-muted);
    margin: 0;
  }
</style>
