<!-- The card a waiting session shows (manual: QuestionCard): the question,
     its age, the exact command in mono, and the answers in the agent's own
     order with their number keys. Nothing is pre-selected or sent: an
     answer is primary only when the consumer says so. `AnswerPrompt` marks
     Jev's quick answer (J5) primary, and `quickOrder` (quick_answer.ts)
     never proposes on a permission dialog or a risky option (a push, an
     allow, a step hard to undo), so Approve on a push or a permission is
     never the highlighted one.
     Redesign 5.9 makes it the one approval card: `AnswerPrompt` draws every
     agent dialog through it, in the Conversation and, as
     `compact`, on a session row. -->
<script lang="ts">
  import type { Snippet } from 'svelte';
  import Button from './Button.svelte';
  import StatusDot from './StatusDot.svelte';
  import type { Answer } from './status';
  import { questionEnter } from '../motion_catalog';

  let {
    question,
    age,
    detail,
    answers,
    onownwords,
    mac,
    testid,
    label = 'Question from the session',
    compact = false,
    keys = true,
    enter = false,
    children,
  }: {
    question: string;
    /** "asked 2m ago" */
    age?: string;
    /** The command or text the person approves, never behind a toggle. */
    detail?: string;
    answers: Answer[];
    /** "Answer in your own words…": offered wherever there is somewhere to
     *  type, which a session row is not. */
    onownwords?: () => void;
    mac?: boolean;
    testid?: string;
    /** The card's accessible name. */
    label?: string;
    /** Row density: smaller padding, no own-words button. */
    compact?: boolean;
    /** Answer 1–9 from the keyboard while focus is in the card. Off when
     *  the consumer handles the number keys itself. */
    keys?: boolean;
    /** A question that just arrived (G4.9): it fades up 8 px and focus
     *  moves to its first answer, unless the person is typing. */
    enter?: boolean;
    /** Below the answers: what was sent, a stale or error line, more acts. */
    children?: Snippet;
  } = $props();

  const kbdOf = (a: Answer, i: number) => a.kbd ?? (i < 9 ? String(i + 1) : undefined);

  function arrive(node: HTMLElement) {
    if (enter) questionEnter(node);
  }

  // The number keys answer while focus is in the card.
  function onkeydown(e: KeyboardEvent) {
    if (!keys || e.metaKey || e.ctrlKey || e.altKey) return;
    if ((e.target as HTMLElement | null)?.closest('input, textarea, [contenteditable="true"]')) return;
    if (!/^[1-9]$/.test(e.key)) return;
    const a = answers.find((x, i) => kbdOf(x, i) === e.key);
    if (!a || a.disabled) return;
    e.preventDefault();
    a.onselect();
  }
</script>

<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<section class="of of-question" class:compact aria-label={label} {onkeydown} data-testid={testid} use:arrive>
  <div class="q">
    <StatusDot state="waiting" label={null} />
    <strong>{question}</strong>
    {#if age}<span class="meta">{age}</span>{/if}
  </div>
  {#if detail}<div class="mono" style:color="var(--fg-2)" style:overflow-wrap="anywhere" data-testid="question-detail">{detail}</div>{/if}
  <div class="acts">
    {#each answers as a, i (i)}
      <Button
        variant={a.primary ? 'primary' : 'default'}
        size={compact ? 'sm' : 'md'}
        kbd={kbdOf(a, i)}
        {mac}
        disabled={a.disabled}
        title={a.title}
        testid={a.testid}
        onclick={a.onselect}
        >{#if a.checked !== undefined}<span class="box" aria-hidden="true">{a.checked ? '✔' : ''}</span><span class="sr">{a.checked ? 'ticked: ' : 'not ticked: '}</span>{/if}{a.label}</Button
      >
    {/each}
    {#if onownwords && !compact}
      <Button variant="quiet" onclick={onownwords} testid="question-own-words">Answer in your own words…</Button>
    {/if}
  </div>
  {@render children?.()}
</section>

<style>
  .compact {
    padding: var(--space-2) var(--space-3);
    gap: var(--control-gap);
  }
  .box {
    display: inline-block;
    width: 12px;
    height: 12px;
    line-height: 12px;
    border: 1px solid currentColor;
    border-radius: var(--radius-xs);
    font-size: var(--text-2xs);
    text-align: center;
  }
  .sr {
    position: absolute;
    width: 1px;
    height: 1px;
    overflow: hidden;
    clip: rect(0 0 0 0);
    white-space: nowrap;
  }
</style>
