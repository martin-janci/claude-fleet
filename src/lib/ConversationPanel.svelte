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
    groupItems,
    toolGroupLabel,
    isLongPrompt,
    CONVERSATION_POLL_MS,
    type Conversation,
  } from './conversation';
  import Markdown from './MarkdownView.svelte';

  let { session, visible }: { session: SessionRow; visible: boolean } = $props();

  let conv = $state<Conversation | null>(null);
  let errorCode = $state<string | null>(null);
  let errorMsg = $state<string | null>(null);
  let loading = $state(false);
  let scroller: HTMLDivElement | undefined = $state();
  let nowMs = $state(Date.now());
  // Turn indexes whose long prompt the user expanded.
  let expanded = $state<Set<number>>(new Set());
  // False once the user scrolls away from the bottom; drives "↓ Latest".
  let atBottom = $state(true);
  let seq = 0;
  // Fetches still pending, per session id. A poll tick never starts a read
  // while one is in flight for the same session (a remote read can take up
  // to its 20 s wall clock); a manual Retry or a session switch still does.
  const inFlight = new Map<number, number>();
  // Keyed on the id, not the row object: a store patch hands a new object
  // for the same session, which must neither reset nor refetch.
  const sessionId = $derived(session.id);

  async function load(opts: { poll?: boolean } = {}) {
    const id = session.id;
    if (!session.claude_session_id) return;
    if (opts.poll && (inFlight.get(id) ?? 0) > 0) return;
    const mine = ++seq;
    loading = conv === null;
    const pinned = scroller ? isPinned(scroller.scrollTop, scroller.clientHeight, scroller.scrollHeight) : true;
    inFlight.set(id, (inFlight.get(id) ?? 0) + 1);
    let r: Awaited<ReturnType<typeof sessionConversation>>;
    try {
      r = await sessionConversation(id);
    } finally {
      const left = (inFlight.get(id) ?? 1) - 1;
      if (left > 0) inFlight.set(id, left);
      else inFlight.delete(id);
    }
    // Drop a stale response: either a newer fetch has started, or the
    // session prop moved on while this one was in flight.
    if (mine !== seq || session.id !== id) return;
    loading = false;
    if (r.ok) {
      errorCode = null;
      errorMsg = null;
      if (!sameConversation(conv, r.value)) {
        conv = r.value;
        if (pinned) {
          await tick();
          scrollToBottom();
        }
      }
    } else {
      errorCode = r.error.code;
      errorMsg = r.error.message;
    }
  }

  // Reset + immediate fetch on session change.
  $effect(() => {
    void sessionId;
    untrack(() => {
      seq++;
      conv = null;
      errorCode = null;
      errorMsg = null;
      expanded = new Set();
      atBottom = true;
      void load();
    });
  });

  // Poll while shown, and refetch at once when it becomes shown again (not
  // up to a poll interval later). Depends only on `visible` so a `conv`
  // update from `load()` does not tear down and restart the interval.
  let wasVisible = untrack(() => visible);
  $effect(() => {
    if (!visible) {
      wasVisible = false;
      return;
    }
    if (!wasVisible) {
      wasVisible = true;
      void untrack(() => load({ poll: true }));
    }
    const t = setInterval(() => {
      if (document.visibilityState === 'visible') void untrack(() => load({ poll: true }));
    }, CONVERSATION_POLL_MS);
    return () => clearInterval(t);
  });

  // Independent clock for the relative-time labels, decoupled from content
  // changes: an unchanged conversation (the common case between polls) must
  // still see "just now" age into "1m ago" etc. (same pattern as
  // SessionDetails.svelte's `nowSec` ticker).
  $effect(() => {
    const t = setInterval(() => (nowMs = Date.now()), 30_000);
    return () => clearInterval(t);
  });

  const empty = $derived(emptyStateText(errorCode, !!session.claude_session_id));

  function scrollToBottom() {
    if (!scroller) return;
    scroller.scrollTop = scroller.scrollHeight;
    atBottom = true;
  }

  function onScroll() {
    if (scroller) atBottom = isPinned(scroller.scrollTop, scroller.clientHeight, scroller.scrollHeight);
  }

  function togglePrompt(i: number) {
    const next = new Set(expanded);
    if (next.has(i)) next.delete(i);
    else next.add(i);
    expanded = next;
  }
</script>

