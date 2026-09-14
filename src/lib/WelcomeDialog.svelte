<script lang="ts">
  // One-time welcome shown on first run. The parent owns visibility and the
  // `onboarding-welcomed` flag; this component just renders + emits intent.
  import Modal from './Modal.svelte';
  let { onstart, onskip }: { onstart: () => void; onskip: () => void } = $props();
</script>

<!-- Escape and backdrop click both mean "skip for now" (handled by Modal). -->
<Modal label="Welcome to claude-fleet" onclose={onskip} width="380px">
  <div class="panel">
    <div class="logo" aria-hidden="true"></div>
    <h2>Welcome to claude-fleet</h2>
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
    gap: 12px;
    padding: 8px;
  }
  .logo {
    width: 40px;
    height: 40px;
    border-radius: 10px;
    background: linear-gradient(135deg, #2563eb, #60a5fa);
  }
  h2 {
    margin: 0;
    font-size: 1.2rem;
  }
  p {
    margin: 0;
    color: var(--fg-muted, #777);
    font-size: 0.9rem;
    line-height: 1.5;
  }
  .actions {
    display: flex;
    gap: 8px;
    margin-top: 4px;
  }
  button {
    padding: 8px 14px;
    border-radius: 7px;
    font-size: 0.9rem;
    cursor: pointer;
  }
  .primary {
    background: var(--accent);
    color: #fff;
    border: none;
  }
  .ghost {
    background: transparent;
    color: var(--fg-muted, #777);
    border: 1px solid var(--border);
  }
</style>
