<script lang="ts">
  // Quick switcher — ⌘K / ⌘P on macOS, Ctrl+Shift+K / Ctrl+Shift+P elsewhere
  // (plain Ctrl+K / Ctrl+P stay with the terminal: readline kill-line and
  // previous-history). One text box, fuzzy-ranked sessions (recent first)
  // plus "New session in <project>" rows. Enter attaches the highlighted
  // session; Cmd/Ctrl+Enter opens the new-session dialog with the query as
  // the name. The chord is taken even while the terminal has focus (VS Code
  // does the same for its quick open), so the switcher is reachable from the
  // state the user is in 90% of the time.
  import { onMount, onDestroy, tick } from 'svelte';
  import Modal from './Modal.svelte';
  import PickerList, { optionId } from './PickerList.svelte';
  import type { PickerItem } from './PickerList.svelte';
  import { sessions, type SessionRow } from './sessions';
  import { projects } from './projects';
  import { hosts } from './hosts';
  import { requestHostsView } from './app_views';
  import { selectedSession, selectSessionExplicitly } from './selection';
  import { requestNewSession } from './new_session_request';
  import { detectMac } from './terminal_keys';
  import {
    buildEntries,
    rankEntries,
    contextProject,
    recentSessions,
    noteRecent,
    isSwitcherChord,
    chordLabel,
    ticketEntries,
    lookupEntry,
    placeForTicket,
    type SwitcherEntry,
    type SwitcherTicket,
  } from './quick_switcher';
  import { workTickets, workLookup, startWork, trackers, type TicketRow } from './trackers';
  import { workKeyFor, worktreeBranchById } from './work_keys';
  import { settingsOpen } from './app_views';
  import { push, pushError } from './toasts';
  import { hubActionBlocked, hubStatus } from './hub';
  import { hubConnection } from './hub_connection';

  let {
    // Injectable for tests; defaults to the real platform.
    isMac = detectMac(typeof navigator === 'undefined' ? undefined : navigator),
  }: { isMac?: boolean } = $props();

  const LIST_ID = 'quick-switcher-list';

  let open = $state(false);
  let query = $state('');
  let activeKey = $state<string | null>(null);

  const modKey = $derived(isMac ? '⌘' : 'Ctrl');
  const chord = $derived(chordLabel(isMac));

  // Any selection (sidebar click, switcher, restore-on-launch) feeds the
  // MRU list, so "recent first" reflects what the user actually opened.
  const unsubSelected = selectedSession.subscribe((s) => {
    if (s) noteRecent(s);
  });
  onDestroy(unsubSelected);

  // Tickets (work graph M3): the cached My work / Current sprint / Recent,
  // loaded when the switcher opens and there is a tracker.
  let tickets = $state<SwitcherTicket[]>([]);
  async function loadTickets() {
    if ($trackers.length === 0) {
      tickets = [];
      return;
    }
    const views: [string, string][] = [
      ['mine', 'My work'],
      ['sprint', 'Current sprint'],
      ['recent', 'Recent'],
    ];
    const answers = await Promise.all(views.map(([view]) => workTickets({ view, limit: 20 })));
    const out: SwitcherTicket[] = [];
    answers.forEach((r, i) => {
      if (r.ok && Array.isArray(r.value)) {
        for (const ticket of r.value) out.push({ ticket, section: views[i][1] });
      }
    });
    tickets = out;
  }
  const ticketRows = $derived(ticketEntries(tickets));
  const lookupRow = $derived(
    lookupEntry(
      query,
      new Set(ticketRows.map((e) => (e.ticket?.key ?? '').toUpperCase())),
    ),
  );
  const entries = $derived([
    ...buildEntries($sessions, $projects, $hosts),
    ...ticketRows,
    ...(lookupRow ? [lookupRow] : []),
  ]);
  const ranked: SwitcherEntry[] = $derived(rankEntries(entries, query, $recentSessions));
  const items: PickerItem[] = $derived(
    ranked.map((e) => ({
      key: e.key,
      label: e.label,
      description: e.description,
      meta: e.meta,
      group:
        e.kind === 'session'
          ? 'Sessions'
          : e.kind === 'host'
            ? 'Hosts'
            : e.kind === 'ticket'
              ? (e.section ?? 'Tickets')
              : e.kind === 'lookup'
                ? 'Lookup'
                : 'Projects',
      testid: `switcher-${e.kind}`,
    })),
  );

  // Keep the highlight on a row that still exists; default to the first.
  // Only while open: a closed switcher must not re-rank on every session
  // event (reading `ranked` here is what would make it recompute).
  $effect(() => {
    if (!open) return;
    const keys = ranked.map((e) => e.key);
    if (activeKey === null || !keys.includes(activeKey)) {
      activeKey = keys[0] ?? null;
    }
  });

  function show() {
    query = '';
    activeKey = null;
    open = true;
    void loadTickets();
  }
  function hide() {
    open = false;
  }

  function onWindowKeydown(e: KeyboardEvent) {
    if (!isSwitcherChord(e, isMac)) return;
    // Another modal (settings, new-session…) owns the keyboard while open;
    // don't stack the switcher on top of it.
    if (!open && (e.target as Element | null)?.closest?.('dialog')) return;
    e.preventDefault();
    e.stopPropagation();
    if (open) hide();
    else show();
  }

  onMount(() => {
    // Capture phase: beat the terminal's own keydown handler to the chord.
    window.addEventListener('keydown', onWindowKeydown, true);
  });
  onDestroy(() => {
    window.removeEventListener('keydown', onWindowKeydown, true);
  });

  function move(delta: number) {
    if (ranked.length === 0) return;
    const i = ranked.findIndex((e) => e.key === activeKey);
    const next = i === -1 ? 0 : (i + delta + ranked.length) % ranked.length;
    activeKey = ranked[next].key;
  }

  function pick(key: string) {
    const e = ranked.find((x) => x.key === key);
    if (!e) return;
    if (e.kind === 'session' && e.session) {
      if (e.session.status === 'ghost') {
        push({ kind: 'info', message: 'That session is a ghost — recreate it from the sidebar.' });
        return;
      }
      selectSessionExplicitly(e.session);
      hide();
    } else if (e.kind === 'project' && e.project) {
      requestNewSession({ project: e.project });
      hide();
    } else if (e.kind === 'ticket' && e.ticket) {
      openTicket(e.ticket);
    } else if (e.kind === 'lookup' && e.lookup) {
      void lookupThenOpen(e.lookup);
    } else if (e.kind === 'host' && e.host) {
      const alias = e.host.alias;
      hide();
      // After the switcher has unmounted and handed focus back, so the Hosts
      // view remembers the right element to restore on close.
      void tick().then(() => requestHostsView(alias));
    }
  }

  // ── Tickets ──
  const branchById = $derived(worktreeBranchById($projects));
  function liveSessionOf(t: TicketRow): SessionRow | null {
    const ids = t.live_session_ids ?? [];
    return $sessions.find((s) => ids.includes(s.id) && s.status !== 'ghost') ?? null;
  }
  /** Enter on a ticket: jump to its live session, else the dialog, prefilled. */
  function openTicket(t: TicketRow) {
    const live = liveSessionOf(t);
    if (live) {
      selectSessionExplicitly(live);
      hide();
      return;
    }
    const key = t.key ?? '';
    const place = placeForTicket(key, $sessions, $projects, (s) => workKeyFor(s, branchById)?.key ?? null);
    const project = place?.project ?? contextProject(ranked, $selectedSession, $projects);
    if (!project) {
      push({ kind: 'info', message: 'No projects yet — refresh the sidebar first.' });
      return;
    }
    requestNewSession({
      project,
      initialName: t.title ? `${key} ${t.title}` : key,
      initialHost: place?.host,
      ticket: t,
    });
    hide();
  }
  async function lookupThenOpen(reference: string) {
    const r = await workLookup(reference);
    if (r.ok) {
      openTicket(r.value);
      return;
    }
    const site = (r.error.details as { site_url?: string } | null | undefined)?.site_url;
    if (r.error.code === 'E_NOTFOUND' && site) {
      push({
        kind: 'info',
        message: `${site} is not connected — connect Jira in Settings → Work to look up its tickets.`,
        action: { label: 'Settings', run: () => settingsOpen.set(true) },
      });
      return;
    }
    pushError(r.error, 'Lookup failed');
  }
  /** Cmd/Ctrl+Enter on a ticket: start with the defaults, no dialog. */
  async function startTicketNow(e: SwitcherEntry) {
    const blocked = hubActionBlocked('start_work', $hubStatus, $hubConnection);
    if (blocked) {
      push({ kind: 'info', message: blocked });
      return;
    }
    const ref = e.ticket?.id != null ? { item_id: e.ticket.id } : { reference: e.lookup ?? '' };
    hide();
    const r = await startWork({ ...ref, with_brief: true });
    if (r.ok) {
      selectSessionExplicitly(r.value);
      return;
    }
    const d = r.error.details as { session_id?: number } | null | undefined;
    if (r.error.code === 'E_EXISTS' && d?.session_id != null) {
      const live = $sessions.find((s) => s.id === d.session_id);
      if (live) {
        selectSessionExplicitly(live);
        return;
      }
    }
    if (r.error.code === 'E_AMBIGUOUS' && e.ticket) {
      // No project to default to: the dialog asks.
      openTicket(e.ticket);
      return;
    }
    pushError(r.error, 'Start work failed');
  }

  function newWithQuery() {
    const p = contextProject(ranked, $selectedSession, $projects);
    if (!p) {
      push({ kind: 'info', message: 'No projects yet — refresh the sidebar first.' });
      return;
    }
    requestNewSession({ project: p, initialName: query.trim() || undefined });
    hide();
  }

  function onInputKeydown(e: KeyboardEvent) {
    if (e.key === 'ArrowDown' || (e.ctrlKey && e.key.toLowerCase() === 'n')) {
      e.preventDefault();
      move(1);
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      move(-1);
    } else if (e.key === 'Enter') {
      e.preventDefault();
      const active = ranked.find((x) => x.key === activeKey);
      if ((e.metaKey || e.ctrlKey) && (active?.kind === 'ticket' || active?.kind === 'lookup')) {
        void startTicketNow(active);
      } else if (e.metaKey || e.ctrlKey) newWithQuery();
      else if (activeKey !== null) pick(activeKey);
      else newWithQuery();
    }
  }
