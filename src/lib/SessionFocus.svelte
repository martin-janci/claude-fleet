<!--
  The session in focus in a Thread room's right panel (Thread room,
  2026-10-14; boards MCSession, A3Mission, W1Missions): a strip of the
  sessions beside it in the same thread or mission, its name with ↗ to open
  it in full, "status · host · account · PR", its last prompt, and its
  conversation. Control's Session tab and Work → Missions' Session tab both
  draw it; each says which siblings to show and what a click does.
-->
<script lang="ts">
  import type { Snippet } from 'svelte';
  import ConversationPanel from './ConversationPanel.svelte';
  import StatusDot from './kit/StatusDot.svelte';
  import { sessionState } from './handoffs';
  import { claudeStatusLabel, displayName, promptPreview } from './attention';
  import { accountByUuid, accountLabel, sessionAccountUuid } from './accounts';
  import { hostByAlias } from './hosts';
  import type { SessionRow } from './sessions';

  let {
    session,
    siblings = [],
    siblingsTitle = '',
    onpick,
    onfull,
    back,
    prefix = 'control-session',
  }: {
    session: SessionRow;
    /** The strip: the sessions beside it, itself included; empty hides it. */
    siblings?: readonly SessionRow[];
    siblingsTitle?: string;
    /** A chip of the strip was pressed. */
    onpick: (row: SessionRow) => void;
    /** ↗: open it as a full session. */
    onfull: () => void;
    /** Drawn above the name: a way back to where it was opened from. */
    back?: Snippet;
    /** The test ids' stem: `<prefix>-focus`, `-siblings`, `-sibling`, `-full`, `-account`. */
    prefix?: string;
  } = $props();

  /** The focus header's account (G7.8, MCSession board: "host · account ·
   *  PR"): the one the session bills, by its label. */
  const account = $derived.by(() => {
    const uuid = sessionAccountUuid(session, $hostByAlias.get(session.host_alias)?.account_uuid);
    const a = uuid ? $accountByUuid.get(uuid) : undefined;
    return a ? accountLabel(a) : null;
  });
  const name = $derived(displayName(session, true));
</script>

<div class="focus" data-testid="{prefix}-focus">
  {#if siblings.length > 0}
    <div class="siblings" data-testid="{prefix}-siblings">
      <span class="sib-title">{siblingsTitle}</span>
      <div class="sib-row">
        {#each siblings as sib (sib.id)}
          <button
            type="button"
            class="sib"
            class:on={sib.id === session.id}
            aria-current={sib.id === session.id ? 'true' : undefined}
            data-testid="{prefix}-sibling"
            onclick={() => onpick(sib)}><StatusDot state={sessionState(sib)} size={6} />{displayName(sib, true)}</button
          >
        {/each}
      </div>
    </div>
  {/if}
  <div class="focus-head">
    {@render back?.()}
    <div class="focus-title">
      <p class="row-name">{name}</p>
      <button
        type="button"
        class="icon"
        data-testid="{prefix}-full"
        title="Open {name} as a full session"
        aria-label="Open as a full session"
        onclick={onfull}>↗</button
      >
    </div>
    <p class="row-why">
      {claudeStatusLabel(session.claude_status)} · {session.host_alias}{#if account}{' '}· <span data-testid="{prefix}-account">{account}</span>{/if}{#if session.pr_url}{' '}· {session.pr_url.replace(/^.*\/pull\//, 'PR #')}{/if}
    </p>
    {#if session.last_prompt}<p class="prompt">{promptPreview(session.last_prompt, 160)}</p>{/if}
  </div>
  <div class="conv">
    <ConversationPanel {session} visible={true} advanceOnAnswer={false} />
  </div>
</div>

<style>
  .focus {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 0;
  }
  .siblings {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    padding: 10px var(--space-3);
    border-bottom: 1px solid var(--border);
  }
  .sib-title {
    color: var(--fg-muted);
    font-size: var(--text-2xs);
    font-weight: 500;
  }
  .sib-row {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }
  .sib {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    height: 20px;
    padding: 0 7px;
    border: 0;
    border-radius: var(--radius-sm);
    background: var(--chip-bg);
    color: var(--fg-2);
    font: inherit;
    font-size: var(--text-2xs);
    font-weight: 500;
    cursor: pointer;
    white-space: nowrap;
  }
  .sib.on {
    background: var(--accent-soft);
    color: var(--fg);
    box-shadow: inset 0 0 0 1px var(--accent);
  }
  .sib:focus-visible {
    outline: var(--ring-w) solid var(--ring);
    outline-offset: 1px;
  }
  .focus-head {
    padding: var(--space-2) var(--space-3);
    border-bottom: 1px solid var(--border);
  }
  .focus-head p {
    margin: 0 0 var(--space-1);
  }
  .focus-title {
    display: flex;
    align-items: center;
    gap: var(--space-2);
  }
  .row-name {
    flex: 1 1 auto;
    min-width: 0;
    font-weight: 500;
    font-size: var(--text-sm);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .row-why,
  .prompt {
    color: var(--fg-muted);
    font-size: var(--text-xs);
  }
  .icon {
    border: 0;
    background: transparent;
    color: var(--fg-muted);
    font: inherit;
    font-size: var(--text-sm);
    padding: 4px 6px;
    border-radius: var(--radius-md);
    cursor: pointer;
  }
  .icon:hover {
    background: var(--bg-hover);
    color: var(--fg);
  }
  .icon:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: -2px;
  }
  .conv {
    flex: 1 1 auto;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
</style>
