<script lang="ts" module>
  import type { FieldProblem } from './forms';

  /** What pressing the last step's button did. */
  export type ChatFormOutcome =
    | { ok: true; summary?: string; starting?: string }
    | { ok: false; problems?: FieldProblem[]; error?: string };

  /** How a card ended: kept by the caller so a re-drawn card stays one line. */
  export type ChatFormEnded =
    | { state: 'answered'; summary: string; starting: string | null; at?: number }
    | { state: 'declined'; note: string; at?: number };
</script>

<script lang="ts">
  // A wizard in the conversation (redesign step 10.12, the ChatWizards
  // board): the same fleet.form/1 spec a dialog shows, as a card Control or
  // the app writes into the chat. While the spec is still being written it
  // draws in (a small Atom and what is being read); once whole it is the
  // wizard; the last button runs with a Comet in it; then the card shrinks
  // to one line, and a Pulse says what is starting. Nothing runs until the
  // last step's button is pressed, and nothing here is a modal or an
  // overlay. An agent's own form is FormCard; this is the app's.
  import { untrack } from 'svelte';
  import FormWizard from './FormWizard.svelte';
  import SavedLaterLine from './SavedLaterLine.svelte';
  import Loader from '../Loader.svelte';
  import { finishedSpec, partialSpec } from './partial_spec';
  import { answerSummary, decidedClock, endedMark } from './receipt';
  import type { FormSpec, FormView, Values } from './forms';

  let {
    spec = null,
    draft = '',
    from,
    reading = null,
    why = null,
    sending = 'Sending…',
    initial = {},
    building = false,
    outcome = null,
    saveKey = null,
    onsubmit,
    ondecline,
    onended,
  }: {
    /** The whole spec, or null while `draft` is still being written. */
    spec?: FormSpec | null;
    /** The spec's JSON as it streams in. */
    draft?: string;
    /** Who wrote the form ("Control"). */
    from: string;
    /** What the writer is reading while it builds ("2 repos and your hosts"). */
    reading?: string | null;
    why?: string | null;
    sending?: string;
    initial?: Values;
    /** Still being written: `draft` stays a sketch even once it parses
     *  (an agent's `ask { draft }`; its `ask { form }` opens the form). */
    building?: boolean;
    /** How it ended, when the card is drawn again after it did. */
    outcome?: ChatFormEnded | null;
    /** Where "Save and finish later" keeps the answers on this device (the
     *  card's own key). With the spec's `save_later`, the card offers it,
     *  folds to one line with Resume, and opens again from what was kept. */
    saveKey?: string | null;
    onsubmit: (values: Values) => Promise<ChatFormOutcome>;
    ondecline?: (note: string) => void | Promise<void>;
    onended?: (ended: ChatFormEnded) => void;
  } = $props();

  const whole = $derived(spec ?? (building ? null : finishedSpec(draft)));
  const sketch = $derived(whole ? null : partialSpec(draft));

  let busy = $state(false);
  let error = $state<string | null>(null);
  let problems = $state<FieldProblem[]>([]);
  let declining = $state(false);
  let note = $state('');
  let ended = $state<ChatFormEnded | null>(untrack(() => outcome));
  let wizard: { clearSecrets: () => void; forgetSaved: () => void } | undefined = $state();
  // Saved to finish later: the card is one line until Resume.
  let later = $state(false);

  /** The receipt line's summary, read as a decided form's would be. */
  function summaryOf(s: FormSpec, values: Values): string {
    // A secret counts ("1 secret"), never with its value.
    const secrets: Record<string, string> = {};
    for (const st of s.steps)
      for (const f of st.fields ?? []) if (f.type === 'secret' && typeof values[f.name] === 'string' && values[f.name] !== '') secrets[f.name] = '';
    const view = { spec: s, answers: values, secrets, host_alias: '', state: 'answered' } as unknown as FormView;
    return answerSummary(view);
  }

  async function submit(values: Values) {
    if (!whole) return;
    busy = true;
    error = null;
    problems = [];
    const r = await onsubmit(values);
    busy = false;
    wizard?.clearSecrets();
    if (r.ok) {
      wizard?.forgetSaved();
      ended = { state: 'answered', summary: r.summary ?? summaryOf(whole, values), starting: r.starting ?? null, at: Math.floor(Date.now() / 1000) };
      onended?.(ended);
      return;
    }
    if (r.problems?.length) problems = r.problems;
    else error = r.error ?? 'That did not go through.';
  }

  async function decline() {
    busy = true;
    await ondecline?.(note.trim());
    busy = false;
    wizard?.forgetSaved();
    ended = { state: 'declined', note: note.trim(), at: Math.floor(Date.now() / 1000) };
    onended?.(ended);
  }
</script>

