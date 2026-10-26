<script lang="ts">
  // The routine form (FormsAutomation board, "New routine"; the same form
  // edits one): a header that names it, then the body in the order a
  // routine is read: its name and how it starts, when, where it runs, the
  // prompt, its limits, then the dry run with Run once now. The footer
  // keeps "Use a template…" on the left, Cancel and the verb on the right,
  // and says under a disabled verb what is missing (FormsAnatomy rules).
  // ⌘↵ saves, Esc cancels; a changed form asks "Discard changes?" once.
  //
  // "When" is one panel that grows with what was picked: the schedule
  // picker, the event fields (RoutineEventTrigger, whose list of events
  // may grow) or a line for Run now only.
  import { hosts } from '../hosts';
  import { projects } from '../projects';
  import { accountByUuid } from '../accounts';
  import type { IpcError } from '../result';
  import Button from '../kit/Button.svelte';
  import Kbd from '../kit/Kbd.svelte';
  import FormBanner from '../forms/FormBanner.svelte';
  import DiscardAsk from '../forms/DiscardAsk.svelte';
  import { CloseGuard } from '../forms/close_guard.svelte';
  import NewRoutineMenu from './NewRoutineMenu.svelte';
  import RoutineEventTrigger from './RoutineEventTrigger.svelte';
  import RoutineMemory from './RoutineMemory.svelte';
  import RoutineSeen from './RoutineSeen.svelte';
  import {
    AUTONOMY_CHOICES,
    budgetProblem,
    capProblem,
    clockChange,
    deviceZone,
    dryRunLine,
    isCronProblem,
    nextRunLabel,
    offsetWords,
    previewRoutine,
    routineAccountLabel,
    routineMemory,
    routineSeen,
    SCHEDULE_DAYS,
    triggerWords,
    zoneChoices,
    zoneOffsetMin,
    type RoutineAccount,
    type RoutinePreview,
    type RoutineTrigger,
  } from '../routines';
  import { draftCron, draftInput, draftMissing, draftZone, withDays, type RoutineDraft } from './routine_draft';
  import { errorText } from '../error_copy';
  import type { ScheduleDays } from '../routines';
  import { onMount, tick, untrack } from 'svelte';
  import PromptTune from '../PromptTune.svelte';
  import { tuneRoutinePrompt } from '../prompt_tune';
  import { helpModel } from '../context_help';
  import { fleetSettings, settingBool, SETTING_KEYS } from '../fleet_settings';

  let {
    draft = $bindable(),
    enabled = true,
    dirty = false,
    busy = false,
    error = null,
    onsave,
    onrunonce,
    oncancel,
    ontemplate,
  }: {
    draft: RoutineDraft;
    /** The saved routine's switch; a new one starts on. */
    enabled?: boolean;
    /** The draft differs from what was opened. */
    dirty?: boolean;
    busy?: boolean;
    /** Saving failed: the banner at the top of the body; input kept. */
    error?: IpcError | string | null;
    onsave: () => void;
    onrunonce: () => void;
    oncancel: () => void;
    /** "Use a template…": start over from a template (a new routine only). */
    ontemplate?: (template: string) => void;
  } = $props();

  const STARTS: { id: RoutineTrigger; label: string }[] = [
    { id: 'cron', label: 'On a schedule' },
    { id: 'event', label: 'On an event' },
    { id: 'manual', label: 'Only with Run now' },
  ];

  const isNew = $derived(draft.id === undefined);
  /** A notebook box holds text not saved yet (RoutineMemory): leaving asks,
   *  as for a changed form. */
  let notebookPending = $state(false);
  /** Anything here that leaving would lose. */
  const unsaved = $derived(dirty || notebookPending);
  /** Whether the fleet keeps notebooks (migration 204): a hub that predates
   *  them answers the `memory` action E_INVALID and refuses a routine that
   *  names one, so the form offers none. `null` until it answers. */
  let memoryServed: boolean | null = $state(null);
  onMount(() => {
    // Routine 0 is never one: a hub with notebooks answers E_NOT_FOUND.
    void routineMemory(0).then((r) => (memoryServed = r.ok || r.error.code !== 'E_INVALID'));
    void routineSeen(0).then((r) => (seenServed = r.ok || r.error.code !== 'E_INVALID'));
  });
  /** Whether the fleet keeps seen items (migration 212), asked the same way;
   *  a saved routine's row says so itself (`track_seen`). */
  let seenServed: boolean | null = $state(null);
  const seenOffered = $derived(draft.track_seen !== undefined || (isNew && seenServed === true));
  // The prompt tuner (`prompt_tune.ts`): a model rewrites the prompt on the
  // routine's host and profile, saved or not; "Use this" writes draft.prompt.
  const tuneOn = $derived(settingBool($fleetSettings, SETTING_KEYS.workPromptTune));
  let tuneOpen = $state(false);
  let promptBox: HTMLTextAreaElement | undefined = $state();
  // One editor serves every draft RoutinesPanel opens (a template picked
  // while it is open replaces `draft`): a tuner left open would write the
  // old draft's rewrite into the new one.
  let tunedDraft: RoutineDraft | null = null;
  $effect.pre(() => {
    const d = draft;
    if (d !== tunedDraft) {
      tunedDraft = d;
      untrack(() => (tuneOpen = false));
    }
  });

  /** Close the tuner and put focus back on the prompt it was tuning. */
  async function closeTune() {
    tuneOpen = false;
    await tick();
    promptBox?.focus();
  }
  const tuneWhy = $derived(draft.host_alias ? null : 'Pick a host first: the tune runs there');
  const visibleHosts = $derived($hosts.filter((h) => !h.hidden));
  const pickable = $derived($projects.filter((p) => !p.project.system));
  /** The projects a routine may name; undefined until they are read. */
  const projectIds = $derived($projects.length ? $projects.map((p) => p.project.id) : undefined);
  // The saved host, project or fallback may be one the selects no longer
  // offer (hidden, removed, deleted): shown as such, never as a blank
  // (review w05-11).
  const hostRow = (alias: string) => $hosts.find((h) => h.alias === alias);
  const hostOrg = $derived(hostRow(draft.host_alias)?.org_id ?? null);
  /** Fallback hosts: visible ones in the routine host's organisation, as the hub takes. */
  const fallbackHosts = $derived(visibleHosts.filter((h) => h.alias !== draft.host_alias && (h.org_id ?? null) === hostOrg));
  const whyOff = (alias: string) => {
    const h = hostRow(alias);
    return !h ? 'removed' : h.hidden ? 'hidden' : 'another organisation';
  };
  const projectGone = $derived(draft.project_id !== '' && !pickable.some((p) => String(p.project.id) === draft.project_id));
  /** The prompt's limit (the hub's BRIEF_MAX_CHARS), and when to count. */
  const PROMPT_MAX = 4000;
  const PROMPT_COUNT_FROM = 3500;
  const projectName = (id: number) => {
    const p = $projects.find((x) => x.project.id === id)?.project;
    return p ? `${p.owner}/${p.repo}` : `project ${id}`;
  };

  const zone = deviceZone();
  const zoneOf = (d: RoutineDraft) => draftZone(d, zone);
  const cron = $derived(draftCron(draft));
  const missing = $derived(draftMissing(draft, projectIds));

  // ---- the dry run (gap plan G2.3): `routines { preview }` of the draft ----

  let preview = $state<RoutinePreview | null>(null);
  /** The dry run's own failure (hub gone, no right to change it): said,
   *  with Retry, not "checking…" forever (review w05-9). */
  let previewError = $state<IpcError | null>(null);
  let previewTry = $state(0);
  let previewSeq = 0;
  $effect(() => {
    const input = draftInput(draft, enabled, zone);
    const id = draft.id;
    void previewTry;
    const seq = ++previewSeq;
    const t = setTimeout(async () => {
      const r = await previewRoutine(input, id);
      if (seq !== previewSeq) return;
      preview = r.ok ? r.value : null;
      previewError = r.ok ? null : r.error;
    }, 250);
    return () => clearTimeout(t);
  });

  /** The Account picker's value: the login the profile names, else `?`. */
  const accountPick = $derived(
    !preview ? '' : preview.logins.some((l) => (l.profile ?? '') === draft.profile.trim()) ? draft.profile.trim() : '?',
  );
  /** The account a login bills, by its nickname or email, not the profile. */
  const accountName = (l: RoutineAccount) => routineAccountLabel({ ...l, profile: null }, $accountByUuid.get(l.account_uuid));
  const loginLabel = (l: RoutineAccount) => `${accountName(l)} · ${l.profile ? `profile ${l.profile}` : 'own login'}`;
  /** The option's full words, as its title (the select clips them). */
  const loginTitle = (l: RoutineAccount) => `${accountName(l)} · ${l.profile ? `profile ${l.profile}` : "the host's own login"}`;

  const moved = $derived(preview && draft.trigger === 'cron' ? clockChange(preview.next_runs, preview.utc_offset_min, zoneOf(draft)) : null);

  const dryLine = $derived.by(() => {
    if (!preview) return previewError ? `Dry run unavailable: ${errorText(previewError)}.` : 'Dry run: checking what a run would do…';
    if (preview.problem) return `Dry run: it would not save: ${preview.problem}.`;
    const inp = draftInput(draft, enabled, zone);
    return dryRunLine(inp, projectName(inp.project_id), preview.account ? accountName(preview.account) : null);
  });

  // ---- checks on blur and on submit, never on each key -----------------------

  let touched = $state<Record<string, boolean>>({});
  const blur = (name: string) => () => (touched[name] = true);
  const nameError = $derived(touched.name && draft.name.trim() === '' ? 'A routine needs a name.' : null);
  const promptError = $derived(touched.prompt && draft.prompt.trim() === '' ? 'Write what each run should do.' : null);
  // A budget or cap the form cannot read is said, never saved as none
  // (review w05-5, w05-6).
  const budgetRunError = $derived(touched.budget_run ? budgetProblem(draft.budget_run) : null);
  const budgetDayError = $derived(touched.budget_day ? budgetProblem(draft.budget_day) : null);
  const capError = $derived(touched.cap ? capProblem(draft.cap_min) : null);
  /** A cron line the dry run cannot read, said under it (review w05-8). */
  const cronError = $derived(draft.trigger === 'cron' && draft.days === 'custom' && isCronProblem(preview?.problem) ? preview!.problem! : null);
  /** "never runs" only for a line that reads but has no next fire. */
  const neverRuns = $derived(!!preview && !!cron && !preview.next_runs.length && (!preview.problem || /never fires$/.test(preview.problem)));

  // ---- saving, closing -------------------------------------------------------

  /** Where to go once a changed form is let go, when it is not Cancel:
   *  another routine, a template, a request from elsewhere (review w05-1). */
  let leaveTo: (() => void) | null = null;
  const guard = new CloseGuard(
    () => unsaved,
    () => {
      const then = leaveTo;
      leaveTo = null;
      (then ?? oncancel)();
    },
  );
  function keepEditing() {
    leaveTo = null;
    guard.keep();
  }

  /** Leave this draft for `then`: at once when it is unchanged, else after
   *  "Discard changes?" in the footer. RoutinesPanel sends every way out
   *  that replaces the draft through here. */
  /** Whether a notebook box holds unsaved text, for a Save that would
   *  otherwise close the editor over it. */
  export function hasNotebookEdit(): boolean {
    return notebookPending;
  }

  export function askLeave(then: () => void): void {
    if (!unsaved) {
      leaveTo = null;
      guard.keep();
      then();
      return;
    }
    leaveTo = then;
    guard.asking = true;
  }

  // ---- focus (review w05-12) and a refusal kept in sight (review w05-4) -----

  let nameBox: HTMLInputElement | undefined = $state();
  let body: HTMLDivElement | undefined = $state();
  let bannerBox: HTMLDivElement | undefined = $state();
  let focusedDraft: RoutineDraft | null = null;
  $effect(() => {
    const d = draft;
    if (d === focusedDraft) return;
    focusedDraft = d;
    // A template fills the name: the prompt is what is left to write.
    untrack(() => void tick().then(() => (d.id === undefined && d.name.trim() !== '' ? promptBox : nameBox)?.focus()));
  });
  $effect(() => {
    if (!error) return;
    untrack(() =>
      void tick().then(() => {
        if (body) body.scrollTop = 0;
        bannerBox?.focus();
      }),
    );
  });

  function submit(e?: Event) {
    e?.preventDefault();
    touched = { name: true, prompt: true, budget_run: true, budget_day: true, cap: true };
    if (missing || busy) return;
    onsave();
  }

  function keydown(e: KeyboardEvent) {
    if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) {
      submit(e);
    } else if (e.key === 'Escape' && !e.defaultPrevented) {
      e.preventDefault();
      guard.request();
    }
  }
