<script lang="ts">
  import { sessions, showBgAgents, showFriendlyNames } from './sessions';
  import { hosts, hostFilter } from './hosts';
  import { hintAnchor } from './hints';
  import { accounts, type AccountRow } from './accounts';
  import { attentionIdleMinutes } from './notify';
  import Attention from './Attention.svelte';
  import { RECENCY_VALUES, type Recency } from './session_status';

  let {
    search = $bindable(),
    recency = $bindable(),
    stuckOnly = $bindable(),
    attentionOnly = $bindable(),
    loading,
    loadError,
    onRefresh,
    onCollapse,
    showTasks,
    showSettings,
    onOpenTasks,
    onOpenSettings,
    stuckCount,
    attentionCount,
    selectMode,
    toggleSelectMode,
    selectedCount,
    onBulkSend,
    onBulkKill,
    clearSelected,
  }: {
    search: string;
    recency: Recency;
    stuckOnly: boolean;
    attentionOnly: boolean;
    loading: boolean;
    loadError: string | null;
    onRefresh: () => void;
    onCollapse?: () => void;
    showTasks: boolean;
    showSettings: boolean;
    onOpenTasks: () => void;
    onOpenSettings: () => void;
    stuckCount: number;
    attentionCount: number;
    selectMode: boolean;
    toggleSelectMode: () => void;
    selectedCount: number;
    onBulkSend: () => void;
    onBulkKill: () => void;
    clearSelected: () => void;
  } = $props();

  // Lookup map for tooltips + components that resolve a host's account.
  const accountByUuid = $derived(
    new Map<string, AccountRow>($accounts.map((a) => [a.uuid, a])),
  );

  function accountLabel(host: { account_uuid: string | null }): string {
    if (!host.account_uuid) return '';
    const acc = accountByUuid.get(host.account_uuid);
    if (!acc) return `\n${host.account_uuid}`;
    const email = acc.email ?? acc.uuid;
    return acc.seat_tier ? `\n${email} (${acc.seat_tier})` : `\n${email}`;
  }
</script>

