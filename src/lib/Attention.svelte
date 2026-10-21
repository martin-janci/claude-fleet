<script lang="ts">
  // Stuck-transition watcher (PROD-3). Mounted once in the Sidebar header:
  // diffs consecutive `sessions` snapshots (the backend records the same
  // transitions as `stuck` session_events, so no extra wire event is needed),
  // and on every newly-stuck row announces it through an `aria-live` region,
  // an in-app toast, and — when enabled and permitted — an OS notification.
  //
  // Rows that were already stuck when the app opened are NOT announced: they
  // are visible in the tree, and a burst of notifications on every launch
  // would train the operator to ignore them. Every snapshot that arrives
  // before `sessionsLoaded` (the empty store at mount, the bootstrap fill)
  // only re-seeds the baseline; announcements start with the first change
  // after the fleet is known.
  //
  // The hub's notifications matrix (11.9) adds the other states: a session
  // that comes to need you, fails or finishes, and a routine run that fails,
  // reach the OS notification (when this desktop has them on) and the chime
  // as the Desktop and Sound columns and quiet hours say. Only while the
  // window is in the background: on screen, the app already shows it.
  //
  // Multi-hub Phase 2: an extra fleet's sessions notify the same way, by
  // the same table, named with their fleet. A fleet's first non-empty list
  // (`FleetRows.loaded`) is its baseline, as the primary's bootstrap fill is.
  // A duplicate fleet never notifies.
  import { onMount } from 'svelte';
  import { get } from 'svelte/store';
  import {
    sessions,
    sessionsLoaded,
    showFriendlyNames,
    type SessionRow,
    type StuckKind,
  } from './sessions';
  import { newlyStuck, stuckMessage, stuckSnapshot } from './attention';
  import {
    attentionIdleMinutes,
    newlyNotifiable,
    notificationAllowed,
    notifySnapshot,
    notifyStuckOs,
    notifyStuckToast,
    playChime,
    showOsNotification,
    type NotifyState,
  } from './notify';
  import { fleetSettings } from './fleet_settings';
  import { attentionFacts, attentionFactsFrom } from './attention_facts';
  import { fleetRows, fleets } from './fleets';
  import { failing } from './routines';
  import type { AttentionState } from './attention';
  import { push } from './toasts';
  import { selectSessionExplicitly } from './selection';
  import { sidebarView } from './work_view';

  /** Open a session of this window's fleet from a toast or a notification:
   *  the live row, if it is still there (review w11-4, w11-12). */
  function openRow(id: number) {
    const row = get(sessions).find((r) => r.id === id);
    if (row) selectSessionExplicitly(row);
  }

  /** Latest announcement for the live region (screen readers read changes). */
  let announcement = $state('');
  let prev: Map<number, StuckKind> | null = null;

  function announce(rows: SessionRow[]) {
    const friendly = get(showFriendlyNames);
    const messages = rows.map((r) => stuckMessage(r, friendly));
    announcement = messages.join('. ');
    if (get(notifyStuckToast)) {
      rows.forEach((r, i) =>
        push({
          kind: 'error',
          code: 'STUCK',
          message: messages[i],
          sticky: false,
          timeoutMs: 8000,
          action: { label: 'Open', run: () => openRow(r.id) },
        }),
      );
    }
    // A stuck session is Blocked in the hub's notifications matrix (11.9):
    // the desktop column and quiet hours decide the OS notification.
    if (get(notifyStuckOs) && notificationAllowed('desktop', 'blocked', get(fleetSettings))) {
      for (let i = 0; i < rows.length; i++) {
        const id = rows[i].id;
        showOsNotification('Orbit Fleet: session stuck', messages[i], `stuck-${id}`, () => openRow(id));
      }
    }
    const away = typeof document !== 'undefined' && !document.hasFocus();
    if (away && notificationAllowed('sound', 'blocked', get(fleetSettings))) playChime();
  }

  const TITLES: Record<NotifyState, string> = {
    needs_you: 'needs you',
    failed: 'failed',
    blocked: 'is blocked',
    done: 'is done',
    routine_failed: 'failed',
  };

  /** OS notification and chime for one matrix state, by the hub's columns.
   *  Returns whether it chimed, so a batch makes one sound. */
  function notifyMatrix(
    state: NotifyState,
    title: string,
    body: string,
    tag: string,
    mayChime: boolean,
    onclick?: () => void,
  ): boolean {
    if (typeof document !== 'undefined' && document.hasFocus()) return false;
    const settings = get(fleetSettings);
    if (get(notifyStuckOs) && notificationAllowed('desktop', state, settings)) showOsNotification(title, body, tag, onclick);
    return mayChime && notificationAllowed('sound', state, settings) && playChime();
  }

  let prevStates: Map<number, AttentionState> | null = null;
  /** Failed routine runs already announced or known. */
  const seenRuns = new Set<number>();
  /** Runs that failed before the app opened are in the Inbox, not news. */
  const openedAt = Math.floor(Date.now() / 1000);

  onMount(() => {
    const unsub = sessions.subscribe((rows) => {
      if (prev === null || !get(sessionsLoaded)) {
        prev = stuckSnapshot(rows);
        return;
      }
      const fresh = newlyStuck(prev, rows);
      prev = stuckSnapshot(rows);
      if (fresh.length > 0) announce(fresh);
    });
    const unsubStates = sessions.subscribe((rows) => {
      const opts = { idleSecs: get(attentionIdleMinutes) * 60, now: Math.floor(Date.now() / 1000), facts: get(attentionFacts) };
      const before = prevStates;
      prevStates = notifySnapshot(rows, opts);
      if (before === null || !get(sessionsLoaded)) return;
      const friendly = get(showFriendlyNames);
      let chimed = false;
      for (const { row, state } of newlyNotifiable(before, rows, opts)) {
        const name = friendly && row.friendly_name ? row.friendly_name : row.tmux_name;
        const id = row.id;
        chimed =
          notifyMatrix(state, `${name} ${TITLES[state]}`, row.host_alias, `${state}-${id}`, !chimed, () => openRow(id)) || chimed;
      }
    });
    const unsubRuns = failing.subscribe((list) => {
      let chimed = false;
      for (const f of list) {
        if (seenRuns.has(f.run.id)) continue;
        seenRuns.add(f.run.id);
        if ((f.run.finished_at ?? f.run.started_at) < openedAt) continue;
        const title = `Routine ${f.routine.name} failed`;
        // A failed routine is listed in the Inbox, with its Fix and Retry.
        chimed =
          notifyMatrix('routine_failed', title, f.run.reason ?? '', `routine-${f.run.id}`, !chimed, () =>
            sidebarView.set('inbox'),
          ) || chimed;
      }
    });
    const fleetPrev = new Map<string, Map<number, AttentionState>>();
    const fleetStuckPrev = new Map<string, Map<number, StuckKind>>();
    const unsubFleets = fleetRows.subscribe((all) => {
      const now = Math.floor(Date.now() / 1000);
      const idleSecs = get(attentionIdleMinutes) * 60;
      const friendly = get(showFriendlyNames);
      let chimed = false;
      const known = get(fleets);
      const watched = new Set<string>();
      for (const [fid, rows] of all) {
        // A duplicate is another fleet's hub twice: its rows notify there
        // (review w01-3). A fleet not yet listed has no baseline: rows an
        // event brought first would make every listed row look new (w01-13).
        if (known.find((f) => f.id === fid)?.duplicate_of || !rows.loaded) continue;
        watched.add(fid);
        const list = [...rows.sessions.values()];
        const opts = { idleSecs, now, facts: attentionFactsFrom([...rows.hosts.values()], {}, now) };
        const before = fleetPrev.get(fid);
        const beforeStuck = fleetStuckPrev.get(fid);
        fleetPrev.set(fid, notifySnapshot(list, opts));
        fleetStuckPrev.set(fid, stuckSnapshot(list));
        if (!before || before.size === 0) continue;
        const label = known.find((f) => f.id === fid)?.label ?? fid;
        for (const { row, state } of newlyNotifiable(before, list, opts)) {
          const name = friendly && row.friendly_name ? row.friendly_name : row.tmux_name;
          chimed = notifyMatrix(state, `${name} ${TITLES[state]}`, `${label} · ${row.host_alias}`, `${state}-${fid}-${row.id}`, !chimed) || chimed;
        }
        if (beforeStuck) {
          for (const row of newlyStuck(beforeStuck, list)) {
            const name = friendly && row.friendly_name ? row.friendly_name : row.tmux_name;
            chimed =
              notifyMatrix('blocked', `${name} ${TITLES.blocked}`, `${label} · ${stuckMessage(row, friendly)}`, `stuck-${fid}-${row.id}`, !chimed) ||
              chimed;
          }
        }
      }
      // A fleet that is gone (or not watched now) forgets its baseline.
      for (const fid of [...fleetPrev.keys()]) {
        if (!watched.has(fid)) {
          fleetPrev.delete(fid);
          fleetStuckPrev.delete(fid);
        }
      }
    });
    return () => {
      unsub();
      unsubStates();
      unsubRuns();
      unsubFleets();
    };
  });
</script>

<!-- Assertive: a stuck session is the one thing the operator must not miss.
     Visually hidden; the sidebar chip + toast carry the visual signal. -->
<div class="sr-only" role="alert" aria-live="assertive" data-testid="stuck-announcer">{announcement}</div>

<style>
  .sr-only {
    position: absolute;
    width: 1px;
    height: 1px;
    padding: 0;
    margin: -1px;
    overflow: hidden;
    clip: rect(0, 0, 0, 0);
    white-space: nowrap;
    border: 0;
  }
</style>
