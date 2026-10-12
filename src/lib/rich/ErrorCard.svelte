<script lang="ts">
  // An `error` block (docs/chat-blocks.md): what failed, its code, and the
  // next steps the agent offers. A next step fills the session's composer,
  // as a choice does, and the person presses Enter; one with an `action`
  // (G7.4) is run by the app instead: a login on a host the app knows,
  // that host's page, or the accounts. The block names only which and the
  // host, never anything the app sends.
  import Markdown from '../MarkdownView.svelte';
  import AddAccountDialog from '../AddAccountDialog.svelte';
  import { requestHostsView } from '../app_views';
  import { goTo } from '../destination';
  import { hosts } from '../hosts';
  import type { UiBlock, UiNextStep } from '../rich_blocks';

  let {
    block,
    onfill,
    canFill,
  }: { block: Extract<UiBlock, { kind: 'error' }>; onfill: (text: string) => void; canFill: boolean } = $props();

  /** The login dialog an action opened, on its host. */
  let loginOn = $state<string | null>(null);

  /** Why a next step cannot run here; null when it can. */
  function blockedWhy(o: UiNextStep): string | null {
    if (!o.action) return canFill ? null : 'Next steps fill the composer of a running session.';
    if (o.host !== undefined && !$hosts.some((h) => h.alias === o.host)) return `This app has no host named ${o.host}.`;
    return null;
  }

  function run(o: UiNextStep) {
    if (o.action === 'login' && o.host) loginOn = o.host;
    else if (o.action === 'open_host' && o.host) requestHostsView(o.host);
    else if (o.action === 'open_accounts') goTo('accounts');
    else onfill(o.prompt);
  }

  const anyFill = $derived(block.next.some((o) => !o.action));
</script>

<section class="card" data-testid="rich-error" role="alert" aria-label={block.title}>
  <header>
    <strong>{block.title}</strong>
    <code class="code" data-testid="rich-error-code">{block.code}</code>
  </header>
  {#if block.body}<Markdown source={block.body} />{/if}
  {#if block.detail}
    <details class="detail">
      <summary>Details</summary>
      <pre data-testid="rich-error-detail">{block.detail}</pre>
    </details>
  {/if}
  {#if block.next.length}
    <div class="options">
      {#each block.next as o, k (k)}
        {@const why = blockedWhy(o)}
        <button
          type="button"
          class="option"
          class:action={o.action !== undefined}
          data-testid="rich-error-next"
          data-action={o.action}
          disabled={why !== null}
          title={why ?? o.hint ?? (o.action ? o.label : o.prompt)}
          onclick={() => run(o)}
          ><span class="option-label">{o.label}</span>{#if o.hint}<span class="hint">{o.hint}</span>{/if}</button
        >
      {/each}
    </div>
    {#if anyFill}
      <p class="note">{canFill ? 'A next step fills the composer; press Enter to send it.' : 'Next steps fill the composer of a running session.'}</p>
    {/if}
  {/if}
</section>

{#if loginOn}
  <AddAccountDialog host={loginOn} onclose={() => (loginOn = null)} />
{/if}

<style>
  .card {
    display: flex;
    flex-direction: column;
    gap: 0.45rem;
    margin: 0.4em 0 0.7em;
    padding: 0.65rem 0.8rem;
    border: 1px solid var(--border);
    border-left: 3px solid var(--usage-crit);
    border-radius: var(--radius-md);
    background: color-mix(in srgb, var(--usage-crit) 6%, var(--bg-pane));
  }
  header {
    display: flex;
    justify-content: space-between;
    align-items: baseline;
    gap: 0.5rem;
  }
  .code {
    font-size: var(--text-2xs);
    color: var(--usage-crit);
    white-space: nowrap;
  }
  .detail summary {
    cursor: pointer;
    font-size: var(--text-2xs);
    color: var(--fg-muted);
  }
  .detail pre {
    margin: 0.3rem 0 0;
    padding: 0.4rem 0.5rem;
    max-height: 14rem;
    overflow: auto;
    font-size: var(--text-2xs);
    background: var(--bg);
    border-radius: var(--radius-sm);
    white-space: pre-wrap;
    word-break: break-word;
  }
  .options {
    display: flex;
    flex-wrap: wrap;
    gap: 0.4rem;
  }
  .option {
    display: inline-flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 0.1rem;
    padding: 0.35rem 0.65rem;
    border: 1px solid var(--control-border);
    border-radius: var(--radius-md);
    background: var(--control-bg);
    color: var(--control-fg);
    font: inherit;
    font-size: var(--text-xs);
    cursor: pointer;
    text-align: left;
  }
  .option:hover:not(:disabled) { background: var(--control-bg-hover); border-color: var(--accent); }
  .option:disabled { opacity: 0.6; cursor: default; }
  .option.action { border-color: var(--accent); }
  .option-label { font-weight: 600; }
  .hint { font-size: var(--text-2xs); color: var(--fg-muted); }
  .note {
    margin: 0;
    color: var(--fg-muted);
    font-size: var(--text-2xs);
  }
</style>
