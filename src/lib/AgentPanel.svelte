<script lang="ts">
  // Control's Chat tab (redesign step 9.1): a conversation over the operator
  // session, the removable context chip, and nothing else. Everything that renders turns
  // — and everything that sends — is ConversationPanel's job; what lives
  // here is the frame, the chip, and the four states where the agent cannot
  // simply be talked to.
  //
  // Every blocked state gets a next step (redesign step 9.1, `blockedCopy`'s
  // `next`): wake, restart and move run here; Control API opens Settings,
  // add/open host open the Hosts view; replace kills the agent's session
  // after an inline confirm (a kill is always confirmed) and starts a new one.
  //
  // It is drawn in Control's centre (the Thread room, 2026-10-14), as a
  // chat: ConversationPanel's `look="chat"`. Until step 13.1 it was also
  // Classic's floating sheet, with a grip, maximize and close.
  //
  // The sheet used to own a composer of its own, because the chip's prefix
  // had to be glued onto the prompt and ConversationPanel knew nothing about
  // it. That bought a second sender and cost every live signal the panel
  // had: `pending`, `optimistic` (which is what keeps the transcript on the
  // 5 s cadence instead of the 15 s quiet one) and the refetch-on-send are
  // all set inside ConversationPanel's `send()`. Sending around it meant a
  // prompt that vanished, no working indicator, and a reply that appeared
  // up to fifteen seconds after it existed. The prefix is a prop now, and
  // there is exactly one composer again.
  import ConversationPanel from './ConversationPanel.svelte';
  import {
    operatorState,
    operatorHost,
    operatorFallback,
    operatorRow,
    operatorError,
    blockedCopy,
    ensureAgent,
    restartOperator,
    replaceOperator,
    operatorAccount,
    operatorAccountLine,
    type OperatorBlocked,
  } from './operator';
  import { openSettingsAt, requestHostsView, requestNewSessionOnHost } from './app_views';
  import { hosts } from './hosts';
  import { fleetSettings, hubHasSetting, hubSettingKeys, settingBool, SETTING_KEYS } from './fleet_settings';
  import { agentContext, missionContext, type AgentContextInput } from './agent_context';
  import { missionList } from './mission_room';
  import ControlPlusMenu from './ControlPlusMenu.svelte';
  import { insertIntoComposer } from './conversation';
  import ConfirmCards from './ConfirmCards.svelte';
  import ControlEscalations from './ControlEscalations.svelte';
  import HandoffCards from './HandoffCards.svelte';
  import { controlThinking } from './control_loaders';
  import ControlRouteReceipts from './ControlRouteReceipts.svelte';
  import ControlCommandReceipts from './ControlCommandReceipts.svelte';
  import ControlSuggestions from './ControlSuggestions.svelte';
  import AutonomyChip from './AutonomyChip.svelte';
  import { takeControlCommand } from './control_slash';
  import { linkableTasks, type TaskLink } from './control_task_link';
  import { workTree } from './work_view';
  import { sessions } from './sessions';
  import { activeThread, openInControl } from './control_room';
  import { handoffs } from './handoffs';
  import { selectView } from './control_views';
  import type { ChatRef } from './chat_refs';
  import FixCard from './FixCard.svelte';
  import { operatorFinding, watchFindings } from './autofix';
  import { onMount } from 'svelte';

  // Autofix detector 1: the agent's token belongs to nobody, so it sees no
  // sessions. Shown under the operator status, with its one-click fix.
  onMount(() => watchFindings());

  // Gap plan G3.9: the box's words in Control. `#KEY` names a task and
  // `@name` a host or session in Control's commands; `/` opens them.
  const CONTROL_HINT = '# task · @ host · / command · ↵ send';
  // Thread room: the old header's line ("talk to the fleet · it hands work
  // to sessions and missions") is the box's placeholder now.
  const CONTROL_PLACEHOLDER = 'Ask the fleet, or hand off work: it goes to sessions and missions…';

  /** A `#`/`@` chip in the chat: a session (or the session on a task)
   *  opens in the right panel's Session tab, a host in the Hosts view, a
   *  task with no live session in the panel's Tasks view. */
  function openRef(ref: ChatRef) {
    if (ref.kind === 'session') {
      openInControl(ref.id, ref.label);
    } else if (ref.kind === 'host') {
      requestHostsView(ref.alias);
    } else {
      const live = $sessions.find((s) => s.status !== 'ghost' && (s.work?.key === ref.key || s.work?.display_key === ref.key));
      if (live) openInControl(live.id);
      else selectView('tasks');
    }
  }

  /** G7.8: the tasks `#` offers, read from the Work tree when first typed. */
  async function linkTasks(): Promise<TaskLink[]> {
    const r = await workTree({ filters: {}, limit: 200, per_task: 1 });
    return r.ok ? linkableTasks(r.value.tasks) : [];
  }

  let { contextInput = null }: { contextInput?: AgentContextInput | null } = $props();

  // Control shows the agent whenever it is open, so it makes sure there is
  // one.
  $effect(() => {
    void ensureAgent();
  });

  // Which context's chip the person dismissed, by label rather than a bare
  // boolean: "not this context" outlives the click, but only for as long as
  // it stays the SAME context. The moment `agentContext(...)` describes
  // something else, the labels no longer match and the chip is back — pure
  // derived state, no effect required to bring it back.
  let droppedLabel = $state<string | null>(null);

  // The LIVE row, not the snapshot `ensure_operator` returned. This sheet
  // owns no composer any more, so what hangs off this row is what the sheet
  // hands DOWN to ConversationPanel's: the row it renders, and through it the
  // `claude_status` / `stuck_kind` that drive both the note under the box and
  // — because this sheet passes `blockWhileBusy` — the gate that holds a
  // send while the agent is mid-turn (queued, sent when the turn ends, with
  // a Stop button beside Send: review 2026-10-17, F4). A frozen row would
  // leave both stale.
  // See `operatorRow` in operator.ts.
  const session = $derived($operatorRow);
  // A mission thread open in Control speaks about that mission (w04-13):
  // its name from the missions list, else from Control's receipt for it.
  const missionCtx = $derived.by(() => {
    const t = $activeThread;
    if (t?.kind !== 'mission') return null;
    const name =
      $missionList.missions.find((m) => m.id === t.id)?.name ?? $handoffs.find((h) => h.mission_id === t.id)?.mission_name ?? `#${t.id}`;
    return missionContext({ id: t.id, name });
  });
  const rawCtx = $derived(missionCtx ?? (contextInput ? agentContext(contextInput) : null));
  const ctx = $derived(rawCtx && rawCtx.chipLabel !== droppedLabel ? rawCtx : null);

  const blocked = $derived(
    $operatorState !== 'ready' && $operatorState !== 'waking' && $operatorState !== 'unknown'
      ? blockedCopy(
          $operatorState as OperatorBlocked,
          $operatorHost,
          $operatorFallback,
          // Only a hub that has auto-restore (0.6.3+) restarts the agent by
          // itself; an older one answers without the key, and promising a
          // restart that never comes left Control looking handled while stuck.
          hubHasSetting($hubSettingKeys, SETTING_KEYS.recoveryAutoRestore) &&
            settingBool($fleetSettings, SETTING_KEYS.recoveryAutoRestore),
        )
      : null,
  );
  // Which function a blocked-state button runs, keyed on `blocked.next`
  // rather than the copy string.
  function runNext() {
    if (!blocked) return;
    switch (blocked.next) {
      case 'wake':
      case 'move':
        void ensureAgent();
        return;
      case 'restart':
        void busyWhile(restartOperator);
        return;
      case 'control_api':
        openSettingsAt('control-api');
        return;
      case 'replace':
        confirmingReplace = true;
        return;
      case 'add_host':
        requestHostsView();
        return;
      case 'open_host':
        requestHostsView($operatorHost);
        return;
    }
  }
  // A restart or a replace is an SSH round trip or two (and after a host
  // reboot, a new tmux session); the button says so and takes no second
  // press meanwhile.
  let restarting = $state(false);
  let confirmingReplace = $state(false);
  async function busyWhile(fn: () => Promise<void>) {
    if (restarting) return;
    restarting = true;
    try {
      await fn();
    } finally {
      restarting = false;
    }
  }
  async function replace() {
    confirmingReplace = false;
    await busyWhile(replaceOperator);
  }
  // A confirm left open belongs to the state that asked it.
  $effect(() => {
    if ($operatorState !== 'token_revoked') confirmingReplace = false;
  });

  // Gap plan G3.13 (board Finish, "Control · first run"): an agent nobody
  // has written to yet opens on a card with three first steps. The first
  // two fill the box (nothing is sent until Enter); the third opens New
  // session on the first host.
  let firstRunClosed = $state(false);
  const firstRun = $derived(!!session && !session.last_prompt && !firstRunClosed);
  const firstHost = $derived($hosts.find((h) => !h.hidden)?.alias ?? $operatorHost);
  // Control review F7: Control's own login, said only when it is in trouble.
  const accountLine = $derived(
    $operatorAccount && $operatorAccount.state !== 'ok' && $operatorAccount.state !== 'unknown' ? operatorAccountLine($operatorAccount) : null,
  );
