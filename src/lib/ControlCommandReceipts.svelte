<!--
  Gap plan G3.9: what each slash command Control ran itself did
  (`control_slash.ts`), under the line it was typed as: "Created TASK-232 ·
  Rotate the NAS sudo password · Open", "TASK-219 is done · Undo", or why
  not. The latest few stay; ✕ drops one.
-->
<script lang="ts">
  import { commandReceipts, dismissCommandReceipt, openCommandResult, undoCommand } from './control_slash';
  import { openInControl } from './control_room';
  import TaskGraphCard from './TaskGraphCard.svelte';

  const OPEN_LABEL = { task: 'Open in Work ↗', session: 'Open session ↗', hosts: 'Open hosts ↗', mission: 'Open mission ↗', runs: 'Open runs ↗' } as const;
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
            onclick={() => openCommandResult(r, (id, label) => void openInControl(id, label))}
            >{OPEN_LABEL[r.open.kind]}</button
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
        {#if r.rows?.length}
          <dl class="rows" data-testid="control-command-rows">
            {#each r.rows as row (row.label)}
              <dt data-level={row.level}>{row.label}</dt>
              <dd data-level={row.level}>{row.text}</dd>
            {/each}
          </dl>
        {/if}
        {#if r.graph}
          <div class="graph" data-testid="control-command-graph">
            <TaskGraphCard graph={r.graph} kind="mission" onopen={() => openCommandResult(r, (id, label) => void openInControl(id, label))} />
          </div>
        {/if}
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
  .rows {
    flex-basis: 100%;
    display: grid;
    grid-template-columns: max-content 1fr;
    gap: 2px var(--space-3);
    margin: 0;
  }
  .rows dt {
    font-weight: 600;
    color: var(--fg);
  }
  .rows dd {
    margin: 0;
  }
  .rows [data-level='warn'] {
    color: var(--usage-warn);
  }
  .rows [data-level='crit'] {
    color: var(--usage-crit);
  }
  .graph {
    flex-basis: 100%;
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
