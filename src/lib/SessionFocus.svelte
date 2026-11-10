<!--
  The session in focus in a Thread room's right panel (Thread room,
  2026-10-14; boards MCSession, A3Mission, W1Missions): a strip of the
  sessions beside it in the same thread or mission, its name with ↗ to open
  it in full, "status · host · account · PR", its last prompt, and its
  conversation. Control's Session tab and Work → Missions' Session tab both
  draw it; each says which siblings to show and what a click does.

  The strip folds (2026-11-06): its title is a toggle with the count, a long
  strip starts folded, the choice is kept (`siblingsFold` in prefs.ts), and
  an open strip scrolls past a third of the panel, so the conversation
  always keeps its room.
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
  import { foldOpen, siblingsFold } from './prefs';

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
  const open = $derived(foldOpen($siblingsFold, siblings.length));
  /** What a folded strip hides that wants a look: its waiting and failed
   *  sessions, said on the toggle ("· 1 waiting · 2 failed"). */
  const attention = $derived.by(() => {
    const states = siblings.map((s) => sessionState(s));
    const waiting = states.filter((s) => s === 'waiting').length;
    const failed = states.filter((s) => s === 'failed').length;
    return [waiting > 0 ? `${waiting} waiting` : '', failed > 0 ? `${failed} failed` : ''].filter(Boolean);
  });
</script>

<div class="focus" data-testid="{prefix}-focus">
  {#if siblings.length > 0}
    <div class="siblings" data-testid="{prefix}-siblings" data-open={open}>
      <button
        type="button"
        class="sib-title"
        aria-expanded={open}
        data-testid="{prefix}-siblings-toggle"
        title={open ? 'Fold the sessions beside it' : 'Show the sessions beside it'}
        onclick={() => siblingsFold.set(open ? 'closed' : 'open')}
        ><span class="chev" aria-hidden="true">{open ? '▾' : '▸'}</span>{siblingsTitle}<span class="sib-n">{siblings.length}</span
        >{#if !open && attention.length > 0}<span class="sib-attn" data-testid="{prefix}-siblings-attention">· {attention.join(' · ')}</span>{/if}</button
      >
      {#if open}
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
      {/if}
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
    flex: none;
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    padding: 6px var(--space-3);
    border-bottom: 1px solid var(--border);
  }
  .sib-title {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    align-self: flex-start;
    max-width: 100%;
    border: 0;
    background: transparent;
    padding: 2px 0;
    color: var(--fg-muted);
    font: inherit;
    font-size: var(--text-2xs);
    font-weight: 500;
    cursor: pointer;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .sib-title:hover {
    color: var(--fg);
  }
  .sib-title:focus-visible {
    outline: var(--ring-w) solid var(--ring);
    outline-offset: 1px;
  }
  .chev {
    width: 10px;
  }
  .sib-n {
    padding: 0 5px;
    border-radius: var(--radius-sm);
    background: var(--chip-bg);
    color: var(--fg-2);
  }
  .sib-attn {
    color: var(--status-waiting);
  }
  /* An open strip never takes more than a third of the panel. */
  .sib-row {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    max-height: 30vh;
    overflow-y: auto;
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
    flex: none;
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
  .prompt {
    display: -webkit-box;
    -webkit-line-clamp: 2;
    line-clamp: 2;
    -webkit-box-orient: vertical;
    overflow: hidden;
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
  /* The conversation keeps room to read: below this the panel scrolls
     instead of squeezing it. */
  .conv {
    flex: 1 1 auto;
    min-height: 280px;
    display: flex;
    flex-direction: column;
  }
</style>
