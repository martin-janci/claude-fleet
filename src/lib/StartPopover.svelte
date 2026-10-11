<script lang="ts">
  import Icon from './kit/Icon.svelte';
  // The start popover (task → session spec §2.2): where a start of this task
  // would land — repository, host, branch, brief — and what is in the way,
  // each conflict with its choice, before anything is made. Every change of
  // a choice re-reads the preview, so what Start does is what is shown.
  // Ticket text (the brief) renders as text.
  import { onDestroy, onMount, tick } from 'svelte';
  import { get } from 'svelte/store';
  import { sessions, type SessionRow } from './sessions';
  import { selectSessionExplicitly } from './selection';
  import { decideWorkProposal } from './work';
  import { readErrorText } from './work_view';
  import ProposedBy from './ProposedBy.svelte';
  import DraftField from './DraftField.svelte';
  import { startWork, type StartWorkArgs } from './trackers';
  import { acceptStartRule, dismissStartRule, ruleLine } from './start_rules';
  import { fleetSettings, settingBool, SETTING_KEYS } from './fleet_settings';
  import {
    argsWithChoice,
    choiceFromPreview,
    draftBrief,
    draftSource,
    previewStartWork,
    planLaunchLine,
    projectLabel,
    projectProposal,
    startBlockedBy,
    suggestedProjectId,
    type BriefDraft,
    type HeldBrief,
    type StartChoice,
    type StartPreview,
  } from './start_preview';

  let {
    base,
    preview: initial,
    heading,
    blocked = null,
    onclose,
    onstarted,
    held = null,
    /** The re-preview debounce, ms; injectable for tests. */
    debounceMs = 250,
  }: {
    /** A brief drafted in the task page (G7.6): the popover opens with it
     *  as its draft, so Start sends it. */
    held?: HeldBrief | null;
    /** The start's arguments before any choice (by item, else by key). */
    base: StartWorkArgs;
    preview: StartPreview;
    heading: string;
    /** Why starting is refused from here (a hub that is down), if it is. */
    blocked?: string | null;
    onclose: (refocus: boolean) => void;
    onstarted: (row: SessionRow) => void;
    debounceMs?: number;
  } = $props();

  // The popover's own copy of the preview: it is re-read as choices change.
  // svelte-ignore state_referenced_locally
  let preview = $state.raw<StartPreview>(initial);
  // svelte-ignore state_referenced_locally
  let choice = $state<StartChoice>(choiceFromPreview(initial, base.with_brief ?? true));
  // Jev's proposal (K1) from the first preview: the re-read with the
  // project chosen no longer carries it, so it is kept here for the hint.
  // svelte-ignore state_referenced_locally
  const suggested = suggestedProjectId(initial);
  // svelte-ignore state_referenced_locally
  const suggestedProposal = projectProposal(initial);
  // Redesign 8.11: the rule fleet offers after five identical starts, from
  // the first preview; gone once answered.
  // svelte-ignore state_referenced_locally
  let ruleOffer = $state.raw(initial.rule_offer ?? null);
  let ruleAdded = $state<string | null>(null);
  /** The person picked a repository: Jev's ring comes off (ai.md, r15 F15). */
  let projectTouched = $state(false);
  let branch = $state('');
  let showBrief = $state(false);
  let busy = $state(false);
  let reading = $state(false);
  let error = $state<string | null>(null);
  let root: HTMLElement | undefined = $state();

  const planned = $derived(preview.plan);
  const conflicts = $derived(preview.conflicts);
  const proposal = $derived(conflicts.some((c) => c.kind === 'proposal'));
  const crossOrg = $derived(conflicts.find((c) => c.kind === 'cross_org') ?? null);
  const why = $derived(blocked ?? startBlockedBy(preview, choice));
  const liveRow = (id: number | null | undefined) => (id == null ? undefined : get(sessions).find((r) => r.id === id));

  onMount(async () => {
    branch = planned?.branch ?? '';
    // A pre-selected repository still needs its host and plan resolved.
    if (suggested != null && choice.project_id === suggested) void readNow();
    await tick();
    // The unresolved field first, else Start.
    const first =
      preview.missing === 'project'
        ? root?.querySelector<HTMLElement>('[data-field="project"]')
        : preview.missing === 'host'
          ? root?.querySelector<HTMLElement>('[data-field="host"]')
          : root?.querySelector<HTMLElement>('[data-testid="start-popover-go"]:not(:disabled)');
    (first ?? root)?.focus();
  });

  let seq = 0;
  let timer: ReturnType<typeof setTimeout> | undefined;
  function reread() {
    clearTimeout(timer);
    timer = setTimeout(() => void readNow(), debounceMs);
  }
  // Closed mid-debounce: no preview read for a popover that is gone.
  onDestroy(() => {
    clearTimeout(timer);
    seq++;
  });
  async function readNow() {
    const mine = ++seq;
    reading = true;
    const r = await previewStartWork(argsWithChoice(base, { ...choice, worktree: branchEdited() }));
    if (mine !== seq) return;
    reading = false;
    if (!r.ok) {
      error = readErrorText(r.error);
      return;
    }
    error = null;
    preview = r.value;
    if (!branchTouched) branch = r.value.plan?.branch ?? '';
  }

  // A branch the person typed is sent as is; the planned one is the
  // backend's to choose (a parallel start's `-N`).
  let branchTouched = false;
  const branchEdited = () => (branchTouched && branch.trim() ? branch.trim() : null);

  function setProject(v: string) {
    choice.project_id = v === '' ? null : Number(v);
    // Another repository: its own last host, unless the person picked one.
    if (preview.missing !== 'host') choice.host_alias = null;
    reread();
  }
  function setHost(v: string) {
    choice.host_alias = v || null;
    reread();
  }

  /** The arguments exactly where the preview on screen said: its resolved
   *  repository and host, not a fresh resolution that could land elsewhere. */
  const resolvedArgs = () =>
    argsWithChoice(base, {
      ...choice,
      project_id: choice.project_id ?? planned?.project_id ?? null,
      host_alias: choice.host_alias ?? planned?.host_alias ?? null,
      worktree: branchEdited(),
    });

  // Redesign 6.10: a brief drafted on the start's host from the ticket and
  // the task's earlier work. It is the person's to edit and goes only with
  // Start; Clear goes back to the task's brief.
  // svelte-ignore state_referenced_locally
  let draftMeta = $state<BriefDraft | null>(held?.draft ?? null);
  /** Writing help's "Draft agent briefs" (G4.6), off by default. */
  const draftsBriefs = $derived(settingBool($fleetSettings, SETTING_KEYS.workDraftBriefs));
  // svelte-ignore state_referenced_locally
  let draftText = $state(held?.brief ?? '');
  let drafting = $state(false);
  let draftSeq = 0;
  async function draft() {
    const mine = ++draftSeq;
    draftText = draftMeta ? draftText : (preview.brief ?? '');
    drafting = true;
    error = null;
    const r = await draftBrief(resolvedArgs());
    if (mine !== draftSeq) return;
    drafting = false;
    if (!r.ok) {
      error = readErrorText(r.error);
      return;
    }
    draftText = r.value.brief;
    draftMeta = r.value.draft;
  }
  function clearDraft() {
    draftSeq++;
    drafting = false;
    draftMeta = null;
    draftText = '';
  }

  async function go() {
    if (busy || why) return;
    busy = true;
    error = null;
    const drafted = choice.with_brief && draftMeta && draftText.trim() ? { brief: draftText } : {};
    const r = await startWork({ ...resolvedArgs(), ...drafted });
    busy = false;
    if (!r.ok) {
      error = readErrorText(r.error);
      // Lost a race, or something moved: show what is true now.
      void readNow();
      return;
    }
    onstarted(r.value);
  }

  async function acceptAndStart() {
    if (preview.item_id == null || busy) return;
    busy = true;
    const a = await decideWorkProposal(preview.item_id, true);
    busy = false;
    if (!a.ok) {
      error = readErrorText(a.error);
      return;
    }
    await readNow();
    await go();
  }

  async function answerOffer(add: boolean) {
    const offer = ruleOffer;
    if (!offer) return;
    const r = add ? await acceptStartRule(offer.id) : await dismissStartRule(offer.id);
    if (!r.ok) {
      error = readErrorText(r.error);
      return;
    }
    ruleOffer = null;
    ruleAdded = add ? ruleLine(r.value, preview.projects) : null;
  }

  function openSession(id: number | null | undefined) {
    const row = liveRow(id);
    if (row) {
      onclose(false);
      selectSessionExplicitly(row);
    }
  }
