<script lang="ts" module>
  // What a person did on a card (ticked steps, the open section, a form
  // sent) by session and block text, so it survives the transcript being
  // re-read and the turn re-drawn. Per window, never saved: a card is a view
  // of the reply, not a record.
  const ticked = new Map<string, Set<number>>();
  const sent = new Set<string>();
</script>

<script lang="ts">
  // One fleet.ui/1 block (docs/chat-blocks.md). Display kinds draw from the
  // block alone; `choices`, `form` and an error's next steps put text in the session's composer and
  // never send it, so the person reads and presses Enter themselves.
  import { blockKey, fenced, formAnswerPrompt, type UiBlock } from '../rich_blocks';
  import { insertIntoComposer } from '../conversation';
  import FormWizard from '../forms/FormWizard.svelte';
  import type { Values } from '../forms/forms';
  import Markdown from '../MarkdownView.svelte';
  import ErrorCard from './ErrorCard.svelte';
  import GuidePageCard from './GuidePageCard.svelte';
  import ProgressCard from './ProgressCard.svelte';
  import ReportCard from './ReportCard.svelte';
  import ResultsCard from './ResultsCard.svelte';
  import SettingCard from './SettingCard.svelte';
  import WizardChatCard from '../forms/WizardChatCard.svelte';
  import { WIZARDS } from '../forms/wizards';

  let { block, raw, sessionId = null }: { block: UiBlock; raw: string; sessionId?: number | null } = $props();

  const key = $derived(`${sessionId ?? '-'}:${blockKey(raw)}`);
  let done = $state<Set<number>>(new Set());
  let formSent = $state(false);
  $effect(() => {
    done = new Set(ticked.get(key) ?? []);
    formSent = sent.has(key);
  });

  function tick(k: number, on: boolean) {
    const next = new Set(done);
    if (on) next.add(k);
    else next.delete(k);
    done = next;
    ticked.set(key, next);
  }

  function fill(text: string) {
    if (sessionId !== null) insertIntoComposer(sessionId, text);
  }

  function submitForm(values: Values) {
    if (block.kind !== 'form' || sessionId === null) return;
    fill(formAnswerPrompt(block.form, values));
    sent.add(key);
    formSent = true;
  }

</script>

