<script lang="ts">
  // Redesign step 1.1: one "12 stopped on trn · Restore" row for a mass loss
  // (lost_fold.ts). The label expands to the rows themselves; Restore runs
  // the same `restore_host_sessions` flow as HostDetail's "Restore n lost
  // sessions": a dry run for the plan, a confirm that lists it, then the
  // batch, narrowed to the rows this client may restore.
  import type { Snippet } from 'svelte';
  import type { RestorePlanEntry, SessionRow } from './sessions';
  import { restoreHostSessions } from './sessions';
  import { foldLabel, type LostFold } from './lost_fold';
  import { hubStatus, hubActionBlocked } from './hub';
  import { hubConnection } from './hub_connection';
  import { bulkTargets, sessionBlocked } from './share';
  import { push, pushError } from './toasts';
  import ConfirmDialog from './ConfirmDialog.svelte';

  let {
    fold,
    open,
    ontoggle,
    row,
  }: {
    fold: LostFold;
    open: boolean;
    ontoggle: () => void;
    /** Renders one session row (the Sidebar's own row snippet). */
    row: Snippet<[SessionRow]>;
  } = $props();

  const mine = $derived(bulkTargets(fold.rows, 'restore_host_sessions', $sessionBlocked));
  const mineIds = $derived(new Set(mine.map((s) => s.id)));
  // The hub's half first (Restore routes through it), then the access half.
  const blocked = $derived(
    hubActionBlocked('restore_host_sessions', $hubStatus, $hubConnection) ??
      (mine.length === 0
        ? ($sessionBlocked(fold.rows[0], 'restore_host_sessions') ??
          'None of these sessions are yours to restore.')
        : null),
  );

  let busy = $state(false);
  let plan = $state<RestorePlanEntry[] | null>(null);
  const planCount = $derived((plan ?? []).filter((e) => e.action === 'restore' && mineIds.has(e.session_id)).length);
  const notMine = $derived(fold.rows.length - mine.length);

  async function onRestore() {
    // Asked again at the click: a grant can narrow after the render.
    if (blocked !== null || busy) return;
    busy = true;
    const r = await restoreHostSessions(fold.host, { dryRun: true, sessionIds: mine.map((s) => s.id) });
    busy = false;
    if (!r.ok) {
      pushError(r.error, `Restore on ${fold.host} failed`);
      return;
    }
    plan = r.value.plan;
  }

  async function confirmRestore() {
    if (blocked !== null) return;
    const host = fold.host;
    const ids = (plan ?? []).filter((e) => e.action === 'restore' && mineIds.has(e.session_id)).map((e) => e.session_id);
    if (ids.length === 0) {
      plan = null;
      return;
    }
    busy = true;
    const r = await restoreHostSessions(host, { sessionIds: ids });
    busy = false;
    plan = null;
    if (!r.ok) {
      pushError(r.error, `Restore on ${host} failed`);
      return;
    }
    const results = r.value.results;
    const ok = results.filter((x) => x.ok).length;
    const failed = results.filter((x) => !x.ok);
    if (failed.length === 0) {
      push({ kind: 'success', message: `Restored ${ok} of ${results.length} sessions on ${host}.` });
    } else {
      push({
        kind: 'warning',
        sticky: true,
        message: `Restored ${ok} of ${results.length} sessions on ${host}. Failed: ${failed
          .map((x) => `${x.tmux_name}: ${x.error ?? 'unknown error'}`)
          .join('; ')}`,
      });
    }
  }
</script>

<div class="lost-fold" data-testid="lost-fold" data-host={fold.host}>
  <div class="fold-row">
    <button
      type="button"
      class="fold-label"
      data-testid="lost-fold-toggle"
      aria-expanded={open}
      onclick={ontoggle}
    >
      <span class="caret" class:collapsed={!open}>▾</span>
      {foldLabel(fold)}
    </button>
    <button
      type="button"
      class="btn btn--quiet fold-restore"
      data-testid="lost-fold-restore"
      disabled={blocked !== null || busy}
      title={blocked ?? `Restore the stopped sessions on ${fold.host}; each resumes its Claude conversation`}
      onclick={onRestore}>Restore…</button
    >
  </div>
  {#if open}
    {#each fold.rows as s (s.id)}
      {@render row(s)}
    {/each}
  {/if}
</div>

{#if plan !== null}
  <ConfirmDialog
    title="Restore stopped sessions on {fold.host}?"
    confirmLabel="Restore"
    {busy}
    confirmDisabled={planCount === 0}
    confirmTestId="lost-fold-confirm"
    onconfirm={confirmRestore}
    oncancel={() => (plan = null)}
  >
    <ul class="restore-plan">
      {#each plan as entry (entry.session_id)}
        <li>
          <span class="name">{entry.friendly_name ?? entry.tmux_name}</span>
          {#if entry.action === 'skip'}<span class="skip">skipped: {entry.reason}</span>{/if}
        </li>
      {/each}
    </ul>
    {#if notMine > 0}
      <p class="note" data-testid="lost-fold-not-mine">
        {notMine} of the stopped sessions belong to someone else and will be left alone.
      </p>
    {/if}
    {#if planCount === 0}
      <p class="note">Nothing here can be restored.</p>
    {:else}
      <p class="note">Each session resumes its Claude conversation. Any first-run prompt waits for you.</p>
    {/if}
  </ConfirmDialog>
{/if}

<style>
  /* The Sidebar's section look (Outside fleet), scoped here. */
  .lost-fold {
    border-top: 1px solid var(--border);
    padding-top: 0.35rem;
    margin-top: 0.35rem;
  }
  .fold-row {
    display: flex;
    align-items: center;
    gap: 6px;
    padding-right: 0.4rem;
  }
  .fold-label {
    display: flex;
    align-items: center;
    gap: 0.3rem;
    flex: 1 1 auto;
    min-width: 0;
    background: transparent;
    border: none;
    cursor: pointer;
    font-family: inherit;
    font-size: 0.75rem;
    color: var(--fg-2);
    padding: 0.15rem 0 0.2rem 0.4rem;
    text-align: left;
  }
  .caret {
    color: var(--fg-muted);
    font-size: 0.65rem;
    width: 0.7rem;
    text-align: center;
    transition: transform var(--dur-fast) ease;
    display: inline-block;
  }
  .caret.collapsed { transform: rotate(-90deg); }
  .fold-restore {
    color: var(--accent);
    flex: 0 0 auto;
  }
  .restore-plan {
    margin: 0 0 0.5rem;
    padding-left: 1.1rem;
    max-height: 14rem;
    overflow-y: auto;
  }
  .restore-plan .skip {
    margin-left: 0.4rem;
    color: var(--fg-muted);
  }
  .note {
    margin: 0.4rem 0 0;
    color: var(--fg-muted);
  }
</style>
