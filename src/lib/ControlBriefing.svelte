<!--
  Control's Today briefing view (board MCViews): the compact read of the
  day beside the chat, not the whole Today page. When the brief was
  drafted, one line per session that needs you, the brief itself as
  "Overnight", and links to what lives elsewhere. ⤢ in the strip opens the
  full Today page. It never drafts: Refresh on the Today page does.
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import type { SessionRow } from './sessions';
  import { displayName } from './attention';
  import { attentionFacts } from './attention_facts';
  import { attentionIdleMinutes } from './notify';
  import { accountByUuid, accountLabel } from './accounts';
  import { HUB_HAS_NO_DRAFTS, draftedAt, todayBrief, type Draft } from './drafts';
  import { briefingLine, fleetGroupsStore, rowReason } from './control_fleet';
  import { openElsewhere, type ElsewhereId } from './control_views';
  import { controlTab } from './control';
  import { dayLabel } from './today';
  import Count from './kit/Count.svelte';

  let { onopen, now = () => Date.now() }: { onopen: (s: SessionRow) => void; now?: () => number } = $props();

  let draft = $state<Draft | null>(null);
  /** Why there is no brief to show, when it is not just "none yet". */
  let note = $state<string | null>(null);

  onMount(async () => {
    const r = await todayBrief(false);
    if (r.ok) draft = r.value?.draft ?? null;
    else if (r.error.code === 'E_FORBIDDEN') note = r.error.message;
    else if (!HUB_HAS_NO_DRAFTS.includes(r.error.code)) note = r.error.message;
  });

  const needs = $derived($fleetGroupsStore.needsYou);
  const opts = $derived({ idleSecs: $attentionIdleMinutes * 60, now: Math.floor(now() / 1000), facts: $attentionFacts });
  const name = (u: string) => accountLabel($accountByUuid.get(u));

  const LINKS: readonly { id: ElsewhereId; label: string }[] = [
    { id: 'routines', label: 'Routines in Automation' },
    { id: 'missions', label: 'Missions and tasks in Work' },
    { id: 'hosts', label: 'Hosts and usage in Accounts' },
  ];
</script>

<div class="briefing" data-testid="control-briefing">
  <h3>Today briefing</h3>
  <p class="meta" data-testid="control-briefing-when">
    {dayLabel(now(), true)}{#if draft}{' '}· {draftedAt(draft.at).toLowerCase()} by {draft.model}{/if}
  </p>

  <h4 class="needs">Needs you <Count n={needs.length} /></h4>
  {#if needs.length === 0}
    <p class="muted">Nothing needs you.</p>
  {:else}
    <ul>
      {#each needs as s (s.id)}
        {@const r = rowReason(s, opts, $attentionFacts, name)}
        <li>
          <button type="button" class="line" data-testid="control-briefing-line" onclick={() => onopen(s)}
            >{briefingLine(displayName(s, true), r)}</button
          >
        </li>
      {/each}
    </ul>
  {/if}

  {#if draft}
    <p class="overnight" data-testid="control-briefing-overnight"><strong>Overnight:</strong> {draft.text}</p>
  {:else}
    <p class="muted" data-testid="control-briefing-none">
      {note ?? 'No brief drafted today.'}
      <button type="button" class="link" onclick={() => controlTab.set('today')}>Open Today</button>
    </p>
  {/if}

  <h4>Not views here, open them where they live</h4>
  <ul>
    {#each LINKS as l (l.id)}
      <li>
        <button type="button" class="link" data-testid="control-briefing-link-{l.id}" onclick={() => openElsewhere(l.id)}
          >{l.label} ↗</button
        >
      </li>
    {/each}
  </ul>
</div>

<style>
  .briefing {
    padding: var(--space-3);
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
    font-size: var(--text-sm);
  }
  h3 {
    margin: 0;
    font-size: var(--text-lg);
    font-weight: 600;
  }
  h4 {
    margin: var(--space-3) 0 0;
    font-size: var(--text-xs);
    font-weight: 500;
    color: var(--fg-muted);
    display: flex;
    align-items: center;
    gap: 6px;
  }
  h4.needs {
    color: var(--status-waiting);
  }
  .meta,
  .muted {
    margin: 0;
    font-size: var(--text-xs);
    color: var(--fg-muted);
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
  }
  .line,
  .link {
    border: 0;
    background: transparent;
    font: inherit;
    text-align: left;
    padding: 2px 0;
    cursor: pointer;
  }
  .line {
    color: var(--fg);
  }
  .line:hover {
    text-decoration: underline;
  }
  .link {
    color: var(--accent);
  }
  .line:focus-visible,
  .link:focus-visible {
    outline: 2px solid var(--ring);
    outline-offset: 1px;
  }
  .overnight {
    margin: var(--space-2) 0 0;
    color: var(--fg-2);
    white-space: pre-wrap;
  }
</style>
