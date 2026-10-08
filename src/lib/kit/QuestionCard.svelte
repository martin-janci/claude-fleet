<!-- The card a waiting session shows (manual: QuestionCard): the question,
     its age, the exact command in mono, and the answers in the agent's own
     order with their number keys. Nothing is pre-selected: an answer is
     primary only when the consumer says so, never as the AI's pick. -->
<script lang="ts">
  import Button from './Button.svelte';
  import StatusDot from './StatusDot.svelte';
  import type { Answer } from './status';

  let {
    question,
    age,
    detail,
    answers,
    onownwords,
    mac,
    testid,
  }: {
    question: string;
    /** "asked 2m ago" */
    age?: string;
    /** The command or text the person approves, never behind a toggle. */
    detail?: string;
    answers: Answer[];
    /** "Answer in your own words…", always offered. */
    onownwords: () => void;
    mac?: boolean;
    testid?: string;
  } = $props();

  // 1, 2 and 3 answer from the keyboard while focus is in the card.
  function onkeydown(e: KeyboardEvent) {
    if (e.metaKey || e.ctrlKey || e.altKey) return;
    if ((e.target as HTMLElement | null)?.closest('input, textarea, [contenteditable="true"]')) return;
    const n = Number(e.key);
    if (Number.isInteger(n) && n >= 1 && n <= Math.min(answers.length, 9)) {
      e.preventDefault();
      answers[n - 1].onselect();
    }
  }
</script>

<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<section class="of of-question" aria-label="Question from the session" {onkeydown} data-testid={testid}>
  <div class="q">
    <StatusDot state="waiting" label={null} />
    <strong>{question}</strong>
    {#if age}<span class="meta">{age}</span>{/if}
  </div>
  {#if detail}<div class="mono" style:color="var(--fg-2)" style:overflow-wrap="anywhere">{detail}</div>{/if}
  <div class="acts">
    {#each answers as a, i (i)}
      <Button variant={a.primary ? 'primary' : 'default'} kbd={i < 9 ? String(i + 1) : undefined} {mac} onclick={a.onselect}
        >{a.label}</Button
      >
    {/each}
    <Button variant="quiet" onclick={onownwords}>Answer in your own words…</Button>
  </div>
</section>
