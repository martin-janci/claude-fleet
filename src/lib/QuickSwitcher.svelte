<script lang="ts">
  // Quick switcher — ⌘K / ⌘P on macOS, Ctrl+Shift+K / Ctrl+Shift+P elsewhere
  // (plain Ctrl+K / Ctrl+P stay with the terminal: readline kill-line and
  // previous-history). One text box, fuzzy-ranked sessions (recent first)
  // plus "New session in <project>" rows. Enter attaches the highlighted
  // session; Cmd/Ctrl+Enter opens the new-session dialog with the query as
  // the name. The chord is taken even while the terminal has focus (VS Code
  // does the same for its quick open), so the switcher is reachable from the
  // state the user is in 90% of the time.
  //
  // New session mode (project picker spec v2): ⌘N / Ctrl+Shift+N, the
  // sidebar's "+ New session" (`switcher_request.ts`) and the Hosts view's
  // `n` (`newSessionHostRequest`) open the same box listing only tickets and
  // projects — Start from work, Pinned, Suggested, every group, Hidden — with
  // keys to pin, hide, group and undo. Its ranking is a snapshot taken on
  // open: it is recomputed on a query change, a fold or the person's own
  // action, never because data arrived.
  import { onMount, onDestroy, tick, untrack } from 'svelte';
  import { get } from 'svelte/store';
  import { newTaskOwnsChord } from './new_task';
  import ProjectActionsMenu from './ProjectActionsMenu.svelte';
  import Icon from './kit/Icon.svelte';
  import { switcherRequest } from './switcher_request';
  import { newSessionHostRequest, addProjectRequest } from './app_views';
  import { loadProjectPicks, pickKey, previousPick, projectPicks, setProjectPick } from './project_picks';
  import { readFrecency, recordPick } from './frecency';
  import { buildSections, entriesOf, hiddenReason, searchEntries, type Entry, type ViewRow } from './project_rank';
  import { fuzzyMatchFields } from './fuzzy';
  import { parseRepoUrl } from './repo_url';
  import Modal from './Modal.svelte';
  import PickerList, { optionId } from './PickerList.svelte';
  import AccountPill from './AccountPill.svelte';
  import type { PickerItem } from './PickerList.svelte';
  import { sessions, sessionsAnswered, type SessionRow } from './sessions';
  import Loader from './Loader.svelte';
  import { searchLoader } from './search_loader';
  import { projects } from './projects';
  import { hosts } from './hosts';
  import { requestAssetsView, requestHostsView } from './app_views';
  import { catalog } from './assets';
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
    assetEntries,
    commandEntries,
    placeForTicket,
    isNewSessionChord,
    workBlock,
    type SwitcherEntry,
    type SwitcherTicket,
    scopeEntries,
    TICKET_SEARCH_MIN,
    TICKET_SEARCH_DEBOUNCE_MS,
    switcherEmptyText,
  } from './quick_switcher';
  import { effectiveScope, scopeOf } from './orgs';
  import {
    workTickets,
    workLookup,
    startWork,
    trackers,
    providerInfo,
    showProviderBadges,
    keyFamily,
    type TicketRow,
  } from './trackers';
  import { previewStartWork, projectProposal } from './start_preview';
  import { preselect, type ProposalLike } from './ai_proposal';
  import { workKeyFor, worktreeBranchById } from './work_keys';
  import { settingsOpen } from './app_views';
  import { push, pushError } from './toasts';
  import { hubActionBlocked, hubStatus, ownsTheFleet } from './hub';
  import { hubConnection } from './hub_connection';
  import {
    commandRows,
    keepsKind,
    paletteCommands,
    runCommand,
    runSetting,
    settingRow,
    splitPrefix,
  } from './commands';
  import { descriptors, loadDescriptors } from './pages/pages';
  import { splitCommand } from './pages/settings_nl';
  import { settingsWritable } from './pages/review';
  import { sessionView } from './prefs';

  let {
    // Injectable for tests; defaults to the real platform.
    isMac = detectMac(typeof navigator === 'undefined' ? undefined : navigator),
  }: { isMac?: boolean } = $props();

  const LIST_ID = 'quick-switcher-list';

  let open = $state(false);
  let query = $state('');
  let activeKey = $state<string | null>(null);
  let mode = $state<'switch' | 'new'>('switch');
  // New session mode: the host to prefer ("on <host>" suggestions, the
  // dialog's preselected host), the folds the person toggled, and the
  // actions menu.
  let preferredHost = $state<string | null>(null);
  /** The ticket a project pick starts (the dialog's Change, redesign 3.12). */
  let pendingTicket = $state<TicketRow | null>(null);
  let toggled = $state<ReadonlySet<string>>(new Set());
  // Re-rank triggers besides the query: open, the person's own actions, folds.
  let seq = $state(0);
  let menu = $state<{ key: string; startIn: 'main' | 'groups' } | null>(null);
  let menuAt = $state<{ top?: number; bottom?: number }>({ top: 0 });
  let listWrap: HTMLElement | undefined = $state();
  let lastUndo: (() => void) | null = null;
  const nowSec = () => Math.floor(Date.now() / 1000);

  const modKey = $derived(isMac ? '⌘' : 'Ctrl');
  const chord = $derived(chordLabel(isMac));

  // Any selection (sidebar click, switcher, restore-on-launch) feeds the
  // MRU list, so "recent first" reflects what the user actually opened.
  const unsubSelected = selectedSession.subscribe((s) => {
    if (s) noteRecent(s);
  });
  onDestroy(unsubSelected);

  // Tickets (work graph M3): the cached My work / Current sprint / Recent
  // when there is a tracker, and the person's own tasks (TASK-n) always.
  let tickets = $state<SwitcherTicket[]>([]);
  // Only the newest load lands (review r07).
  let ticketsSeq = 0;
  // The ticket views are a step of the search while they load (9.12).
  let ticketsLoading = $state(false);
  async function loadTickets() {
    const mine = ++ticketsSeq;
    ticketsLoading = true;
    const views: [string, string][] =
      $trackers.length === 0
        ? []
        : [
            ['mine', 'My work'],
            ['sprint', 'Current sprint'],
            ['recent', 'Recent'],
          ];
    // The unfiltered list with `include_local` puts the caller's tasks after
    // the tickets; only the tasks are kept from it (an older hub ignores the
    // flag and answers tickets alone).
    const [answers, own] = await Promise.all([
      Promise.all(views.map(([view]) => workTickets({ view, limit: 20 }))),
      workTickets({ include_local: true, limit: 40 }),
    ]);
    if (mine !== ticketsSeq) return;
    ticketsLoading = false;
    const out: SwitcherTicket[] = [];
    answers.forEach((r, i) => {
      if (r.ok && Array.isArray(r.value)) {
        for (const ticket of r.value) out.push({ ticket, section: views[i][1] });
      }
    });
    if (own.ok && Array.isArray(own.value)) {
      for (const ticket of own.value) if (ticket.source === 'local') out.push({ ticket, section: 'My tasks' });
    }
    tickets = out;
  }
  // A query also searches the whole cache on the hub (key, title and
  // assignees, every word, accents ignored), so a ticket outside the three
  // views is found by its title, not only by its exact key.
  let searched = $state<SwitcherTicket[]>([]);
  let searchedSeq = 0;
  $effect(() => {
    const q = prefix.rest.trim();
    const wanted = open && q.length >= TICKET_SEARCH_MIN && (prefix.mode === 'all' || prefix.mode === 'work');
    const mine = ++searchedSeq;
    if (!wanted) {
      searched = [];
      return;
    }
    const t = setTimeout(async () => {
      const r = await workTickets({ query: q, include_local: true, limit: 30 });
      if (mine !== searchedSeq) return;
      searched = r.ok && Array.isArray(r.value) ? r.value.map((ticket) => ({ ticket, section: 'Search' })) : [];
    }, TICKET_SEARCH_DEBOUNCE_MS);
    return () => clearTimeout(t);
  });
  // Work graph M6: a provider badge per ticket, once trackers of two or
  // more providers exist.
  const ticketBadges = $derived(
    new Map(
      showProviderBadges($trackers)
        ? $trackers.flatMap((t) => {
            const p = providerInfo(t.provider);
            return p ? [[t.id, { icon: p.icon, title: p.label }] as const] : [];
          })
        : [],
    ),
  );
  // The views first: a key in both keeps its view's section.
  const ticketRows = $derived(ticketEntries([...tickets, ...searched], ticketBadges));
  // Step 3.13: what the switcher holds shows at once; until the first
  // session list answers, one line says which hosts it is still hearing
  // from, with the kit's Dot wave (it appears only after 400 ms).
  const stillHearing = $derived.by(() => {
    if ($sessionsAnswered) return null;
    const names = $hosts.filter((h) => !h.hidden).map((h) => h.alias);
    if (names.length === 0) return 'Sessions still arriving';
    const shown = names.slice(0, 3).join(', ');
    const more = names.length > 3 ? ` and ${names.length - 3} more` : '';
    return `Still hearing from ${shown}${more}`;
  });
  // Step 9.12: the search's current step (the hosts it still hears from,
  // then the ticket views), with the Dot wave while it is short and Comet
  // trails once it has run long (`search_loader.ts`). One loader on the line.
  const searchStep = $derived(stillHearing ?? (ticketsLoading ? ($trackers.length > 0 ? 'Reading your tickets: My work, Current sprint, Recent' : 'Reading your tasks') : null));
  let searchSince = $state(0);
  let searchNow = $state(0);
  $effect(() => {
    if (!open || !searchStep) return;
    const t = setInterval(() => (searchNow = Date.now()), 500);
    return () => clearInterval(t);
  });
  const searchMark = $derived(searchLoader(searchNow - searchSince));
  // Step 3.9: `>` commands, `#` tasks and tickets, `@` hosts.
  const prefix = $derived(splitPrefix(query));
  const lookupRow = $derived(
    lookupEntry(
      prefix.rest,
      new Set(ticketRows.map((e) => (e.ticket?.key ?? '').toUpperCase())),
    ),
  );
  // Settings in plain words ("set recent work to 3 days"): the registry is
  // loaded the first time a query reads like a command.
  const canWriteSettings = $derived(ownsTheFleet($hubStatus) && $settingsWritable);
  const settingChange = $derived(settingRow(prefix.rest, $descriptors.values(), canWriteSettings));
  let descriptorsAsked = false;
  $effect(() => {
    if (!open || descriptorsAsked || !canWriteSettings || !splitCommand(prefix.rest)) return;
    descriptorsAsked = true;
    if (untrack(() => $descriptors.size) === 0) void loadDescriptors();
  });
  // Work graph M5: the sidebar's org scope narrows ⌘K too (one
  // `rowMatches` for both, so they never disagree).
  const trackerOrg = $derived(new Map($trackers.map((t) => [t.id, t.org_id ?? null])));
  const entries = $derived(
    scopeEntries(
      [
        ...buildEntries($sessions, $projects, $hosts),
        ...assetEntries($catalog),
        ...commandEntries(),
        ...commandRows(paletteCommands({ selected: $selectedSession, sessionView: $sessionView }), isMac),
        ...(settingChange ? [settingChange] : []),
        ...ticketRows,
        ...(lookupRow ? [lookupRow] : []),
      ],
      $effectiveScope,
      $scopeOf,
      trackerOrg,
    ),
  );
  // Normal ⌘K drops picker-hidden projects from its empty-query list, so
  // both surfaces agree on what is hidden; a query still finds them. A
  // prefix keeps only its own kind of row.
  const visibleEntries = $derived(
    (prefix.rest.trim()
      ? entries
      : entries.filter(
          (e) =>
            e.kind !== 'project' ||
            !e.project ||
            !hiddenReason(
              e.project,
              $projectPicks.get(pickKey(e.project.project.owner, e.project.project.repo)),
              nowSec(),
            ),
        )
    )
      .filter((e) => keepsKind(prefix.mode, e.kind))
      // Palette commands wait for a query or `>`: the empty ⌘K stays the
      // list of sessions it has always been.
      .filter((e) => !e.action || prefix.mode === 'commands' || prefix.rest.trim() !== ''),
  );
  const ranked: SwitcherEntry[] = $derived(rankEntries(visibleEntries, prefix.rest, $recentSessions));
  const items: PickerItem[] = $derived(
    ranked.map((e) => ({
      key: e.key,
      label: e.label,
      description: e.description,
      meta: e.meta,
      badge: e.badge,
      group:
        e.kind === 'session'
          ? 'Sessions'
          : e.kind === 'host'
            ? 'Hosts'
            : e.kind === 'ticket'
              ? (e.section ?? 'Tickets')
              : e.kind === 'lookup'
                ? 'Lookup'
                : e.kind === 'asset'
                  ? 'Assets'
                  : e.kind === 'command'
                    ? (e.section ?? 'Commands')
                    : e.kind === 'setting'
                      ? 'Settings'
                      : 'Projects',
      testid: `switcher-${e.kind}`,
    })),
  );

  // ── New session mode ──
  // The frozen view: the stores are read untracked, so data arriving while
  // open never re-ranks; `seq`, the query and the folds do.
  const newView = $derived.by(() => {
    void seq;
    void query;
    void toggled;
    if (mode !== 'new') return null;
    return untrack(() => {
      const now = nowSec();
      const entries = entriesOf(get(projects), get(projectPicks), now);
      const ctx = {
        selectedProjectId: get(selectedSession)?.project_id ?? null,
        preferredHost,
        sessions: get(sessions),
      };
      const f = readFrecency();
      return {
        entries,
        sections: buildSections(entries, ctx, f, now),
        search: searchEntries(entries, query, ctx, f, now),
      };
    });
  });

  /** ⌘1 on macOS, Ctrl+1 elsewhere (the ranking labels them ⌘N). */
  const kbdLabel = (k: string) => (isMac || !k ? k : k.replace('⌘', 'Ctrl+'));
  const countText = (n: number) => `${n} project${n === 1 ? '' : 's'}`;
  /** A section's rule subtitle, or '' when it is only the member count. */
  const ruleSub = (sub: string) => (/^\d+ projects?$/.test(sub) ? '' : sub);
  const isFoldable = (sectionKey: string) => sectionKey.startsWith('g:') || sectionKey === 'hidden';

  // A project is listed once per section it is in (Suggested and its
  // group both list it, D5), so its row key names the section too.
  const rowKey = (id: number, sectionKey: string) => `project:${id}@${sectionKey}`;
  const idOfKey = (key: string | null): number | null => {
    const m = key ? /^project:(\d+)@/.exec(key) : null;
    return m ? Number(m[1]) : null;
  };
  const projectItem = (r: ViewRow, group: string, groupKey: string, groupSub: string, description?: string): PickerItem => ({
    key: rowKey(r.entry.id, groupKey),
    label: r.entry.label,
    description,
    meta: r.meta,
    chip: r.chip || undefined,
    kbd: kbdLabel(r.kbd) || undefined,
    dim: r.entry.dormant || !!r.entry.hidden,
    group,
    groupKey,
    groupSub,
    actionable: true,
    testid: 'switcher-project',
  });

  const newItems: PickerItem[] = $derived.by(() => {
    const v = newView;
    if (!v) return [];
    const out: PickerItem[] = [];
    const q = query.trim();
    const work = q
      ? [...ticketRows.filter((e) => fuzzyMatchFields(q, e.fields) !== null), ...(lookupRow ? [lookupRow] : [])]
      : workBlock(tickets)
          .map((t) => ticketRows.find((e) => e.ticket?.key === t.ticket.key))
          .filter((e): e is SwitcherEntry => !!e);
    for (const e of work) {
      const place = e.ticket
        ? placeForTicket(e.ticket.key ?? '', $sessions, $projects, (s) => workKeyFor(s, branchById)?.key ?? null)
        : null;
      out.push({
        key: e.key,
        label: e.label,
        description: place ? `→ ${place.project.project.repo} · ${place.host}` : e.description,
        badge: e.badge,
        group: q ? 'Tickets' : 'Start from work',
        groupKey: 'work',
        groupSub: q ? '' : 'My work · no session yet',
        testid: e.kind === 'lookup' ? 'switcher-lookup' : 'switcher-ticket',
      });
    }
    if (q) {
      const n = v.search.length;
      for (const r of v.search) {
        const g = r.entry.group;
        // The group says where a match lives, unless it is only the owner.
        const where = g.key.startsWith('o:') || g.key === 'f' ? undefined : g.name;
        out.push(projectItem(r, countText(n), 'search', '', where));
      }
    } else {
      for (const s of v.sections) {
        const open = s.foldable ? (s.openByDefault ? !toggled.has(s.key) : toggled.has(s.key)) : true;
        if (open) {
          const sub = s.foldable && s.key !== 'hidden' ? ruleSub(s.sub) || countText(s.rows.length) : s.sub;
          s.rows.forEach((r) => out.push(projectItem(r, s.label, s.key, sub)));
        } else {
          // A folded section is one option row, so the keyboard reaches it.
          out.push({
            key: `fold:${s.key}`,
            label:
              s.key === 'hidden' ? `Show ${s.rows.length} in Hidden` : `${s.label} · ${countText(s.rows.length)}`,
            description: s.key === 'hidden' ? s.sub : ruleSub(s.sub) || undefined,
            meta: '▸',
            testid: 'switcher-fold',
          });
        }
      }
    }
    out.push({
      key: 'add',
      label: q ? `Add project “${q}”…` : 'Add project…',
      group: q ? 'Not here?' : ' ',
      testid: 'switcher-add',
    });
    return out;
  });

  /** The rows ↑↓ walk and the highlight lives on, in either mode. */
  const listKeys = $derived(mode === 'new' ? newItems.map((i) => i.key) : ranked.map((e) => e.key));

  // Keep the highlight on a row that still exists; default to the first.
  // Only while open: a closed switcher must not re-rank on every session
  // event (reading `ranked` here is what would make it recompute). A
  // project whose row moved (pinned, unpinned, regrouped) keeps it.
  $effect(() => {
    if (!open) return;
    const keys = listKeys;
    if (activeKey === null || !keys.includes(activeKey)) {
      const id = idOfKey(activeKey);
      const moved = id === null ? undefined : keys.find((k) => idOfKey(k) === id);
      activeKey = moved ?? keys[0] ?? null;
    }
  });

  function show(next: 'switch' | 'new' = 'switch', host: string | null = null, ticket: TicketRow | null = null) {
    query = '';
    activeKey = null;
    mode = next;
    preferredHost = host;
    pendingTicket = ticket;
    toggled = new Set();
    menu = null;
    // Undo belongs to this open: a ⌘Z an hour later never reverts a pin.
    lastUndo = null;
    open = true;
    seq++;
    searchSince = searchNow = Date.now();
    void loadTickets();
    // Fresh picks for the NEXT open: this one's view is frozen.
    if (next === 'new') void loadProjectPicks();
  }
  function hide() {
    open = false;
    menu = null;
  }

  // The sidebar's "+ New session" and the Hosts view's `n` (which names the
  // host to prefer) cannot reach this component; they publish a request.
  const unsubReq = switcherRequest.subscribe((r) => {
    if (!r) return;
    switcherRequest.set(null);
    if (r.mode === 'switch') show('switch');
    else show('new', r.host ?? null, r.ticket ?? null);
  });
  const unsubHost = newSessionHostRequest.subscribe((h) => {
    if (h === null) return;
    newSessionHostRequest.set(null);
    show('new', h);
  });
  onDestroy(() => {
    unsubReq();
    unsubHost();
  });

  function onWindowKeydown(e: KeyboardEvent) {
    if (isNewSessionChord(e, isMac)) {
      if (!open && (e.target as Element | null)?.closest?.('dialog')) return;
      // In the Work view the chord is New task (G2.1): WorkTree takes it.
      if (!open && newTaskOwnsChord()) return;
      e.preventDefault();
      e.stopPropagation();
      if (open && mode === 'new') hide();
      else show('new');
      return;
    }
    // ⌘P is the pin key in New session mode: leave it to the input. (⌘K, and
    // Ctrl+Shift+K / P elsewhere, still close the switcher.)
    if (open && mode === 'new' && isMac && e.metaKey && !e.shiftKey && e.key.toLowerCase() === 'p') return;
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
    const keys = listKeys;
    if (keys.length === 0) return;
    const i = keys.findIndex((k) => k === activeKey);
    const next = i === -1 ? 0 : (i + delta + keys.length) % keys.length;
    activeKey = keys[next];
  }

  const entryOf = (key: string | null): Entry | null => {
    const id = idOfKey(key);
    return id === null ? null : (newView?.entries.find((e) => e.id === id) ?? null);
  };
  /** A ticket or lookup row of New session mode. */
  const workEntryOf = (key: string | null): SwitcherEntry | null =>
    key === null ? null : (ticketRows.find((e) => e.key === key) ?? (lookupRow?.key === key ? lookupRow : null));

  /** Pin / hide / group one project. The store changes at once (the person
   *  sees their own change: `seq` re-ranks); Undo is ready at once too, and
   *  its toast comes once the write is saved. */
  function act(e: Entry, patch: Parameters<typeof setProjectPick>[2], said: string) {
    const { owner, repo } = e.project.project;
    const write = setProjectPick(owner, repo, patch);
    seq++;
    // Recorded by setProjectPick before its first await.
    const prev = previousPick(owner, repo);
    // Chained after the write, so a late answer to it never lands on top of
    // the undo; a failed write was rolled back already and needs none.
    const undo = prev
      ? () => {
          void write.then((r) => {
            if (!r.ok) return;
            void setProjectPick(owner, repo, { pinned: prev.pinned, vis: prev.vis, grp: prev.grp }).then((u) => {
              seq++;
              if (u.ok) push({ kind: 'info', message: `Undid: ${said}` });
            });
            seq++;
          });
        }
      : null;
    lastUndo = undo;
    void write.then((r) => {
      seq++;
      if (!r.ok) {
        if (lastUndo === undo) lastUndo = null;
        return;
      }
      push({
        kind: 'info',
        message: said,
        action: undo
          ? {
              label: 'Undo',
              // Only while it is still the latest change: an older toast
              // never reverts a newer pin / hide / group.
              run: () => {
                if (lastUndo !== undo) return;
                lastUndo = null;
                undo();
              },
            }
          : undefined,
      });
    });
  }
  function togglePin(e: Entry) {
    if (e.pinned) act(e, { pinned: false }, `Unpinned ${e.label}`);
    // Never in both Pinned and Hidden: pinning what the person hid unhides it.
    else if (e.hidden === 'hidden by you') act(e, { pinned: true, vis: null }, `Pinned ${e.label} (unhidden)`);
    else act(e, { pinned: true }, `Pinned ${e.label}`);
  }
  function toggleHide(e: Entry) {
    if (e.hidden) {
      act(e, { vis: 'keep' }, `Unhid ${e.label}`);
      return;
    }
    // The highlight stays where it was in the list: on the row that followed
    // the hidden one, else the one before (never another row of the same
    // project, which leaves the list with it).
    const keys = listKeys;
    const at = entryOf(activeKey)?.id === e.id ? keys.indexOf(activeKey!) : -1;
    const other = (k: string) => idOfKey(k) !== e.id;
    const next =
      at === -1 ? null : (keys.slice(at + 1).find(other) ?? keys.slice(0, at).reverse().find(other) ?? null);
    if (e.pinned) act(e, { vis: 'hide', pinned: false }, `Hid ${e.label} (unpinned)`);
    else act(e, { vis: 'hide' }, `Hid ${e.label}`);
    if (next !== null) activeKey = next;
  }
  function setGroup(e: Entry, g: string | null) {
    act(e, { grp: g }, g ? `Moved ${e.label} to ${g}` : `${e.label} is grouped automatically`);
    closeMenu();
  }
  function pickProject(e: Entry, autostart = false) {
    recordPick(e.key);
    const t = pendingTicket;
    if (t) {
      // The person chose the repository for a ticket start: the ticket stays.
      requestNewSession({ project: e.project, initialHost: preferredHost ?? undefined, ...ticketName(t), ticket: t });
    } else {
      requestNewSession({ project: e.project, initialHost: preferredHost ?? undefined, autostart });
    }
    hide();
  }
  function toggleFold(sectionKey: string) {
    const n = new Set(toggled);
    if (n.has(sectionKey)) n.delete(sectionKey);
    else n.add(sectionKey);
    toggled = n;
  }
  /** Unfold a folded section and land on its first row. */
  function unfold(sectionKey: string) {
    toggleFold(sectionKey);
    const first = newView?.sections.find((s) => s.key === sectionKey)?.rows[0];
    activeKey = first ? rowKey(first.entry.id, sectionKey) : null;
  }
  function onGroupClick(sectionKey: string) {
    if (!isFoldable(sectionKey)) return;
    const inside = newItems.find((i) => i.key === activeKey)?.groupKey === sectionKey;
    toggleFold(sectionKey);
    if (inside) activeKey = `fold:${sectionKey}`;
  }

  function openMenu(key: string, startIn: 'main' | 'groups') {
    if (!entryOf(key)) return;
    activeKey = key;
    // Beside the row: below it, or above when the room is below the fold.
    const row = listWrap?.querySelector<HTMLElement>(`[data-key="${CSS.escape(key)}"]`);
    if (listWrap && row) {
      const w = listWrap.getBoundingClientRect();
      const r = row.getBoundingClientRect();
      const room = 200;
      menuAt = r.bottom - w.top + room > w.height && r.top - w.top > room ? { bottom: w.bottom - r.top } : { top: r.bottom - w.top };
    } else {
      menuAt = { top: 0 };
    }
    menu = { key, startIn };
  }
  function closeMenu() {
    menu = null;
    void tick().then(() => document.querySelector<HTMLInputElement>('[data-testid=switcher-input]')?.focus());
  }
  /** Existing group names for the menu: the person's and the clusters. */
  const menuGroups = $derived(
    [
      ...new Set(
        (newView?.entries ?? [])
          .map((x) => x.group)
          .filter((g) => !g.key.startsWith('o:') && g.key !== 'f')
          .map((g) => g.name),
      ),
    ].sort((a, b) => a.localeCompare(b, undefined, { sensitivity: 'base' })),
  );

  function pickNew(key: string) {
    if (key === 'add') {
      const q = query.trim();
      // An `owner/repo` or a GitHub URL prefills the Clone URL field.
      addProjectRequest.set({ cloneUrl: q && parseRepoUrl(q) ? q : undefined });
      hide();
      return;
    }
    if (key.startsWith('fold:')) {
      unfold(key.slice(5));
      return;
    }
    const e = entryOf(key);
    if (e) {
      pickProject(e);
      return;
    }
    const w = workEntryOf(key);
    if (w?.ticket) void openTicket(w.ticket);
    else if (w?.lookup) void lookupThenOpen(w.lookup);
  }

  function pick(key: string) {
    if (mode === 'new') {
      pickNew(key);
      return;
    }
    const e = ranked.find((x) => x.key === key);
    if (!e) return;
    if (e.kind === 'session' && e.session) {
      if (e.session.status === 'ghost') {
        push({ kind: 'info', message: 'That session is lost. Recreate it from the sidebar.' });
        return;
      }
      selectSessionExplicitly(e.session);
      hide();
    } else if (e.kind === 'project' && e.project) {
      requestNewSession({ project: e.project });
      hide();
    } else if (e.kind === 'ticket' && e.ticket) {
      void openTicket(e.ticket);
    } else if (e.kind === 'lookup' && e.lookup) {
      void lookupThenOpen(e.lookup);
    } else if (e.kind === 'asset' && e.asset) {
      const select = e.asset.key;
      hide();
      void tick().then(() => requestAssetsView({ select }));
    } else if (e.kind === 'command' && e.command) {
      const command = e.command;
      hide();
      void tick().then(() => requestAssetsView({ command }));
    } else if (e.kind === 'command' && e.action) {
      const id = e.action;
      const ctx = { selected: $selectedSession, sessionView: $sessionView };
      hide();
      // After the switcher has unmounted, so a view it opens gets the focus.
      void tick().then(() => runCommand(id, ctx));
    } else if (e.kind === 'setting' && e.setting) {
      const setting = e.setting;
      hide();
      void tick().then(() => runSetting(setting));
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
  const ticketName = (t: TicketRow) => ({ initialName: t.title ? `${t.key ?? ''} ${t.title}` : (t.key ?? '') });
  /** Enter on a ticket: jump to its live session, else the dialog, prefilled. */
  async function openTicket(t: TicketRow) {
    const live = liveSessionOf(t);
    if (live) {
      selectSessionExplicitly(live);
      hide();
      return;
    }
    const key = t.key ?? '';
    const place = placeForTicket(key, $sessions, $projects, (s) => workKeyFor(s, branchById)?.key ?? null);
    // Redesign 3.12 (K1): a rule (earlier work on the key's family) beats
    // Jev; with neither, the dialog opens on the context project as before.
    let proposal: ProposalLike | null = null;
    let project = place?.project ?? null;
    if (place) {
      proposal = { value: String(place.project.project.id), source: 'rule', reason: `${keyFamily(key)} work runs here` };
    } else if (t.id != null) {
      hide();
      const r = await previewStartWork({ item_id: t.id, with_brief: true });
      const jev = r.ok ? projectProposal(r.value) : null;
      const id = preselect('project', jev);
      const hit = id == null ? undefined : $projects.find((p) => p.project.id === Number(id));
      if (hit) {
        project = hit;
        proposal = jev;
      }
    }
    project ??= contextProject(ranked, $selectedSession, $projects);
    if (!project) {
      push({ kind: 'info', message: 'No projects yet — refresh the sidebar first.' });
      return;
    }
    requestNewSession({ project, ...ticketName(t), initialHost: place?.host, ticket: t, proposal });
    hide();
  }
  async function lookupThenOpen(reference: string) {
    const r = await workLookup(reference);
    if (r.ok) {
      void openTicket(r.value);
      return;
    }
    const d = r.error.details as { site_url?: string; provider?: string } | null | undefined;
    const site = d?.site_url ?? (d?.provider ? (providerInfo(d.provider)?.label ?? d.provider) : null);
    if (r.error.code === 'E_NOTFOUND' && site) {
      push({
        kind: 'info',
        message: `${site} is not connected — connect it in Settings → Trackers to look up its tickets.`,
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
    const d = r.error.details as { session_id?: number; orphan_session_id?: number } | null | undefined;
    if (r.error.code === 'E_EXISTS' && d?.session_id != null) {
      const live = $sessions.find((s) => s.id === d.session_id);
      if (live) {
        selectSessionExplicitly(live);
        // A lost race: this start's own session spawned and runs unlinked.
        // The hub names it; say so rather than leave it to be found.
        if (d.orphan_session_id != null) push({ kind: 'warning', message: r.error.message });
        return;
      }
    }
    if (r.error.code === 'E_AMBIGUOUS' && e.ticket) {
      // No project to default to: the dialog asks.
      void openTicket(e.ticket);
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

  /** New session mode's keys; true when the key was handled. */
  function onNewModeKeydown(e: KeyboardEvent): boolean {
    const mod = e.metaKey || e.ctrlKey;
    const cur = entryOf(activeKey);
    const handled = () => {
      e.preventDefault();
      return true;
    };
    if (e.key === 'Escape' && (menu || query)) {
      e.stopPropagation();
      if (menu) closeMenu();
      else {
        query = '';
        activeKey = null;
      }
      return handled();
    }
    if (e.key === 'Backspace' && query === '' && !mod) {
      mode = 'switch';
      activeKey = null;
      menu = null;
      return handled();
    }
    if (e.key === 'Enter') {
      if (mod) {
        const w = workEntryOf(activeKey);
        if (cur) pickProject(cur, true);
        else if (w) void startTicketNow(w);
      } else if (activeKey !== null) pickNew(activeKey);
      return handled();
    }
    if (mod && e.key.toLowerCase() === 'p' && cur) {
      togglePin(cur);
      return handled();
    }
    // With a query, ⌘⌫ / ⌘Z are the input's own (delete, undo typing).
    if (mod && e.key === 'Backspace' && query === '' && cur) {
      toggleHide(cur);
      return handled();
    }
    if (mod && e.key.toLowerCase() === 'g' && cur) {
      openMenu(activeKey!, 'groups');
      return handled();
    }
    if (mod && e.key.toLowerCase() === 'z' && query === '' && lastUndo) {
      const u = lastUndo;
      lastUndo = null;
      u();
      return handled();
    }
    if ((e.key === 'F10' && e.shiftKey) || e.key === 'ContextMenu') {
      if (cur) openMenu(activeKey!, 'main');
      return handled();
    }
    if (mod && /^[1-9]$/.test(e.key)) {
      const it = newItems.find((i) => i.kbd === kbdLabel(`⌘${e.key}`));
      const en = entryOf(it?.key ?? null);
      if (en) {
        pickProject(en);
        return handled();
      }
      return false;
    }
    if (e.key === 'ArrowRight' && activeKey?.startsWith('fold:')) {
      unfold(activeKey.slice(5));
      return handled();
    }
    if (e.key === 'ArrowLeft' && cur) {
      const sec = newItems.find((i) => i.key === activeKey)?.groupKey;
      if (sec && isFoldable(sec)) {
        toggleFold(sec);
        activeKey = `fold:${sec}`;
        return handled();
      }
    }
    return false;
  }

  function onInputKeydown(e: KeyboardEvent) {
    if (mode === 'new' && onNewModeKeydown(e)) return;
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
  <Modal label={mode === 'new' ? 'New session' : 'Quick switcher'} onclose={hide} width="560px" testid="quick-switcher">
    <div class="qrow">
      {#if mode === 'new'}<span class="mode-chip" data-testid="mode-chip">New session in</span>{/if}
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
        oninput={() => {
          // A new query highlights its best match.
          if (mode === 'new') activeKey = null;
        }}
        onfocus={() => {
          if (menu) menu = null;
        }}
        onkeydown={onInputKeydown}
        placeholder={mode === 'new'
          ? pendingTicket
            ? `Repository for ${pendingTicket.key ?? 'this ticket'}…`
            : 'project or ticket…'
          : 'Jump to a session, host, task, ticket or asset… (name, key, title, project, host, branch, or paste a ticket URL)'}
        autocomplete="off"
        spellcheck="false"
      />
    </div>
    <div class="listwrap" bind:this={listWrap}>
      <PickerList
        items={mode === 'new' ? newItems : items}
        {activeKey}
        onactivate={(k) => (activeKey = k)}
        onpick={pick}
        maxHeight={mode === 'new' ? 'min(70vh, 34rem)' : 'min(60vh, 24rem)'}
        emptyText={switcherEmptyText(prefix, $trackers.length > 0, modKey)}
        ariaLabel={mode === 'new' ? 'Projects and tickets' : 'Sessions'}
        listId={LIST_ID}
        testid="switcher-list"
        ongroupclick={mode === 'new' ? onGroupClick : undefined}
        oncontext={mode === 'new' ? (k) => openMenu(k, 'main') : undefined}
        rowActions={mode === 'new' ? actions : undefined}
        rowTrail={mode !== 'new' ? accountTrail : undefined}
      />
      {#if menu && entryOf(menu.key)}
        {@const e = entryOf(menu.key)!}
        <div
          class="menu-anchor"
          class:up={menuAt.bottom !== undefined}
          style:top={menuAt.top !== undefined ? `${menuAt.top}px` : undefined}
          style:bottom={menuAt.bottom !== undefined ? `${menuAt.bottom}px` : undefined}
        >
          <ProjectActionsMenu
            title={e.key}
            pinned={e.pinned}
            hidden={!!e.hidden}
            groups={menuGroups}
            currentGroup={e.group.name}
            manualGroup={e.manualGroup}
            startIn={menu.startIn}
            onpin={() => {
              togglePin(e);
              closeMenu();
            }}
            onhide={() => {
              toggleHide(e);
              closeMenu();
            }}
            ongroup={(g) => setGroup(e, g)}
            onclose={closeMenu}
          />
        </div>
      {/if}
    </div>
    {#if mode !== 'new' && searchStep}
      <div class="still-hearing" data-testid="switcher-still-hearing" role="status" data-long={searchMark === 'comet-trails'}>
        {#key searchMark}
          <Loader name={searchMark} size={searchMark === 'comet-trails' ? 20 : 40} stage={false} />
        {/key}
        <span>{searchStep}</span>
      </div>
    {/if}
    <div class="hint">
      {#if mode === 'new'}
        <span>↵ open</span>
        <span>{modKey}↵ start with last settings</span>
        <span>{modKey}P pin</span>
        <!-- With a query ⌘⌫ / ⌘Z are the input's own; the menu still hides. -->
        {#if !query}<span>{modKey}⌫ hide</span>{/if}
        <span>⇧F10 more</span>
        <span>esc close</span>
      {:else}
        <span>↑↓ move</span>
        <span>↵ attach / open</span>
        <span>{modKey}↵ new session named “{query.trim() || '…'}” (on a ticket: start it)</span>
        <span>&gt; commands · # tasks · @ hosts</span>
        <span>esc / {chord} close</span>
      {/if}
    </div>
  </Modal>
{/if}

<!-- Redesign 4.3: a session row carries its account. -->
{#snippet accountTrail(item: PickerItem)}
  {@const uuid = ranked.find((e) => e.key === item.key)?.session?.account_uuid}
  {#if uuid}
    <AccountPill {uuid} testid="switcher-account-pill" onopen={hide} />
  {/if}
{/snippet}

<!-- Hover actions on a project row: a mouse convenience (aria-hidden, not
     focusable); the keys and the actions menu are the accessible path. -->
{#snippet actions(item: PickerItem)}
  {@const e = entryOf(item.key)}
  {#if e}
    <button
      type="button"
      tabindex="-1"
      class="ib"
      class:on={e.pinned}
      title={e.pinned ? 'Unpin' : 'Pin to top'}
      onclick={(ev) => {
        ev.stopPropagation();
        togglePin(e);
      }}
      ><Icon name="pin" size={14} /></button
    >
    <button
      type="button"
      tabindex="-1"
      class="ib"
      title="Move to group…"
      onclick={(ev) => {
        ev.stopPropagation();
        openMenu(item.key, 'groups');
      }}
      ><Icon name="folder" size={14} /></button
    >
    <button
      type="button"
      tabindex="-1"
      class="ib"
      title={e.hidden ? 'Unhide' : 'Hide'}
      onclick={(ev) => {
        ev.stopPropagation();
        toggleHide(e);
      }}
      ><Icon name="hide" size={14} /></button
    >
  {/if}
{/snippet}

<style>
  .query {
    font: inherit;
    font-size: var(--text-sm);
    padding: 0.45rem 0.6rem;
    border: 1px solid var(--border);
    background: var(--bg-pane);
    color: var(--fg);
    border-radius: var(--radius-sm);
    width: 100%;
    box-sizing: border-box;
  }
  .query:focus {
    outline: none;
    border-color: var(--accent);
  }
  .query:focus-visible {
    outline: var(--ring-w) solid var(--ring);
    outline-offset: var(--ring-offset);
  }
  .qrow {
    display: flex;
    align-items: center;
    gap: 0.5rem;
  }
  .mode-chip {
    flex: 0 0 auto;
    font-size: var(--text-2xs);
    font-weight: 600;
    padding: 0.15rem 0.5rem;
    border-radius: var(--radius-sm);
    background: var(--accent-soft);
    color: var(--accent);
    white-space: nowrap;
  }
  .listwrap {
    position: relative;
    display: flex;
    flex-direction: column;
    min-height: 0;
  }
  /* A zero-height line beside the row; the menu hangs below it, or above
     it (.up) when the room is below the list's fold. */
  .menu-anchor {
    position: absolute;
    left: 0;
    right: 0;
    height: 0;
  }
  .menu-anchor.up :global([role='menu']) {
    bottom: 0;
  }
  .ib {
    width: 24px;
    height: 24px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    border: none;
    background: transparent;
    border-radius: var(--radius-sm);
    color: var(--fg-muted);
    cursor: pointer;
  }
  .ib:hover {
    background: var(--bg);
    color: var(--fg);
  }
  .ib.on {
    color: var(--accent);
  }
  .still-hearing {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    padding: var(--space-1) var(--space-3);
    color: var(--fg-muted);
    font-size: var(--text-2xs);
  }
  .hint {
    display: flex;
    flex-wrap: wrap;
    gap: 0.8rem;
    font-size: var(--text-2xs);
    color: var(--fg-muted);
  }
</style>
