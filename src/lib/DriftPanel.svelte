<script lang="ts">
  import ConfirmDialog from './ConfirmDialog.svelte';
  import DiffView from './DiffView.svelte';
  import { driftDiff, type ChangesetView, type DriftDiff, type DriftFile } from './assets_workspace';
  import { olderHubWords } from './assets_cards';
  import { unifiedDiff } from './line_diff';

  /** A Drift card's diff (spec: "Drift shows a DiffView with Take /
   *  Restore"; mockups screen 2; R7, R14): the catalog's and the host's text
   *  of each file, diffed here; Take imports the host copy into the catalog
   *  (a commit, undoable); Restore puts the catalog copy back on the host
   *  after a confirm — the only overwrite a card makes, with a backup.
   *  Read-only here means no grant on the card's catalog: the hub would
   *  refuse the read, so none is made. Take and Restore stay disabled until
   *  the two texts are read. */
  let { view, readOnly, busy, onapply }: { view: ChangesetView; readOnly: boolean; busy: boolean; onapply: (positions: number[]) => void } = $props();

  const take = $derived(view.items.find((i) => i.action === 'take_host' && i.state === 'pending') ?? null);
  const restore = $derived(view.items.find((i) => i.action === 'restore' && i.state === 'pending') ?? null);
  const subject = $derived(view.items[0]);
  const host = $derived(subject?.params.host ?? '');
  const catalog = $derived(subject?.catalog ?? 'personal');
  // What the two texts are read for. A string, so a refreshed card (a new
  // view object, the same asset) does not read the files again. None
  // without a grant.
  const request = $derived(
    subject && !readOnly ? JSON.stringify([subject.params.host ?? '', subject.kind, subject.name, subject.params.harness ?? null, subject.catalog ?? null]) : null,
  );

  let diff = $state<DriftDiff | null>(null);
  let note = $state<string | null>(null);
  let confirming = $state(false);

  $effect(() => {
    if (!request) return;
    const [hostAlias, kind, name, harness, cat] = JSON.parse(request) as [string, string, string, string | null, string | null];
    let live = true;
    diff = null;
    note = null;
    void driftDiff({ host_alias: hostAlias, kind, name, harness, catalog: cat }).then((r) => {
      if (!live) return;
      if (r.ok) {
        diff = r.value;
        if (r.value.merges_only) note = 'This asset lives in a config file (an MCP entry); its diff is not shown, so no secret leaves the host.';
      } else {
        note = olderHubWords(r.error, 'show this diff') ?? `Could not read the two copies: ${r.error.message}`;
      }
    });
    return () => (live = false);
  });

  /** Without a grant nothing was read, and nothing from before is shown. */
  const files = $derived(readOnly ? [] : (diff?.files ?? []));

  /** A text absent on one side only is a difference; absent on both is not. */
  const same = (f: DriftFile) => f.catalog === f.host || (f.catalog == null && f.host == null);
</script>

<div class="drift" data-testid="drift-panel">
  {#if readOnly}
    <p class="note" data-testid="drift-note">Needs a grant on {catalog} to show the diff.</p>
  {:else if note}<p class="note" data-testid="drift-note">{note}</p>{/if}
  {#each files as f (f.path)}
    <section class="file" data-testid={`drift-file-${f.path}`}>
      <header><span class="mono">{f.path}</span><span class="muted">catalog → {host}</span></header>
      {#if f.secret}
        <!-- Never read, so there are no texts to compare: not "Identical." -->
        <p class="muted">Contains a secret — its text is not shown.</p>
      {:else if f.binary}
        <p class="muted">Binary file; not compared.</p>
      {:else}
        {@const u = unifiedDiff(f.catalog, f.host, `catalog/${f.path}`, `${host}/${f.path}`)}
        {#if u}
          <DiffView diff={u} />
        {:else if same(f)}
          <p class="muted">Identical.</p>
        {:else}
          <p class="muted">Differs only in line endings or a final newline.</p>
        {/if}
        {#if f.truncated}<p class="muted">Only the first 256 KiB of each side is compared.</p>{/if}
      {/if}
    </section>
  {/each}
  {#if !readOnly}
    <div class="verbs">
      {#if take}
        <button type="button" class="btn btn--primary" data-testid="drift-take" disabled={busy || !diff} onclick={() => onapply([take.position])}>Take {host}'s version into {catalog}</button>
      {/if}
      {#if restore}
        <button type="button" class="btn" data-testid="drift-restore" disabled={busy || !diff} title="Keeps a .fleet-bak copy on {host}" onclick={() => (confirming = true)}>Restore catalog version on {host}…</button>
      {/if}
      <span class="muted small">Taking it makes a commit you can undo. Restoring keeps a .fleet-bak copy on {host}.</span>
    </div>
  {/if}
</div>

{#if confirming && restore}
  <ConfirmDialog
    title="Restore the catalog version?"
    message={`Restore the catalog version of ${subject.kind}/${subject.name} on ${host}? The host copy is overwritten; a .fleet-bak copy is kept.`}
    confirmLabel="Restore"
    danger
    {busy}
    confirmTestId="confirm-restore"
    onconfirm={() => { confirming = false; onapply([restore.position]); }}
    oncancel={() => (confirming = false)}
  />
{/if}

<style>
  .drift { display: grid; gap: 10px; }
  .file { border: 1px solid var(--border); border-radius: var(--radius-sm); overflow: hidden; }
  .file header { display: flex; justify-content: space-between; gap: 8px; padding: 4px 8px; font-size: var(--text-xs); background: var(--bg-pane); border-bottom: 1px solid var(--border); }
  .file p { padding: 6px 8px; }
  .file :global([data-testid='diff-view']) { max-height: 320px; height: auto; }
  .verbs { display: grid; gap: 6px; justify-items: start; }
  .muted { color: var(--fg-muted); font-size: var(--text-xs); margin: 0; }
  .small { font-size: var(--text-2xs); }
  .note { margin: 0; font-size: var(--text-xs); }
  .mono { font-family: var(--mono); }
</style>
