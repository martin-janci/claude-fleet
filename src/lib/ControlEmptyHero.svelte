<!--
  Control's first run (board Finish, "Control · first run"): while the
  agent's transcript is empty, say what Control is for and offer three
  starters. A starter fills the composer, like the operator commands below
  it; nothing is sent until the person presses Enter.
-->
<script lang="ts">
  import { insertIntoComposer } from './conversation';

  let { sessionId, host = null }: { sessionId: number; host?: string | null } = $props();

  const starters = $derived([
    { label: 'What needs me?', text: 'What needs me right now?' },
    { label: 'Plan a task', text: 'Plan a task: ' },
    host
      ? { label: `Start a session on ${host}`, text: `Start a session on ${host}: ` }
      : { label: 'Start a session', text: 'Start a session: ' },
  ]);
</script>

<section class="hero" aria-label="Ask Control about your fleet" data-testid="control-empty-hero">
  <h3>Ask Control about your fleet</h3>
  <p>It can start sessions, plan tasks and answer "what needs me?". It asks before it acts.</p>
  <div class="starters">
    {#each starters as s (s.label)}
      <button
        type="button"
        class="btn btn--chip"
        data-testid="control-starter"
        title="Put this in the box; nothing is sent until you press Enter"
        onclick={() => insertIntoComposer(sessionId, s.text)}>{s.label}</button
      >
    {/each}
  </div>
</section>

<style>
  .hero {
    max-width: 520px;
    margin: var(--space-6) auto var(--space-2);
    padding: var(--space-3) var(--space-4);
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    background: var(--bg);
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
  }
  h3 {
    margin: 0;
    font-size: var(--text-sm);
    font-weight: 600;
    color: var(--fg);
  }
  p {
    margin: 0;
    font-size: var(--text-xs);
    color: var(--fg-2);
  }
  .starters {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-1);
  }
</style>
