<script lang="ts">
  // An `action` item (declarative pages P5): one button that runs a page
  // action's command with no arguments (`pages/actions.rs`), asking first
  // when the action says so. The page re-reads its data items afterwards.
  import ConfirmDialog from '../ConfirmDialog.svelte';
  import { invokeCmd } from '../result';
  import { push, pushError } from '../toasts';
  import type { PageAction } from './pages';

  let { action, onran }: { action: PageAction; onran: () => void } = $props();

  let busy = $state(false);
  let confirming = $state(false);

  async function run() {
    confirming = false;
    busy = true;
    const r = await invokeCmd<unknown>(action.command);
    busy = false;
    if (r.ok) push({ kind: 'success', message: `${action.label}: done` });
    else pushError(r.error, action.label);
    onran();
  }
</script>

<div class="page-action">
  <button
    class="btn"
    type="button"
    disabled={busy}
    data-testid={`page-action-${action.id}`}
    onclick={() => (action.confirm ? (confirming = true) : void run())}>{action.label}</button
  >
  <span class="help">{action.help}</span>
</div>

{#if confirming && action.confirm}
  <ConfirmDialog
    title={action.label}
    message={action.confirm}
    confirmLabel={action.label}
    danger
    confirmTestId={`page-action-confirm-${action.id}`}
    onconfirm={() => void run()}
    oncancel={() => (confirming = false)} />
{/if}

<style>
  .page-action {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    flex-wrap: wrap;
  }
  .help {
    font-size: 11px;
    color: var(--fg-muted);
  }
</style>