</script>

<div
  class="start-pop"
  role="dialog"
  tabindex="-1"
  aria-label={heading}
  data-testid="start-popover"
  bind:this={root}
  onkeydown={(e) => {
    if (e.key === 'Escape') {
      e.stopPropagation();
      onclose(true);
    } else if (e.key === 'Enter' && !(e.target instanceof HTMLTextAreaElement) && !(e.target instanceof HTMLButtonElement)) {
      e.preventDefault();
      if (proposal) void acceptAndStart();
      else void go();
    }
  }}
>
  <h3 class="head">{heading}</h3>

  <label class="field">
    <span>Repo</span>
    <select
      data-field="project"
      data-testid="start-popover-project"
      class:ai-pre={suggested != null && choice.project_id === suggested && !projectTouched}
      value={choice.project_id ?? ''}
      onchange={(e) => {
        projectTouched = true;
        setProject((e.currentTarget as HTMLSelectElement).value);
      }}
    >
      {#if choice.project_id == null}<option value="">Pick a repository…</option>{/if}
      {#each preview.projects as p (p.id)}
        <option value={p.id}>{projectLabel(p)}</option>
      {/each}
      {#if choice.project_id != null && !preview.projects.some((p) => p.id === choice.project_id)}
        <option value={choice.project_id}>project {choice.project_id}</option>
      {/if}
    </select>
  </label>
  {#if suggested != null && choice.project_id === suggested}
      <ProposedBy
        proposal={suggestedProposal}
        field="project"
        testid="start-popover-suggested"
        onchange={() => {
          setProject('');
          root?.querySelector<HTMLElement>('[data-field="project"]')?.focus();
        }}
      />
  {/if}

  {#if planned?.rule_id != null && choice.project_id === planned.project_id}
    <span class="hint" data-testid="start-popover-by-rule">Picked by a start rule: change the repository to start elsewhere.</span>
  {/if}
  {#if planned && planLaunchLine(planned)}
    <span class="hint" data-testid="start-popover-launch">From a rule: {planLaunchLine(planned)}</span>
  {/if}

  <label class="field">
    <span>Host</span>
    <select
      data-field="host"
      data-testid="start-popover-host"
      value={choice.host_alias ?? planned?.host_alias ?? ''}
      onchange={(e) => setHost((e.currentTarget as HTMLSelectElement).value)}
    >
      {#if !(choice.host_alias ?? planned?.host_alias)}<option value="">Pick a host…</option>{/if}
      {#each preview.hosts as h (h.alias)}
        <option value={h.alias}>{h.alias}{h.reachable ? '' : ' (offline)'}</option>
      {/each}
    </select>
  </label>

  <label class="field">
    <span>Branch</span>
    <input
      data-testid="start-popover-branch"
      spellcheck="false"
      value={branch}
      oninput={(e) => {
        branch = (e.currentTarget as HTMLInputElement).value;
        branchTouched = true;
        reread();
      }}
    />
    {#if planned}
      <span class="hint" data-testid="start-popover-checkout">{preview.checkout?.exists ? 'existing checkout' : 'new'}</span>
    {/if}
  </label>

  <div class="field">
    <span>Brief</span>
    <label class="check">
      <input
        type="checkbox"
        data-testid="start-popover-brief"
        checked={choice.with_brief}
        onchange={(e) => {
          choice.with_brief = (e.currentTarget as HTMLInputElement).checked;
          reread();
        }}
      />
      Send the task's brief
    </label>
    {#if preview.brief && !draftMeta && !drafting}
      <button class="btn btn--quiet link" type="button" aria-expanded={showBrief} data-testid="start-popover-brief-toggle" onclick={() => (showBrief = !showBrief)}
        >{showBrief ? 'Hide' : 'Preview'}</button
      >
    {/if}
    {#if choice.with_brief && planned && !draftMeta && !drafting && draftsBriefs}
      <button
        class="btn btn--quiet link"
        type="button"
        data-testid="start-popover-brief-draft-ask"
        title="Write the brief from the task and earlier sessions on it, with a model call on {planned.host_alias}"
        onclick={() => void draft()}>Draft with Claude</button
      >
    {/if}
  </div>
  {#if choice.with_brief && (draftMeta || drafting)}
    <DraftField
      label="Brief"
      bind:value={draftText}
      model={draftMeta?.model}
      host={draftMeta?.host_alias}
      from={draftMeta ? draftSource(draftMeta) : null}
      busy={drafting}
      rows={6}
      onregenerate={() => void draft()}
      onclear={clearDraft}
      testid="start-popover-brief-draft"
    />
  {:else if showBrief && preview.brief}
    <pre class="brief" data-testid="start-popover-brief-text">{preview.brief}</pre>
  {/if}

  {#if ruleOffer}
    <div class="offer" data-testid="start-popover-rule-offer">
      <span>Add rule <strong>{ruleLine(ruleOffer, preview.projects)}</strong>? Next time it starts there without asking.</span>
      <button class="btn btn--quiet" type="button" data-testid="start-popover-rule-add" onclick={() => void answerOffer(true)}>Add rule</button>
      <button class="btn btn--quiet" type="button" data-testid="start-popover-rule-dismiss" onclick={() => void answerOffer(false)}>Dismiss</button>
    </div>
  {:else if ruleAdded}
    <span class="hint" role="status" data-testid="start-popover-rule-added">Rule added: {ruleAdded}. Edit it in Automation.</span>
  {/if}

  {#if conflicts.length > 0}
    <ul class="conflicts" data-testid="start-popover-conflicts">
      {#each conflicts as c, i (i)}
        <li class="conflict conflict--{c.kind}" data-kind={c.kind}>
          <span class="warn"><Icon name="warning" size={12} /></span>
          <span>
            {c.message}
            {#if c.kind === 'live_session'}
              Start makes a parallel session in its own checkout.
            {/if}
          </span>
          {#if (c.kind === 'live_session' || c.kind === 'worktree_busy') && liveRow(c.session_id)}
            <button class="btn btn--quiet" type="button" data-testid="start-popover-open-live" onclick={() => openSession(c.session_id)}>Open it</button>
          {/if}
        </li>
      {/each}
    </ul>
  {/if}
  {#if crossOrg}
    <label class="check">
      <input
        type="checkbox"
        data-testid="start-popover-cross-org"
        checked={choice.force_cross_org}
        onchange={(e) => {
          choice.force_cross_org = (e.currentTarget as HTMLInputElement).checked;
        }}
      />
      Start across organisations
    </label>
  {/if}

  {#if error}<p class="err" role="alert" data-testid="start-popover-error">{error}</p>{/if}

  <div class="acts">
    {#if why}<span class="why" data-testid="start-popover-why">{why}</span>{/if}
    <button class="btn btn--quiet" type="button" data-testid="start-popover-cancel" onclick={() => onclose(true)}>Cancel</button>
    {#if proposal}
      <button
        class="btn btn--primary"
        type="button"
        data-testid="start-popover-accept"
        disabled={busy || blocked !== null}
        onclick={() => void acceptAndStart()}>Accept &amp; start</button
      >
    {:else}
      <button
        class="btn btn--primary"
        type="button"
        data-testid="start-popover-go"
        disabled={busy || reading || why !== null}
        onclick={() => void go()}>{busy ? 'Starting…' : choice.parallel ? 'Start parallel ⏎' : 'Start ⏎'}</button
      >
    {/if}
  </div>
</div>

<style>
  .start-pop {
    display: grid;
    gap: 8px;
    width: min(360px, calc(100vw - 32px));
    padding: 10px 12px;
    border: 1px solid var(--control-border);
    border-radius: var(--radius-md);
    background: var(--bg);
    color: var(--fg);
    box-shadow: var(--shadow-pop);
    font-size: var(--text-xs);
  }
  .start-pop:focus-visible {
    outline: var(--ring-w) solid var(--ring);
  }
  .head {
    margin: 0;
    font-size: var(--text-xs);
    font-weight: 600;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .field {
    display: grid;
    grid-template-columns: 52px minmax(0, 1fr) auto;
    gap: 6px;
    align-items: center;
  }
  .field > span:first-child {
    color: var(--fg-muted);
  }
  .field select,
  .field input:not([type='checkbox']) {
    min-width: 0;
    height: 24px;
    font: inherit;
  }
  .field input:not([type='checkbox']) {
    font-family: var(--mono);
  }
  .hint {
    color: var(--fg-muted);
    font-size: var(--text-2xs);
  }
  .check {
    display: flex;
    gap: 6px;
    align-items: center;
  }
  .link {
    padding: 0 4px;
  }
  .brief {
    max-height: 160px;
    overflow: auto;
    margin: 0;
    padding: 6px 8px;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    font-family: var(--mono);
    font-size: var(--text-2xs);
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    user-select: text;
  }
  .conflicts {
    display: grid;
    gap: 4px;
    margin: 0;
    padding: 0;
    list-style: none;
  }
  .conflict {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr) auto;
    gap: 6px;
    align-items: start;
  }
  .warn {
    color: var(--usage-warn);
  }
  .offer {
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto auto;
    gap: 6px;
    align-items: center;
    padding: 6px 8px;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
  }
  .err {
    margin: 0;
    color: var(--usage-crit);
  }
  .acts {
    display: flex;
    gap: 6px;
    align-items: center;
    justify-content: flex-end;
    flex-wrap: wrap;
  }
  .why {
    margin-right: auto;
    color: var(--fg-muted);
    font-size: var(--text-2xs);
  }
</style>
