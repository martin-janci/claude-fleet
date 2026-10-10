<!--
  Redesign step 9.9 (Jev K2): under what the person just sent in Control,
  where it goes: "About Hub federation v2 · Proposed by Jev · Change", or,
  for a message too short or unclear to route, a question with the
  missions and sessions to pick from. Gap plan G3.9: a receipt about a
  session offers "↳ Send to <session>", which hands the message on (and
  then reads "↳ Sent to session <name> ↗"); G7.8: one about a mission
  offers "↳ Send to <mission>", which answers its open question with the
  message ("↳ Sent to mission <name> ↗"), and its name opens it.
  Mounted beside Control's composer;
  it watches the outbox for this session's prompts once
  they are sent, and routes each one once.
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import ProposedBy from './ProposedBy.svelte';
  import { outbox, type OutboxMessage } from './outbox';
  import { focusSession } from './session_focus';
  import { openMission } from './missions';
  import { CONTROL, choose, handOn, optionOf, receipts, routeSent, shownOption, targetOf, type Receipt } from './control_route';

  let { sessionId }: { sessionId: number } = $props();

  /** Prompts already routed (or present before this mounted). */
  const seen = new Set<string>();
  let primed = false;
  let changing = $state<string | null>(null);
  /** Why a hand-on did not happen, per receipt. */
  let handErrors = $state<Record<string, string>>({});

  async function send(r: Receipt) {
    const why = await handOn(r.key);
    const { [r.key]: _drop, ...rest } = handErrors;
    handErrors = why ? { ...rest, [r.key]: why } : rest;
  }

  const DELIVERED: ReadonlySet<OutboxMessage['state']> = new Set(['sent', 'queued', 'received']);

  onMount(() =>
    outbox.store.subscribe((v) => {
      const msgs = v.msgs[sessionId] ?? [];
      for (const m of msgs) {
        if (seen.has(m.id)) continue;
        if (!primed) {
          // Messages from before Control was on screen are not routed.
          seen.add(m.id);
          continue;
        }
        if (m.kind !== 'prompt' || !DELIVERED.has(m.state)) continue;
        seen.add(m.id);
        void routeSent(m.id, m.text);
      }
      primed = true;
    }),
  );

  function label(r: Receipt): string | null {
    const opt = shownOption(r);
    if (opt === CONTROL) return 'For Control itself';
    const t = targetOf(r.route, opt);
    return t ? `About ${t.name}` : null;
  }

  function open(r: Receipt) {
    const t = r.handed ?? targetOf(r.route, shownOption(r));
    if (!t) return;
    if (t.kind === 'mission') {
      openMission(t.id);
      if (r.chosen === null) void choose(r.key, optionOf(t));
      return;
    }
    if (focusSession(t.id, t.name) && r.chosen === null) void choose(r.key, optionOf(t));
  }

  function pick(r: Receipt, option: string) {
    changing = null;
    void choose(r.key, option);
  }
</script>

{#if $receipts.length > 0}
  <div class="receipts" data-testid="control-route-receipts">
    {#each $receipts as r (r.key)}
      {@const text = label(r)}
      <div class="receipt" data-testid="control-route-receipt" data-outcome={r.route.outcome}>
        {#if r.handed}
          <button type="button" class="target link" data-testid="control-route-handed" onclick={() => open(r)}
            >↳ Sent to {r.handed.kind} {r.handed.name} ↗</button
          >
        {:else if text && changing !== r.key}
          {@const t = targetOf(r.route, shownOption(r))}
          {#if t}
            <button type="button" class="target link" data-testid="control-route-target" onclick={() => open(r)}
              >{text}</button
            >
          {:else}
            <span class="target" data-testid="control-route-target">{text}</span>
          {/if}
          {#if r.chosen === null}
            <ProposedBy
              proposal={r.route.proposal}
              field="control_route"
              testid="control-route-proposed"
              onchange={() => (changing = r.key)}
            />
          {:else}
            <span aria-hidden="true">·</span>
            <button type="button" class="link" data-testid="control-route-change" onclick={() => (changing = r.key)}
              >Change</button
            >
          {/if}
          {#if t}
            <button type="button" class="hand" data-testid="control-route-send" onclick={() => void send(r)}
              >↳ Send to {t.name}</button
            >
          {/if}
          {#if handErrors[r.key]}
            <span class="hand-error" role="alert" data-testid="control-route-send-error">{handErrors[r.key]}</span>
          {/if}
        {:else}
          <span class="ask" data-testid="control-route-ask">Which mission or session is this about?</span>
          <div class="choices">
            {#each r.route.targets as t (optionOf(t))}
              <button type="button" class="choice" data-testid="control-route-choice" onclick={() => pick(r, optionOf(t))}
                >{t.name}</button
              >
            {/each}
            <button type="button" class="choice" data-testid="control-route-choice-control" onclick={() => pick(r, CONTROL)}
              >Control itself</button
            >
          </div>
        {/if}
      </div>
    {/each}
  </div>
{/if}

<style>
  .receipts {
    flex-basis: 100%;
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
  }
  .receipt {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px;
    font-size: var(--text-xs);
    color: var(--fg-muted);
  }
  .target {
    color: var(--fg);
  }
  .link {
    background: none;
    border: none;
    padding: 0;
    color: var(--accent);
    cursor: pointer;
    font: inherit;
  }
  .hand {
    font: inherit;
    padding: 1px 8px;
    border-radius: var(--radius-pill);
    border: 1px solid var(--border);
    background: var(--bg-raise);
    color: var(--fg);
    cursor: pointer;
  }
  .hand:hover {
    border-color: var(--accent);
  }
  .hand-error {
    color: var(--usage-crit);
  }
  .choices {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-1);
  }
  .choice {
    font: inherit;
    padding: 2px 8px;
    border-radius: var(--radius-sm);
    border: 1px solid var(--border);
    background: var(--bg-raise);
    color: var(--fg);
    cursor: pointer;
  }
</style>
