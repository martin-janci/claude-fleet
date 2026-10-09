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
  import { skillMatrix, toolkitTab, TOOLKIT_KIND, type ToolkitTab } from './toolkit_skills';
  import { composerPresets } from './composer_presets';
  import { downloads, downloadsOpen } from './downloads';
  import { goTo } from './destination';
  import AssetsPanel from './AssetsPanel.svelte';
  import PromptsSnippets from './PromptsSnippets.svelte';
  import Button from './kit/Button.svelte';

  let { visible }: { visible: boolean } = $props();

  type MatrixTab = Exclude<ToolkitTab, 'assets' | 'prompts'>;
  const PAGES: Record<MatrixTab, { title: string; noun: string; col: string }> = {
    skills: { title: 'Skills', noun: 'skill', col: 'Skill' },
    mcp: { title: 'MCP servers', noun: 'MCP server', col: 'Server' },
    hooks: { title: 'Hooks', noun: 'hook', col: 'Hook' },
  };
  const NAV: { id: ToolkitTab; label: string }[] = [
    { id: 'skills', label: 'Skills' },
    { id: 'mcp', label: 'MCP servers' },
    { id: 'hooks', label: 'Hooks' },
    { id: 'assets', label: 'Assets catalog' },
    { id: 'prompts', label: 'Prompts & snippets' },
  ];
  const matrix = $derived(
    skillMatrix($catalog, $toolkitTab === 'assets' || $toolkitTab === 'prompts' ? 'skill' : TOOLKIT_KIND[$toolkitTab]),
  );
  const counts = $derived<Partial<Record<ToolkitTab, number>>>({
    ...($catalog
      ? {
          skills: skillMatrix($catalog, 'skill').rows.length,
          mcp: skillMatrix($catalog, 'mcp_server').rows.length,
          hooks: skillMatrix($catalog, 'hook').rows.length,
          assets: $catalog.assets.length,
        }
      : {}),
    prompts: $composerPresets.length,
  });
  const activeDownloads = $derived($downloads.filter((d) => d.state === 'fetching').length);

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
  <!-- Toolkit's own column (UX audit 2026-10-09, A2; board Toolkit): what
       is installed on the hosts, then the links to where the rest lives. -->
  <nav class="side" aria-label="Toolkit">
    <h1>Toolkit</h1>
    <ul class="nav" data-testid="toolkit-tabs">
      {#each NAV as n (n.id)}
        <li>
          <button
            type="button"
            class="nav-item"
            aria-current={$toolkitTab === n.id ? 'page' : undefined}
            data-tab={n.id}
            onclick={() => toolkitTab.set(n.id)}
            >{n.label}{#if counts[n.id] !== undefined}<span class="n">{counts[n.id]}</span>{/if}</button
          >
        </li>
      {/each}
      <li>
        <button type="button" class="nav-item" data-testid="toolkit-downloads" onclick={() => downloadsOpen.set(true)}
          >Downloads…{#if activeDownloads > 0}<span class="n">{activeDownloads} active</span>{/if}</button
        >
      </li>
    </ul>
    <div class="side-foot">
      <button type="button" class="link" data-testid="toolkit-agents-link" onclick={() => goTo('automation')}
        >Agents are in Automation ↗</button
      >
      <p>They run on a schedule, so they live with Routines and Runs.</p>
      <p>Everything here is installed per host. Fleet keeps the hosts in sync and shows drift.</p>
    </div>
  </nav>

  {#if $toolkitTab === 'prompts'}
    <div class="body skills" role="region" aria-label="Prompts & snippets" data-testid="toolkit-prompts">
      <PromptsSnippets />
    </div>
  {:else if $toolkitTab === 'assets'}
    <div class="body assets" role="region" aria-label="Assets catalog">
      <AssetsPanel {visible} />
    </div>
  {:else}
    {@const tab = $toolkitTab}
    {@const page = PAGES[tab]}
    <div class="body skills" role="region" aria-label={page.title} data-testid="toolkit-{tab === 'skills' ? 'skills' : tab}">
      {#if !$catalog}
        <div class="empty" data-testid="toolkit-skills-empty">
          <p>No catalog is loaded yet. Set one up, or pull it, in the Assets catalog.</p>
          <Button onclick={() => openAssets()}>Open the Assets catalog</Button>
        </div>
      {:else}
        <div class="bar">
          <div>
            <h2>{page.title}</h2>
            <p class="sub" data-testid="toolkit-{tab}-summary">
              {matrix.rows.length}
              {matrix.rows.length === 1 ? page.noun : `${page.noun}s`}{#if matrix.outOfSync > 0}{' '}· {matrix.outOfSync} out of sync{/if}
            </p>
          </div>
          <span class="grow"></span>
          <Button variant="primary" testid="toolkit-sync" onclick={() => openAssets({ command: 'sync' })}
            >Sync all hosts…</Button
          >
        </div>
        {#if matrix.rows.length === 0}
          <p class="none">The catalog has no {page.noun}s yet.</p>
        {:else}
          <div class="scroll">
            <table class="matrix" data-testid="toolkit-{tab}-table">
              <thead>
                <tr>
                  <th scope="col">{page.col}</th>
                  <th scope="col">Agents</th>
                  {#each matrix.hosts as host (host)}
                    <th scope="col" class="host">{host}</th>
                  {/each}
                  <th scope="col"><span class="sr">Actions</span></th>
                </tr>
              </thead>
              <tbody>
                {#each matrix.rows as row (row.key)}
                  <tr data-testid="toolkit-{page.noun.replaceAll(' ', '-')}-{row.name}">
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
    display: grid;
    grid-template-columns: var(--settings-nav-w) minmax(0, 1fr);
    background: var(--bg);
    color: var(--fg);
  }
  .side {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    min-height: 0;
    overflow: auto;
    padding: var(--space-3) var(--space-2) var(--space-3) var(--space-3);
    border-right: 1px solid var(--border);
    background: var(--bg-pane);
  }
  h1 {
    margin: 0 0 var(--space-1) var(--space-1);
    font-size: var(--text-lg);
    line-height: var(--text-lg-lh);
    font-weight: 600;
  }
  .nav {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .nav-item {
    width: 100%;
    display: flex;
    align-items: center;
    gap: var(--space-2);
    padding: 6px var(--space-2);
    border: 0;
    border-left: 2px solid transparent;
    border-radius: var(--radius-sm);
    background: none;
    color: var(--fg);
    font: inherit;
    font-size: var(--text-sm);
    text-align: left;
    cursor: pointer;
  }
  .nav-item:hover { background: var(--bg-hover); }
  .nav-item[aria-current='page'] {
    background: var(--accent-soft);
    border-left-color: var(--accent);
    font-weight: 500;
  }
  .n {
    margin-left: auto;
    font-size: var(--text-2xs);
    color: var(--fg-muted);
    font-variant-numeric: tabular-nums;
  }
  .side-foot {
    margin-top: auto;
    padding: var(--space-3) var(--space-1) 0;
    border-top: 1px solid var(--border);
    font-size: var(--text-xs);
    color: var(--fg-muted);
  }
  .side-foot p { margin: var(--space-2) 0 0; }
  .link {
    padding: 0;
    border: 0;
    background: none;
    font: inherit;
    color: var(--accent);
    cursor: pointer;
  }
  .link:hover { text-decoration: underline; }
  @media (max-width: 640px) {
    .toolkit { grid-template-columns: minmax(0, 1fr); grid-template-rows: auto minmax(0, 1fr); }
    .side { border-right: 0; border-bottom: 1px solid var(--border); }
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
