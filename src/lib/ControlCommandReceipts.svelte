<!--
  Gap plan G3.9: what each slash command Control ran itself did
  (`control_slash.ts`), under the line it was typed as: "Created TASK-232 ·
  Rotate the NAS sudo password · Open", "TASK-219 is done · Undo", or why
  not. The latest few stay; ✕ drops one.
-->
<script lang="ts">
  import { commandReceipts, dismissCommandReceipt, openCommandResult, undoCommand } from './control_slash';
  import { focusSession } from './session_focus';
</script>

{#if $commandReceipts.length > 0}
  <ul class="cmd-receipts" data-testid="control-command-receipts" aria-live="polite">
    {#each $commandReceipts as r (r.id)}
      <li class="cmd-receipt" data-testid="control-command-receipt" data-state={r.state}>
        <span class="typed">{r.typed}</span>
        <span class="glyph" aria-hidden="true">{r.state === 'done' ? '✓' : r.state === 'failed' ? '✕' : '…'}</span>
        <span class="line" data-testid="control-command-line">{r.line}</span>
        {#if r.open}
          <button
            type="button"
            class="link"
            data-testid="control-command-open"
            onclick={() => openCommandResult(r, (id, label) => void focusSession(id, label))}
            >{r.open.kind === 'task' ? 'Open in Work ↗' : 'Open session ↗'}</button
          >
        {/if}
        {#if r.undo}
          <button type="button" class="link" data-testid="control-command-undo" onclick={() => void undoCommand(r.id)}>Undo</button>
        {/if}
        <button
          type="button"
          class="link dismiss"
          aria-label="Dismiss"
          data-testid="control-command-dismiss"
          onclick={() => dismissCommandReceipt(r.id)}>✕</button
        >
      </li>
    {/each}
  </ul>
{/if}

<style>
  .cmd-receipts {
    flex-basis: 100%;
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
  }
  .cmd-receipt {
    display: flex;
    flex-wrap: wrap;
    align-items: baseline;
    gap: 6px;
    font-size: var(--text-xs);
    color: var(--fg-muted);
  }
  .typed {
    font-family: var(--mono, ui-monospace, SFMono-Regular, Menlo, monospace);
    color: var(--accent);
  }
  .cmd-receipt[data-state='done'] .glyph {
    color: var(--usage-ok);
  }
  .cmd-receipt[data-state='failed'] .glyph {
    color: var(--usage-crit);
  }
  .line {
    color: var(--fg);
  }
  .link {
    background: none;
    border: none;
    padding: 0;
    color: var(--accent);
    cursor: pointer;
    font: inherit;
  }
  .dismiss {
    color: var(--fg-muted);
  }
</style>
