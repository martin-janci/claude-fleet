<script lang="ts">
  import { spawnReview, DEFAULT_REVIEW_PROMPT, type SessionRow } from './sessions';
  import { selectSession } from './selection';

  let { source, onClose }: { source: SessionRow; onClose: () => void } = $props();

  let prompt = $state(DEFAULT_REVIEW_PROMPT);
  let spawning = $state(false);
  let error = $state<string | null>(null);
  let controller: AbortController | null = null;

  const canStart = $derived(prompt.trim().length > 0 && !spawning);

  async function start() {
    spawning = true;
    error = null;
    controller = new AbortController();
    try {
      const r = await spawnReview(source.id, prompt, controller.signal);
      if (r.ok) {
        selectSession(r.value);
        onClose();
      } else if (r.error.code !== 'E_CANCELLED') {
        error = r.error.message;
      }
    } finally {
      spawning = false;
      controller = null;
    }
  }
</script>

<div class="modal-backdrop" onclick={onClose} role="presentation">
  <div class="dialog" onclick={(e) => e.stopPropagation()} role="dialog" aria-label="Start review">
    <h3>Review session</h3>
    <p class="src">
      <span class="host-badge">[{source.host_alias}]</span>
      <span class="sess-name">{source.tmux_name}</span>
    </p>
    <p class="muted">Spawns a claude review session in this session's worktree, seeded with the prompt below. Reviews the worktree's current state.</p>

    <section class="prompt-section">
      <h4>Review prompt</h4>
      <textarea bind:value={prompt} rows="10" data-testid="review-textarea"></textarea>
    </section>

    {#if error}
      <p class="err" data-testid="review-error">{error}</p>
    {/if}

    <div class="actions">
      <button onclick={onClose}>Cancel</button>
      <button class="primary" disabled={!canStart} onclick={start} data-testid="review-start">
        {spawning ? 'Starting…' : 'Start review'}
      </button>
    </div>
  </div>
</div>

<style>
  .modal-backdrop { position: fixed; inset: 0; background: rgba(0,0,0,0.4); display: flex; align-items: center; justify-content: center; z-index: 20; }
  .dialog { background: var(--bg); border: 1px solid var(--border); border-radius: 6px; padding: 1rem; width: 560px; max-height: 80vh; overflow: auto; color: var(--fg); display: flex; flex-direction: column; gap: 0.7rem; }
  .dialog h3 { margin: 0; font-size: 1rem; }
  .dialog h4 { margin: 0 0 0.3rem 0; font-size: 0.7rem; color: var(--fg-muted); text-transform: uppercase; letter-spacing: 0.05em; }
  .src { margin: 0; display: flex; gap: 0.4rem; align-items: center; }
  .host-badge { font-family: ui-monospace, SFMono-Regular, Menlo, monospace; font-size: 0.7rem; color: var(--fg-muted); border: 1px solid var(--border); padding: 0.05rem 0.3rem; border-radius: 3px; }
  .sess-name { font-family: ui-monospace, SFMono-Regular, Menlo, monospace; font-size: 0.8rem; }
  .muted { color: var(--fg-muted); font-size: 0.8rem; margin: 0; }
  .prompt-section textarea { width: 100%; font-family: ui-monospace, SFMono-Regular, Menlo, monospace; font-size: 0.82rem; padding: 0.5rem; border: 1px solid var(--border); background: var(--bg-pane); color: var(--fg); border-radius: 4px; resize: vertical; min-height: 8rem; }
  .err { color: #e64a4a; font-size: 0.8rem; margin: 0; }
  .actions { display: flex; gap: 0.4rem; justify-content: flex-end; }
  .actions button { font-size: 0.85rem; padding: 0.3rem 0.8rem; border: 1px solid var(--border); background: transparent; color: var(--fg); border-radius: 4px; cursor: pointer; }
  .actions button:disabled { opacity: 0.5; cursor: not-allowed; }
  .actions button.primary { border-color: var(--accent); }
</style>
