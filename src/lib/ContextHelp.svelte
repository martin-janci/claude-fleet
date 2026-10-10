<script lang="ts" module>
  /** Questions asked this run, oldest first, shared by every prompt line:
   *  the question box's own ↑ / ↓ recall. */
  const asked: string[] = [];
  const ASKED_MAX = 50;

  /** Remember `q` as the newest question (a repeat moves to the end). */
  export function rememberQuestion(q: string): void {
    const t = q.trim();
    if (!t) return;
    const at = asked.indexOf(t);
    if (at !== -1) asked.splice(at, 1);
    asked.push(t);
    if (asked.length > ASKED_MAX) asked.shift();
  }

  /** The questions asked this run, oldest first (for tests). */
  export function askedQuestions(): readonly string[] {
    return asked;
  }
</script>

<script lang="ts">
  // Context help (`context_help.ts`): ask the model about the prompt line,
  // with its history as context. The panel only asks and shows: the
  // proposal goes back to the host component through `oninsert`, which puts
  // it on the line — never sent, never run.
  import { onMount } from 'svelte';
  import DraftedLabel from './DraftedLabel.svelte';
  import CopyButton from './CopyButton.svelte';
  import { draftedBy } from './ai_proposal';
  import { askBlocked, type HelpAnswer } from './context_help';
  import type { Result } from './result';

  let {
    model,
    line,
    what,
    ask,
    oninsert,
    onclose,
    insertLabel = 'Insert',
    insertBlocked = null,
    testid = 'context-help',
  }: {
    /** Who answers, for the heading (`haiku`). */
    model: string;
    /** What is on the prompt line now. */
    line: string;
    /** What the history is, in words ("this terminal's history"). */
    what: string;
    ask: (question: string) => Promise<Result<HelpAnswer>>;
    /** Put the proposal on the prompt line. */
    oninsert: (command: string) => void;
    onclose: () => void;
    insertLabel?: string;
    /** Why the proposal cannot go on the line now (a program is running
     *  in the shell); Copy still works. */
    insertBlocked?: string | null;
    testid?: string;
  } = $props();

  let question = $state('');
  let busy = $state(false);
  let answer = $state<HelpAnswer | null>(null);
  let error = $state<string | null>(null);
  let input: HTMLInputElement | undefined = $state();
  /** Where ↑ / ↓ stand in `asked`; `null` is the question being typed. */
  let recallAt = $state<number | null>(null);

  const blocked = $derived(askBlocked(question, line));
  const meta = $derived(
    answer
      ? draftedBy(answer.model, answer.host_alias || null, `${answer.history_items} lines of history`)
      : '',
  );

  onMount(() => input?.focus());

  async function run() {
    if (busy || blocked) return;
    busy = true;
    error = null;
    rememberQuestion(question);
    recallAt = null;
    const r = await ask(question);
    busy = false;
    if (r.ok) answer = r.value;
    else error = r.error.message;
  }

  function recall(dir: -1 | 1): boolean {
    if (asked.length === 0) return false;
    if (recallAt === null) {
      if (dir === 1) return false;
      recallAt = asked.length - 1;
    } else {
      const next = recallAt + dir;
      if (next < 0) return true;
      if (next >= asked.length) {
        recallAt = null;
        question = '';
        return true;
      }
      recallAt = next;
    }
    question = asked[recallAt];
    return true;
  }

  function onKey(e: KeyboardEvent) {
    // Nothing typed here may reach a terminal grid or composer under it.
    e.stopPropagation();
    if (e.key === 'Escape') {
      e.preventDefault();
      onclose();
    } else if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      void run();
    } else if ((e.key === 'ArrowUp' || e.key === 'ArrowDown') && !e.altKey && !e.metaKey && !e.ctrlKey) {
      if (recall(e.key === 'ArrowUp' ? -1 : 1)) e.preventDefault();
    }
  }

  function insert() {
    if (!answer?.command || insertBlocked !== null) return;
    oninsert(answer.command);
    onclose();
  }
</script>

<section class="help" data-testid={testid} aria-label="Ask {model}">
  <div class="head">
    <h3>Ask {model}</h3>
    <span class="muted">with {what}</span>
    <button
      type="button"
      class="btn btn--icon btn--quiet close"
      data-testid="{testid}-close"
      aria-label="Close help"
      title="Close (Esc)"
      onclick={onclose}>×</button
    >
  </div>
  <div class="ask">
    <input
      bind:this={input}
      bind:value={question}
      class="q"
      data-testid="{testid}-question"
      aria-label="Question"
      placeholder={line.trim() ? 'What do you want to know? (Enter asks about the line)' : 'What do you want to know?'}
      oninput={() => (recallAt = null)}
      onkeydown={onKey}
    />
    <button
      type="button"
      class="btn btn--primary"
      data-testid="{testid}-ask"
      disabled={busy || blocked !== null}
      title={blocked ?? 'Ask (Enter)'}
      onclick={() => void run()}>{busy ? 'Asking…' : 'Ask'}</button
    >
  </div>
  {#if error}
    <p class="error" role="alert" data-testid="{testid}-error">{error}</p>
  {/if}
  {#if answer}
    <p class="text" data-testid="{testid}-answer">{answer.answer}</p>
    {#if answer.command}
      <div class="proposal">
        <code data-testid="{testid}-command">{answer.command}</code>
        <CopyButton text={answer.command} label="Copy command" />
        <button
          type="button"
          class="btn"
          data-testid="{testid}-insert"
          title={insertBlocked ?? 'Put it on the prompt line. It is not sent: your Enter sends it.'}
          disabled={insertBlocked !== null}
          onclick={insert}>{insertLabel}</button
        >
      </div>
    {/if}
    <p class="muted drafted">
      <DraftedLabel testid="{testid}-drafted" />
      <span data-testid="{testid}-meta">{meta}</span>
    </p>
  {/if}
</section>

<style>
  .help {
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 0.5rem 0.6rem;
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    background: var(--bg-raise);
    box-shadow: var(--shadow-pop);
  }
  .head {
    display: flex;
    align-items: baseline;
    gap: 8px;
  }
  .close {
    margin-left: auto;
  }
  h3 {
    margin: 0;
    font-size: var(--text-xs);
  }
  .ask {
    display: flex;
    gap: 6px;
  }
  .q {
    flex: 1;
    min-width: 0;
    font-size: var(--text-xs);
  }
  .text {
    margin: 0;
    white-space: pre-wrap;
    font-size: var(--text-xs);
    line-height: 1.45;
  }
  .proposal {
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .proposal code {
    flex: 1;
    min-width: 0;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    font-size: var(--text-xs);
    padding: 4px 6px;
    border-radius: var(--radius-sm);
    background: var(--chip-bg);
  }
  .muted {
    margin: 0;
    font-size: var(--text-2xs);
    color: var(--fg-muted);
  }
  .drafted {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px;
  }
  .error {
    margin: 0;
    font-size: var(--text-2xs);
    color: var(--danger);
  }
</style>
