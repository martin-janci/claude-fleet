<!--
  What approving a control-API call does, in words (`guard::ConfirmAbout`):
  its facts, a body folded when long, and the exact call folded below.
  One rendering for Control's confirm cards and the confirm dialog.
-->
<script lang="ts">
  import { openExternal } from './open_external';
  import type { ConfirmAbout } from './mcp';

  let { about: a, tool, summary }: { about: ConfirmAbout; tool: string; summary: string } = $props();

  /** A body this long folds: its first lines show, the rest on a click. */
  const BODY_FOLD = 280;

  const exact = $derived(summary ? `${tool} ${summary}` : tool);
</script>

<div class="about" data-testid="confirm-about">
  {#if a.facts?.length}
    <dl class="facts">
      {#each a.facts as f, i (i)}
        <dt>{f.label}</dt>
        <dd>
          {#if f.url}<button type="button" class="link" onclick={() => void openExternal(f.url ?? '')}>{f.value} ↗</button>{:else}{f.value}{/if}
        </dd>
      {/each}
    </dl>
  {/if}
  {#if a.body}
    {#if a.body.text.length > BODY_FOLD}
      <!-- Open, the summary drops its preview: the full text is right below. -->
      <details class="body" data-testid="confirm-about-body">
        <summary><span class="label">{a.body.label}</span><span class="preview">: {a.body.text.slice(0, BODY_FOLD)}…</span></summary>
        <p>{a.body.text}</p>
      </details>
    {:else}
      <p class="body" data-testid="confirm-about-body"><span class="label">{a.body.label}:</span> {a.body.text}</p>
    {/if}
  {/if}
  <details class="exact" data-testid="confirm-about-exact">
    <summary>Exact call</summary>
    <span class="mono">{exact}</span>
  </details>
</div>

<style>
  .about {
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
    font-size: var(--text-xs);
  }
  .facts {
    margin: 0;
    display: grid;
    grid-template-columns: max-content 1fr;
    gap: 2px var(--space-2);
  }
  .facts dt {
    color: var(--fg-muted);
  }
  .facts dd {
    margin: 0;
    overflow-wrap: anywhere;
  }
  .link {
    padding: 0;
    border: 0;
    background: none;
    color: var(--accent);
    font: inherit;
    text-align: left;
    cursor: pointer;
  }
  .body {
    margin: 0;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }
  .body .label,
  .exact summary {
    color: var(--fg-muted);
  }
  .body[open] .preview {
    display: none;
  }
  .body p {
    margin: var(--space-1) 0 0;
  }
  .exact .mono {
    color: var(--fg-muted);
    overflow-wrap: anywhere;
  }
</style>
