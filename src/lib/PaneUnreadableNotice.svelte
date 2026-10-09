<!--
  J8 (redesign step 5.11): a visible warning in the agent tab when no pane
  rule could read the screen at the end of the session's last turn. The
  rules are how fleet knows a session is waiting, so its state may be
  wrong: Claude Code's display may have changed. The reader writes a
  `pane_unreadable` timeline entry (`service::decide::turn_outcome`); this
  reads the session's newest entries and the live pushes, and shows the
  warning while the session is idle after that turn. Local only.
-->
<script lang="ts">
  import Banner from './kit/Banner.svelte';
  import { onTimelineEvent } from './live_events';
  import { paneUnreadable, sessionHistory, type SessionEvent } from './timeline';
  import type { SessionRow } from './sessions';

  let { session }: { session: Pick<SessionRow, 'id' | 'claude_status'> } = $props();

  /** How many of the newest timeline entries are read. */
  const READ = 30;

  let events = $state<SessionEvent[]>([]);
  const sessionId = $derived(session.id);

  $effect(() => {
    const id = sessionId;
    let alive = true;
    events = [];
    const off = onTimelineEvent(id, (e) => {
      events = [...events.filter((x) => x.id !== e.id), e];
    });
    void sessionHistory(id, READ).then((r) => {
      if (!alive || !r.ok || !Array.isArray(r.value)) return;
      const pushed = events.filter((e) => !r.value.some((x) => x.id === e.id));
      events = [...r.value, ...pushed];
    });
    return () => {
      alive = false;
      off();
    };
  });

  const hit = $derived(session.claude_status === 'idle' ? paneUnreadable(events) : null);
</script>

{#if hit}
  <Banner
    tone="waiting"
    testid="pane-unreadable"
    headline="Fleet could not read this screen"
    meta="No pane rule matched the end of the last turn, so this session's state may be wrong. Claude Code's display may have changed: check the screen yourself."
  />
{/if}
