<script lang="ts">
  // What a saved routine's runs already handled (migration 212; fleet-core
  // `routines::seen`): each item with the state the last run saw, so the
  // next run skips the ones that did not change. Forget drops one (the next
  // run looks at it again); Forget all drops every one.
  import { onMount } from 'svelte';
  import Button from '../kit/Button.svelte';
  import { errorText } from '../error_copy';
  import { forgetRoutineSeen, routineSeen, type RoutineSeen } from '../routines';

  // One per routine: the editor keys it on the routine's id, so a switch
  // to another routine mounts a fresh one.
  let { routineId }: { routineId: number } = $props();

  /** Rows shown before "Show all". */
  const SHOWN = 8;

  let seen = $state<RoutineSeen | null>(null);
  let error: string | null = $state(null);
  let busy = $state(false);
  let all = $state(false);
  /** The newest load or write: an older reply is dropped. */
  let seq = 0;

  async function load() {
    const mine = ++seq;
    error = null;
    const r = await routineSeen(routineId);
    if (mine !== seq) return;
    if (r.ok) seen = r.value;
    else {
      seen = null;
      // A hub older than seen items answers E_INVALID: nothing to show.
      error = r.error.code === 'E_INVALID' ? null : errorText(r.error);
    }
  }

  onMount(() => void load());

  async function forget(item?: string) {
    const mine = ++seq;
    busy = true;
    error = null;
    const r = await forgetRoutineSeen(routineId, item);
    busy = false;
    if (mine !== seq) return;
    if (r.ok) seen = r.value;
    else error = errorText(r.error);
  }

  const rows = $derived(seen ? (all ? seen.items : seen.items.slice(0, SHOWN)) : []);
  const when = (at: number) => new Date(at * 1000).toLocaleString();
</script>

{#if seen || error}
  <div class="seen" data-testid="routine-seen">
    {#if error}<p class="errt" data-testid="routine-seen-error">{error}</p>{/if}
    {#if seen}
      {#if seen.total === 0}
        <p class="help" data-testid="routine-seen-empty">Nothing handled yet: the next run lists what it looks at.</p>
      {:else}
        <p class="help" data-testid="routine-seen-count">
          {seen.total === 1 ? '1 item' : `${seen.total} items`} its runs handled. A run skips one whose state is the same and reports it only once it changes.
        </p>
        <ul class="list">
          {#each rows as row (row.item)}
            <li class="it" data-testid="routine-seen-item">
              <div class="txt">
                <span class="name mono">{row.item}</span>
                {#if row.state}<span class="state mono">{row.state}</span>{/if}
                <span class="meta">Seen {when(row.seen_at)}{row.changed_at !== row.first_seen_at ? ` · changed ${when(row.changed_at)}` : ''}</span>
              </div>
              {#if seen.may_change}
                <Button size="sm" variant="quiet" testid="routine-seen-forget" disabled={busy} onclick={() => forget(row.item)}>Forget</Button>
              {/if}
            </li>
          {/each}
        </ul>
        <div class="row">
          {#if !all && seen.items.length > SHOWN}
            <Button size="sm" variant="quiet" testid="routine-seen-all" onclick={() => (all = true)}>Show all {seen.items.length}</Button>
          {/if}
          {#if seen.may_change}
            <Button size="sm" variant="quiet" testid="routine-seen-forget-all" disabled={busy} onclick={() => forget()}>Forget all</Button>
          {/if}
        </div>
      {/if}
    {/if}
  </div>
{/if}

<style>
  .seen { display: flex; flex-direction: column; gap: 6px; }
  .help { margin: 0; font-size: var(--text-xs); line-height: 16px; color: var(--fg-muted); }
  .errt { margin: 0; font-size: var(--text-xs); line-height: 16px; color: var(--status-failed); }
  .list { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: 2px; }
  .it { display: flex; align-items: center; gap: var(--space-2); padding: 4px 6px; border-radius: var(--radius-sm); background: var(--bg-pane); min-width: 0; }
  .txt { display: flex; flex-direction: column; min-width: 0; flex: 1; }
  .name { font-size: var(--text-xs); color: var(--fg); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .state { font-size: var(--text-2xs); color: var(--fg-2); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .meta { font-size: var(--text-2xs); color: var(--fg-muted); }
  .mono { font-family: var(--font-mono); }
  .row { display: flex; gap: var(--space-2); }
</style>