</script>

{#if open}
  <Modal label="Quick switcher" onclose={hide} width="560px" testid="quick-switcher">
    <input
      class="query"
      data-testid="switcher-input"
      data-autofocus
      role="combobox"
      aria-expanded="true"
      aria-controls={LIST_ID}
      aria-autocomplete="list"
      aria-activedescendant={activeKey !== null ? optionId(LIST_ID, activeKey) : undefined}
      bind:value={query}
      onkeydown={onInputKeydown}
      placeholder="Jump to a session, host or ticket… (name, key, project, host, branch, status, or paste a ticket URL)"
      autocomplete="off"
      spellcheck="false"
    />
    <PickerList
      {items}
      {activeKey}
      onactivate={(k) => (activeKey = k)}
      onpick={pick}
      maxHeight="min(60vh, 24rem)"
      emptyText={query ? `No session matches “${query}” — ${modKey}↵ creates one with that name.` : 'No sessions yet.'}
      ariaLabel="Sessions"
      listId={LIST_ID}
      testid="switcher-list"
    />
    <div class="hint">
      <span>↑↓ move</span>
      <span>↵ attach / open</span>
      <span>{modKey}↵ new session named “{query.trim() || '…'}” (on a ticket: start it)</span>
      <span>esc / {chord} close</span>
    </div>
  </Modal>
{/if}

<style>
  .query {
    font: inherit;
    font-size: 0.95rem;
    padding: 0.45rem 0.6rem;
    border: 1px solid var(--border);
    background: var(--bg-pane);
    color: var(--fg);
    border-radius: 4px;
    width: 100%;
    box-sizing: border-box;
  }
  .query:focus {
    outline: none;
    border-color: var(--accent);
  }
  .hint {
    display: flex;
    flex-wrap: wrap;
    gap: 0.8rem;
    font-size: 0.7rem;
    color: var(--fg-muted);
  }
</style>
