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
    type ToolLine,
    type ToolDetail,
  } from './conversation';
  import CopyButton from './CopyButton.svelte';
  import Loader from './Loader.svelte';
  import Icon from './kit/Icon.svelte';
  import type { OfIconName as IconName } from './kit/icons';
  import { lineDiff, splitPath, parseNumbered, parseTodos, parseFileList, inputField, detailKind } from './tool_view';

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

  /** Diff rows shown before "N more lines". */
  const DIFF_MAX_LINES = 200;
  /** Read lines / listed files shown before "Show all". */
  const READ_MAX_LINES = 40;
  const FILES_MAX = 20;
  /** Result lines shown before "Show all". */
  const RESULT_MAX_LINES = 20;

  let open = $state(false);
  let detail = $state<ToolDetail | null>(null);
  let loadError = $state<string | null>(null);
  // Two refusals a hub-connected desktop can answer with that no amount of
  // retrying can change: the hub has no tool for this read at all
  // (`E_LOCAL_ONLY`), and the hub's wire contract is outside this build's
  // range (`E_HUB_CONTRACT`), which stands until one side is upgraded. No
  // Retry is offered for either.
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
  // Redesign 5.14 (LoadersInFlows, Chat · Agent tool calls): a
  // Comet only on the call that is running; a finished call goes still.
  const running = $derived(!line.done && live);
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
      loadRetryable = !['E_LOCAL_ONLY', 'E_HUB_CONTRACT'].includes(r.error.code);
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

  const kind = $derived(detail ? detailKind(detail.name, detail.edit !== null, detail.command !== null) : 'raw');
  const diffView = $derived(detail?.edit ? lineDiff(detail.edit.old, detail.edit.new) : null);
  const diff = $derived(diffView?.rows ?? []);
  const shownDiff = $derived(fullDiff ? diff : diff.slice(0, DIFF_MAX_LINES));
  const isNewFile = $derived(detail?.name === 'Write' || (detail?.edit != null && detail.edit.old === ''));
  /** The path a detail's header names: the edit's file, Read's file_path,
   *  Grep / Glob's search path. */
  const headPath = $derived(
    detail?.edit?.file_path ?? (detail && kind === 'read' ? inputField(detail.input, 'file_path') : null),
  );
  const numbered = $derived(kind === 'read' && detail?.result != null && !detail.is_error ? parseNumbered(detail.result) : null);
  const todos = $derived(kind === 'todos' && detail ? parseTodos(detail.input) : null);
  const files = $derived(kind === 'files' && detail?.result != null && !detail.is_error ? parseFileList(detail.result) : null);
  const pattern = $derived(kind === 'files' && detail ? inputField(detail.input, 'pattern') : null);
  /** A structured view stands in for the raw result; the success line an
   *  edit or TodoWrite answers with ("The file … has been updated") says
   *  nothing the view does not, so it is shown only when it is an error. */
  const hideResult = $derived(
    detail !== null &&
      !detail.is_error &&
      ((kind === 'edit' && diffView !== null) || numbered !== null || files !== null || (kind === 'todos' && todos !== null)),
  );

  const ICONS: Record<string, IconName> = {
    Edit: 'edit',
    MultiEdit: 'edit',
    Write: 'edit',
    NotebookEdit: 'edit',
    Bash: 'terminal',
    Read: 'file',
    Grep: 'search',
    Glob: 'search',
    WebSearch: 'search',
    TodoWrite: 'list',
  };
  const icon = $derived<IconName>(ICONS[line.name || toolName(line.summary)] ?? 'toolkit');
  const TODO_ICON: Record<string, IconName> = { completed: 'circle-check', in_progress: 'circle-half', pending: 'circle' };
  const resultLines = $derived(detail?.result != null ? detail.result.split('\n') : []);
  const longResult = $derived(resultLines.length > RESULT_MAX_LINES);
  const shownResult = $derived(
    longResult && !fullResult ? resultLines.slice(0, RESULT_MAX_LINES).join('\n') : (detail?.result ?? ''),
  );
</script>

