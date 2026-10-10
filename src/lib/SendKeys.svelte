<script lang="ts">
  /**
   * Send keys… (Agent board, a popped-out terminal): the keys a window of
   * its own may not pass on, or that are awkward to type there, and a line
   * of text, sent to this terminal as if typed. Pure presentation: the pane
   * writes the bytes.
   */
  import { onMount } from 'svelte';
  import { SEND_KEYS, sendKeysText } from './terminal_popout';

  let {
    target,
    onsend,
    onclose,
  }: {
    /** What the keys go to, as the bar names it: "Claude Code", "Shell 2". */
    target: string;
    onsend: (bytes: string) => void;
    onclose: () => void;
  } = $props();

  let text = $state('');
  let enter = $state(true);
  let input = $state<HTMLInputElement | null>(null);

  onMount(() => input?.focus());

  function sendText(e?: Event) {
    e?.preventDefault();
    if (!text) return;
    onsend(sendKeysText(text, enter));
    text = '';
  }

  function onkeydown(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      e.preventDefault();
      e.stopPropagation();
      onclose();
    }
  }
</script>

<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<div class="send-keys" role="dialog" aria-label="Send keys to {target}" tabindex="-1" {onkeydown} data-testid="send-keys">
  <div class="head">
    <span>Send keys to {target}</span>
    <button type="button" class="x" aria-label="Close" onclick={onclose} data-testid="send-keys-close">×</button>
  </div>
  <div class="keys" role="group" aria-label="Keys">
    {#each SEND_KEYS as k (k.id)}
      <button type="button" class="key" title={k.title} onclick={() => onsend(k.bytes)} data-testid="send-key-{k.id}"
        >{k.label}</button
      >
    {/each}
  </div>
  <form class="line" onsubmit={sendText}>
    <input
      bind:this={input}
      bind:value={text}
      type="text"
      aria-label="Text to send"
      placeholder="Text to type"
      autocomplete="off"
      spellcheck="false"
      data-testid="send-keys-text"
    />
    <label class="enter"><input type="checkbox" bind:checked={enter} data-testid="send-keys-enter" /> then Enter</label>
    <button type="submit" class="key" disabled={!text} data-testid="send-keys-send">Send</button>
  </form>
  <p class="meta">Goes to this terminal as if typed. Nothing is sent until you press a key or Send.</p>
</div>

<style>
  .send-keys {
    position: absolute;
    top: calc(100% + 4px);
    right: 0.5rem;
    z-index: 4;
    width: min(22rem, calc(100vw - 1rem));
    padding: 0.5rem;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    box-shadow: var(--shadow-pop);
    font-size: var(--text-xs);
    color: var(--fg);
  }
  .head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: 0.4rem;
  }
  .keys {
    display: flex;
    flex-wrap: wrap;
    gap: 0.25rem;
    margin-bottom: 0.5rem;
  }
  .key,
  .x {
    font: inherit;
    color: var(--fg);
    background: transparent;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    padding: 0.15rem 0.5rem;
    cursor: pointer;
  }
  .x {
    border-color: transparent;
    color: var(--fg-muted);
  }
  .key:disabled {
    opacity: 0.5;
    cursor: default;
  }
  .line {
    display: flex;
    align-items: center;
    gap: 0.35rem;
  }
  .line input[type='text'] {
    flex: 1 1 auto;
    min-width: 0;
    font: inherit;
    font-family: var(--font-mono, ui-monospace, monospace);
    color: var(--fg);
    background: var(--bg-pane);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    padding: 0.2rem 0.4rem;
  }
  .enter {
    display: inline-flex;
    align-items: center;
    gap: 0.2rem;
    white-space: nowrap;
    color: var(--fg-muted);
  }
  .meta {
    margin: 0.4rem 0 0;
    color: var(--fg-muted);
    font-size: var(--text-2xs);
  }
</style>
