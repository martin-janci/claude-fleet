<script lang="ts">
  import { importHost, type ImportReport } from './assets';
  import { hosts } from './hosts';
  import Modal from './Modal.svelte';

  let { onclose, ondone }: { onclose: () => void; ondone: () => void } = $props();

  let hostAlias = $state('local');
  let report = $state<ImportReport | null>(null);
  let error = $state<string | null>(null);
  let busy = $state(false);

  async function run(dryRun: boolean) {
    busy = true; error = null;
    const r = await importHost(hostAlias, dryRun);
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
          <option value={h.alias} disabled={h.alias !== 'local'}>{h.alias}{h.alias !== 'local' ? ' (local only in this version)' : ''}</option>
        {/each}
      </select>
    </label>
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
      <button onclick={() => run(true)} disabled={busy} data-testid="import-dry-run">Dry run</button>
      <button class="primary" onclick={() => run(false)} disabled={busy || !report?.dry_run} data-testid="import-confirm" title={report?.dry_run ? '' : 'Run a dry run first'}>Import</button>
    </div>
  </div>
</Modal>

<style>
  .dialog { max-height: 70vh; overflow: auto; }
  h4 { margin: 10px 0 4px; font-size: 12px; }
  .muted { color: var(--fg-muted); font-size: 12px; } .error { color: #dc2626; }
  .list { margin: 0; padding-left: 18px; font-size: 12px; max-height: 200px; overflow: auto; }
  .actions { display: flex; gap: 8px; justify-content: flex-end; margin-top: 12px; }
  .primary { background: var(--accent); color: white; border: 0; border-radius: 4px; padding: 4px 10px; }
</style>
