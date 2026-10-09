<!--
  Toolkit (Orbit Fleet redesign step 3.16, board Toolkit). The screen for
  today's Assets workspace: Skills, the catalog's skills as a host
  matrix with drift per host, and Assets, the workspace itself (layers and
  changesets) unchanged. Sync and Edit hand over to the Assets tab, where the
  plan, the confirm and the editor already live.
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import { catalog, loadAssets } from './assets';
  import { requestAssetsView } from './app_views';
  import { skillMatrix, toolkitTab, type ToolkitTab } from './toolkit_skills';
  import AssetsPanel from './AssetsPanel.svelte';
  import Tabs from './kit/Tabs.svelte';
  import Button from './kit/Button.svelte';

  let { visible }: { visible: boolean } = $props();

  const matrix = $derived(skillMatrix($catalog));
  const assetCount = $derived($catalog?.assets.length);

  // The catalog is primed at launch; refresh it quietly when Toolkit opens.
  onMount(() => {
    void loadAssets();
  });

  function openAssets(r?: Parameters<typeof requestAssetsView>[0]) {
    toolkitTab.set('assets');
    if (r) requestAssetsView(r);
  }
</script>

<section class="toolkit" aria-label="Toolkit" data-testid="toolkit">
  <header class="head">
    <h1>Toolkit</h1>
    <Tabs
      label="Toolkit"
      testid="toolkit-tabs"
      selected={$toolkitTab}
      onselect={(id) => toolkitTab.set(id as ToolkitTab)}
      tabs={[
        { id: 'skills', label: 'Skills', count: $catalog ? matrix.rows.length : undefined },
        { id: 'assets', label: 'Assets', count: assetCount },
      ]}
    />
  </header>

  {#if $toolkitTab === 'assets'}
    <div class="body assets" role="tabpanel" aria-label="Assets">
      <AssetsPanel {visible} />
    </div>
  {:else}
    <div class="body skills" role="tabpanel" aria-label="Skills" data-testid="toolkit-skills">
      <p class="hint">Everything here is installed per host. Fleet keeps the hosts in sync and shows drift.</p>
      {#if !$catalog}
        <div class="empty" data-testid="toolkit-skills-empty">
          <p>No catalog is loaded yet. Set one up, or pull it, in Assets.</p>
          <Button onclick={() => openAssets()}>Open Assets</Button>
        </div>
      {:else}
        <div class="bar">
          <div>
            <h2>Skills</h2>
            <p class="sub" data-testid="toolkit-skills-summary">
              {matrix.rows.length}
              {matrix.rows.length === 1 ? 'skill' : 'skills'}{#if matrix.outOfSync > 0}{' '}· {matrix.outOfSync} out of sync{/if}
            </p>
          </div>
          <span class="grow"></span>
          <Button variant="primary" testid="toolkit-sync" onclick={() => openAssets({ command: 'sync' })}
            >Sync all hosts…</Button
          >
        </div>
        {#if matrix.rows.length === 0}
          <p class="none">The catalog has no skills yet.</p>
        {:else}
          <div class="scroll">
            <table class="matrix" data-testid="toolkit-skills-table">
              <thead>
                <tr>
                  <th scope="col">Skill</th>
                  <th scope="col">Agents</th>
                  {#each matrix.hosts as host (host)}
                    <th scope="col" class="host">{host}</th>
                  {/each}
                  <th scope="col"><span class="sr">Actions</span></th>
                </tr>
              </thead>
              <tbody>
                {#each matrix.rows as row (row.key)}
                  <tr data-testid="toolkit-skill-{row.name}">
                    <th scope="row" class="name">
                      <span class="mono">{row.name}</span>
                      {#if row.description}<span class="desc">{row.description}</span>{/if}
                    </th>
                    <td class="agents">{row.agents}</td>
                    {#each matrix.hosts as host (host)}
                      {@const cell = row.cells[host]}
                      <td class="cell {cell.state}" title={cell.title} data-host={host} data-state={cell.state}
                        >{cell.word}</td
                      >
                    {/each}
                    <td class="act">
                      <Button size="sm" variant="quiet" label="Edit {row.name}" onclick={() => openAssets({ select: row.key })}
                        >Edit</Button
                      >
                    </td>
                  </tr>
                {/each}
              </tbody>
            </table>
          </div>
        {/if}
      {/if}
    </div>
  {/if}
</section>

<style>
  .toolkit {
    height: 100%;
    min-height: 0;
    display: flex;
    flex-direction: column;
    background: var(--bg);
    color: var(--fg);
  }
  .head {
    display: flex;
    align-items: flex-end;
    gap: var(--space-6);
    padding: var(--space-3) var(--space-4) 0;
    border-bottom: 1px solid var(--border);
  }
  h1 {
    margin: 0 0 var(--space-2);
    font-size: var(--text-lg);
    line-height: var(--text-lg-lh);
    font-weight: 600;
  }
  .body {
    flex: 1 1 auto;
    min-height: 0;
  }
  .skills {
    overflow: auto;
    padding: var(--space-3) var(--space-4);
  }
  .hint,
  .sub,
  .none,
  .desc,
  .agents {
    color: var(--fg-muted);
  }
  .hint {
    margin: 0 0 var(--space-3);
    font-size: var(--text-sm);
  }
  .bar {
    display: flex;
    align-items: center;
    gap: var(--space-3);
    margin-bottom: var(--space-3);
  }
  h2 {
    margin: 0;
    font-size: var(--text-md);
    font-weight: 600;
  }
  .sub {
    margin: 0;
    font-size: var(--text-sm);
  }
  .grow {
    flex: 1 1 auto;
  }
  .empty {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: var(--space-2);
  }
  .scroll {
    overflow-x: auto;
  }
  .matrix {
    width: 100%;
    border-collapse: collapse;
    font-size: var(--text-sm);
  }
  .matrix th,
  .matrix td {
    padding: var(--space-2);
    border-bottom: 1px solid var(--border);
    text-align: left;
    vertical-align: middle;
  }
  thead th {
    color: var(--fg-muted);
    font-weight: 500;
  }
  .host,
  .cell {
    text-align: center;
    white-space: nowrap;
  }
  .name {
    font-weight: 400;
  }
  .mono {
    display: block;
    font-family: var(--font-mono);
  }
  .desc {
    display: block;
    font-size: var(--text-xs);
  }
  /* Colour backs the word, never alone (7.2). */
  .cell.in_sync {
    color: var(--status-done);
  }
  .cell.behind,
  .cell.edited,
  .cell.drifted {
    color: var(--status-waiting);
  }
  .cell.missing {
    color: var(--status-failed);
  }
  .cell.none {
    color: var(--fg-muted);
  }
  .act {
    text-align: right;
  }
  .sr {
    position: absolute;
    width: 1px;
    height: 1px;
    overflow: hidden;
    clip: rect(0 0 0 0);
    white-space: nowrap;
  }
</style>
