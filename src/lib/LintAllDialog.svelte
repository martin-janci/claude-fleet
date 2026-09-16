<script lang="ts">
  import { onMount } from 'svelte';
  import Modal from './Modal.svelte';
  import { lintAll, type LintAll } from './assets';

  let {
    onclose,
    onselect,
  }: {
    onclose: () => void;
    /** Jump to an asset from its lint row. */
    onselect: (kind: string, name: string) => void;
  } = $props();

  let report = $state<LintAll | null>(null);
  let error = $state<string | null>(null);
  let busy = $state(false);

  async function load() {
    busy = true;
    error = null;
    const r = await lintAll();
    busy = false;
    if (!r.ok) { error = r.error.message; return; }
    report = r.value;
  }

  onMount(load);

  const flagged = $derived(
    (report?.assets ?? []).filter((a) => a.report.errors.length > 0 || a.report.warnings.length > 0),
  );
</script>

<Modal title="Lint all" onclose={onclose} width="560px" testid="lint-all-dialog">
  {#if error}<p class="error" data-testid="lint-all-error">{error}</p>{/if}
  {#if busy && !report}
    <p class="muted">Linting…</p>
  {:else if report}
    <div class="counts" data-testid="lint-all-counts">
      <span class="chip errors">{report.errors} errors</span>
      <span class="chip warnings">{report.warnings} warnings</span>
      {#if report.problems.length}<span class="chip">{report.problems.length} problems</span>{/if}
    </div>
    {#if report.problems.length}
      <ul class="list">{#each report.problems as p}<li><code>{p.path}</code> {p.message}</li>{/each}</ul>
    {/if}
    <div class="assets">
      {#each flagged as a (a.kind + '::' + a.name)}
        <div class="asset-block" data-testid={`lint-all-asset-${a.kind}-${a.name}`}>
          <button class="link" onclick={() => onselect(a.kind, a.name)} data-testid={`lint-all-select-${a.kind}-${a.name}`}>{a.kind}/{a.name}</button>
          {#each a.report.errors as f, i (i)}<p class="finding error">{f.field}: {f.message}</p>{/each}
          {#each a.report.warnings as f, i (i)}<p class="finding warn">{f.field}: {f.message}</p>{/each}
        </div>
      {/each}
      {#if flagged.length === 0}<p class="muted">No findings — the catalog is clean.</p>{/if}
    </div>
  {/if}
  <div class="actions">
    <button onclick={onclose}>Close</button>
  </div>
</Modal>

<style>
  .counts { display: flex; gap: 6px; margin-bottom: 8px; }
  .chip { border: 1px solid var(--border); border-radius: 8px; padding: 1px 8px; font-size: 12px; }
  .chip.errors { color: #dc2626; } .chip.warnings { color: #d97706; }
  .list { margin: 0 0 8px; padding-left: 18px; font-size: 12px; }
  .assets { display: flex; flex-direction: column; gap: 10px; max-height: 50vh; overflow: auto; }
  .asset-block { border: 1px solid var(--border); border-radius: 6px; padding: 6px 8px; }
  .link { background: none; border: 0; color: var(--accent); cursor: pointer; padding: 0; font-family: ui-monospace, monospace; font-size: 13px; }
  .finding { margin: 4px 0 0; font-size: 12px; }
  .finding.error { color: #dc2626; } .finding.warn { color: #d97706; }
  .muted { color: var(--fg-muted); font-size: 12px; }
  .error { color: #dc2626; }
  .actions { display: flex; gap: 8px; justify-content: flex-end; margin-top: 10px; }
  .actions button { font-size: 0.85rem; padding: 0.3rem 0.8rem; border: 1px solid var(--border); background: transparent; color: var(--fg); border-radius: 4px; cursor: pointer; }
</style>
