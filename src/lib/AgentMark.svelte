<!-- The agent's own mark in its own colour (design manual, Iconography;
     redesign step 5.1). Claude Code has its mark in `--agent-claude`; an
     agent with no mark of its own yet draws the terminal glyph in
     currentColor. -->
<script lang="ts">
  import type { SessionAgent } from './sessions';

  let { agent, size = 13 }: { agent: SessionAgent; size?: number } = $props();
</script>

{#if agent === 'claude'}
  <svg class="agent-mark claude" width={size} height={size} viewBox="0 0 16 16" aria-hidden="true" data-agent={agent}
    ><path
      d="M8 1.2l1.1 4.3 3.9-2.2-2.2 3.9 4.3 1.1-4.3 1.1 2.2 3.9-3.9-2.2L8 14.8l-1.1-4.3-3.9 2.2 2.2-3.9L.9 7.7l4.3-1.1L3 2.7l3.9 2.2z"
    /></svg
  >
{:else}
  <svg class="agent-mark other" width={size} height={size} viewBox="0 0 16 16" aria-hidden="true" data-agent={agent}
    ><rect x="2" y="3" width="12" height="10" rx="1.5" /><path d="M4.5 6.5 6.5 8l-2 1.5M8 10h3" /></svg
  >
{/if}

<style>
  .agent-mark {
    flex-shrink: 0;
    vertical-align: -2px;
  }
  .claude { fill: var(--agent-claude); }
  .other {
    fill: none;
    stroke: currentColor;
    stroke-width: 1.5;
  }
</style>
