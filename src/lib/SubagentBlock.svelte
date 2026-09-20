<script lang="ts">
  // A Task / Agent call in the Conversations tab: a bordered block with the
  // agent type, what it was asked to do and how long it took, and the
  // subagent's final report (Markdown) folded to a few lines.
  import { formatDuration, toolDurationMs, isLongPrompt, PROMPT_CLAMP_LINES, type ConvGroup } from './conversation';
  import Markdown from './MarkdownView.svelte';

  let {
    item,
    nowMs,
    live,
  }: {
    item: Extract<ConvGroup, { kind: 'subagent' }>;
    nowMs: number;
    /** In the running turn of the current conversation (see ToolLine). */
    live: boolean;
  } = $props();

  let expanded = $state(false);

  const elapsed = $derived(toolDurationMs(item.at, item.ended_at, item.done || !live ? null : nowMs));
  const noResult = $derived(!item.done && !live);
  const duration = $derived(
    item.done
      ? elapsed === null
        ? null
        : formatDuration(elapsed)
      : !live
        ? 'no result'
        : elapsed === null
          ? 'running'
          : `running ${formatDuration(elapsed)}`,
  );
  const long = $derived(item.result !== null && isLongPrompt(item.result));
</script>

<div class="subagent" class:err={item.error} data-testid="conv-subagent" data-error={item.error || undefined}>
  <div class="sub-head" data-testid="conv-subagent-head">
    {#if item.error}<span class="sub-err" title="Subagent failed">✕</span>{/if}
    <span class="sub-type">{item.agent_type ?? 'subagent'}</span>
    {#if item.description}<span class="sep" aria-hidden="true">·</span><span class="sub-desc">{item.description}</span>{/if}
    {#if duration}<span class="sub-dur" class:muted={noResult}>{duration}</span>{/if}
  </div>
  {#if item.result}
    <div
      class="sub-result"
      class:clamped={long && !expanded}
      style:--clamp-lines={PROMPT_CLAMP_LINES}
      data-testid="conv-subagent-result"
    >
      <Markdown source={item.result} />
    </div>
    {#if long}
      <button type="button" class="linkish" onclick={() => (expanded = !expanded)}>{expanded ? 'Show less' : 'Show more'}</button>
    {/if}
  {/if}
</div>

<style>
  .subagent {
    margin: 0.4rem 0 0.6rem;
    padding: 0.4rem 0.7rem 0.45rem;
    border: 1px solid var(--border);
    border-left: 3px solid color-mix(in srgb, var(--accent) 55%, var(--border));
    border-radius: 6px;
    background: var(--bg-pane);
  }
  .subagent.err {
    border-left-color: var(--usage-crit);
  }
  .sub-head {
    display: flex;
    align-items: baseline;
    gap: 0.4rem;
    min-width: 0;
    font-size: 0.78rem;
    color: var(--fg-muted);
  }
  .sub-type {
    flex: 0 0 auto;
    font-weight: 600;
    color: var(--fg);
  }
  .sub-desc {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .sub-dur {
    flex: 0 0 auto;
    margin-left: auto;
    font-size: 0.7rem;
  }
  .sub-dur.muted {
    font-style: italic;
    opacity: 0.7;
  }
  .sub-err {
    flex: 0 0 auto;
    color: var(--usage-crit);
    font-weight: 600;
  }
  .sub-result {
    margin-top: 0.3rem;
    font-size: 0.82rem;
    line-height: 1.6;
    color: var(--fg);
  }
  .sub-result.clamped {
    max-height: calc(var(--clamp-lines) * 1.6em);
    overflow: hidden;
    mask-image: linear-gradient(to bottom, #000 70%, transparent);
  }
  .linkish {
    margin-top: 0.2rem;
    padding: 0;
    background: none;
    border: none;
    color: var(--accent);
    font-size: 0.75rem;
    cursor: pointer;
  }
</style>