<div class="conversation-panel" data-testid="conversation-panel">
  {#if empty}
    <p class="muted" data-testid="conv-empty">{empty}</p>
  {:else if loading}
    <p class="muted">Loading…</p>
  {:else}
    <div class="scroller" data-testid="conv-scroller" bind:this={scroller} onscroll={onScroll}>
      <div class="thread">
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
            <section class="turn">
              {#if turn.prompt !== null}
                {@const long = isLongPrompt(turn.prompt)}
                <div class="prompt" data-testid="conv-prompt">
                  <div class="prompt-head">
                    <span class="who">You</span>
                    {#if turn.at}
                      <time datetime={turn.at} title={new Date(turn.at).toLocaleString()}
                        >{relativeTime(turn.at, nowMs)}</time
                      >
                    {/if}
                  </div>
                  <div class="prompt-text" class:clamped={long && !expanded.has(i)}>{turn.prompt}</div>
                  {#if long}
                    <button type="button" class="linkish" data-testid="conv-prompt-toggle" onclick={() => togglePrompt(i)}
                      >{expanded.has(i) ? 'Show less' : 'Show more'}</button
                    >
                  {/if}
                </div>
              {/if}
              <div class="reply">
                {#each groupItems(turn.items) as g, j (j)}
                  {#if g.kind === 'text'}
                    <div class="text" data-testid="conv-text"><Markdown source={g.text} /></div>
                  {:else if g.tools.length === 1}
                    <div class="tool" data-testid="conv-tool" title={g.tools[0]}>{g.tools[0]}</div>
                  {:else}
                    <details class="tools" data-testid="conv-tools">
                      <summary>{toolGroupLabel(g.tools)}</summary>
                      {#each g.tools as summary, k (k)}
                        <div class="tool" data-testid="conv-tool" title={summary}>{summary}</div>
                      {/each}
                    </details>
                  {/if}
                {/each}
              </div>
            </section>
          {/each}
        {/if}
      </div>
    </div>
    {#if !atBottom}
      <button type="button" class="latest" data-testid="conv-latest" onclick={scrollToBottom}>↓ Latest</button>
    {/if}
  {/if}
</div>

<style>
  .conversation-panel {
    position: relative;
    height: 100%;
    display: flex;
    flex-direction: column;
    min-height: 0;
    background: var(--bg);
  }
  .scroller {
    flex: 1 1 auto;
    min-height: 0;
    overflow: auto;
  }
  .thread {
    max-width: 80ch;
    margin: 0 auto;
    padding: 1rem 1.1rem 2.5rem;
  }
  .muted { color: var(--fg-muted); font-style: italic; font-size: 0.8rem; margin: 0.6rem; }
  .truncated { text-align: center; margin: 0 0 1rem; }
  .error-row {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    margin-bottom: 0.75rem;
  }
  .err { color: #e64a4a; font-size: 0.8rem; }
  .turn {
    padding: 0.2rem 0 1.1rem;
  }
  .turn + .turn {
    border-top: 1px solid var(--border);
    padding-top: 1.1rem;
  }
  .prompt {
    margin: 0 0 0.8rem;
    padding: 0.5rem 0.75rem 0.55rem;
    border: 1px solid var(--border);
    border-left: 3px solid var(--accent);
    border-radius: 6px;
    background: color-mix(in srgb, var(--accent) 6%, var(--bg-pane));
  }
  .prompt-head {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 0.6rem;
    margin-bottom: 0.2rem;
  }
  .who {
    font-size: 0.7rem;
    font-weight: 600;
    letter-spacing: 0.02em;
    text-transform: uppercase;
    color: var(--accent);
  }
  time {
    flex: 0 0 auto;
    color: var(--fg-muted);
    font-size: 0.7rem;
    white-space: nowrap;
  }
  .prompt-text {
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    font-size: 0.85rem;
    line-height: 1.5;
    color: var(--fg);
  }
  .prompt-text.clamped {
    display: -webkit-box;
    -webkit-box-orient: vertical;
    -webkit-line-clamp: 6;
    line-clamp: 6;
    overflow: hidden;
  }
  .linkish {
    margin-top: 0.25rem;
    padding: 0;
    background: none;
    border: none;
    color: var(--accent);
    font-size: 0.75rem;
    cursor: pointer;
  }
  .reply {
    font-size: 0.875rem;
    line-height: 1.6;
    color: var(--fg);
    overflow-wrap: break-word;
  }
  .text {
    margin: 0.35rem 0 0.6rem;
  }
  .tool {
    font-family: var(--mono, ui-monospace, SFMono-Regular, Menlo, monospace);
    font-size: 0.74rem;
    line-height: 1.5;
    color: var(--fg-muted);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    margin: 0.1rem 0;
    padding-left: 1.1rem;
    position: relative;
  }
  .tool::before {
    content: '⚙';
    position: absolute;
    left: 0;
    opacity: 0.6;
  }
  .tools {
    margin: 0.3rem 0 0.5rem;
  }
  .tools summary {
    cursor: pointer;
    font-size: 0.76rem;
    color: var(--fg-muted);
    list-style: none;
    user-select: none;
  }
  .tools summary::-webkit-details-marker {
    display: none;
  }
  .tools summary::before {
    content: '▸';
    display: inline-block;
    width: 1.1rem;
    transition: transform 0.12s ease;
  }
  .tools[open] summary::before {
    transform: rotate(90deg);
  }
  .tools summary:hover {
    color: var(--fg);
  }
  .tools .tool {
    margin-left: 1.1rem;
  }
  .latest {
    position: absolute;
    right: 1rem;
    bottom: 1rem;
    padding: 0.3rem 0.7rem;
    border: 1px solid var(--border);
    border-radius: 999px;
    background: var(--bg-pane);
    color: var(--fg);
    font-size: 0.75rem;
    cursor: pointer;
    box-shadow: 0 2px 8px color-mix(in srgb, var(--fg) 15%, transparent);
  }
  .latest:hover {
    border-color: var(--accent);
  }
</style>
