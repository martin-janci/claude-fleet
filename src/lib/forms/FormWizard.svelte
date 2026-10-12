<script lang="ts">
  // One chat form, step by step. The field markup is FlowView's; what is
  // asked follows the answers (form_model.ts). A secret is cleared once
  // sent and never prefilled. A choice of up to nine options is drawn as
  // numbered options; when it is the step's only one, 1–9 pick from it
  // (step 10.1), as on the Conversation's question card.
  import { untrack } from 'svelte';
  import { destination } from '../destination';
  import { matchShortcut } from '../shortcuts';
  import { detectMac, isEditable } from '../terminal_keys';
  import { hostCheckWarnings, shownBecause, startingValues, stepProblems, visibleSteps } from './form_model';
  import { hosts } from '../hosts';
  import { optionsOfField, type FieldProblem, type FormField, type FormProposal, type FormSpec, type OptionSpec, type Values } from './forms';
  import { reviewSections } from './receipt';
  import { clearSaved, loadSaved, restorable, saveForLater } from './saved_answers';
  import ProposedBy from '../ProposedBy.svelte';
  import DraftedLabel from '../DraftedLabel.svelte';
  import { neverDecidesField, preselect } from '../ai_proposal';
  import { QUICK_ANSWER, risky } from '../quick_answer';
  import Loader from '../Loader.svelte';
  import { fieldCount, submitHint, submitKey } from './form_frame';

  let {
    spec,
    busy = false,
    disabled = false,
    serverProblems = [],
    proposal = null,
    initial = {},
    sending = 'Sending…',
    buttonLoader = true,
    ownDefaults = false,
    saveKey = null,
    onsubmit,
    onunplaced,
    oncancel,
    onsavelater,
    hostAlias = null,
  }: {
    spec: FormSpec;
    /** The host a spec's `checks` read when they name no host field (the
     *  form's session's host). */
    hostAlias?: string | null;
    /** Jev's likely option for one choice (step 10.9): shown first and
     *  pre-selected when the field is empty; never a risky option. */
    proposal?: FormProposal | null;
    busy?: boolean;
    disabled?: boolean;
    serverProblems?: FieldProblem[];
    /** Starting values over the spec's own defaults (a wizard opened with
     *  what the screen already knows). Read once, as the defaults are. */
    initial?: Values;
    /** The last button while `busy`, after a Comet (step 10.12). */
    sending?: string;
    /** The Comet in the submit button while sending; off when the host
     *  draws the flow's own loader (one loader per screen, review r12). */
    buttonLoader?: boolean;
    /** The spec is the app's own (a wizard), so its defaults stand. An
     *  agent's form starts with no risky default (`startingValues`). */
    ownDefaults?: boolean;
    onsubmit: (values: Values) => void;
    /** Server problems whose field is on no visible step (nowhere to show them). */
    onunplaced?: (problems: FieldProblem[]) => void;
    /** A quiet Cancel at the row's start (a wizard in a dialog). */
    oncancel?: () => void;
    /** Where "Save and finish later" keeps the answers (a form's id). With
     *  the spec's `save_later` and this, the button shows and the wizard
     *  starts from what was kept. */
    saveKey?: string | null;
    /** The person saved and left: the host collapses the card. */
    onsavelater?: () => void;
  } = $props();
  const uid = $props.id();

  // The spec of one form never changes under the wizard (FormCard unmounts the
  // wizard while it loads another form), so the starting values are read once on purpose.
  // svelte-ignore state_referenced_locally
  // svelte-ignore state_referenced_locally
  const saved = spec.save_later && saveKey ? loadSaved(saveKey) : null;
  // svelte-ignore state_referenced_locally
  const kept = saved ? restorable(spec, saved) : null;
  // svelte-ignore state_referenced_locally
  const started: Values = { ...startingValues(spec, !ownDefaults), ...initial, ...(kept ?? {}) };
  let values = $state<Values>({ ...started });
  let index = $state(0);
  const steps = $derived(visibleSteps(spec, values));
  // G7.4: what the answers need from a host and it lacks, said above the
  // last button before sending. A warning only: the person may still send.
  const hostWarnings = $derived(
    spec.checks?.length ? hostCheckWarnings(spec, values, hostAlias, (a) => $hosts.find((h) => h.alias === a)) : [],
  );
  const step = $derived(steps[Math.min(index, steps.length - 1)]);
  const last = $derived(index >= steps.length - 1);
  const localProblems = $derived(stepProblems(spec, Math.min(index, steps.length - 1), values));
  const ready = $derived(localProblems.length === 0);

  // Checked on blur and on submit, never on each key (FormsAnatomy): a
  // field's own problem shows once the person has left it, or once they
  // tried to go on with the step unfinished.
  let touched = $state<Record<string, true>>({});
  let tried = $state(false);
  function leave(name: string) {
    if (!touched[name]) touched = { ...touched, [name]: true };
  }
  const problemOf = (name: string) =>
    serverProblems.find((p) => p.field === name)?.problem ??
    (touched[name] || tried ? (localProblems.find((p) => p.field === name)?.problem ?? null) : null);
  /** Why Next / the last button is off, said under it. */
  const why = $derived.by(() => {
    const p = localProblems[0];
    if (!p) return null;
    const label = step?.fields.find((f) => f.name === p.field)?.label ?? p.field;
    return `${label.replace(/[\s:*]+$/, '')} ${p.problem}.`;
  });

  /** Whether anything differs from where the form started. */
  export function isDirty(): boolean {
    const keys = new Set([...Object.keys(values), ...Object.keys(started)]);
    for (const k of keys) {
      const a = values[k];
      const b = started[k];
      const blank = (v: unknown) => v === undefined || v === '' || (Array.isArray(v) && v.length === 0);
      if (blank(a) && blank(b)) continue;
      if (JSON.stringify(a) !== JSON.stringify(b)) return true;
    }
    return false;
  }

  // A problem on another step is invisible where the user stands: go to the
  // first visible step that holds one. Only a new problem list moves the
  // wizard, never the user's own typing (hence untrack).
  $effect(() => {
    const probs = serverProblems;
    if (probs.length === 0) return;
    untrack(() => {
      const names = new Set(probs.map((p) => p.field));
      const at = steps.findIndex((s) => s.fields.some((f) => names.has(f.name)));
      if (at >= 0) index = at;
      else onunplaced?.(probs);
    });
  });

  function set(name: string, v: unknown) {
    values = { ...values, [name]: v };
  }

  // Whether `busy` is this wizard's own submit (the card is also busy while
  // it declines, and that is not "Sending…").
  let sent = $state(false);
  $effect(() => {
    if (!busy) sent = false;
  });

  function next() {
    index += 1;
    tried = false;
  }

  // Enter in a one-field step and ⌘↵ / Ctrl+Enter anywhere go on: Next, or
  // the last button. An unfinished step shows its problems instead.
  let stepEl: HTMLDivElement | undefined = $state();
  function onFormKeydown(e: KeyboardEvent) {
    if (e.defaultPrevented || off) return;
    if (!submitKey(e, fieldCount(stepEl), isMac)) return;
    e.preventDefault();
    if (!ready) tried = true;
    else if (last) submit();
    else next();
  }

  /** Only what a visible field holds is sent: a hidden step's values stay
   *  behind, as the backend would drop them anyway. */
  function submit() {
    sent = true;
    // A disabled field is shown, never answered.
    const shown = new Set(steps.flatMap((s) => s.fields.filter((f) => f.disabled_reason === undefined).map((f) => f.name)));
    const out: Values = {};
    for (const [k, v] of Object.entries(values)) if (shown.has(k)) out[k] = v;
    onsubmit(out);
  }

  /** What a successful answer or a decline calls: nothing more to resume. */
  export function forgetSaved() {
    if (saveKey) clearSaved(saveKey);
  }

  const canSaveLater = $derived(!!spec.save_later && !!saveKey);
  let saveFailed = $state(false);
  function saveLater() {
    if (!saveKey) return;
    saveFailed = !saveForLater(saveKey, spec, values);
    if (!saveFailed) onsavelater?.();
  }

  // Step chips: every visible step by its short name; a done one is a way
  // back to it.
  const chipName = (i: number) => steps[i]?.name ?? steps[i]?.title ?? '';
  const review = $derived(step?.kind === 'review' ? reviewSections(spec, values) : []);

  // Spec-level proposals (`options[].proposed`): the agent's likely choice,
  // first and pre-selected while the field is empty, unless the hub's own
  // Jev proposal speaks for the field, AI never decides it, or it is risky.
  let specDismissed = $state<Set<string>>(new Set());
  function specProposal(f: FormField): (OptionSpec & { proposed: NonNullable<OptionSpec['proposed']> }) | null {
    if (f.type !== 'select' || specDismissed.has(f.name) || proposed?.field === f.name || f.disabled_reason !== undefined) return null;
    if (neverDecidesField(f)) return null;
    const o = optionsOfField(f).find((x) => x.proposed);
    if (!o?.proposed || risky(o.label) || risky(o.value)) return null;
    return o as OptionSpec & { proposed: NonNullable<OptionSpec['proposed']> };
  }
  $effect(() => {
    const picks = spec.steps.flatMap((s) => s.fields ?? []).flatMap((f) => {
      const o = specProposal(f);
      return o ? [[f.name, o.value] as const] : [];
    });
    untrack(() => {
      for (const [name, v] of picks) if (values[name] === undefined) set(name, v);
    });
  });
  function dismissSpecProposal(f: FormField) {
    const o = specProposal(f);
    if (o && values[f.name] === o.value) set(f.name, undefined);
    specDismissed = new Set([...specDismissed, f.name]);
  }

  // "Another…": a select whose spec says `other` takes the person's own text.
  // The dropdown's entry for it has a value no option can have (a value is
  // never empty and a spec is JSON text, but no agent writes a NUL).
  const OTHER = '\u0000another';
  let otherOpen = $state<Set<string>>(new Set());
  const isOther = (f: FormField) =>
    !!f.other &&
    (otherOpen.has(f.name) || (typeof values[f.name] === 'string' && !optionsOfField(f).some((o) => o.value === values[f.name])));
  function openOther(f: FormField) {
    otherOpen = new Set([...otherOpen, f.name]);
    if (optionsOfField(f).some((o) => o.value === values[f.name])) set(f.name, undefined);
  }
  function closeOther(f: FormField) {
    const next = new Set(otherOpen);
    next.delete(f.name);
    otherOpen = next;
  }

  /** The drafted line while the value is still the drafted one. */
  const showDrafted = (f: FormField) => !!f.drafted && values[f.name] === f.value;

  export function clearSecrets() {
    const next = { ...values };
    for (const s of spec.steps) for (const f of s.fields ?? []) if (f.type === 'secret') delete next[f.name];
    values = next;
  }

  const off = $derived(busy || disabled);
  const str = (f: FormField) => (typeof values[f.name] === 'string' ? (values[f.name] as string) : '');

  // J5 quick answer: the proposed option goes first and is pre-selected when
  // nothing is chosen yet; "Change" puts the order back and clears it.
  let dismissed = $state(false);
  const proposed = $derived.by(() => {
    if (dismissed || !proposal || preselect(QUICK_ANSWER, proposal) === null) return null;
    const f = spec.steps.flatMap((s) => s.fields ?? []).find((x) => x.name === proposal.field);
    if (!f || neverDecidesField(f)) return null;
    const opt = f.type === 'select' ? optionsOfField(f).find((o) => o.value === proposal.value) : undefined;
    return opt && !risky(opt.label) && !risky(opt.value) ? proposal : null;
  });
  $effect(() => {
    const p = proposed;
    if (!p) return;
    untrack(() => {
      if (values[p.field] === undefined) set(p.field, p.value);
    });
  });
  /** A field's options in the order shown: the proposed one first. */
  function optionsOf(f: FormField): OptionSpec[] {
    const all = optionsOfField(f);
    const first = proposed?.field === f.name ? proposed.value : (specProposal(f)?.value ?? null);
    if (first === null) return all;
    const at = all.findIndex((o) => o.value === first);
    return at < 0 ? all : [all[at], ...all.slice(0, at), ...all.slice(at + 1)];
  }
  function dismissProposal(f: FormField) {
    if (proposed && values[f.name] === proposed.value) set(f.name, undefined);
    dismissed = true;
  }

  /** A choice short enough to number: a select or multiselect of 1–9 options. */
  const numbered = (f: FormField) =>
    (f.type === 'select' || f.type === 'multiselect') && (f.options?.length ?? 0) > 0 && (f.options?.length ?? 0) <= 9;
  /** The field 1–9 answer: the step's only numbered choice, or none. */
  const keyField = $derived.by(() => {
    const choices = step?.fields.filter((f) => numbered(f) && f.disabled_reason === undefined) ?? [];
    return choices.length === 1 ? choices[0] : null;
  });

  function pick(f: FormField, v: string) {
    if (f.type === 'multiselect') {
      const cur = Array.isArray(values[f.name]) ? (values[f.name] as string[]) : [];
      set(f.name, cur.includes(v) ? cur.filter((x) => x !== v) : [...cur, v]);
    } else {
      // Picking the chosen option of an optional choice clears it, as the
      // dropdown's empty entry did.
      set(f.name, values[f.name] === v && !f.required ? undefined : v);
    }
  }

  // 1–9: only while the session view shows, no field, terminal or dialog has
  // the keyboard, and no question card takes the digits first.
  const isMac = detectMac(typeof navigator === 'undefined' ? undefined : navigator);
  const hint = submitHint(isMac);
  function onWindowKeydown(e: KeyboardEvent) {
    const f = keyField;
    if (!f || off || e.defaultPrevented || $destination !== 'session') return;
    if (matchShortcut('form-card', e, isMac) !== 'form-card.option') return;
    // A key sent to the window itself has no element to ask.
    const target = e.target instanceof HTMLElement ? e.target : null;
    if (isEditable(target) || target?.dataset?.imeProxy !== undefined || target?.closest?.('dialog')) return;
    if (document.querySelector('[data-testid="answer-card"]:not(.compact)')) return;
    const o = optionsOf(f)[Number(e.key) - 1];
    if (!o) return;
    e.preventDefault();
    pick(f, o.value);
  }
