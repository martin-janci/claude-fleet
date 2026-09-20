<script lang="ts">
  // One tool call in the Conversations tab: a compact row (verb, short
  // target, duration, failure mark) that expands to the call's input and
  // result. The detail is not in the poll payload; it is read on the first
  // expand through `session_tool_detail` and kept for the row's lifetime.
  import { untrack } from 'svelte';
  import {
    toolDetail,
    toolVerb,
    toolName,
    shortTarget,
    formatDuration,
    toolDurationMs,
    editDiffLines,
    type ToolLine,
    type ToolDetail,
  } from './conversation';
  import CopyButton from './CopyButton.svelte';

  let {
    line,
    sessionId,
    claudeSessionId,
    nowMs,
    live,
  }: {
    line: ToolLine;
    sessionId: number;
    claudeSessionId: string | null;
    nowMs: number;
    /** The call belongs to the running turn of the current conversation. An
     *  unfinished call anywhere else never got its result (the session was
     *  interrupted or died), so it must not count up forever. */
    live: boolean;
  } = $props();

  /** Diff lines shown before "N more lines". */
  const DIFF_MAX_LINES = 200;
  /** Result lines shown before "Show all". */
  const RESULT_MAX_LINES = 20;

  let open = $state(false);
  let detail = $state<ToolDetail | null>(null);
  let loadError = $state<string | null>(null);
  // A hub-connected desktop refuses the detail read (`E_LOCAL_ONLY`, the hub
  // has no tool for it); retrying cannot help, so no Retry is offered.
  let loadRetryable = $state(true);
  let fetching = $state(false);
  let fullDiff = $state(false);
  let fullResult = $state(false);

  // Which call this row shows. A primitive derived, so a poll handing a new
  // (equal) line object neither collapses the row nor drops its cache; a
  // row reused for another call starts over.
  const key = $derived(`${sessionId}|${claudeSessionId ?? ''}|${line.id ?? ''}`);
  $effect(() => {
    void key;
    untrack(() => {
      open = false;
      detail = null;
      loadError = null;
      loadRetryable = true;
      fetching = false;
      fullDiff = false;
      fullResult = false;
    });
  });

  /** `Bash(cargo test)` → `cargo test`, for a call the backend gave no target. */
  function argsOf(summary: string): string | null {
    const paren = summary.indexOf('(');
    if (paren <= 0 || !summary.endsWith(')')) return null;
    const args = summary.slice(paren + 1, -1).trim();
    return args.length > 0 ? args : null;
  }

  const verb = $derived(toolVerb(line.name || toolName(line.summary)));
  const target = $derived(shortTarget(line.target) ?? argsOf(line.summary));
  const elapsed = $derived(toolDurationMs(line.at, line.ended_at, line.done || !live ? null : nowMs));
  const noResult = $derived(!line.done && !live);
  const duration = $derived(
    line.done
      ? elapsed === null
        ? null
        : formatDuration(elapsed)
      : !live
        ? 'no result'
        : elapsed === null
          ? 'running'
          : `running ${formatDuration(elapsed)}`,
  );

  async function fetchDetail() {
    if (line.id === null || fetching) return;
    const mine = key;
    fetching = true;
    loadError = null;
    const r = await toolDetail(sessionId, line.id, claudeSessionId ?? undefined);
    if (mine !== key) return;
    fetching = false;
    if (r.ok) detail = r.value;
    else {
      loadError = r.error.message;
      loadRetryable = r.error.code !== 'E_LOCAL_ONLY';
    }
  }

  function toggle() {
    open = !open;
    // A detail read while the call was still running has no result yet:
    // read it again once the call is done.
    if (open && (detail === null || (detail.result === null && line.done))) void fetchDetail();
  }

  /** `session_tool_detail` caps a result at this many chars plus "…". */
  const RESULT_CAP_CHARS = 8_000;
  const resultCapped = $derived(
    detail?.result != null && detail.result.endsWith('…') && [...detail.result].length === RESULT_CAP_CHARS + 1,
  );

  const diff = $derived(detail?.edit ? editDiffLines(detail.edit.old, detail.edit.new) : []);
  const shownDiff = $derived(fullDiff ? diff : diff.slice(0, DIFF_MAX_LINES));
  const resultLines = $derived(detail?.result != null ? detail.result.split('\n') : []);
  const longResult = $derived(resultLines.length > RESULT_MAX_LINES);
  const shownResult = $derived(
    longResult && !fullResult ? resultLines.slice(0, RESULT_MAX_LINES).join('\n') : (detail?.result ?? ''),
  );
</script>

