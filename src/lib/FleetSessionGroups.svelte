<script lang="ts">
  // An extra fleet's sessions in the sidebar (multi-hub design, Phase 1):
  // one group per fleet the filter shows, under that fleet's badge, with its
  // live link and the actions that route to its hub — send a prompt, rename,
  // pin, close, start a session, copy the attach command. The full session
  // view (conversation, terminal) for an extra fleet is Phase 2.
  import {
    visibleExtraFleets,
    connectionWord,
    retryFleet,
    loadFleetRows,
    fleetSendPrompt,
    fleetKillSession,
    fleetRenameSession,
    fleetPinSession,
    fleetNewSession,
    fleetAttachCommand,
    fleetKey,
    type FleetInfo,
    type FleetRows,
  } from './fleets';
  import type { SessionRow } from './sessions';
  import { copyText } from './clipboard';
  import { push } from './toasts';
  import Button from './kit/Button.svelte';

  // Which row has which inline form open, by fleet-qualified key.
  let open = $state<{ key: string; kind: 'prompt' | 'rename' | 'close' } | null>(null);
  let text = $state('');
  let busy = $state(false);
  let starting = $state<string | null>(null);
  let newHost = $state('');
  let newProject = $state<number | null>(null);
  let newName = $state('');

  function sorted(rows: FleetRows): SessionRow[] {
    return [...rows.sessions.values()]
      .filter((s) => s.status !== 'lost')
      .sort((a, b) => (b.pinned_at ?? 0) - (a.pinned_at ?? 0) || b.last_activity_at - a.last_activity_at);
  }

  function label(s: SessionRow): string {
    return s.friendly_name || s.tmux_name;
  }

  function toggle(fleet: string, s: SessionRow, kind: 'prompt' | 'rename' | 'close') {
    const key = fleetKey(fleet, s.id);
    if (open?.key === key && open.kind === kind) {
      open = null;
      return;
    }
    open = { key, kind };
    text = kind === 'rename' ? (s.friendly_name ?? '') : '';
  }

  async function run(what: string, p: Promise<{ ok: boolean; error?: { message: string } }>) {
    busy = true;
    const r = await p;
    busy = false;
    if (r.ok) {
      open = null;
      text = '';
    } else {
      push({ kind: 'error', message: `${what} failed`, detail: r.error?.message });
    }
  }

  async function copyAttach(s: SessionRow) {
    if (await copyText(fleetAttachCommand(s))) push({ kind: 'success', message: 'Attach command copied' });
  }

  function startForm(f: FleetInfo, rows: FleetRows) {
    starting = starting === f.id ? null : f.id;
    newHost = [...rows.hosts.keys()][0] ?? '';
    newProject = rows.projects.find((p) => !p.project.system)?.project.id ?? null;
    newName = '';
  }

  async function start(f: FleetInfo) {
    if (!newHost || newProject === null || !newName.trim()) return;
    busy = true;
    const r = await fleetNewSession(f.id, { host_alias: newHost, project_id: newProject, name: newName.trim() });
    busy = false;
    if (r.ok) starting = null;
    else push({ kind: 'error', message: `Starting a session on ${f.label} failed`, detail: r.error.message });
  }
</script>

