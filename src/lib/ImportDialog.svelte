<script lang="ts">
  import { untrack } from 'svelte';
  import { importHost, importOnlyFor, KIND_LABEL, KIND_ORDER, type AssetKind, type ImportReport } from './assets';
  import { hosts } from './hosts';
  import Modal from './Modal.svelte';
  import TransferMark from './TransferMark.svelte';

  let {
    host = 'local',
    only = [],
    onclose,
    ondone,
  }: { host?: string; only?: string[]; onclose: () => void; ondone: () => void } = $props();

  // Preset once, at open: the dialog does not track a later change to the
  // `host` prop (the caller opens a fresh instance each time anyway, via
  // AssetsPanel's `{#if showImport}`).
  let hostAlias = $state(untrack(() => host));
  let report = $state<ImportReport | null>(null);
  let error = $state<string | null>(null);
  let busy = $state(false);
  // G2.6: What to import, one box per kind. A caller's own `only` (one
  // asset's Import button) wins and the boxes are not shown.
  let what = $state<AssetKind[]>([...KIND_ORDER]);
  const asked = $derived(only.length > 0 ? only : importOnlyFor(what));
  function toggle(k: AssetKind, on: boolean) {
    what = on ? KIND_ORDER.filter((x) => x === k || what.includes(x)) : what.filter((x) => x !== k);
    report = null;
  }

  async function run(dryRun: boolean) {
    if (asked.length === 0 && what.length === 0) return;
    busy = true; error = null;
    const r = await importHost(hostAlias, dryRun, asked);
    busy = false;
    if (!r.ok) { error = r.error.message; return; }
    report = r.value;
    if (!dryRun) ondone();
  }
</script>

<Modal title="Import from host" {onclose} width="520px" testid="import-dialog">
  <div class="dialog">
    <p class="muted">Reads the host's Claude config and writes new assets into the catalog working tree. Existing catalog assets are never overwritten. Nothing is committed.</p>
    <label>Host
      <select bind:value={hostAlias} data-testid="import-host">
        {#each $hosts.filter((h) => !h.hidden) as h (h.alias)}
          <option value={h.alias}>{h.alias}</option>
        {/each}
      </select>
    </label>
    {#if only.length > 0}<p class="muted" data-testid="import-only">Only: {only.join(', ')}</p>
    {:else}
      <fieldset class="what" data-testid="import-what">
        <legend>What</legend>
        {#each KIND_ORDER as k (k)}
          <label><input type="checkbox" checked={what.includes(k)} onchange={(e) => toggle(k, (e.currentTarget as HTMLInputElement).checked)} data-testid="import-what-{k}" />{KIND_LABEL[k]}</label>
        {/each}
      </fieldset>
    {/if}
    {#if busy}
      <!-- Step 10.10: an import cannot say its size, so Data rain. -->
      <p class="running" data-testid="import-running"><TransferMark fraction={null} label="Reading the host's config" testid="import-transfer" /> Reading the host's config…</p>
    {/if}
    {#if error}<p class="error">{error}</p>{/if}
    {#if report}
      <h4>{report.dry_run ? 'Would create' : 'Created'} {report.created.length}</h4>
      <ul class="list">{#each report.created as [kind, name]}<li>{kind} <code>{name}</code></li>{/each}</ul>
      {#if report.problems.length}<h4>Problems {report.problems.length}</h4><ul class="list">{#each report.problems as p}<li><code>{p.path}</code> {p.message}</li>{/each}</ul>{/if}
      {#if report.warnings?.length}<h4>Warnings</h4><ul class="list" data-testid="import-warnings">{#each report.warnings as w}<li><code>{w.path}</code> {w.message}</li>{/each}</ul>{/if}
      {#if report.flagged_secrets.length}<h4>Secrets to replace</h4><ul class="list">{#each report.flagged_secrets as s}<li>{s}</li>{/each}</ul>{/if}
    {/if}
    <div class="actions">
      <button onclick={onclose}>Close</button>
      <button onclick={() => run(true)} disabled={busy || (only.length === 0 && what.length === 0)} data-testid="import-dry-run">Dry run</button>
      <button class="btn btn--primary" onclick={() => run(false)} disabled={busy || !report?.dry_run} data-testid="import-confirm" title={report?.dry_run ? '' : 'Run a dry run first'}>Import</button>
    </div>
  </div>
</Modal>

<style>
  .dialog { max-height: 70vh; overflow: auto; }
  .what { display: flex; flex-wrap: wrap; gap: 4px 12px; border: 0; padding: 0; margin: 8px 0; font-size: var(--text-xs); }
  .what legend { color: var(--fg-muted); padding: 0; margin-bottom: 4px; }
  .what label { display: inline-flex; align-items: center; gap: 4px; }
  .running { display: flex; align-items: center; gap: var(--space-2, 8px); color: var(--fg-muted); font-size: var(--text-xs); }
  h4 { margin: 10px 0 4px; font-size: var(--text-xs); }
  .muted { color: var(--fg-muted); font-size: var(--text-xs); } .error { color: var(--usage-crit); }
  .list { margin: 0; padding-left: 18px; font-size: var(--text-xs); max-height: 200px; overflow: auto; }
  .actions { display: flex; gap: 8px; justify-content: flex-end; margin-top: 12px; }
</style>