</script>

<svelte:window onkeydown={onWindowKeydown} />

{#snippet optionText(o: OptionSpec)}
  <span class="opt-text">
    <span>{o.label}</span>
    {#if o.detail}<span class="detail" data-testid={`form-option-detail-${o.value}`}>{o.detail}</span>{/if}
  </span>
{/snippet}

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="wizard" onkeydown={onFormKeydown}>
  {#if steps.length > 1}
    <ol class="chips" data-testid="form-step-chips">
      {#each steps as s, i (s.title)}
        <li>
          {#if i < index}
            <button type="button" class="chip done" data-testid={`form-step-chip-${i}`} disabled={busy} onclick={() => (index = i)}
              ><span aria-hidden="true">✓</span> {chipName(i)}</button>
          {:else}
            <span class="chip" class:here={i === index} data-testid={`form-step-chip-${i}`} aria-current={i === index ? 'step' : undefined}
              >{chipName(i)}</span>
          {/if}
        </li>
      {/each}
    </ol>
    <div class="count" data-testid="form-step-count">Step {index + 1} of {steps.length}</div>
  {/if}
  {#if step}
    <h6 data-testid="form-step-title">{step.title}</h6>
    {#if step.intro}<p class="intro">{step.intro}</p>{/if}
    {#if shownBecause(step.when, spec)}<p class="because" data-testid="form-step-because">{shownBecause(step.when, spec)}</p>{/if}
    {#if step.kind === 'review'}
      <div class="review" data-testid="form-review">
        {#each review as sec (sec.step)}
          <section class="review-step" data-testid={`form-review-step-${sec.step}`}>
            <div class="review-head">
              <strong>{sec.title}</strong>
              <button type="button" class="link" data-testid={`form-review-edit-${sec.step}`} disabled={busy} onclick={() => (index = sec.step)}>Edit</button>
            </div>
            <dl>
              {#each sec.rows as r (r.name)}<dt>{r.label}</dt><dd>{r.text}</dd>{/each}
            </dl>
          </section>
        {/each}
      </div>
    {/if}
    <div class="fields" bind:this={stepEl}>
    {#each step.fields as f (f.name)}
      {@const locked = off || f.disabled_reason !== undefined}
      {@const specPick = specProposal(f)}
      <div class="field" class:disabled={f.disabled_reason !== undefined} onfocusout={() => leave(f.name)}>
        {#if f.type === 'bool'}
          <label class="check">
            <input
              type="checkbox"
              data-testid={`form-field-${f.name}`}
              checked={values[f.name] === true}
              disabled={locked}
              onchange={(e) => set(f.name, (e.currentTarget as HTMLInputElement).checked)} />
            {f.label}
          </label>
        {:else if f.type === 'multiselect'}
          <span class="label">{f.label}</span>
          {#each optionsOfField(f) as o, i (o.value)}
            <label class="check">
              <input
                type="checkbox"
                data-testid={`form-field-${f.name}-${o.value}`}
                checked={Array.isArray(values[f.name]) && (values[f.name] as string[]).includes(o.value)}
                disabled={locked}
                onchange={(e) => {
                  const cur = Array.isArray(values[f.name]) ? (values[f.name] as string[]) : [];
                  set(f.name, (e.currentTarget as HTMLInputElement).checked ? [...cur, o.value] : cur.filter((x) => x !== o.value));
                }} />
              {#if keyField === f}<span class="kbd" aria-hidden="true">{i + 1}</span>{/if}
              {@render optionText(o)}
            </label>
          {/each}
        {:else if f.type === 'select' && numbered(f)}
          <span class="label" id={`${uid}-${f.name}`}>{f.label}{f.required ? ' *' : ''}</span>
          <div class="options" role="radiogroup" aria-labelledby={`${uid}-${f.name}`}>
            {#each optionsOf(f) as o, i (o.value)}
              <button
                type="button"
                role="radio"
                class="opt"
                class:on={values[f.name] === o.value}
                class:ai-pre={((proposed?.field === f.name && proposed.value === o.value) || specPick?.value === o.value) && values[f.name] === o.value}
                aria-checked={values[f.name] === o.value}
                data-testid={`form-field-${f.name}-${o.value}`}
                disabled={locked}
                onclick={() => {
                  closeOther(f);
                  pick(f, o.value);
                }}>
                {#if keyField === f}<span class="kbd" aria-hidden="true">{i + 1}</span>{/if}
                {@render optionText(o)}
              </button>
            {/each}
            {#if f.other}
              <button
                type="button"
                role="radio"
                class="opt"
                class:on={isOther(f)}
                aria-checked={isOther(f)}
                data-testid={`form-field-${f.name}-other`}
                disabled={locked}
                onclick={() => openOther(f)}>Another…</button>
            {/if}
          </div>
          {#if proposed?.field === f.name}
            <ProposedBy
              proposal={proposed}
              field={QUICK_ANSWER}
              testid={`form-proposed-${f.name}`}
              onchange={() => dismissProposal(f)} />
          {:else if specPick}
            <ProposedBy
              proposal={{ value: specPick.value, source: specPick.proposed.by, reason: specPick.proposed.reason }}
              field={f.name}
              stated
              testid={`form-proposed-${f.name}`}
              onchange={() => dismissSpecProposal(f)} />
          {/if}
        {:else}
          <label for={`${uid}-${f.name}`}>
            {f.label}{f.required ? ' *' : ''}
            {#if showDrafted(f)}<DraftedLabel testid={`form-drafted-${f.name}`} />{/if}
          </label>
          {#if f.type === 'select'}
            <select
              id={`${uid}-${f.name}`}
              data-testid={`form-field-${f.name}`}
              class:ai-pre={(proposed?.field === f.name && values[f.name] === proposed.value) || (specPick !== null && values[f.name] === specPick.value)}
              value={isOther(f) ? OTHER : str(f)}
              disabled={locked}
              onchange={(e) => {
                const v = (e.currentTarget as HTMLSelectElement).value;
                if (v === OTHER) openOther(f);
                else {
                  closeOther(f);
                  set(f.name, v || undefined);
                }
              }}>
              <option value="">—</option>
              {#each optionsOf(f) as o (o.value)}<option value={o.value}>{o.label}{o.detail ? ` · ${o.detail}` : ''}</option>{/each}
              {#if f.other}<option value={OTHER}>Another…</option>{/if}
            </select>
            {#if specPick && proposed?.field !== f.name}
              <ProposedBy
                proposal={{ value: specPick.value, source: specPick.proposed.by, reason: specPick.proposed.reason }}
                field={f.name}
                stated
                testid={`form-proposed-${f.name}`}
                onchange={() => dismissSpecProposal(f)} />
            {/if}
          {:else if f.type === 'textarea'}
            <textarea
              id={`${uid}-${f.name}`}
              rows="3"
              placeholder={f.placeholder ?? ''}
              data-testid={`form-field-${f.name}`}
              value={str(f)}
              disabled={locked}
              oninput={(e) => set(f.name, (e.currentTarget as HTMLTextAreaElement).value)}></textarea>
          {:else if f.type === 'number'}
            <input
              id={`${uid}-${f.name}`}
              type="number"
              min={f.min}
              max={f.max}
              step={f.integer ? 1 : 'any'}
              data-testid={`form-field-${f.name}`}
              value={typeof values[f.name] === 'number' ? values[f.name] : ''}
              disabled={locked}
              oninput={(e) => {
                const raw = (e.currentTarget as HTMLInputElement).value;
                set(f.name, raw === '' ? undefined : Number(raw));
              }} />
          {:else}
            <input
              id={`${uid}-${f.name}`}
              type={f.type === 'secret' ? 'password' : 'text'}
              autocomplete="off"
              spellcheck="false"
              placeholder={f.placeholder ?? ''}
              data-testid={`form-field-${f.name}`}
              value={str(f)}
              disabled={locked}
              oninput={(e) => set(f.name, (e.currentTarget as HTMLInputElement).value)} />
          {/if}
        {/if}
        {#if f.type === 'select' && isOther(f)}
          <input
            type="text"
            class="other"
            autocomplete="off"
            maxlength="500"
            placeholder="Another…"
            aria-label={`${f.label}: another`}
            data-testid={`form-field-${f.name}-other-text`}
            value={str(f)}
            disabled={locked}
            oninput={(e) => set(f.name, (e.currentTarget as HTMLInputElement).value || undefined)} />
        {/if}
        {#if showDrafted(f) && f.drafted}
          <span class="help" data-testid={`form-drafted-from-${f.name}`}
            >{#if f.type === 'bool' || f.type === 'multiselect' || (f.type === 'select' && numbered(f))}<DraftedLabel testid={`form-drafted-${f.name}`} />{' '}{/if}by {f.drafted.by} · from {f.drafted.from}</span>
        {/if}
        {#if f.help}<span class="help">{f.help}</span>{/if}
        {#if shownBecause(f.when, spec)}<span class="because" data-testid={`form-because-${f.name}`}>{shownBecause(f.when, spec)}</span>{/if}
        {#if f.type === 'secret' && f.secret_note}<span class="help secret-note" data-testid={`form-secret-note-${f.name}`}>{f.secret_note}</span>{/if}
        {#if f.disabled_reason !== undefined}<span class="reason" data-testid={`form-disabled-${f.name}`}>{f.disabled_reason}</span>{/if}
        {#if problemOf(f.name)}<span class="err" data-testid={`form-problem-${f.name}`}>{problemOf(f.name)}</span>{/if}
      </div>
    {/each}
    </div>
  {/if}
  {#if last && hostWarnings.length}
    <div class="host-warn" role="status" data-testid="form-host-warnings">
      {#each hostWarnings as w (w)}<p data-testid="form-host-warning">{w}</p>{/each}
    </div>
  {/if}
  {#if saveFailed}<span class="err" data-testid="form-save-later-failed">This device could not keep the answers.</span>{/if}
  <div class="row">
    {#if oncancel}
      <button type="button" class="cancel" data-testid="form-cancel" disabled={busy} onclick={oncancel}>Cancel</button>
    {/if}
    {#if canSaveLater}
      <button type="button" class="later" data-testid="form-save-later" disabled={off} onclick={saveLater}>Save and finish later</button>
    {/if}
    {#if index > 0}
      <button type="button" data-testid="form-back" disabled={busy} onclick={() => ((index -= 1), (tried = false))}>Back</button>
    {/if}
    {#if last}
      <button
        type="button"
        class="primary"
        data-testid="form-submit"
        data-shortcut={busy ? undefined : hint.label}
        aria-keyshortcuts={hint.aria}
        disabled={off || !ready}
        onclick={submit}>
        {#if busy && sent}{#if buttonLoader}<Loader name="comet" size={12} class="btn-loader" />{/if}{sending}{:else}{spec.submit ?? 'Submit'}{/if}
      </button>
    {:else}
      <button type="button" class="primary" data-testid="form-next" disabled={off || !ready} onclick={next}>Next</button>
    {/if}
  </div>
  {#if why && !off}<p class="why" data-testid="form-submit-why">{why}</p>{/if}
</div>

<style>
  .wizard { display: flex; flex-direction: column; gap: 0.6rem; }
  .count { font-size: var(--text-2xs); color: var(--fg-muted); }
  h6 { margin: 0; font-size: var(--text-sm); }
  .intro { margin: 0; font-size: var(--text-2xs); color: var(--fg-muted); }
  .host-warn { padding: 0.35rem 0.55rem; border-left: 3px solid var(--usage-warn); background: color-mix(in srgb, var(--usage-warn) 8%, transparent); border-radius: var(--radius-sm); font-size: var(--text-2xs); }
  .host-warn p { margin: 0; }
  .because { margin: 0; font-size: var(--text-2xs); color: var(--fg-muted); font-style: italic; }
  .fields { display: flex; flex-direction: column; gap: 0.6rem; }
  .field { display: flex; flex-direction: column; gap: 0.2rem; }
  [data-shortcut]::after { content: ' ' attr(data-shortcut); opacity: 0.7; font-size: 0.9em; }
  .why { margin: -0.3rem 0 0; text-align: right; font-size: var(--text-2xs); color: var(--fg-muted); }
  label, .label { font-size: var(--text-2xs); }
  .check { display: flex; gap: 0.35rem; align-items: flex-start; }
  input:not([type='checkbox']), select, textarea { font: inherit; font-size: var(--text-2xs); padding: 0.25rem 0.4rem; }
  .help { font-size: var(--text-2xs); color: var(--fg-muted); }
  .err { font-size: var(--text-2xs); color: var(--usage-crit); }
  .row { display: flex; gap: 0.4rem; justify-content: flex-end; }
  .row .cancel { margin-right: auto; }
  .options { display: flex; flex-direction: column; gap: 0.15rem; }
  .opt { display: flex; gap: 0.45rem; align-items: center; text-align: left; font: inherit; font-size: var(--text-2xs); padding: 0.25rem 0.4rem; border: 1px solid transparent; border-radius: var(--radius-sm); background: none; color: inherit; cursor: pointer; }
  .opt:hover:not(:disabled) { background: var(--bg-hover); }
  .opt.on { border-color: var(--accent); background: color-mix(in srgb, var(--accent) 14%, transparent); }
  .chips { display: flex; flex-wrap: wrap; gap: 0.3rem; margin: 0; padding: 0; list-style: none; }
  .chip { display: inline-flex; gap: 0.25rem; align-items: center; font: inherit; font-size: var(--text-2xs); line-height: 16px; padding: 0 0.45rem; border: 1px solid var(--border); border-radius: var(--radius-sm); background: none; color: var(--fg-muted); }
  .chip.here { color: var(--fg); border-color: var(--accent); }
  .chip.done { cursor: pointer; color: var(--fg-2); }
  .chip.done:hover:not(:disabled) { background: var(--bg-hover); }
  .opt-text { display: flex; flex-direction: column; }
  .detail { font-size: var(--text-2xs); color: var(--fg-muted); }
  .field.disabled label, .field.disabled .label { color: var(--fg-muted); }
  .reason { font-size: var(--text-2xs); color: var(--fg-muted); font-style: italic; }
  .secret-note { color: var(--fg-2); }
  .review { display: flex; flex-direction: column; gap: 0.45rem; }
  .review-step { display: flex; flex-direction: column; gap: 0.15rem; padding: 0.35rem 0.5rem; border: 1px solid var(--border); border-radius: var(--radius-sm); font-size: var(--text-2xs); }
  .review-head { display: flex; justify-content: space-between; align-items: baseline; }
  .review dl { display: grid; grid-template-columns: max-content 1fr; gap: 0.1rem 0.6rem; margin: 0; }
  .review dt { color: var(--fg-muted); }
  .review dd { margin: 0; overflow-wrap: anywhere; }
  .link { background: none; border: none; padding: 0; color: var(--accent); cursor: pointer; font: inherit; font-size: var(--text-2xs); }
  .link:hover:not(:disabled) { text-decoration: underline; }
  .row .later { margin-right: auto; }
  .kbd { font-size: var(--text-2xs); line-height: 1; padding: 0.1rem 0.3rem; border: 1px solid var(--border); border-radius: var(--radius-xs); color: var(--fg-muted); }
</style>