{#snippet row()}
  <span class="chev" aria-hidden="true"></span>
  <span class="verb">{verb}</span>
  {#if target}<span class="target" title={line.summary}>{target}</span>{/if}
  {#if duration}<span class="dur" class:muted={noResult}>{duration}</span>{/if}
  {#if line.error}<span class="x" title="Failed">✕</span>{/if}
{/snippet}

<div class="tool-line" class:open>
  {#if line.id !== null}
    <button
      type="button"
      class="tool"
      class:err={line.error}
      data-testid="conv-tool"
      data-error={line.error || undefined}
      aria-expanded={open}
      title={line.error ? `Failed: ${line.summary}` : line.summary}
      onclick={toggle}>{@render row()}</button
    >
  {:else}
    <div
      class="tool static"
      class:err={line.error}
      data-testid="conv-tool"
      data-error={line.error || undefined}
      title={line.error ? `Failed: ${line.summary}` : line.summary}
    >
      {@render row()}
    </div>
  {/if}
  {#if open}
    {#if loadError}
      <div class="detail-error" data-testid="conv-tool-detail-error" role="alert">
        <span>{loadError}</span>
        {#if loadRetryable}
          <button type="button" class="linkish" onclick={() => void fetchDetail()}>Retry</button>
        {/if}
      </div>
    {:else if detail}
      <div class="detail" data-testid="conv-tool-detail">
        {#if detail.edit}
          <div class="path">{detail.edit.file_path}</div>
          <pre class="diff">{#each shownDiff as d, i (i)}<span class={d.kind}>{d.kind === 'del' ? '-' : d.kind === 'add' ? '+' : ' '} {d.text}</span>{/each}</pre>
          {#if diff.length > DIFF_MAX_LINES && !fullDiff}
            <button type="button" class="linkish" onclick={() => (fullDiff = true)}>{diff.length - DIFF_MAX_LINES} more lines</button>
          {/if}
        {:else if detail.command !== null}
          <pre class="cmd">$ {detail.command}</pre>
        {:else}
          <pre class="input">{detail.input}</pre>
        {/if}
        {#if detail.result !== null}
          <div class="result-wrap">
            <pre class="result" data-testid="conv-tool-result" data-error={detail.is_error || undefined}>{shownResult}</pre>
            <div class="copy-slot"><CopyButton
                text={detail.result}
                label="Copy result"
                copiedNote={resultCapped ? 'truncated at 8 000 chars' : null}
              /></div>
          </div>
          {#if longResult}
            <button type="button" class="linkish" onclick={() => (fullResult = !fullResult)}>{fullResult ? 'Show less' : 'Show all'}</button>
          {/if}
        {:else}
          <p class="muted">No result yet.</p>
        {/if}
      </div>
    {:else}
      <p class="muted">Loading…</p>
    {/if}
  {/if}
</div>

<style>
  .tool-line {
    margin: 0.1rem 0;
  }
  .tool {
    display: flex;
    align-items: baseline;
    gap: 0.45rem;
    width: 100%;
    min-width: 0;
    padding: 0.05rem 0;
    background: none;
    border: none;
    border-radius: 4px;
    font-family: var(--mono, ui-monospace, SFMono-Regular, Menlo, monospace);
    font-size: 0.74rem;
    line-height: 1.5;
    color: var(--fg-muted);
    text-align: left;
    white-space: nowrap;
  }
  button.tool {
    cursor: pointer;
  }
  button.tool:hover {
    color: var(--fg);
  }
  button.tool:focus-visible {
    outline: 1px solid var(--accent);
    outline-offset: 1px;
  }
  .chev {
    flex: 0 0 auto;
    display: inline-block;
    width: 0.7rem;
    text-align: center;
    opacity: 0.6;
    transition: transform 0.12s ease;
  }
  button.tool .chev::before {
    content: '›';
  }
  .tool.static .chev::before {
    content: '·';
  }
  .open button.tool .chev {
    transform: rotate(90deg);
  }
  .verb {
    flex: 0 0 auto;
    color: var(--fg);
  }
  .target {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .dur {
    flex: 0 0 auto;
    margin-left: auto;
    font-size: 0.7rem;
    opacity: 0.8;
  }
  .x {
    flex: 0 0 auto;
    color: var(--usage-crit);
    font-weight: 600;
  }
  .tool.err .verb,
  .tool.err .target {
    color: var(--usage-crit);
  }
  .detail,
  .detail-error {
    margin: 0.2rem 0 0.5rem 1.15rem;
    padding: 0.4rem 0.6rem;
    border: 1px solid var(--border);
    border-radius: 6px;
    background: var(--bg-pane);
    font-size: 0.74rem;
  }
  .detail-error {
    display: flex;
    align-items: baseline;
    gap: 0.6rem;
    color: var(--usage-crit);
  }
  .path {
    margin-bottom: 0.3rem;
    font-family: var(--mono, ui-monospace, SFMono-Regular, Menlo, monospace);
    color: var(--fg-muted);
    overflow-wrap: anywhere;
  }
  pre {
    margin: 0 0 0.35rem;
    max-height: 32rem;
    overflow: auto;
    padding: 0.35rem 0.5rem;
    border-radius: 4px;
    background: var(--bg);
    font-family: var(--mono, ui-monospace, SFMono-Regular, Menlo, monospace);
    font-size: 0.72rem;
    line-height: 1.45;
    color: var(--fg);
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }
  .diff span {
    display: block;
  }
  .diff .del {
    background: color-mix(in srgb, var(--usage-crit) 14%, transparent);
  }
  .diff .add {
    background: color-mix(in srgb, var(--accent) 16%, transparent);
  }
  .diff .ctx {
    color: var(--fg-muted);
  }
  .dur.muted {
    font-style: italic;
    opacity: 0.6;
  }
  .result-wrap {
    position: relative;
  }
  .copy-slot {
    position: absolute;
    top: 0.25rem;
    right: 0.35rem;
    opacity: 0;
    transition: opacity 0.1s ease;
  }
  .result-wrap:hover .copy-slot,
  .result-wrap:focus-within .copy-slot {
    opacity: 1;
  }
  .result[data-error] {
    color: var(--usage-crit);
  }
  .muted {
    margin: 0.2rem 0 0.4rem 1.15rem;
    color: var(--fg-muted);
    font-style: italic;
    font-size: 0.74rem;
  }
  .detail .muted {
    margin-left: 0;
  }
  .linkish {
    padding: 0;
    background: none;
    border: none;
    color: var(--accent);
    font-size: 0.72rem;
    cursor: pointer;
  }
</style>
