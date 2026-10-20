<script lang="ts">
  // An extra fleet's sessions in the sidebar (multi-hub design): one group
  // per fleet the filter shows, under that fleet's badge, with its live link
  // and the actions that route to its hub — send a prompt, rename, pin,
  // close, start a session, copy the attach command. A row's name opens the
  // session in the main view (`FleetSessionView`, Phase 2), and a row that
  // needs you says so with the same table the Inbox uses.
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
    fleetAttachRefusal,
    fleetTrouble,
    fleetHostChoices,
    defaultFleetHost,
    fleetKey,
    fleetNeedsIds,
    fleetSelection,
    openFleetSession,
    type FleetInfo,
    type FleetRows,
  } from './fleets';
  import type { SessionRow } from './sessions';
  import { copyText } from './clipboard';
  import { push } from './toasts';
  import Button from './kit/Button.svelte';

  const NONE: ReadonlySet<number> = new Set();

  let {
    match = () => true,
    filtered = false,
  }: {
    /** The sidebar's filters, for an extra fleet's row (Sidebar's
     *  `fleetMatch`): its fleet, the row, its project's name there and
     *  whether it needs you. */
    match?: (fleet: string, s: SessionRow, proj: string, needs: boolean) => boolean;
    /** A filter narrows the list, so an empty group says so. */
    filtered?: boolean;
  } = $props();

  // Which row has which inline form open, by fleet-qualified key.
  let open = $state<{ key: string; kind: 'prompt' | 'rename' | 'close' } | null>(null);
  let text = $state('');
  let busy = $state(false);
  let starting = $state<string | null>(null);
  let newHost = $state('');
  let newProject = $state<number | null>(null);
  let newName = $state('');

  /** A fleet's sessions in sidebar order, each with the project it belongs
   *  to there as `owner/repo` (Phase 2: an extra fleet's sessions group by
   *  project, like the primary's tree); `Other` for one with none. Each
   *  project is named once and each row's key computed once, not per
   *  comparison. */
  function sorted(rows: FleetRows): { s: SessionRow; proj: string }[] {
    const names = new Map(rows.projects.map((t) => [t.project.id, `${t.project.owner}/${t.project.repo}`]));
    return [...rows.sessions.values()]
      .filter((s) => s.status !== 'lost')
      .map((s) => ({ s, proj: (s.project_id == null ? undefined : names.get(s.project_id)) ?? 'Other' }))
      .sort(
        (a, b) =>
          (a.proj === 'Other' ? 1 : 0) - (b.proj === 'Other' ? 1 : 0) ||
          a.proj.localeCompare(b.proj) ||
          (b.s.pinned_at ?? 0) - (a.s.pinned_at ?? 0) ||
          b.s.last_activity_at - a.s.last_activity_at,
      );
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
    if (fleetAttachRefusal(s)) return;
    if (await copyText(fleetAttachCommand(s))) push({ kind: 'success', message: 'Attach command copied' });
  }

  function startForm(f: FleetInfo, rows: FleetRows) {
    starting = starting === f.id ? null : f.id;
    newHost = defaultFleetHost(rows);
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
  {@const needs = $fleetNeedsIds.get(f.id) ?? NONE}
  {@const all = sorted(rows)}
  {@const list = all.filter(({ s, proj }) => match(f.id, s, proj, needs.has(s.id)))}
  <section class="fleet-group" style="--fleet: {f.color}" data-testid="fleet-group-{f.id}">
    <header class="head">
      <span class="badge" data-testid="fleet-badge-{f.id}">{f.label}</span>
      <span class="state {cw.tone}" data-testid="fleet-state-{f.id}">{cw.word}</span>
      <span class="count">{filtered && list.length !== all.length ? `${list.length} of ${all.length}` : list.length}</span>
      <span class="grow"></span>
      {#if cw.tone === 'bad' || cw.tone === 'warn'}
        <Button size="sm" variant="quiet" testid="fleet-retry-{f.id}" onclick={() => void retryFleet(f.id)}>Retry</Button>
      {/if}
      <Button size="sm" variant="quiet" testid="fleet-refresh-{f.id}" onclick={() => void loadFleetRows(f.id)}>Refresh</Button>
      {#if !f.unavailable}
        <Button size="sm" variant="quiet" testid="fleet-new-{f.id}" onclick={() => startForm(f, rows)}>New…</Button>
      {/if}
    </header>
    {#if f.duplicate_of}
      <p class="note err" data-testid="fleet-duplicate-{f.id}">
        This is the same hub as {f.duplicate_of}, so its sessions are shown there. Remove one of the two in Settings → Hub &amp; sync.
      </p>
    {:else if f.unavailable}
      <p class="note err" data-testid="fleet-unavailable-{f.id}">{f.unavailable}</p>
    {:else if fleetTrouble(f)}
      <p class="note err" data-testid="fleet-trouble-{f.id}">{fleetTrouble(f)}</p>
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
          {#each fleetHostChoices(rows) as h (h.alias)}
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
    {#if filtered && list.length === 0 && all.length > 0}
      <p class="note" data-testid="fleet-filtered-{f.id}">No session here matches the filters.</p>
    {/if}
    <ul class="rows">
      {#each list as { s, proj }, i (s.id)}
        {@const key = fleetKey(f.id, s.id)}
        {#if i === 0 || list[i - 1].proj !== proj}
          <li class="proj" data-testid="fleet-project-{f.id}-{proj}">{proj}</li>
        {/if}
        <li
          class="row"
          class:selected={$fleetSelection?.fleet === f.id && $fleetSelection?.id === s.id}
          data-testid="fleet-session-{f.id}-{s.id}"
        >
          <div class="line">
            <button
              type="button"
              class="name"
              title="Open {s.tmux_name}"
              data-testid="fleet-open-{f.id}-{s.id}"
              onclick={() => openFleetSession(f.id, s.id)}
            >{#if s.pinned_at}<span class="pin" aria-label="pinned">◆</span>{/if}{label(s)}</button>
            {#if needs.has(s.id)}<span class="needs" data-testid="fleet-needs-{f.id}-{s.id}">needs you</span>{/if}
            <span class="host">{s.host_alias}</span>
            {#if s.claude_status}<span class="status">{s.claude_status}</span>{/if}
          </div>
          <div class="actions">
            <Button size="sm" variant="quiet" testid="fleet-prompt-{f.id}-{s.id}" onclick={() => toggle(f.id, s, 'prompt')}>Prompt…</Button>
            <Button size="sm" variant="quiet" onclick={() => toggle(f.id, s, 'rename')}>Rename…</Button>
            <Button size="sm" variant="quiet" onclick={() => void run('Pinning', fleetPinSession(f.id, s, !s.pinned_at))}>{s.pinned_at ? 'Unpin' : 'Pin'}</Button>
            <Button
              size="sm"
              variant="quiet"
              disabled={!!fleetAttachRefusal(s)}
              title={fleetAttachRefusal(s) ?? undefined}
              onclick={() => void copyAttach(s)}>Copy attach</Button
            >
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
    background: none;
    border: 0;
    padding: 0;
    font: inherit;
    text-align: left;
    cursor: pointer;
  }
  .name:hover {
    text-decoration: underline;
  }
  .proj {
    margin-top: var(--space-1);
    font-size: var(--text-xs);
    color: var(--fg-muted);
    font-weight: 600;
  }
  .row.selected {
    background: color-mix(in srgb, var(--fleet) 12%, transparent);
    border-radius: var(--radius-xs);
  }
  .needs {
    font-size: var(--text-xs);
    color: var(--status-waiting);
    font-weight: 600;
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