{#if ended && whole}
  <div class="receipt" data-testid="chat-form-outcome" data-state={ended.state} role="status">
    <div class="line">
      <span class={`mark ${ended.state}`} aria-hidden="true">{endedMark(ended.state)}</span>
      <strong>{whole.title}</strong>
      <span class="meta"
        >{ended.state === 'answered' ? 'answered by you on the desktop' : 'declined'}{ended.at ? ` · ${decidedClock(ended.at, Math.floor(Date.now() / 1000))}` : ''}</span
      >
    </div>
    {#if ended.state === 'answered' && ended.summary}<div class="summary" data-testid="chat-form-summary">{ended.summary}</div>{/if}
    {#if ended.state === 'declined' && ended.note}<div class="summary">“{ended.note}”</div>{/if}
  </div>
  {#if ended.state === 'answered' && ended.starting}
    <div class="starting" data-testid="chat-form-starting">
      <Loader name="pulse-sequence" size={24} label={ended.starting} />
      <span>{ended.starting}</span>
    </div>
  {/if}
{:else if whole && later}
  <SavedLaterLine title={whole.title} onresume={() => (later = false)} />
{:else}
  <section class="card" data-testid="chat-form" aria-label={`Form from ${from}`} aria-busy={!whole}>
    {#if whole}
      <header>
        <strong>{whole.title}</strong>
        <span class="who">from {from}</span>
      </header>
      {#if why}<p class="why">{why}</p>{/if}
      {#if whole.intro}<p class="intro">{whole.intro}</p>{/if}
      <p class="rule">Nothing runs until you press {whole.submit ?? 'Submit'}.</p>
      <FormWizard
        bind:this={wizard}
        spec={whole}
        {busy}
        {sending}
        {initial}
        serverProblems={problems}
        {saveKey}
        onsavelater={() => (later = true)}
        onunplaced={(ps) => (error = ps.map((p) => `${p.field}: ${p.problem}`).join('; '))}
        onsubmit={(v) => void submit(v)} />
      {#if ondecline}
        <div class="foot">
          {#if declining}
            <div class="decline">
              <input
                type="text"
                placeholder="Why not (optional)"
                data-testid="chat-form-decline-note"
                maxlength="500"
                value={note}
                oninput={(e) => (note = (e.currentTarget as HTMLInputElement).value)} />
              <button type="button" data-testid="chat-form-decline-confirm" disabled={busy} onclick={decline}>Decline</button>
              <button type="button" disabled={busy} onclick={() => (declining = false)}>Keep</button>
            </div>
          {:else}
            <button type="button" class="link" data-testid="chat-form-decline" disabled={busy} onclick={() => (declining = true)}
              >Decline…</button>
          {/if}
        </div>
      {/if}
    {:else if sketch}
      <div class="building" data-testid="chat-form-building">
        <Loader name="atom" size={20} label="Building the form" />
        <span>Building a form{reading ? ` · reading ${reading}` : ''}</span>
      </div>
      {#if sketch.title}<strong data-testid="chat-form-draft-title">{sketch.title}</strong>{/if}
      {#each sketch.steps as s (s.title)}
        {#each s.fields as f (f.name)}
          <div class="field" data-testid={`chat-form-draft-${f.name}`}>
            <span class="label">{f.label}</span>
            <span class="skel" aria-hidden="true"></span>
          </div>
        {/each}
      {/each}
      <div class="field" aria-hidden="true">
        <span class="skel short"></span>
        <span class="skel"></span>
      </div>
    {/if}
    {#if error}<p class="err" data-testid="chat-form-error">{error}</p>{/if}
  </section>
{/if}

<style>
  .card { display: flex; flex-direction: column; gap: 0.5rem; padding: 0.7rem 0.8rem; border: 1px solid var(--border); border-left: 3px solid var(--accent); border-radius: var(--radius-md); background: color-mix(in srgb, var(--accent) 6%, var(--bg-pane)); }
  header { display: flex; gap: 0.5rem; align-items: baseline; }
  .who, .meta, .rule { font-size: var(--text-2xs); color: var(--fg-muted); }
  .why, .intro, .rule { margin: 0; }
  .why, .intro { font-size: var(--text-2xs); }
  .building { display: flex; gap: 0.5rem; align-items: center; font-size: var(--text-2xs); color: var(--fg-muted); }
  .field { display: flex; flex-direction: column; gap: 0.2rem; }
  .label { font-size: var(--text-2xs); }
  .skel { display: block; height: 1.4rem; border-radius: var(--radius-sm); background: color-mix(in srgb, var(--fg-muted) 14%, transparent); }
  .skel.short { width: 30%; height: 0.8rem; }
  .foot { display: flex; gap: 0.5rem; align-items: center; }
  .decline { display: flex; gap: 0.4rem; flex: 1; }
  .decline input { flex: 1; font: inherit; font-size: var(--text-2xs); }
  .link { align-self: flex-start; min-block-size: var(--control-h); background: none; border: none; padding: 0; color: var(--fg-muted); text-decoration: underline; cursor: pointer; font-size: var(--text-2xs); }
  .err { margin: 0; font-size: var(--text-2xs); color: var(--usage-crit); }
  .receipt { display: flex; flex-direction: column; gap: 0.25rem; padding: 0.55rem 0.8rem; border: 1px solid var(--border); border-radius: var(--radius-md); font-size: var(--text-2xs); }
  .receipt .line { display: flex; gap: 0.5rem; align-items: baseline; }
  .receipt strong { flex: 1; }
  .mark.answered { color: var(--usage-ok); }
  .mark.declined { color: var(--usage-crit); }
  .summary { color: var(--fg-muted); overflow-wrap: anywhere; }
  .starting { display: flex; gap: 0.5rem; align-items: center; font-size: var(--text-2xs); color: var(--fg-muted); }
</style>