{#each $visibleExtraFleets as { fleet: f, rows } (f.id)}
  {@const cw = connectionWord(f)}
  {@const list = sorted(rows)}
  <section class="fleet-group" style="--fleet: {f.color}" data-testid="fleet-group-{f.id}">
    <header class="head">
      <span class="badge" data-testid="fleet-badge-{f.id}">{f.label}</span>
      <span class="state {cw.tone}" data-testid="fleet-state-{f.id}">{cw.word}</span>
      <span class="count">{list.length}</span>
      <span class="grow"></span>
      {#if cw.tone === 'bad' || cw.tone === 'warn'}
        <Button size="sm" variant="quiet" testid="fleet-retry-{f.id}" onclick={() => void retryFleet(f.id)}>Retry</Button>
      {/if}
      <Button size="sm" variant="quiet" testid="fleet-refresh-{f.id}" onclick={() => void loadFleetRows(f.id)}>Refresh</Button>
      {#if !f.unavailable}
        <Button size="sm" variant="quiet" testid="fleet-new-{f.id}" onclick={() => startForm(f, rows)}>New…</Button>
      {/if}
    </header>
    {#if f.unavailable}
      <p class="note err" data-testid="fleet-unavailable-{f.id}">{f.unavailable}</p>
    {:else if rows.error}
      <p class="note err">{rows.error}</p>
    {/if}
    {#if starting === f.id}
      <form
        class="inline"
        data-testid="fleet-new-form-{f.id}"
        onsubmit={(e) => {
          e.preventDefault();
          void start(f);
        }}
      >
        <select bind:value={newHost} aria-label="Host">
          {#each [...rows.hosts.values()].filter((h) => !h.hidden) as h (h.alias)}
            <option value={h.alias}>{h.alias}</option>
          {/each}
        </select>
        <select bind:value={newProject} aria-label="Project">
          {#each rows.projects.filter((p) => !p.project.system) as p (p.project.id)}
            <option value={p.project.id}>{p.project.owner}/{p.project.repo}</option>
          {/each}
        </select>
        <input bind:value={newName} placeholder="session name" aria-label="Session name" />
        <Button size="sm" variant="primary" type="submit" {busy} busyLabel="Starting…" disabled={!newHost || newProject === null || !newName.trim()}>Start</Button>
      </form>
    {/if}
    <ul class="rows">
      {#each list as s (s.id)}
        {@const key = fleetKey(f.id, s.id)}
        <li class="row" data-testid="fleet-session-{f.id}-{s.id}">
          <div class="line">
            <span class="name" title={s.tmux_name}>{#if s.pinned_at}<span class="pin" aria-label="pinned">◆</span>{/if}{label(s)}</span>
            <span class="host">{s.host_alias}</span>
            {#if s.claude_status}<span class="status">{s.claude_status}</span>{/if}
          </div>
          <div class="actions">
            <Button size="sm" variant="quiet" testid="fleet-prompt-{f.id}-{s.id}" onclick={() => toggle(f.id, s, 'prompt')}>Prompt…</Button>
            <Button size="sm" variant="quiet" onclick={() => toggle(f.id, s, 'rename')}>Rename…</Button>
            <Button size="sm" variant="quiet" onclick={() => void run('Pinning', fleetPinSession(f.id, s, !s.pinned_at))}>{s.pinned_at ? 'Unpin' : 'Pin'}</Button>
            <Button size="sm" variant="quiet" onclick={() => void copyAttach(s)}>Copy attach</Button>
            <Button size="sm" variant="quiet" testid="fleet-close-{f.id}-{s.id}" onclick={() => toggle(f.id, s, 'close')}>Close…</Button>
          </div>
          {#if open?.key === key && open.kind === 'prompt'}
            <form
              class="inline"
              onsubmit={(e) => {
                e.preventDefault();
                if (text.trim()) void run('Sending', fleetSendPrompt(f.id, s, text));
              }}
            >
              <input bind:value={text} placeholder="Prompt for {label(s)} on {f.label}" aria-label="Prompt" data-testid="fleet-prompt-input-{f.id}-{s.id}" />
              <Button size="sm" variant="primary" type="submit" {busy} busyLabel="Sending…" disabled={!text.trim()}>Send</Button>
            </form>
          {:else if open?.key === key && open.kind === 'rename'}
            <form
              class="inline"
              onsubmit={(e) => {
                e.preventDefault();
                void run('Renaming', fleetRenameSession(f.id, s, text));
              }}
            >
              <input bind:value={text} placeholder="Label" aria-label="Label" />
              <Button size="sm" variant="primary" type="submit" {busy} busyLabel="Saving…">Save</Button>
            </form>
          {:else if open?.key === key && open.kind === 'close'}
            <div class="inline">
              <span>Close {label(s)} on {f.label}? Its tmux session ends on {s.host_alias}.</span>
              <Button size="sm" variant="danger" {busy} busyLabel="Closing…" testid="fleet-close-confirm-{f.id}-{s.id}" onclick={() => void run('Closing', fleetKillSession(f.id, s))}>Close</Button>
            </div>
          {/if}
        </li>
      {:else}
        {#if !f.unavailable}<li class="note">No sessions on {f.label}.</li>{/if}
      {/each}
    </ul>
  </section>
{/each}

<style>
  .fleet-group {
    border-top: 1px solid var(--border);
    border-left: 3px solid var(--fleet);
    padding: var(--space-1) var(--space-2) var(--space-2);
  }
  .head {
    display: flex;
    align-items: center;
    gap: var(--space-1);
    font-size: var(--text-xs);
  }
  .badge {
    padding: 0 var(--space-1);
    border-radius: var(--radius-xs);
    background: var(--fleet);
    color: var(--accent-fg);
    font-weight: 600;
  }
  .state {
    color: var(--fg-muted);
  }
  .state.bad {
    color: var(--status-failed);
  }
  .state.warn {
    color: var(--status-waiting);
  }
  .count {
    color: var(--fg-muted);
  }
  .grow {
    flex: 1;
  }
  .rows {
    list-style: none;
    margin: 0;
    padding: 0;
  }
  .row {
    padding: var(--space-1) 0;
    border-bottom: 1px solid var(--border);
  }
  .line {
    display: flex;
    gap: var(--space-2);
    align-items: baseline;
    font-size: var(--text-sm);
    min-width: 0;
  }
  .name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--fg);
  }
  .pin {
    margin-right: var(--space-1);
    color: var(--fleet);
  }
  .host,
  .status {
    color: var(--fg-muted);
    font-size: var(--text-xs);
  }
  .actions {
    display: flex;
    flex-wrap: wrap;
    gap: 2px;
  }
  .inline {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-1);
    align-items: center;
    padding-top: var(--space-1);
    font-size: var(--text-xs);
  }
  .inline input {
    flex: 1;
    min-width: 8em;
  }
  .note {
    color: var(--fg-muted);
    font-size: var(--text-xs);
    margin: var(--space-1) 0;
  }
  .note.err {
    color: var(--status-failed);
  }
</style>
