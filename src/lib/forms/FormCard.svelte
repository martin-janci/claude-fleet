<script lang="ts">
  // The chat form card: who asks, why, the wizard, Decline, when it
  // expires. A decided form collapses to its receipt (step 10.1): how it
  // ended, a one-line summary of the answers, the answers behind "Show
  // answers". `closed` shows only the receipt, for a form that left the row
  // (answered on the phone, withdrawn, expired).
  import { onDestroy } from 'svelte';
  import FormWizard from './FormWizard.svelte';
  import SavedLaterLine from './SavedLaterLine.svelte';
  import { answerForm, declineForm, getForm, type FieldProblem, type FormView, type Values } from './forms';
  import { answerList, answerSummary, endedMark, endedWords, expiresIn } from './receipt';

  let {
    formId,
    sessionName,
    blocked,
    closed = false,
    ondismiss,
  }: {
    formId: string;
    sessionName: string;
    blocked: string | null;
    closed?: boolean;
    ondismiss?: () => void;
  } = $props();

  let form = $state<FormView | null>(null);
  let error = $state<string | null>(null);
  let busy = $state(false);
  let problems = $state<FieldProblem[]>([]);
  let declining = $state(false);
  let note = $state('');
  let wizard: { clearSecrets: () => void; forgetSaved: () => void } | undefined = $state();
  // "Save and finish later" (a spec's `save_later`): the card folds to one
  // line until the person resumes; the answers stay on this device.
  let later = $state(false);

  $effect(() => {
    const id = formId;
    form = null;
    error = null;
    problems = [];
    declining = false;
    note = '';
    busy = false;
    later = false;
    void getForm(id).then((r) => {
      if (id !== formId) return;
      if (r.ok) form = r.value;
      else error = r.error.message;
    });
  });

  // The clock the expiry line and the receipt's age read; a minute is the
  // finest either says.
  let now = $state(Math.floor(Date.now() / 1000));
  const tick = setInterval(() => (now = Math.floor(Date.now() / 1000)), 30_000);
  onDestroy(() => clearInterval(tick));

  let showAnswers = $state(false);
  const summary = $derived(form && form.state === 'answered' ? answerSummary(form) : '');
  const answers = $derived(form && showAnswers ? answerList(form) : []);
  const ended = $derived(form ? endedWords(form, now) : '');

  async function submit(values: Values) {
    if (blocked !== null) return;
    busy = true;
    error = null;
    problems = [];
    const r = await answerForm(formId, values);
    busy = false;
    wizard?.clearSecrets();
    if (r.ok) {
      wizard?.forgetSaved();
      form = r.value;
      return;
    }
    const details = (r.error as { details?: { problems?: FieldProblem[] } }).details;
    if (details?.problems) problems = details.problems;
    else error = r.error.message;
  }

  async function decline() {
    if (blocked !== null) return;
    busy = true;
    error = null;
    const r = await declineForm(formId, note);
    busy = false;
    if (r.ok) {
      wizard?.forgetSaved();
      form = r.value;
    } else error = r.error.message;
  }
</script>

