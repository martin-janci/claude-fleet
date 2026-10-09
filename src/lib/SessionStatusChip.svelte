<script lang="ts">
  // A session row's status chip, split out of SessionRowItem (redesign step
  // 3.6). One chip at most: stuck, then an inactive bg agent, then
  // claude_status. `brief` is the read-only "Outside fleet" row's variant:
  // no activity in the tooltip, no inactive chip and no spinner.
  import { isInactiveAgent, type SessionRow } from './sessions';
  import { claudeStatusColor, claudeStatusLabel, jevOutcome, stuckStatus, STUCK_COLOR } from './attention';
  import Loader from './Loader.svelte';

  let { sess, brief = false }: { sess: SessionRow; brief?: boolean } = $props();

  const activity = $derived(!brief && sess.current_activity ? ' — ' + sess.current_activity : '');
  /** Review r15 F17: a row Jev's turn reading moved into Needs you says so. */
  const jev = $derived(brief || sess.stuck_kind ? null : jevOutcome(sess));
  const JEV_WORDS = { asked: 'asked you', stuck: 'stuck' } as const;
</script>

{#if sess.stuck_kind}
  <!-- Stuck outranks claude_status: one red chip, no green "working"
       next to it to soften the signal. -->
  <span
    class="claude-chip stuck-chip"
    data-testid="stuck-chip"
    style="background: color-mix(in srgb, {STUCK_COLOR} 13%, transparent); color: {STUCK_COLOR}; border-color: color-mix(in srgb, {STUCK_COLOR} 40%, transparent);"
    title="{stuckStatus(sess.stuck_kind)}{activity}"
  >{stuckStatus(sess.stuck_kind)}</span>
{:else if !brief && isInactiveAgent(sess)}
  <!-- A bg agent whose CLI process is gone: shown as stopped
       (grey), offering Remove from list instead of the usual
       claude_status chip. -->
  <span class="claude-chip inactive-chip" data-testid="inactive-chip">Idle · process ended</span>
{:else if sess.claude_status}
  <span
    class="claude-chip"
    data-testid="claude-chip"
    style="background: color-mix(in srgb, {claudeStatusColor(sess.claude_status)} 13%, transparent); color: {claudeStatusColor(sess.claude_status)}; border-color: color-mix(in srgb, {claudeStatusColor(sess.claude_status)} 27%, transparent);"
    title="Claude: {sess.claude_status}{activity}"
  >{#if !brief && sess.claude_status === 'working'}<Loader name="comet" size={12} class="chip-loader" />{/if}{claudeStatusLabel(sess.claude_status)}</span>
{/if}
{#if jev}
  <span
    class="claude-chip jev-chip"
    data-testid="jev-outcome-chip"
    title="Jev read the end of the last turn as {JEV_WORDS[jev]}. A proposal, not a decision: the next hook replaces it."
    >✦ Jev: {JEV_WORDS[jev]}</span
  >
{/if}

<style>
  .claude-chip {
    font-size: var(--text-2xs);
    padding: 0.05rem 0.3rem;
    border-radius: var(--radius-xs);
    border: 1px solid;
    flex-shrink: 0;
    white-space: nowrap;
  }
  .claude-chip :global(.chip-loader) {
    margin-right: 0.2rem;
    vertical-align: -1px;
  }
  .stuck-chip { font-weight: 600; }
  .jev-chip {
    margin-left: 0.25rem;
    color: var(--fg-2);
    background: var(--chip-bg);
    border-color: color-mix(in srgb, var(--ai-pre) 40%, transparent);
  }
  .inactive-chip {
    background: color-mix(in srgb, var(--fg-muted) 18%, transparent);
    color: var(--fg-muted);
    border-color: color-mix(in srgb, var(--fg-muted) 40%, transparent);
  }
</style>
