<script lang="ts">
  // The chat form card: who asks, why, the wizard, Decline. `closed` shows
  // one line saying how a form that left the row ended (answered on the
  // phone, withdrawn, expired).
  import FormWizard from './FormWizard.svelte';
  import { answerForm, declineForm, getForm, type FieldProblem, type FormView, type Values } from './forms';

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
  let wizard: { clearSecrets: () => void } | undefined = $state();

  $effect(() => {
    const id = formId;
    form = null;
    error = null;
    problems = [];
    declining = false;
    note = '';
    busy = false;
    void getForm(id).then((r) => {
      if (id !== formId) return;
      if (r.ok) form = r.value;
      else error = r.error.message;
    });
  });

  const OUTCOME: Record<string, string> = {
    answered: 'answered',
    declined: 'declined',
    cancelled: 'withdrawn by the agent',
    expired: 'expired unanswered',
    pending: 'still waiting',
  };
  const outcome = $derived(
    form
      ? `${form.title}: ${OUTCOME[form.state] ?? form.state}${form.answered_by && (form.state === 'answered' || form.state === 'declined') ? ` by ${form.answered_by}` : ''}`
      : '',
  );

  async function submit(values: Values) {
    if (blocked !== null) return;
    busy = true;
    error = null;
    problems = [];
    const r = await answerForm(formId, values);
    busy = false;
    wizard?.clearSecrets();
    if (r.ok) {
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
    if (r.ok) form = r.value;
    else error = r.error.message;
  }
</script>

{#if closed}
  {#if form}
    <div class="outcome" data-testid="form-outcome" role="status">
      <span>Form {outcome}</span>
      {#if ondismiss}<button type="button" class="x" aria-label="Dismiss" onclick={ondismiss}>×</button>{/if}
    </div>
  {/if}
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
      {#if form.state === 'pending'}
        <FormWizard
          bind:this={wizard}
          spec={form.spec}
          {busy}
          disabled={blocked !== null}
          serverProblems={problems}
          onunplaced={(ps) => (error = ps.map((p) => `${p.field}: ${p.problem}`).join('; '))}
          onsubmit={submit} />
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
            <button type="button" class="link" data-testid="form-decline" disabled={busy} onclick={() => (declining = true)}>Decline</button>
          {/if}
        {/if}
      {:else}
        <p class="done" data-testid="form-outcome">Form {outcome}</p>
      {/if}
    {/if}
    {#if error}<p class="err" data-testid="form-error">{error}</p>{/if}
  </section>
{/if}

<style>
  .card { display: flex; flex-direction: column; gap: 0.5rem; padding: 0.7rem 0.8rem; border: 1px solid var(--border); border-left: 3px solid var(--usage-warn); border-radius: 6px; background: color-mix(in srgb, var(--usage-warn) 10%, var(--bg-pane)); }
  header { display: flex; gap: 0.5rem; align-items: baseline; }
  .who { font-size: 11px; color: var(--fg-muted); }
  .why, .intro { margin: 0; font-size: 0.8rem; }
  .blocked { margin: 0; font-size: 11px; color: var(--fg-muted); }
  .decline { display: flex; gap: 0.4rem; }
  .decline input { flex: 1; font: inherit; font-size: 0.8rem; }
  .link { align-self: flex-start; background: none; border: none; padding: 0; color: var(--fg-muted); text-decoration: underline; cursor: pointer; font-size: 11px; }
  .err { margin: 0; font-size: 11px; color: var(--usage-crit); }
  .outcome, .done { display: flex; justify-content: space-between; margin: 0; font-size: 0.8rem; color: var(--fg-muted); }
  .x { background: none; border: none; cursor: pointer; color: inherit; }
</style>
