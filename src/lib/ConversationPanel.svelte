<script lang="ts">
  // Conversation tab: transcript view backed by `session_conversation`
  // (spec §6). Reuses the Files-overlay mechanism at the call site, so a
  // tmux session's PTY stays mounted underneath while this is shown.
  import { untrack, tick } from 'svelte';
  import type { SessionRow } from './sessions';
  import {
    sessionConversation,
    sameConversation,
    isPinned,
    emptyStateText,
    relativeTime,
    CONVERSATION_POLL_MS,
    type Conversation,
  } from './conversation';

  let { session, visible }: { session: SessionRow; visible: boolean } = $props();

  let conv = $state<Conversation | null>(null);
  let errorCode = $state<string | null>(null);
  let errorMsg = $state<string | null>(null);
  let loading = $state(false);
  let scroller: HTMLDivElement | undefined = $state();
  let nowMs = $state(Date.now());
  let seq = 0;

  async function load() {
    const id = session.id;
    if (!session.claude_session_id) return;
    const mine = ++seq;
    loading = conv === null;
    const pinned = scroller ? isPinned(scroller.scrollTop, scroller.clientHeight, scroller.scrollHeight) : true;
    const r = await sessionConversation(id);
    // Drop a stale response: either a newer fetch has started, or the
    // session prop moved on while this one was in flight.
    if (mine !== seq || session.id !== id) return;
    loading = false;
    if (r.ok) {
      errorCode = null;
      errorMsg = null;
      if (!sameConversation(conv, r.value)) {
        conv = r.value;
        nowMs = Date.now();
        if (pinned) {
          await tick();
          if (scroller) scroller.scrollTop = scroller.scrollHeight;
        }
      }
    } else {
      errorCode = r.error.code;
      errorMsg = r.error.message;
    }
  }

  // Reset + immediate fetch on session change.
  $effect(() => {
    void session.id;
    untrack(() => {
      seq++;
      conv = null;
      errorCode = null;
      errorMsg = null;
      void load();
    });
  });

  // Poll while shown. Depends only on `visible` so a `conv` update from
  // `load()` does not tear down and restart the interval.
  $effect(() => {
    if (!visible) return;
    const t = setInterval(() => {
      if (document.visibilityState === 'visible') void untrack(load);
    }, CONVERSATION_POLL_MS);
    return () => clearInterval(t);
  });

  const empty = $derived(emptyStateText(errorCode, !!session.claude_session_id));
</script>

<div class="conversation-panel" data-testid="conversation-panel">
  {#if empty}
    <p class="muted" data-testid="conv-empty">{empty}</p>
  {:else if loading}
    <p class="muted">Loading…</p>
  {:else}
    <div class="scroller" bind:this={scroller}>
      {#if errorMsg}
        <div class="error-row">
          <span class="err" data-testid="conv-error">{errorMsg}</span>
          <button type="button" data-testid="conv-retry" onclick={() => void load()}>Retry</button>
        </div>
      {/if}
      {#if conv?.truncated}
        <p class="muted truncated">Older turns not shown</p>
      {/if}
      {#if conv}
        {#each conv.turns as turn, i (i)}
          <div class="turn">
            <blockquote data-testid="conv-prompt">
              <span class="prompt-text">{turn.prompt}</span>
              {#if turn.at}
                <time datetime={turn.at}>{relativeTime(turn.at, nowMs)}</time>
              {/if}
            </blockquote>
            {#each turn.items as item, j (j)}
              {#if item.kind === 'text'}
                <p class="text" data-testid="conv-text">{item.text}</p>
              {:else}
                <div class="tool" data-testid="conv-tool" title={item.summary}>{item.summary}</div>
              {/if}
            {/each}
          </div>
        {/each}
      {/if}
    </div>
  {/if}
</div>

<style>
  .conversation-panel {
    height: 100%;
    display: flex;
    flex-direction: column;
    min-height: 0;
  }
  .scroller {
    flex: 1 1 auto;
    min-height: 0;
    overflow: auto;
    padding: 0.6rem;
  }
  .muted { color: var(--fg-muted); font-style: italic; font-size: 0.8rem; margin: 0.6rem; }
  .truncated { text-align: center; }
  .error-row {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    margin-bottom: 0.5rem;
  }
  .err { color: #e64a4a; font-size: 0.8rem; }
  .turn { margin-bottom: 0.9rem; }
  blockquote {
    margin: 0 0 0.35rem 0;
    padding: 0.3rem 0.6rem;
    border-left: 3px solid var(--border);
    color: var(--fg);
    font-size: 0.85rem;
    display: flex;
    justify-content: space-between;
    gap: 0.6rem;
    align-items: baseline;
  }
  .prompt-text { white-space: pre-wrap; }
  time {
    flex: 0 0 auto;
    color: var(--fg-muted);
    font-size: 0.7rem;
    white-space: nowrap;
  }
  .text {
    white-space: pre-wrap;
    margin: 0.2rem 0;
    font-size: 0.85rem;
    color: var(--fg);
  }
  .tool {
    font-family: var(--mono, ui-monospace, SFMono-Regular, Menlo, monospace);
    font-size: 0.75rem;
    color: var(--fg-muted);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    margin: 0.1rem 0;
  }
</style>
