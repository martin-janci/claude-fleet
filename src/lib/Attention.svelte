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
  import { notifyStuckOs, notifyStuckToast, showOsNotification } from './notify';
  import { push } from './toasts';

  /** Latest announcement for the live region (screen readers read changes). */
  let announcement = $state('');
  let prev: Map<number, StuckKind> | null = null;

  function announce(rows: SessionRow[]) {
    const friendly = get(showFriendlyNames);
    const messages = rows.map((r) => stuckMessage(r, friendly));
    announcement = messages.join('. ');
    if (get(notifyStuckToast)) {
      for (const m of messages) push({ kind: 'error', code: 'STUCK', message: m, sticky: false, timeoutMs: 8000 });
    }
    if (get(notifyStuckOs)) {
      for (let i = 0; i < rows.length; i++) {
        showOsNotification('claude-fleet: session stuck', messages[i], `stuck-${rows[i].id}`);
      }
    }
  }

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
    return unsub;
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