<header class="sidebar-header" data-testid="sidebar-chrome-top">
  <div class="row">
    <input
      class="search"
      placeholder="Search sessions, projects…"
      bind:value={search}
      data-testid="sidebar-search"
    />
    <button class="icon-btn" onclick={onRefresh} disabled={loading} data-testid="sidebar-refresh" title="Refresh">
      {#if loading}…{:else}↻{/if}
    </button>
    {#if onCollapse}
      <button
        class="icon-btn"
        onclick={onCollapse}
        title="Hide sidebar (more room for terminal)"
        aria-label="Hide sidebar"
        data-testid="sidebar-collapse"
      >‹</button>
    {/if}
  </div>

  <nav class="hosts" aria-label="host filter" use:hintAnchor={{ id: 'host-filter', when: $hosts.filter((h) => !h.hidden).length >= 2 }}>
    <button
      class="pill"
      class:active={$hostFilter === 'all'}
      onclick={() => hostFilter.set('all')}
    >all</button>
    {#each $hosts.filter((h) => !h.hidden) as h (h.alias)}
      <button
        class="pill"
        class:active={$hostFilter === h.alias}
        onclick={() => hostFilter.set(h.alias)}
        title={`${h.alias}${h.tmux_version ? ` · tmux ${h.tmux_version}` : ''}${h.claude_version ? ` · claude ${h.claude_version}` : ''}${accountLabel(h)}`}
      >
        <span class="host-dot status-{h.reachable ? 'on' : 'off'}"></span>
        {h.alias}
      </button>
    {/each}
    <button
      class="icon-btn"
      onclick={() => onOpenTasks()}
      title="Tasks (fleet-wide)"
      aria-label="Tasks"
      aria-expanded={showTasks}
      data-testid="tasks-open"
    >☑</button>
    <button
      class="icon-btn"
      onclick={() => onOpenSettings()}
      title="Settings"
      aria-label="Settings"
      aria-expanded={showSettings}
      data-testid="settings-open"
    >⚙</button>
  </nav>

  <nav class="recency" aria-label="recency filter" use:hintAnchor={{ id: 'recency-filter', when: $sessions.length > 0 }}>
    {#each RECENCY_VALUES as opt (opt)}
      <button
        class="pill"
        class:active={recency === opt}
        onclick={() => (recency = opt)}
      >
        {opt}
      </button>
    {/each}
  </nav>

  <nav class="triage" aria-label="triage filter">
    <button
      class="pill stuck-pill"
      class:active={stuckOnly}
      class:hot={stuckCount > 0}
      data-testid="stuck-filter"
      aria-pressed={stuckOnly}
      title={stuckOnly ? 'Show all sessions' : 'Show only stuck sessions'}
      onclick={() => { stuckOnly = !stuckOnly; if (stuckOnly) attentionOnly = false; }}
    >
      ⚠ {stuckCount} stuck
    </button>
    <button
      class="pill"
      class:active={attentionOnly}
      data-testid="attention-filter"
      aria-pressed={attentionOnly}
      title="Stuck, safe-remove pending/failed, lost, failed, or idle > {$attentionIdleMinutes} min"
      onclick={() => { attentionOnly = !attentionOnly; if (attentionOnly) stuckOnly = false; }}
    >
      needs attention ({attentionCount})
    </button>
    <button
      class="pill"
      class:active={selectMode}
      data-testid="select-mode"
      aria-pressed={selectMode}
      title="Select several sessions (or shift/cmd-click rows) for bulk actions"
      onclick={toggleSelectMode}
    >
      ☑ select
    </button>
  </nav>
  <Attention />

  {#if selectedCount > 0}
    <div class="bulk-bar" data-testid="bulk-bar" role="toolbar" aria-label="bulk actions">
      <span class="bulk-count">{selectedCount} selected</span>
      <button class="pill" data-testid="bulk-send" onclick={() => onBulkSend()}>→ Send prompt</button>
      <button class="pill danger" data-testid="bulk-kill" onclick={() => onBulkKill()}>× Kill</button>
      <button class="pill" data-testid="bulk-clear" onclick={clearSelected}>clear</button>
    </div>
  {/if}

  <nav class="bg-toggle" aria-label="background agents filter">
    <button
      class="pill"
      class:active={$showBgAgents}
      data-testid="bg-toggle"
      aria-pressed={$showBgAgents}
      title={$showBgAgents ? 'Hide background agents' : 'Show background agents'}
      onclick={() => showBgAgents.update((v) => !v)}
    >
      🤖 bg {$showBgAgents ? 'on' : 'off'}
    </button>
    <button
      class="pill"
      class:active={$showFriendlyNames}
      data-testid="friendly-name-toggle"
      aria-pressed={$showFriendlyNames}
      title={$showFriendlyNames
        ? 'Show raw tmux names'
        : 'Show agent-set friendly names'}
      onclick={() => showFriendlyNames.update((v) => !v)}
    >
      🏷 friendly {$showFriendlyNames ? 'on' : 'off'}
    </button>
  </nav>

  {#if loadError}
    <p class="err">{loadError}</p>
  {/if}
</header>

<style>
  .sidebar-header {
    flex: 0 0 auto;
    display: flex;
    flex-direction: column;
    gap: 0.35rem;
    padding: 0.5rem 0.6rem 0.4rem;
    border-bottom: 1px solid var(--border);
    background: var(--bg-pane);
  }
  .sidebar-header .row {
    display: flex;
    gap: 0.3rem;
    align-items: center;
  }
  .search {
    flex: 1;
    font-size: 0.85rem;
    padding: 0.3rem 0.5rem;
    border: 1px solid var(--border);
    background: var(--bg);
    color: var(--fg);
    border-radius: 5px;
  }
  .search::placeholder { color: var(--fg-muted); }

  .icon-btn {
    background: transparent;
    border: 1px solid var(--border);
    color: var(--fg-muted);
    padding: 0.25rem 0.5rem;
    border-radius: 5px;
    font-size: 0.9rem;
    line-height: 1;
    cursor: pointer;
    min-width: 1.6rem;
  }
  .icon-btn:hover:not(:disabled) {
    color: var(--fg);
    border-color: var(--accent);
    background: var(--bg-pane);
  }
  .icon-btn:disabled { opacity: 0.6; cursor: progress; }

  .recency { display: flex; gap: 0.25rem; }
  .bg-toggle { display: flex; gap: 0.25rem; }
  .triage { display: flex; gap: 0.25rem; flex-wrap: wrap; align-items: center; }
  .stuck-pill.hot { color: #e64a4a; border-color: rgba(230, 74, 74, 0.5); }
  .stuck-pill.active { background: rgba(230, 74, 74, 0.12); }
  .pill.danger { color: #e64a4a; }
  .pill.danger:hover { border-color: #e64a4a; }
  .bulk-bar {
    display: flex;
    gap: 0.3rem;
    align-items: center;
    padding: 0.25rem 0.4rem;
    border: 1px solid var(--accent);
    border-radius: 5px;
    background: color-mix(in srgb, var(--accent) 10%, transparent);
    font-size: 0.75rem;
  }
  .bulk-count { flex: 1; color: var(--fg); }
  .pill {
    font-size: 0.7rem;
    padding: 0.15rem 0.55rem;
    border: 1px solid var(--border);
    background: transparent;
    color: var(--fg-muted);
    border-radius: 999px;
    cursor: pointer;
  }
  .pill.active { color: var(--fg); border-color: var(--accent); }

  .hosts { display: flex; flex-wrap: wrap; gap: 0.25rem; align-items: center; }
  .host-dot {
    display: inline-block;
    width: 0.4rem;
    height: 0.4rem;
    border-radius: 50%;
    margin-right: 0.3rem;
    vertical-align: middle;
  }
  .host-dot.status-on { background: rgb(80, 200, 110); }
  .host-dot.status-off { background: rgb(220, 130, 130); }

  .err { color: #e64a4a; font-size: 0.8rem; padding: 0.2rem 0; margin: 0; }
</style>