{#snippet row()}
  <span class="chev" aria-hidden="true"></span>
    <span class="state" data-testid="conv-tool-state" data-state={running ? 'running' : line.done && !line.error ? 'done' : 'still'}
      >{#if running}<Loader name="comet" size={12} />{:else if line.done && !line.error}<span class="tick" aria-hidden="true">✓</span>{/if}</span
    >
  <span class="ticon"><Icon name={icon} size={13} /></span>
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
      <div class="detail" data-testid="conv-tool-detail" data-kind={kind}>
        {#if headPath || kind === 'edit' || (kind === 'files' && pattern)}
          {@const p = splitPath(headPath ?? '')}
          <div class="head">
            {#if headPath}
              <span class="path" title={headPath}><span class="dir">{p.dir}</span><span class="base">{p.base}</span></span>
            {:else if pattern}
              <span class="path"><span class="base">{pattern}</span></span>
            {/if}
            {#if kind === 'edit' && isNewFile}<span class="tag">new file</span>{/if}
            <span class="stats">
              {#if diffView}
                {#if diffView.added}<span class="plus">+{diffView.added}</span>{/if}
                {#if diffView.removed}<span class="minus">−{diffView.removed}</span>{/if}
              {:else if numbered}
                <span>{numbered.length} lines</span>
              {:else if files}
                <span>{files.length} {files.length === 1 ? 'file' : 'files'}</span>
              {/if}
            </span>
          </div>
        {/if}
        {#if kind === 'edit' && detail.edit}
          <div class="code diff" role="table" aria-label="Changes">
            {#each shownDiff as d, i (i)}
              {#if d.kind === 'gap'}
                <div class="row gap"><span class="ln"></span><span class="ln"></span><span class="sign"></span><span class="txt">⋯ {d.hidden} unchanged {d.hidden === 1 ? 'line' : 'lines'}</span></div>
              {:else}
                <div class="row {d.kind}"><span class="ln">{d.oldNo ?? ''}</span><span class="ln">{d.newNo ?? ''}</span><span class="sign">{d.kind === 'del' ? '−' : d.kind === 'add' ? '+' : ''}</span><span class="txt">{d.text}</span></div>
              {/if}
            {/each}
          </div>
          {#if diff.length > DIFF_MAX_LINES && !fullDiff}
            <button type="button" class="linkish" onclick={() => (fullDiff = true)}>{diff.length - DIFF_MAX_LINES} more lines</button>
          {/if}
        {:else if kind === 'bash'}
          <pre class="cmd">$ {detail.command}</pre>
        {:else if numbered}
          <div class="code numbered">
            {#each fullResult ? numbered : numbered.slice(0, READ_MAX_LINES) as l (l.no)}
              <div class="row"><span class="ln">{l.no}</span><span class="txt">{l.text}</span></div>
            {/each}
          </div>
          {#if numbered.length > READ_MAX_LINES}
            <button type="button" class="linkish" onclick={() => (fullResult = !fullResult)}>{fullResult ? 'Show less' : `Show all ${numbered.length} lines`}</button>
          {/if}
        {:else if files}
          <ul class="files">
            {#each fullResult ? files : files.slice(0, FILES_MAX) as f, k (k)}
              {@const fp = splitPath(f)}
              <li title={f}><Icon name="file" size={12} /><span class="fname"><span class="dir">{fp.dir}</span><span class="base">{fp.base}</span></span></li>
            {/each}
          </ul>
          {#if files.length > FILES_MAX}
            <button type="button" class="linkish" onclick={() => (fullResult = !fullResult)}>{fullResult ? 'Show less' : `+${files.length - FILES_MAX} more`}</button>
          {/if}
        {:else if todos}
          <ul class="todos" data-testid="conv-tool-todos">
            {#each todos as t, k (k)}
              <li class={t.status}><Icon name={TODO_ICON[t.status]} size={14} /><span>{t.content}</span></li>
            {/each}
          </ul>
        {:else if kind !== 'read' && kind !== 'files'}
          <pre class="input">{detail.input}</pre>
        {/if}
        {#if detail.result !== null && !hideResult}
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
        {:else if detail.result === null}
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
    border-radius: var(--radius-sm);
    font-family: var(--mono);
    font-size: var(--text-2xs);
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
    transition: transform var(--dur-fast) ease;
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
  .state {
    flex: 0 0 12px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 12px;
  }
  .tick {
    color: var(--status-done);
    font-size: var(--text-2xs);
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
    font-size: var(--text-2xs);
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
    border-radius: var(--radius-md);
    background: var(--bg-pane);
    font-size: var(--text-2xs);
  }
  .detail-error {
    display: flex;
    align-items: baseline;
    gap: 0.6rem;
    color: var(--usage-crit);
  }
  .head {
    display: flex;
    align-items: baseline;
    gap: 0.5rem;
    margin: -0.4rem -0.6rem 0.4rem;
    padding: 0.3rem 0.6rem;
    border-bottom: 1px solid var(--border);
    font-family: var(--mono);
    font-size: var(--text-2xs);
  }
  .path {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    direction: ltr;
  }
  .dir {
    color: var(--fg-muted);
  }
  .base {
    color: var(--fg);
    font-weight: 600;
  }
  .tag {
    flex: 0 0 auto;
    padding: 0 6px;
    border-radius: var(--radius-lg);
    font-size: var(--text-2xs);
    color: var(--fg-muted);
    border: 1px solid var(--border);
  }
  .stats {
    flex: 0 0 auto;
    display: flex;
    gap: 0.4rem;
    margin-left: auto;
    color: var(--fg-muted);
  }
  .plus {
    color: var(--diff-add-fg);
  }
  .minus {
    color: var(--diff-del-fg);
  }
  .detail {
    --diff-add-fg: var(--status-done);
    --diff-del-fg: var(--status-failed);
    --diff-add-bg: var(--done-soft);
    --diff-del-bg: var(--failed-soft);
  }
  .code {
    margin: 0 0 0.35rem;
    max-height: 32rem;
    overflow: auto;
    overscroll-behavior: contain;
    padding: 0.25rem 0;
    border-radius: var(--radius-sm);
    background: var(--bg);
    font-family: var(--mono);
    font-size: var(--text-2xs);
    line-height: 1.5;
  }
  .code .row {
    display: flex;
    min-width: max-content;
  }
  .code .ln {
    flex: 0 0 auto;
    width: 3.2ch;
    padding-right: 0.5ch;
    text-align: right;
    color: var(--fg-muted);
    opacity: 0.65;
    user-select: none;
  }
  .numbered .ln {
    width: 4ch;
    margin-right: 0.8ch;
    border-right: 1px solid var(--border);
  }
  .code .sign {
    flex: 0 0 auto;
    width: 2ch;
    text-align: center;
    user-select: none;
    border-left: 1px solid var(--border);
  }
  .code .txt {
    flex: 1 0 auto;
    padding-right: 0.8ch;
    white-space: pre;
    color: var(--fg);
  }
  .diff .add {
    background: var(--diff-add-bg);
  }
  .diff .add .sign {
    color: var(--diff-add-fg);
    box-shadow: inset 2px 0 0 var(--diff-add-fg);
  }
  .diff .del {
    background: var(--diff-del-bg);
  }
  .diff .del .sign {
    color: var(--diff-del-fg);
    box-shadow: inset 2px 0 0 var(--diff-del-fg);
  }
  .diff .gap .txt {
    color: var(--fg-muted);
    font-style: italic;
    padding: 0.1rem 0;
  }
  .diff .gap {
    background: color-mix(in srgb, var(--fg-muted) 8%, transparent);
  }
  .files,
  .todos {
    list-style: none;
    margin: 0 0 0.35rem;
    padding: 0;
  }
  .files li {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    padding: 0.08rem 0;
    font-family: var(--mono);
    font-size: var(--text-2xs);
    color: var(--fg-muted);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .files .fname {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .files .base {
    font-weight: 500;
  }
  .todos li {
    display: flex;
    align-items: center;
    gap: 0.45rem;
    padding: 0.12rem 0;
    font-size: var(--text-2xs);
  }
  .todos li.completed {
    color: var(--fg-muted);
  }
  .todos li.completed :global(.of-ico) {
    color: var(--diff-add-fg);
  }
  .todos li.in_progress {
    font-weight: 600;
  }
  .todos li.in_progress :global(.of-ico) {
    color: var(--accent);
  }
  .todos li.pending :global(.of-ico) {
    color: var(--fg-muted);
  }
  .ticon {
    flex: 0 0 auto;
    align-self: center;
    opacity: 0.75;
  }
  .tool.err .ticon {
    color: var(--usage-crit);
    opacity: 1;
  }
  .dur.muted {
    font-style: italic;
    opacity: 0.6;
  }
  .result-wrap {
    position: relative;
  }
  /* Task 14: CopyButton is a 24px target now, always visible — no more
     opacity: 0 hover-reveal to find it through first. */
  .copy-slot {
    position: absolute;
    top: 0.25rem;
    right: 0.35rem;
  }
  .result[data-error] {
    color: var(--usage-crit);
  }
  .muted {
    margin: 0.2rem 0 0.4rem 1.15rem;
    color: var(--fg-muted);
    font-style: italic;
    font-size: var(--text-2xs);
  }
  .detail .muted {
    margin-left: 0;
  }
  .linkish {
    padding: 0;
    background: none;
    border: none;
    color: var(--accent);
    font-size: var(--text-2xs);
    cursor: pointer;
  }
  @media (prefers-reduced-motion: reduce) {
    .chev {
      transition: none;
    }
  }
</style>
