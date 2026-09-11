<script lang="ts">
  // Quick switcher — ⌘K / ⌘P on macOS, Ctrl+Shift+K / Ctrl+Shift+P elsewhere
  // (plain Ctrl+K / Ctrl+P stay with the terminal: readline kill-line and
  // previous-history). One text box, fuzzy-ranked sessions (recent first)
  // plus "New session in <project>" rows. Enter attaches the highlighted
  // session; Cmd/Ctrl+Enter opens the new-session dialog with the query as
  // the name. The chord is taken even while the terminal has focus (VS Code
  // does the same for its quick open), so the switcher is reachable from the
  // state the user is in 90% of the time.
  import { onMount, onDestroy } from 'svelte';
  import Modal from './Modal.svelte';
  import PickerList, { optionId } from './PickerList.svelte';
  import type { PickerItem } from './PickerList.svelte';
  import { sessions } from './sessions';
  import { projects } from './projects';
  import { selectedSession, selectSession } from './selection';
  import { requestNewSession } from './new_session_request';
  import { push } from './toasts';
  import { detectMac } from './terminal_keys';
  import {
    buildEntries,
    rankEntries,
    contextProject,
    recentSessions,
    noteRecent,
    isSwitcherChord,
    chordLabel,
    type SwitcherEntry,
  } from './quick_switcher';

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

  const entries = $derived(buildEntries($sessions, $projects));
  const ranked: SwitcherEntry[] = $derived(rankEntries(entries, query, $recentSessions));
  const items: PickerItem[] = $derived(
    ranked.map((e) => ({
      key: e.key,
      label: e.label,
      description: e.description,
      meta: e.meta,
      group: e.kind === 'session' ? 'Sessions' : 'Projects',
      testid: e.kind === 'session' ? 'switcher-session' : 'switcher-project',
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
      selectSession(e.session);
      hide();
    } else if (e.kind === 'project' && e.project) {
      requestNewSession({ project: e.project });
      hide();
    }
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
      if (e.metaKey || e.ctrlKey) newWithQuery();
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
      placeholder="Jump to a session… (name, project, host, branch, status)"
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
      <span>{modKey}↵ new session named “{query.trim() || '…'}”</span>
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