</script>

<!-- ⌘↵ and Esc are the form's own keys (FormsAnatomy, Dialog anatomy 5). -->
<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<form class="editor" data-testid="routine-editor" aria-labelledby="routine-editor-title" onsubmit={submit} onkeydown={keydown}>
  <header class="dh">
    <h3 id="routine-editor-title">{isNew ? 'New routine' : `Edit ${draft.name || 'routine'}`}</h3>
    <p class="lead">Starts a session on a schedule or an event. Its runs land in Runs and in Inbox.</p>
  </header>

  <div class="db" bind:this={body}>
    {#if error}
      <div class="banner-box" tabindex="-1" bind:this={bannerBox}><FormBanner {error} testid="routine-save-error" /></div>
    {/if}

    <div class="two">
      <label class="f">
        <span class="fl">Name</span>
        <input
          class="inp"
          class:bad={nameError}
          data-testid="routine-name"
          bind:this={nameBox}
          bind:value={draft.name}
          onblur={blur('name')}
          maxlength="80"
          placeholder="e.g. Morning PR sweep"
          aria-invalid={nameError ? 'true' : undefined} />
        {#if nameError}<span class="errt">{nameError}</span>{/if}
      </label>
      <div class="f">
        <span class="fl" id="routine-starts">Starts</span>
        <div class="seg" role="radiogroup" aria-labelledby="routine-starts" data-testid="routine-trigger">
          {#each STARTS as s (s.id)}
            <label class="opt" class:on={draft.trigger === s.id}
              ><input type="radio" name="routine-trigger" value={s.id} bind:group={draft.trigger} data-testid={`routine-trigger-${s.id}`} />{s.label}</label
            >
          {/each}
        </div>
      </div>
    </div>

    <section class="when" aria-label="When it runs" data-trigger={draft.trigger}>
      {#if draft.trigger === 'cron'}
        <div class="three">
          <label class="f">
            <span class="fl">Repeats</span>
            <select
              class="inp"
              data-testid="routine-days"
              value={draft.days}
              onchange={(e) => withDays(draft, (e.currentTarget as HTMLSelectElement).value as ScheduleDays)}>
              {#each SCHEDULE_DAYS as d (d.id)}<option value={d.id}>{d.label}</option>{/each}
            </select>
          </label>
          {#if draft.days === 'custom'}
            <label class="f">
              <span class="fl">Cron line</span>
              <input
                class="inp mono"
                class:bad={cronError}
                data-testid="routine-cron"
                bind:value={draft.cron}
                placeholder="30 7 * * 1-5"
                spellcheck="false"
                aria-invalid={cronError ? 'true' : undefined} />
              {#if cronError}<span class="errt" data-testid="routine-cron-error">{cronError}</span>{/if}
            </label>
          {:else if draft.days === 'hourly'}
            <label class="f">
              <span class="fl">At minute</span>
              <input class="inp" data-testid="routine-time" bind:value={draft.time} placeholder="00" inputmode="numeric" />
            </label>
          {:else}
            <label class="f">
              <span class="fl">At</span>
              <input class="inp" data-testid="routine-time" type="time" bind:value={draft.time} />
            </label>
          {/if}
          <label class="f">
            <span class="fl">Time zone</span>
            <select class="inp" data-testid="routine-zone-select" bind:value={draft.zone}>
              <option value="">{zone ? `${zone} (this device)` : 'This device'}</option>
              {#each zoneChoices(draft.zone) as z (z)}{#if z !== zone || draft.zone === z}<option value={z}>{z}</option>{/if}{/each}
            </select>
          </label>
        </div>
        <p class="help" data-testid="routine-next-run">
          {triggerWords({ trigger: 'cron', cron })}{#if preview?.next_runs.length}{' · next run '}{nextRunLabel(preview.next_runs[0], zoneOf(draft))}{:else if neverRuns}{' · never runs'}{/if}{#if draft.days === 'custom'}{' · minute hour day month weekday'}{/if}
        </p>
        <p class="help" data-testid="routine-zone">
          Time zone: {zoneOf(draft) ?? 'this device'}{!draft.zone && zone ? ' (this device)' : ''} · saved as {offsetWords(zoneOffsetMin(Date.now() / 1000, zoneOf(draft)))}
        </p>
        {#if draft.saved_offset !== undefined && !draft.zone}
          <p class="bnr w" data-testid="routine-saved-offset">
            It was saved at {offsetWords(draft.saved_offset)} without a time zone. Saving reads its time in {zoneOf(draft) ?? 'this device’s zone'}
            ({offsetWords(zoneOffsetMin(Date.now() / 1000, zoneOf(draft)))}), so it would run at another hour: pick the zone it is meant for.
          </p>
        {/if}
        {#if moved}
          <p class="bnr w" data-testid="routine-clock-change">
            From {nextRunLabel(moved.at, zoneOf(draft))} it runs {Math.abs(moved.shiftMin) === 60 ? 'an hour' : `${Math.abs(moved.shiftMin)} minutes`}
            {moved.shiftMin > 0 ? 'later' : 'earlier'} on your clock: the clocks change and a routine keeps the UTC offset it was saved at. Save it again
            after the change.
          </p>
        {/if}
      {:else if draft.trigger === 'event'}
        <div class="event">
          <RoutineEventTrigger bind:event={draft.event} bind:filter={draft.filter} routineHost={draft.host_alias} repos={pickable.map((p) => `${p.project.owner}/${p.project.repo}`)} />
        </div>
      {:else}
        <p class="help" data-testid="routine-manual-note">It never starts on its own: Run now starts it, here or from the phone.</p>
      {/if}
    </section>

    <div class="three">
      <label class="f">
        <span class="fl">Host</span>
        <select class="inp" data-testid="routine-host" bind:value={draft.host_alias}>
          {#each visibleHosts as h (h.alias)}<option value={h.alias}>{h.alias}</option>{/each}
          {#if draft.host_alias && !visibleHosts.some((h) => h.alias === draft.host_alias)}<option value={draft.host_alias}
              >{draft.host_alias} ({whyOff(draft.host_alias)})</option
            >{/if}
        </select>
      </label>
      <label class="f">
        <span class="fl">Project</span>
        <select class="inp" class:bad={projectGone} data-testid="routine-project" bind:value={draft.project_id}>
          {#each pickable as p (p.project.id)}<option value={String(p.project.id)}>{p.project.owner}/{p.project.repo}</option>{/each}
          {#if projectGone}<option value={draft.project_id}
              >{projectName(Number(draft.project_id))} ({$projects.some((p) => String(p.project.id) === draft.project_id) ? 'not offered' : 'deleted'})</option
            >{/if}
        </select>
      </label>
      <label class="f">
        <span class="fl">Account</span>
        <select
          class="inp"
          data-testid="routine-account"
          value={accountPick}
          onchange={(e) => {
            const v = (e.currentTarget as HTMLSelectElement).value;
            if (v !== '?') draft.profile = v;
          }}>
          {#each preview?.logins ?? [] as l (l.profile ?? '')}<option value={l.profile ?? ''} title={loginTitle(l)}>{loginLabel(l)}</option>{/each}
          {#if accountPick === '?' || !preview}<option value="?"
              >{draft.profile.trim() ? `Profile ${draft.profile.trim()} · no known account yet` : "The host's own login · no known account yet"}</option
            >{/if}
        </select>
      </label>
    </div>

    <label class="f">
      <span class="fl">Prompt</span>
      <textarea
        class="inp prompt"
        class:bad={promptError}
        data-testid="routine-prompt"
        rows="6"
        bind:this={promptBox}
        bind:value={draft.prompt}
        onblur={blur('prompt')}
        maxlength={PROMPT_MAX}
        placeholder="What each run should do, e.g. go through my open PRs and fix what is small."
        aria-invalid={promptError ? 'true' : undefined}></textarea>
      {#if promptError}<span class="errt">{promptError}</span>{:else}<span class="help">Each run opens a new session with this prompt, then the autonomy line under it.</span>{/if}
      {#if draft.prompt.length > PROMPT_COUNT_FROM}<span class="help tnum" data-testid="routine-prompt-count">{draft.prompt.length} / {PROMPT_MAX} characters</span>{/if}
    </label>
    <!-- The prompt tuner (PromptTune) mounts here, under the prompt; its
         accept writes draft.prompt. -->
    {#if tuneOn}
      {#if tuneOpen}
        <PromptTune
          model={helpModel($fleetSettings)}
          draft={draft.prompt}
          tune={(ask, from) => tuneRoutinePrompt(draftInput(draft, enabled, deviceZone()), ask, from)}
          testid="routine-tune"
          onaccept={(p) => (draft.prompt = p)}
          onclose={closeTune}
        />
      {:else}
        <div>
          <Button
            size="sm"
            testid="routine-tune-open"
            disabled={tuneWhy !== null || draft.prompt.trim() === ''}
            title={tuneWhy ??
              (draft.prompt.trim() === ''
                ? 'Write a prompt first: Tune rewrites it'
                : `Tune: ${helpModel($fleetSettings)} rewrites the prompt into a clearer one. You choose whether to use it.`)}
            onclick={() => (tuneOpen = true)}>✦ Tune prompt</Button
          >
        </div>
      {/if}
    {/if}

    <h4 class="grp">Limits</h4>
    <div class="three">
      <label class="f">
        <span class="fl">Budget per run</span>
        <span class="affix"
          ><span class="pre" aria-hidden="true">$</span><input
            class="inp"
            class:bad={budgetRunError}
            data-testid="routine-budget-run"
            bind:value={draft.budget_run}
            onblur={blur('budget_run')}
            placeholder="no limit"
            inputmode="decimal"
            aria-invalid={budgetRunError ? 'true' : undefined} /></span
        >
        {#if budgetRunError}<span class="errt" data-testid="routine-budget-run-error">{budgetRunError}</span>{/if}
      </label>
      <label class="f">
        <span class="fl">Budget per day</span>
        <span class="affix"
          ><span class="pre" aria-hidden="true">$</span><input
            class="inp"
            class:bad={budgetDayError}
            data-testid="routine-budget-day"
            bind:value={draft.budget_day}
            onblur={blur('budget_day')}
            placeholder="no limit"
            inputmode="decimal"
            aria-invalid={budgetDayError ? 'true' : undefined} /></span
        >
        {#if budgetDayError}<span class="errt" data-testid="routine-budget-day-error">{budgetDayError}</span>{/if}
      </label>
      <label class="f">
        <span class="fl">Time cap</span>
        <span class="affix"
          ><input
            class="inp"
            class:bad={capError}
            data-testid="routine-cap"
            bind:value={draft.cap_min}
            onblur={blur('cap')}
            placeholder="no cap"
            inputmode="numeric"
            aria-invalid={capError ? 'true' : undefined} /><span class="post" aria-hidden="true">min</span></span
        >
        {#if capError}<span class="errt" data-testid="routine-cap-error">{capError}</span>{/if}
      </label>
    </div>
    <div class="three">
      <label class="f">
        <span class="fl">If the last run is still going</span>
        <select class="inp" data-testid="routine-overlap" bind:value={draft.overlap}>
          <option value="skip">Skip this run</option>
          <option value="parallel">Run alongside it</option>
        </select>
      </label>
      <label class="f">
        <span class="fl">Fallback host<span class="opt-l">optional</span></span>
        <select class="inp" data-testid="routine-fallback" bind:value={draft.fallback_host}>
          <option value="">None</option>
          {#each fallbackHosts as h (h.alias)}<option value={h.alias}>{h.alias}</option>{/each}
          {#if draft.fallback_host && !fallbackHosts.some((h) => h.alias === draft.fallback_host)}<option value={draft.fallback_host}
              >{draft.fallback_host} ({whyOff(draft.fallback_host)})</option
            >{/if}
        </select>
      </label>
      <label class="f">
        <span class="fl">Autonomy</span>
        <select class="inp" data-testid="routine-autonomy" bind:value={draft.autonomy}>
          {#each AUTONOMY_CHOICES as c (c.level)}<option value={c.level} title={c.label}>{c.short}</option>{/each}
        </select>
      </label>
    </div>
    <label class="ck">
      <input type="checkbox" data-testid="routine-retry-once" bind:checked={draft.retry_once} />
      <span>On failure, retry once before it goes to the Inbox</span>
    </label>
    <p class="help">
      Over its budget or past its time cap, a run is stopped and fails. The fallback host takes a run when its host is unreachable, its login is past
      the line, or its start fails. Autonomy is told to the agent under the prompt: sessions run without permission prompts, so it is an instruction,
      not a sandbox.
    </p>

    {#if memoryServed !== false || draft.memory}
      <h4 class="grp">Memory</h4>
      <div class="two">
        <label class="f">
          <span class="fl">Notebooks</span>
          <select
            class="inp"
            data-testid="routine-memory-mode"
            bind:value={draft.memory}
            onchange={() => {
              if (!draft.memory) draft.memory_auto = false;
            }}>
            <option value="">None: each run starts fresh</option>
            <option value="routine">This routine's own</option>
            <option value="project">The project's, shared by its routines</option>
            <option value="both">Both</option>
          </select>
        </label>
        <label class="ck">
          <input type="checkbox" data-testid="routine-memory-auto" bind:checked={draft.memory_auto} disabled={!draft.memory} />
          <span>Note each finished run in the notebook</span>
        </label>
      </div>
      <p class="help">
        The fleet keeps notebooks in its own database, outside the repo and the host, so the next run reads them on any host. A run rewrites one by
        ending its reply with a &lt;routine-memory&gt; block; the prompt tells it how.
      </p>
      {#if draft.id !== undefined && (draft.memory || dirty)}
        <!-- One per routine: switching to another starts from its own notebooks. -->
        {#key draft.id}
          <RoutineMemory routineId={draft.id} locked={dirty} onpending={(p) => (notebookPending = p)} />
        {/key}
      {/if}
    {:else}
      <h4 class="grp">Memory</h4>
      <p class="help" data-testid="routine-memory-unserved">This fleet's hub keeps no notebooks yet: update it to give a routine a memory.</p>
    {/if}
    {#if seenOffered}
      <label class="ck">
        <input
          type="checkbox"
          data-testid="routine-track-seen"
          checked={draft.track_seen ?? true}
          onchange={(e) => (draft.track_seen = (e.currentTarget as HTMLInputElement).checked)} />
        <span>Remember what its runs already handled (pull requests, issues) and skip what did not change</span>
      </label>
      {#if draft.id !== undefined && (draft.track_seen ?? true)}
        {#key draft.id}
          <RoutineSeen routineId={draft.id} />
        {/key}
      {/if}
    {/if}

    <label class="f">
      <span class="fl">Profile<span class="opt-l">optional</span></span>
      <input class="inp mono" data-testid="routine-profile" bind:value={draft.profile} placeholder="the host's own" spellcheck="false" />
      <span class="help">A claude profile on the host (~/.claude-profiles), for routines that need their own settings; picking an Account fills it.</span>
    </label>

    <div class="bnr" class:i={!preview?.problem && !previewError} class:f={!!preview?.problem || (!preview && !!previewError)} role="status">
      <span class="dry" data-testid="routine-dry-run">{dryLine}</span>
      {#if !preview && previewError}
        <Button size="sm" variant="quiet" testid="routine-dry-run-retry" onclick={() => previewTry++}>Retry</Button>
      {/if}
      <Button size="sm" testid="routine-run-once" disabled={!!missing || busy || !!preview?.problem} onclick={onrunonce}
        >{isNew || dirty ? 'Save and run once' : 'Run once now'}</Button
      >
    </div>
  </div>

  <footer class="df">
    <div class="df-row">
    {#if guard.asking}
      <DiscardAsk onkeep={keepEditing} ondiscard={guard.discard} />
    {:else}
      {#if isNew && ontemplate}
        <NewRoutineMenu up label="Use a template…" testid="routine-use-template" onpick={ontemplate} />
      {/if}
      <span class="sp"></span>
      {#if missing}<span class="why" data-testid="routine-save-why">{missing}</span
        >{:else if error}<span class="why" data-testid="routine-save-why">Not saved: the reason is at the top of the form.</span>{/if}
      <Button variant="quiet" testid="routine-cancel" onclick={guard.request}>Cancel</Button>
      <Button variant="primary" type="submit" testid="routine-save" disabled={!!missing} {busy} busyLabel="Saving"
        >{isNew ? 'Create routine' : 'Save'}</Button
      >
      <span class="keys" aria-hidden="true"><Kbd chord="⌘↵" /></span>
    {/if}
    </div>
  </footer>
</form>

<style>
  .editor { flex: 1; min-width: 0; min-height: 0; display: flex; flex-direction: column; background: var(--bg); }
  .dh > * { max-width: 760px; }
  .dh { padding: var(--space-4) var(--space-6) var(--space-3); border-bottom: 1px solid var(--border); display: flex; flex-direction: column; gap: 4px; }
  .dh h3 { margin: 0; font-size: var(--text-lg); line-height: 24px; font-weight: 600; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .lead { margin: 0; font-size: var(--text-xs); line-height: 16px; color: var(--fg-muted); }
  .db { flex: 1 1 auto; min-height: 0; overflow: auto; padding: var(--space-4) var(--space-6); display: flex; flex-direction: column; gap: 12px; }
  .db > * { max-width: 760px; width: 100%; box-sizing: border-box; }
  .df { padding: 10px var(--space-6); border-top: 1px solid var(--border); background: var(--bg-pane); }
  .df-row { max-width: 760px; display: flex; gap: var(--space-2); align-items: center; flex-wrap: wrap; }
  .df .sp { flex: 1 1 auto; }
  .why { font-size: var(--text-xs); color: var(--fg-muted); }
  .keys { display: inline-flex; }

  .two { display: grid; grid-template-columns: repeat(auto-fit, minmax(240px, 1fr)); gap: 10px; }
  .three { display: grid; grid-template-columns: repeat(auto-fit, minmax(180px, 1fr)); gap: 10px; }
  .f { display: flex; flex-direction: column; gap: 4px; min-width: 0; font-size: var(--text-xs); line-height: 16px; color: var(--fg-2); }
  .fl { display: flex; align-items: baseline; gap: 6px; }
  .opt-l { color: var(--fg-muted); font-size: var(--text-2xs); }
  .help { margin: 0; font-size: var(--text-xs); line-height: 16px; color: var(--fg-muted); }
  .errt { font-size: var(--text-xs); line-height: 16px; color: var(--status-failed); }
  .grp { margin: 6px 0 -2px; font-size: var(--text-2xs); font-weight: 500; color: var(--fg-muted); }

  /* Fields (FormsAnatomy): 28 px, the pane colour, the house ring on focus (review w19-3:
     the old --accent-soft halo measured ~1.2:1 and matched the selected Starts option). */
  .inp { height: 28px; box-sizing: border-box; width: 100%; min-width: 0; padding: 0 8px; border: 1px solid var(--control-border-strong, var(--border)); border-radius: var(--radius-sm); background: var(--bg-pane); color: var(--fg); font: inherit; font-size: var(--text-sm); }
  .inp:focus { border-color: var(--accent); outline: var(--ring-w) solid var(--ring); outline-offset: 1px; }
  .inp.bad { border-color: var(--status-failed); }
  .inp.mono { font-family: var(--font-mono); font-size: var(--text-xs); }
  textarea.inp { height: auto; padding: 6px 8px; line-height: 18px; resize: vertical; min-height: 96px; }
  select.inp { padding-right: 4px; }
  .affix { position: relative; display: flex; align-items: center; }
  .affix .pre, .affix .post { position: absolute; color: var(--fg-muted); font-size: var(--text-xs); pointer-events: none; }
  .affix .pre { left: 8px; }
  .affix .post { right: 8px; }
  .affix .pre + .inp { padding-left: 20px; }
  .affix .inp:has(+ .post) { padding-right: 34px; }

  /* Starts: a segmented choice of radios. */
  .seg { display: flex; gap: 6px; flex-wrap: wrap; }
  .opt { height: 28px; box-sizing: border-box; padding: 0 10px; border-radius: var(--radius-sm); border: 1px solid var(--control-border-strong, var(--border)); display: inline-flex; align-items: center; color: var(--fg-2); font-size: var(--text-xs); cursor: pointer; }
  .opt:hover { background: var(--bg-hover); }
  .opt.on { border-color: var(--accent); background: var(--accent-soft); color: var(--fg); }
  .opt input { position: absolute; opacity: 0; width: 1px; height: 1px; pointer-events: none; }
  .opt:has(input:focus-visible) { outline: var(--ring-w) solid var(--ring); outline-offset: 1px; }

  /* When: one panel, whatever was picked; the event list may grow. */
  .when { display: flex; flex-direction: column; gap: 8px; padding: 12px; border: 1px solid var(--border); border-radius: var(--radius-md); background: var(--bg-sunk, var(--bg-pane)); }
  .event { display: flex; flex-direction: column; gap: 10px; }
  .event :global(label) { color: var(--fg-2); font-size: var(--text-xs); }
  .event :global(select), .event :global(input) { height: 28px; box-sizing: border-box; width: 100%; padding: 0 8px; border: 1px solid var(--control-border-strong, var(--border)); border-radius: var(--radius-sm); background: var(--bg-pane); color: var(--fg); font: inherit; font-size: var(--text-sm); }
  .event :global(select:focus), .event :global(input:focus) { border-color: var(--accent); outline: var(--ring-w) solid var(--ring); outline-offset: 1px; }

  .ck { display: flex; align-items: flex-start; gap: 8px; font-size: var(--text-sm); color: var(--fg); }
  .ck input { accent-color: var(--accent); margin: 2px 0 0; }

  .bnr { margin: 0; border-radius: var(--radius-md); padding: 8px 10px; font-size: var(--text-xs); line-height: 16px; display: flex; gap: 10px; align-items: center; border: 1px solid var(--border); color: var(--fg); }
  .bnr.i { background: var(--accent-soft); border-color: color-mix(in srgb, var(--accent) 35%, transparent); }
  .bnr.f { background: var(--failed-soft); border-color: var(--failed-line); }
  .bnr.w { background: var(--waiting-faint); border-color: var(--waiting-line); display: block; }
  .dry { flex: 1 1 auto; min-width: 0; }
</style>