{#if block.kind === 'report'}
  <ReportCard report={block} {raw} title={block.title} {sessionId} />
{:else if block.kind === 'callout'}
  <aside class="card callout {block.tone}" data-testid="rich-callout" role="note">
    {#if block.title}<strong>{block.title}</strong>{/if}
    <Markdown source={block.body} />
  </aside>
{:else if block.kind === 'facts'}
  <section class="card" data-testid="rich-facts">
    {#if block.title}<strong>{block.title}</strong>{/if}
    <dl>
      {#each block.items as [label, value], k (k)}
        <dt>{label}</dt>
        <dd>{value}</dd>
      {/each}
    </dl>
  </section>
{:else if block.kind === 'steps'}
  {@const total = block.steps.length}
  <section class="card" data-testid="rich-steps" aria-label={block.title}>
    <header>
      <strong>{block.title}</strong>
      <span class="muted" data-testid="rich-steps-progress">{done.size} of {total} done</span>
    </header>
    {#if block.intro}<Markdown source={block.intro} />{/if}
    <ol class="steps">
      {#each block.steps as s, k (k)}
        <li class:checked={done.has(k)}>
          <label class="step-head">
            <input
              type="checkbox"
              checked={done.has(k)}
              data-testid="rich-step-check"
              onchange={(e) => tick(k, (e.currentTarget as HTMLInputElement).checked)} />
            <span class="num">{k + 1}</span>
            <span class="step-title">{s.title}</span>
          </label>
          {#if s.body || s.code}
            <div class="step-body">
              {#if s.body}<Markdown source={s.body} />{/if}
              {#if s.code}<Markdown source={fenced(s.lang ?? '', s.code)} />{/if}
            </div>
          {/if}
        </li>
      {/each}
    </ol>
  </section>
{:else if block.kind === 'guide' && block.page}
  <GuidePageCard pageId={block.page} />
{:else if block.kind === 'guide'}
  <section class="card" data-testid="rich-guide" aria-label={block.title}>
    <strong>{block.title}</strong>
    {#if block.intro}<Markdown source={block.intro} />{/if}
    {#each block.sections as s, k (k)}
      <details class="section" open={k === 0}>
        <summary>{s.title}</summary>
        <div class="section-body"><Markdown source={s.body} /></div>
      </details>
    {/each}
  </section>
{:else if block.kind === 'choices'}
  <section class="card accent" data-testid="rich-choices">
    {#if block.title}<strong>{block.title}</strong>{/if}
    {#if block.question}<Markdown source={block.question} />{/if}
    <div class="options">
      {#each block.options as o, k (k)}
        <button
          type="button"
          class="option"
          data-testid="rich-choice"
          disabled={sessionId === null}
          title={o.hint ?? o.prompt}
          onclick={() => fill(o.prompt)}
          ><span class="option-label">{o.label}</span>{#if o.hint}<span class="hint">{o.hint}</span>{/if}</button
        >
      {/each}
    </div>
    <p class="note">{sessionId === null ? 'Choices fill the composer of a running session.' : 'A choice fills the composer; press Enter to send it.'}</p>
  </section>
{:else if block.kind === 'form'}
  <section class="card accent" data-testid="rich-form" aria-label={block.form.title}>
    <strong>{block.form.title}</strong>
    {#if block.form.intro}<p class="intro">{block.form.intro}</p>{/if}
    {#if formSent}
      <p class="note" data-testid="rich-form-sent">Answers are in the composer. Press Enter to send them.</p>
    {:else}
      <FormWizard spec={block.form} disabled={sessionId === null} onsubmit={submitForm} />
      <p class="note">{sessionId === null ? 'This form fills the composer of a running session.' : 'Submitting puts the answers in the composer; nothing is sent until you press Enter.'}</p>
    {/if}
  </section>
{:else if block.kind === 'progress'}
  <ProgressCard {block} />
{:else if block.kind === 'results'}
  <ResultsCard {block} />
{:else if block.kind === 'error'}
  <ErrorCard {block} onfill={fill} canFill={sessionId !== null} />
{:else if block.kind === 'setting'}
  <SettingCard {block} />
{:else if block.kind === 'wizard'}
  <!-- Step 10.12: one of the app's wizards as a form; its last button runs
       the wizard, and how it went fills the composer for the agent. -->
  {#if sessionId !== null}
    <WizardChatCard id={block.wizard} from="the agent" why={block.why ?? null} drafted={block.values ?? null} stateKey={key} report={fill} />
  {:else}
    <p class="card muted" data-testid="rich-wizard-off">{WIZARDS[block.wizard].spec.title}: opens in the live conversation.</p>
  {/if}
{/if}

<style>
  .card {
    --tone: var(--border);
    display: flex;
    flex-direction: column;
    gap: 0.45rem;
    margin: 0.4em 0 0.7em;
    padding: 0.65rem var(--space-3);
    border: 1px solid var(--border);
    border-left: 3px solid var(--tone);
    border-radius: var(--radius-md);
    background: var(--bg-pane);
  }
  .card.accent { --tone: var(--accent); }
  .callout { background: color-mix(in srgb, var(--tone) 8%, var(--bg-pane)); }
  .callout.info { --tone: var(--accent); }
  .callout.tip { --tone: var(--syn-str); }
  .callout.success { --tone: var(--usage-ok); }
  .callout.warning { --tone: var(--usage-warn); }
  .callout.danger { --tone: var(--usage-crit); }
  header {
    display: flex;
    justify-content: space-between;
    align-items: baseline;
    gap: var(--space-2);
  }
  .muted,
  .note {
    color: var(--fg-muted);
    font-size: var(--text-2xs);
  }
  .note { margin: 0; }
  .intro { margin: 0; font-size: 0.85em; }
  dl {
    display: grid;
    grid-template-columns: max-content 1fr;
    gap: 0.2rem var(--space-3);
    margin: 0;
    font-size: 0.88em;
  }
  dt { color: var(--fg-muted); }
  dd { margin: 0; word-break: break-word; }
  .steps {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
  }
  .step-head {
    display: flex;
    gap: 0.45rem;
    align-items: center;
    cursor: pointer;
  }
  .num {
    display: inline-flex;
    justify-content: center;
    align-items: center;
    min-width: 1.3rem;
    height: 1.3rem;
    border-radius: 50%;
    border: 1px solid var(--control-border);
    font-size: var(--text-2xs);
    color: var(--fg-muted);
  }
  .step-title { font-weight: 600; }
  .checked .step-title {
    color: var(--fg-muted);
    text-decoration: line-through;
  }
  .checked .num {
    border-color: var(--usage-ok);
    color: var(--usage-ok);
  }
  .step-body {
    margin-left: 3.1rem;
    font-size: 0.9em;
  }
  .section {
    border-top: 1px solid var(--border);
    padding-top: var(--space-1);
  }
  .section summary {
    cursor: pointer;
    font-weight: 600;
  }
  .section-body {
    padding: var(--space-1) 0 0.1rem 1rem;
    font-size: 0.9em;
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
    padding: var(--space-1) 0.65rem;
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
  .option-label { font-weight: 600; }
  .hint { font-size: var(--text-2xs); color: var(--fg-muted); }
</style>
