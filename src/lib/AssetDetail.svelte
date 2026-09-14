<script lang="ts">
  import { getAsset, type AssetDetail } from './assets';
  import type { HostRow } from './hosts';

  let { kind, name, hosts }: { kind: string; name: string; hosts: HostRow[] } = $props();

  let detail = $state<AssetDetail | null>(null);
  let error = $state<string | null>(null);
  let harnessTab = $state('claude');

  $effect(() => {
    const k = kind, n = name;
    detail = null; error = null;
    getAsset(k, n).then((r) => {
      if (k !== kind || n !== name) return;
      if (r.ok) detail = r.value; else error = r.error.message;
    });
  });

  const harnesses = $derived(detail ? detail.previews.map((p) => p.harness) : []);
  const preview = $derived(detail?.previews.find((p) => p.harness === harnessTab) ?? null);
  const visibleHosts = $derived(hosts.filter((h) => !h.hidden));

  function cell(hostAlias: string, harness: string): string {
    const host = visibleHosts.find((h) => h.alias === hostAlias);
    if (host && host.alias !== 'local' && !host.reachable) return 'skipped';
    const s = detail?.hosts.find((h) => h.host_alias === hostAlias && h.harness === harness)?.state;
    return s ? s.replace('_', ' ') : 'not scanned';
  }
</script>

<div class="detail">
  {#if error}
    <p class="error">{error}</p>
  {:else if !detail}
    <p class="muted">Loading…</p>
  {:else}
    <h3 data-testid="asset-detail-title"><span class="kind">{detail.asset.kind}</span> {detail.asset.name} <span class="ver">v{detail.asset.version}</span></h3>
    <p class="desc">{detail.asset.description}</p>
    {#if detail.asset.tags.length}<p class="tags">{#each detail.asset.tags as t}<span class="tag">{t}</span>{/each}</p>{/if}

    <h4>Hosts</h4>
    <table class="matrix">
      <thead><tr><th>host</th>{#each harnesses as h}<th>{h}</th>{/each}</tr></thead>
      <tbody>
        {#each visibleHosts as host (host.alias)}
          <tr>
            <td>{host.alias}</td>
            {#each harnesses as h}
              {@const s = cell(host.alias, h)}
              <td class={`state-${s.replace(' ', '-')}`} data-testid={`matrix-cell-${host.alias}-${h}`} title={s === 'skipped' ? 'host unreachable' : ''}>{s}</td>
            {/each}
          </tr>
        {/each}
      </tbody>
    </table>

    <h4>Preview</h4>
    <div class="tabs" role="tablist">
      {#each harnesses as h}
        <button role="tab" class:active={harnessTab === h} aria-selected={harnessTab === h} onclick={() => (harnessTab = h)} data-testid={`preview-tab-${h}`}>{h}</button>
      {/each}
    </div>
    {#if preview?.unsupported}
      <p class="muted">{preview.unsupported}</p>
    {:else if preview?.plan}
      {#each preview.plan.warnings as w}<p class="warn">{w}</p>{/each}
      {#if preview.plan.placeholders.length}<p class="warn">Unresolved placeholders: {preview.plan.placeholders.join(', ')}</p>{/if}
      {#each preview.plan.files as f (f.path)}
        <div class="file">
          <div class="path" data-testid="preview-file-path">{f.path}</div>
          <pre>{f.bytes}</pre>
        </div>
      {/each}
      {#each preview.plan.merges as m (m.file + m.json_path.join('/'))}
        <div class="file">
          <div class="path">{m.file} → {m.json_path.join('.')} <span class="mode">({m.mode})</span></div>
          <pre>{JSON.stringify(m.value, null, 2)}</pre>
        </div>
      {/each}
      {#if preview.plan.files.length === 0 && preview.plan.merges.length === 0}<p class="muted">Nothing to install.</p>{/if}
    {/if}
  {/if}
</div>

<style>
  .detail { padding: 10px 14px; overflow: auto; height: 100%; font-size: 13px; }
  h3 { margin: 0 0 4px; font-size: 15px; font-family: ui-monospace, monospace; }
  .kind, .ver { color: var(--fg-muted); font-size: 11px; font-family: system-ui; }
  h4 { margin: 14px 0 6px; font-size: 11px; text-transform: uppercase; color: var(--fg-muted); }
  .desc { margin: 0; } .tags { margin: 4px 0 0; } .tag { border: 1px solid var(--border); border-radius: 8px; padding: 0 6px; font-size: 11px; margin-right: 4px; }
  .matrix { border-collapse: collapse; } .matrix th, .matrix td { text-align: left; padding: 3px 10px 3px 0; border-bottom: 1px solid var(--border); }
  .state-in-sync { color: #16a34a; } .state-drifted { color: #d97706; } .state-skipped, .state-not-scanned, .state-missing, .state-unsupported { color: var(--fg-muted); }
  .tabs { display: flex; gap: 2px; margin-bottom: 6px; } .tabs button { background: none; border: 1px solid var(--border); border-radius: 4px; padding: 2px 8px; color: var(--fg-muted); cursor: pointer; } .tabs button.active { color: var(--fg); border-color: var(--accent); }
  .file { margin: 6px 0; } .path { font-family: ui-monospace, monospace; font-size: 12px; color: var(--fg-muted); } .mode { opacity: 0.7; }
  pre { margin: 2px 0 0; padding: 8px; background: var(--bg-pane); border: 1px solid var(--border); border-radius: 4px; overflow: auto; max-height: 320px; font-size: 12px; }
  .muted { color: var(--fg-muted); } .warn { color: #d97706; margin: 2px 0; } .error { color: #dc2626; }
</style>
