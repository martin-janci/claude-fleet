<script lang="ts">
  import { untrack } from 'svelte';
  import { importHost, type ImportReport } from './assets';
  import { hosts } from './hosts';
  import Modal from './Modal.svelte';

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

  /** The hosts the select offers. */
  const pickable = $derived($hosts.filter((h) => !h.hidden));

  // `host` defaults to the literal `local`, which is NOT always among the
  // options: on a hub (`hub.local_host=false`) and on Windows there is no
  // `local` host at all, and Svelte leaves a bound value that matches no
  // `<option>` exactly as it is — so the select drew blank and Import sent
  // `local`, which the backend then refuses. Snap to a real option instead,
  // once the list has loaded, and only while nothing has been picked by hand.
  let picked = $state(false);
  $effect(() => {
    const opts = pickable;
    if (picked || opts.length === 0) return;
    if (!opts.some((h) => h.alias === hostAlias)) {
      hostAlias = opts[0].alias;
    }
  });
  let report = $state<ImportReport | null>(null);
  let error = $state<string | null>(null);
  let busy = $state(false);

  async function run(dryRun: boolean) {
    busy = true; error = null;
    const r = await importHost(hostAlias, dryRun, only);
    busy = false;
    if (!r.ok) { error = r.error.message; return; }
    report = r.value;
    if (!dryRun) ondone();
  }
</script>

<Modal title="Import from host" onclose={busy ? undefined : onclose} width="520px" testid="import-dialog">
  <div class="dialog">
    <p class="muted">Reads the host's Claude config and writes new assets into the catalog working tree. Existing catalog assets are never overwritten. Nothing is committed.</p>
    <label>Host
      <select
        bind:value={hostAlias}
        disabled={busy || pickable.length === 0}
        onchange={() => (picked = true)}
        data-testid="import-host"
      >
        {#each pickable as h (h.alias)}
          <option value={h.alias}>{h.alias}</option>
        {/each}
      </select>
    </label>
    {#if only.length > 0}<p class="muted" data-testid="import-only">Only: {only.join(', ')}</p>{/if}
    {#if error}<p class="error">{error}</p>{/if}
    {#if report}
      <h4>{report.dry_run ? 'Would create' : 'Created'} {report.created.length}</h4>
      <ul class="list">{#each report.created as [kind, name]}<li>{kind} <code>{name}</code></li>{/each}</ul>
      {#if report.problems.length}<h4>Problems {report.problems.length}</h4><ul class="list">{#each report.problems as p}<li><code>{p.path}</code> {p.message}</li>{/each}</ul>{/if}
      {#if report.warnings?.length}<h4>Warnings</h4><ul class="list" data-testid="import-warnings">{#each report.warnings as w}<li><code>{w.path}</code> {w.message}</li>{/each}</ul>{/if}
      {#if report.flagged_secrets.length}<h4>Secrets to replace</h4><ul class="list">{#each report.flagged_secrets as s}<li>{s}</li>{/each}</ul>{/if}
    {/if}
    {#if busy}<p class="muted" data-testid="import-busy">Reading {hostAlias}…</p>{/if}
    {#if pickable.length === 0}<p class="muted" data-testid="import-no-hosts">No host to import from.</p>{/if}
    <div class="actions">
      <!-- A remote import is an SSH round trip of up to 64 MiB. Dismissing it
           mid-flight threw the report away — including "Secrets to replace",
           the one thing a person must see before committing. -->
      <button onclick={onclose} disabled={busy} data-testid="import-close">Close</button>
      <button onclick={() => run(true)} disabled={busy || pickable.length === 0} data-testid="import-dry-run">Dry run</button>
      <button class="btn btn--primary" onclick={() => run(false)} disabled={busy || !report?.dry_run} data-testid="import-confirm" title={report?.dry_run ? '' : 'Run a dry run first'}>Import</button>
    </div>
  </div>
</Modal>

<style>
  .dialog { max-height: 70vh; overflow: auto; }
  h4 { margin: 10px 0 4px; font-size: 12px; }
  .muted { color: var(--fg-muted); font-size: 12px; } .error { color: #dc2626; }
  .list { margin: 0; padding-left: 18px; font-size: 12px; max-height: 200px; overflow: auto; }
  .actions { display: flex; gap: 8px; justify-content: flex-end; margin-top: 12px; }
</style>