{#snippet receipt(f: FormView)}
  <div class="receipt" data-testid="form-outcome" data-state={f.state} role="status">
    <div class="line">
      <span class={`mark ${f.state}`} aria-hidden="true">{endedMark(f.state)}</span>
      <strong>{f.title}</strong>
      <span class="meta" data-testid="form-ended">{ended}</span>
      {#if closed && ondismiss}<button type="button" class="x" aria-label="Dismiss" onclick={ondismiss}>×</button>{/if}
    </div>
    {#if summary}<div class="summary" data-testid="form-summary">{summary}</div>{/if}
    {#if f.state === 'declined' && f.note}<div class="summary" data-testid="form-note">“{f.note}”</div>{/if}
    {#if f.state === 'expired'}<div class="summary">No answer in 24 h.</div>{/if}
    {#if f.state === 'answered' && answerList(f).length > 0}
      <button type="button" class="link" data-testid="form-show-answers" aria-expanded={showAnswers} onclick={() => (showAnswers = !showAnswers)}>
        {showAnswers ? 'Hide answers' : 'Show answers'}
      </button>
      {#if showAnswers}
        <dl class="answers" data-testid="form-answers">
          {#each answers as a (a.name)}<dt>{a.label}</dt><dd>{a.text}</dd>{/each}
        </dl>
      {/if}
    {/if}
  </div>
{/snippet}

{#if closed}
  {#if form}{@render receipt(form)}{/if}
{:else if form && form.state !== 'pending'}
  {@render receipt(form)}
  {#if error}<p class="err" data-testid="form-error">{error}</p>{/if}
{:else if form && later}
  <SavedLaterLine title={form.title} meta={`saved to finish later · ${expiresIn(form.created_at, now)}`} onresume={() => (later = false)} />
{:else}
  <section class="card" data-testid="form-card" aria-label={`Form from ${sessionName}`}>
    {#if form}
      <header>
        <strong>{form.title}</strong>
        <span class="who">asked by {sessionName}</span>
      </header>
      {#if form.why}<p class="why" data-testid="form-why">{form.why}</p>{/if}
      {#if form.spec.intro}<p class="intro">{form.spec.intro}</p>{/if}
      {#if blocked}<p class="blocked" data-testid="form-blocked">{blocked}</p>{/if}
      <FormWizard
        bind:this={wizard}
        spec={form.spec}
        hostAlias={form.host_alias}
        {busy}
        disabled={blocked !== null}
        serverProblems={problems}
        proposal={form.proposal ?? null}
        saveKey={form.form_id}
        onsavelater={() => (later = true)}
        onunplaced={(ps) => (error = ps.map((p) => `${p.field}: ${p.problem}`).join('; '))}
        onsubmit={submit} />
      <div class="foot">
        {#if blocked === null}
          {#if declining}
            <div class="decline">
              <input
                type="text"
                placeholder="Why not (optional)"
                data-testid="form-decline-note"
                maxlength="500"
                value={note}
                oninput={(e) => (note = (e.currentTarget as HTMLInputElement).value)} />
              <button type="button" data-testid="form-decline-confirm" disabled={busy} onclick={decline}>Decline</button>
              <button type="button" disabled={busy} onclick={() => (declining = false)}>Keep</button>
            </div>
          {:else}
            <button type="button" class="link" data-testid="form-decline" disabled={busy} onclick={() => (declining = true)}>Decline…</button>
          {/if}
        {/if}
        <span class="meta expires" data-testid="form-expires">{expiresIn(form.created_at, now)}</span>
      </div>
    {/if}
    {#if error}<p class="err" data-testid="form-error">{error}</p>{/if}
  </section>
{/if}

<style>
  .card { display: flex; flex-direction: column; gap: 0.5rem; padding: 0.7rem 0.8rem; border: 1px solid var(--border); border-left: 3px solid var(--usage-warn); border-radius: var(--radius-md); background: color-mix(in srgb, var(--usage-warn) 10%, var(--bg-pane)); }
  header { display: flex; gap: 0.5rem; align-items: baseline; }
  .who { font-size: var(--text-2xs); color: var(--fg-muted); }
  .why, .intro { margin: 0; font-size: var(--text-2xs); }
  .blocked { margin: 0; font-size: var(--text-2xs); color: var(--fg-muted); }
  .decline { display: flex; gap: 0.4rem; }
  .decline input { flex: 1; font: inherit; font-size: var(--text-2xs); }
  .link { align-self: flex-start; background: none; border: none; padding: 0; color: var(--fg-muted); text-decoration: underline; cursor: pointer; font-size: var(--text-2xs); }
  .err { margin: 0; font-size: var(--text-2xs); color: var(--usage-crit); }
  .foot { display: flex; gap: 0.5rem; align-items: center; }
  .foot .decline { flex: 1; }
  .expires { margin-left: auto; }
  .meta { font-size: var(--text-2xs); color: var(--fg-muted); }
  .receipt { display: flex; flex-direction: column; gap: 0.25rem; padding: 0.55rem 0.8rem; border: 1px solid var(--border); border-radius: var(--radius-md); font-size: var(--text-2xs); }
  .receipt .line { display: flex; gap: 0.5rem; align-items: baseline; }
  .receipt strong { flex: 1; }
  .mark { color: var(--fg-muted); }
  .mark.answered { color: var(--usage-ok); }
  .mark.declined { color: var(--usage-crit); }
  .summary { color: var(--fg-muted); overflow-wrap: anywhere; }
  .answers { display: grid; grid-template-columns: max-content 1fr; gap: 0.15rem 0.6rem; margin: 0.2rem 0 0; }
  .answers dt { color: var(--fg-muted); }
  .answers dd { margin: 0; overflow-wrap: anywhere; }
  .x { background: none; border: none; cursor: pointer; color: inherit; }
</style>
