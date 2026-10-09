<script lang="ts">
  // One-time welcome shown on first run. The parent owns visibility and the
  // `onboarding-welcomed` flag; this component just renders + emits intent.
  import Modal from './Modal.svelte';
  import OrbitMark from './kit/OrbitMark.svelte';
  let { onstart, onskip }: { onstart: () => void; onskip: () => void } = $props();
</script>

<!-- Escape and backdrop click both mean "skip for now" (handled by Modal). -->
<Modal label="Welcome to Orbit Fleet" onclose={onskip} width="380px">
  <div class="panel">
    <OrbitMark size={40} label={null} />
    <h2>Welcome to Orbit Fleet</h2>
    <p>
      Run long-lived Claude Code sessions in tmux across your machines. Let's get
      you set up — add a host, pick a project, and start your first session.
      Takes about a minute.
    </p>
    <div class="actions">
      <button class="primary" onclick={onstart}>Let's set up →</button>
      <button class="ghost" onclick={onskip}>Skip for now</button>
    </div>
  </div>
</Modal>

<style>
  .panel {
    display: flex;
    flex-direction: column;
    gap: var(--space-3);
    padding: var(--space-2);
  }
  h2 {
    margin: 0;
    font-size: var(--text-lg);
  }
  p {
    margin: 0;
    color: var(--fg-muted);
    font-size: var(--text-sm);
    line-height: 1.5;
  }
  .actions {
    display: flex;
    gap: 8px;
    margin-top: 4px;
  }
  button {
    padding: 8px 14px;
    border-radius: var(--radius-md);
    font-size: var(--text-sm);
    cursor: pointer;
  }
  .primary {
    background: var(--accent);
    color: var(--accent-fg);
    border: none;
  }
  .ghost {
    background: transparent;
    color: var(--fg-muted);
    border: 1px solid var(--border);
  }
</style>