</script>

{#snippet chip()}
  {#if accountLine}
    <p class="account-line" data-testid="control-account" data-state={$operatorAccount?.state}>{accountLine}</p>
  {/if}
  {#if firstRun && session}
    <!-- Thread room (board A2Empty): the empty chat's starters, centred
         under the greeting. -->
    <div class="first-run" data-testid="control-first-run">
      <p class="first-title">Ask Control about your fleet</p>
      <p class="first-sub">It reads every session, task and routine, and can start, send or plan work for you.</p>
      <div class="first-actions">
        <button class="chip command" data-testid="control-first-needs" onclick={() => insertIntoComposer(session.id, 'What needs me?')}
          >What needs me?</button
        >
        <button class="chip command" data-testid="control-first-plan" onclick={() => insertIntoComposer(session.id, '/plan ')}>Plan a task</button>
        <button class="chip command" data-testid="control-first-start" onclick={() => requestNewSessionOnHost(firstHost)}
          >Start a session on {firstHost}</button
        >
        <button class="chip" data-testid="control-first-close" title="Hide these first steps" onclick={() => (firstRunClosed = true)}
          >Not now</button
        >
      </div>
    </div>
  {/if}
  <!-- Step 9.2: the agent's starts and kills wait here as cards, above the
       composer (not in the transcript; parity P30 accepts the placement).
       Mounted only while this row renders, so a request is never parked on
       a card nobody can see (confirms.ts). -->
  <ConfirmCards />
  <!-- Thread room: what in the fleet needs the person, as amber cards. -->
  <ControlEscalations />
  <!-- Steps 9.3 and 9.6: what the agent handed on, as chips and cards
       that follow their target's state. -->
  <HandoffCards />
  <!-- Step 9.9 (Jev K2): where each message just sent here goes. Keyed, so
       another Control session primes afresh instead of routing its history. -->
  {#if session}{#key session.id}<ControlRouteReceipts sessionId={session.id} />{/key}{/if}
  <!-- G3.9: what /task, /done, /assign and /start did (control_slash.ts). -->
  <ControlCommandReceipts />
  <!-- G3.9: suggestions read from the fleet's rows; a press runs them. -->
  {#if session}<ControlSuggestions sessionId={session.id} />{/if}
{/snippet}

{#snippet tools()}
  <!-- Control chat UX (2026-10-10): these join the quick-prompt row
       (Clear, Compact, Status), so one row of chips sits above the box. -->
  <!-- Thread room: one "+" for Add project, New session and the asks that
       are commands now (/plan, /tidy, /health, /graph); each fills the box. -->
  {#if session}<ControlPlusMenu sessionId={session.id} host={firstHost} />{/if}
  <!-- Thread room (board Autonomy): how much Control does on its own, one
       dial over the mission level, auto-continue and the event feed. -->
  <AutonomyChip />
  {#if ctx}
    <button
      type="button"
      class="btn btn--chip"
      data-testid="agent-context-chip"
      onclick={() => (droppedLabel = ctx!.chipLabel)}
      title="Send without this context"
    >
      {ctx!.chipLabel} ✕
    </button>
  {/if}
{/snippet}

<div class="agent-panel" role="region" aria-label="Agent" data-testid="control-agent">
  {#if $operatorFinding}
    <div class="operator-finding" data-testid="agent-operator-finding">
      <FixCard finding={$operatorFinding} />
    </div>
  {/if}
  {#if blocked}
   <!-- One empty state with one primary step (UX audit 2026-10-09, C4). -->
   <div class="blocked-state" data-testid="agent-blocked">
    <p class="blocked">{blocked.title}</p>
    {#if confirmingReplace}
      <p class="confirm" data-testid="agent-replace-confirm">
        Kill the agent's session on {$operatorHost} and start a new one? Its conversation so far stays in that session's
        transcript.
      </p>
      <div class="confirm-row">
        <button class="btn btn--crit" onclick={() => void replace()} data-testid="agent-replace-yes">Kill and start a new agent</button>
        <button class="btn btn--quiet" onclick={() => (confirmingReplace = false)}>Cancel</button>
      </div>
    {:else}
      <button class="btn btn--primary" onclick={runNext} disabled={restarting} data-testid="agent-next-step"
        >{restarting ? (blocked.next === 'replace' ? 'Replacing…' : 'Restarting…') : blocked.action}</button
      >
    {/if}
    {#if $operatorError}
      <p class="error" role="alert" data-testid="agent-panel-error">{$operatorError}</p>
    {/if}
   </div>
  {:else if session}
    <!-- `blockWhileBusy` is the gate the sheet's own composer had before it
         was deleted (`busy = statusNote !== null` at v0.2.35) and lost in
         the refactor. It is passed explicitly, and only here: the
         Conversation tab never had it, and queueing a prompt behind a
         running turn is a workflow there, not a mistake. Here it holds
         the line as a "queued · cancel" chip until the turn ends (F4). -->
    <ConversationPanel
      {session}
      visible={true}
      promptPrefix={ctx?.prefix ?? null}
      blockWhileBusy={true}
      autoContinue={true}
      composerAbove={chip}
      composerTools={tools}
      thinkingAs={controlThinking}
      runCommand={takeControlCommand}
      composerHint={CONTROL_HINT}
      placeholder={CONTROL_PLACEHOLDER}
      {linkTasks}
      daySeparators={true}
      look="chat"
      onOpenRef={openRef}
    />
  {/if}
</div>

<style>
  .operator-finding {
    padding: var(--space-2) var(--space-3) 0;
  }
  /* Fills Control's centre: the chat's own column centres itself. */
  .agent-panel {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
    height: 100%;
    min-height: 0;
    box-sizing: border-box;
    background: var(--bg);
    color: var(--fg);
  }
  .account-line {
    flex-basis: 100%;
    margin: 0;
    font-size: var(--text-xs);
    color: var(--usage-warn);
  }
  .blocked-state {
    margin: auto;
    max-width: 440px;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: var(--space-3);
    padding: var(--space-6);
    text-align: center;
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    background: var(--bg);
  }
  .blocked {
    margin: 0;
    color: var(--fg);
    font-size: var(--text-sm);
    line-height: 1.5;
  }
  .confirm {
    margin: 0;
  }
  .confirm-row {
    display: flex;
    gap: 0.5rem;
  }
  .error {
    margin: 0;
    color: var(--usage-crit);
    font-size: var(--text-2xs);
  }
  .first-run {
    flex-basis: 100%;
    display: flex;
    flex-direction: column;
    align-items: center;
    text-align: center;
    padding: var(--space-4) var(--space-3);
  }
  .first-title {
    margin: 0;
    font-size: var(--text-xl);
    line-height: var(--text-xl-lh);
    font-weight: 600;
  }
  .first-sub {
    margin: 2px 0 var(--space-2);
    color: var(--fg-muted);
    font-size: var(--text-xs);
  }
  .first-actions {
    display: flex;
    gap: var(--space-1);
    flex-wrap: wrap;
    justify-content: center;
  }
  .chip {
    align-self: flex-start;
    border: 1px solid var(--border);
    border-radius: var(--radius-pill);
    background: var(--bg);
    color: var(--fg-muted);
    font-size: var(--text-2xs);
    padding: 0.15rem 0.6rem;
    cursor: pointer;
  }
  .command {
    border-style: dashed;
  }
  .chip:hover {
    color: var(--fg);
    border-color: var(--accent);
  }
</style>
