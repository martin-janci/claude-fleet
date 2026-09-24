<script lang="ts">
  // Resume past work (roadmap M2.5): where the session would land, the three
  // modes with the hub's reason for any that is not possible, and — for
  // "Fresh with brief" — the brief itself, editable before anything starts
  // (no invisible prompts). Live work offers Jump instead of a second
  // session. Every decision is the hub's (`work_resume_plan`); this only
  // renders it.
  import { onMount, untrack } from 'svelte';
  import { get } from 'svelte/store';
  import Modal from './Modal.svelte';
  import { resumeWork, workResumePlan, type ResumeMode, type ResumePlan } from './work';
  import { sessions } from './sessions';
  import { selectSessionExplicitly } from './selection';

  type Mode = 'last' | 'brief' | 'fresh';

  let {
    workKey,
    linkId = null,
    initialMode = 'last',
    onclose,
    onresumed,
  }: {
    workKey: string;
    linkId?: number | null;
    initialMode?: Mode;
    onclose: () => void;
    /** Called with the new session once a resume started. */
    onresumed?: () => void;
  } = $props();

  const LABELS: Record<Mode, string> = {
    last: 'Continue last conversation',
    brief: 'Fresh with brief',
    fresh: 'Fresh',
  };
  const HINTS: Record<Mode, string> = {
    last: 'claude --resume in the same worktree, with the conversation as it was',
    brief: 'a new conversation that starts from the handover brief below',
    fresh: 'a new conversation, nothing carried',
  };

  let plan = $state<ResumePlan | null>(null);
  let error = $state<string | null>(null);
  let mode = $state<Mode>(untrack(() => initialMode));
  let hostOverride = $state<string | null>(null);
  let brief = $state('');
  let briefFor = $state<string | null>(null);
  let briefLoading = $state(false);
  let busy = $state(false);

  const modes = $derived((plan?.modes ?? []) as ResumeMode[]);
  const current = $derived(modes.find((m) => m.mode === mode) ?? null);
  const live = $derived(plan?.live ?? []);

  function firstOk(p: ResumePlan, prefer: Mode): Mode {
    const ok = (m: string) => p.modes.some((x) => x.mode === m && x.ok);
    if (ok(prefer)) return prefer;
    for (const m of ['last', 'brief', 'fresh'] as Mode[]) if (ok(m)) return m;
    return prefer;
  }

  async function loadPlan(): Promise<void> {
    error = null;
    const r = await workResumePlan(workKey, { linkId, hostAlias: hostOverride });
    if (!r.ok) {
      error = r.error.message;
      plan = null;
      return;
    }
    plan = r.value;
    mode = firstOk(r.value, mode);
    if (mode === 'brief') void loadBrief();
  }

  async function loadBrief(): Promise<void> {
    const where = `${hostOverride ?? ''}|${linkId ?? ''}`;
    if (briefFor === where) return;
    briefLoading = true;
    const r = await workResumePlan(workKey, { linkId, hostAlias: hostOverride, withBrief: true });
    briefLoading = false;
    if (r.ok) {
      brief = r.value.brief ?? '';
      briefFor = where;
    } else {
      error = r.error.message;
    }
  }

  function pick(m: Mode) {
    mode = m;
    if (m === 'brief') void loadBrief();
  }

  async function changeHost(h: string) {
    hostOverride = h === '' ? null : h;
    briefFor = null;
    await loadPlan();
  }

  function jump(sessionId: number) {
    const row = get(sessions).find((s) => s.id === sessionId);
    if (row) selectSessionExplicitly(row);
    onclose();
  }

  async function start() {
    if (!current?.ok || busy) return;
    busy = true;
    error = null;
    const r = await resumeWork({
      key: workKey,
      mode,
      linkId: plan?.link_id ?? linkId,
      hostAlias: hostOverride,
      brief: mode === 'brief' ? brief : null,
    });
    busy = false;
    if (!r.ok) {
      error = r.error.message;
      return;
    }
    selectSessionExplicitly(r.value);
    onresumed?.();
    onclose();
  }

  onMount(() => {
    void loadPlan();
  });
</script>

<Modal label="Resume {workKey}" {onclose} width="560px" testid="resume-dialog">
  <h3 class="title">Resume <code>{workKey}</code>{#if plan?.title}<span class="t"> — {plan.title}</span>{/if}</h3>

  {#if error}
    <p class="err" role="alert" data-testid="resume-error">{error}</p>
  {/if}

  {#if plan}
    {#if live.length > 0}
      <p class="live" data-testid="resume-live">
        Already running as <b>{live[0].friendly_name ?? live[0].tmux_name}</b> on {live[0].host_alias}.
        <button type="button" class="linkish" data-testid="resume-jump" onclick={() => jump(live[0].session_id)}
          >Jump to it</button
        >
      </p>
    {:else}
      <p class="where" data-testid="resume-where">
        {#if plan.host_alias}Lands on <b>{plan.host_alias}</b>{:else}No host to land on{/if}
        {#if plan.branch} · branch <code>{plan.branch}</code>{/if}
        {#if plan.worktree}
          · worktree <code>{plan.worktree}</code>
          {plan.worktree_present ? '' : '(recreated from the branch)'}
        {/if}
      </p>
      {#if (plan.hosts ?? []).length > 0}
        <label class="host">
          Host
          <select
            data-testid="resume-host"
            value={hostOverride ?? ''}
            onchange={(e) => void changeHost((e.currentTarget as HTMLSelectElement).value)}
          >
            <option value="">its own host</option>
            {#each plan.hosts ?? [] as h (h)}<option value={h}>{h}</option>{/each}
          </select>
        </label>
      {/if}
    {/if}

    <ul class="modes" role="radiogroup" aria-label="Resume mode">
      {#each ['last', 'brief', 'fresh'] as const as m (m)}
        {@const info = modes.find((x) => x.mode === m)}
        <li>
          <label class:off={!info?.ok}>
            <input
              type="radio"
              name="resume-mode"
              value={m}
              checked={mode === m}
              disabled={!info?.ok}
              data-testid="resume-mode-{m}"
              onchange={() => pick(m)}
            />
            <span class="lbl">{LABELS[m]}</span>
            <span class="hint">{info?.ok ? HINTS[m] : (info?.reason ?? 'not possible')}</span>
          </label>
        </li>
      {/each}
    </ul>

    {#if mode === 'brief' && current?.ok}
      <label class="brief-lbl" for="resume-brief">Brief (sent as context with the first prompt; edit freely)</label>
      {#if briefLoading}
        <p class="muted">Building the brief…</p>
      {:else}
        <textarea id="resume-brief" rows="12" spellcheck="false" data-testid="resume-brief" bind:value={brief}
        ></textarea>
      {/if}
    {/if}
  {:else if !error}
    <p class="muted">Reading past work…</p>
  {/if}

  <div class="actions">
    <button type="button" onclick={onclose}>Cancel</button>
    <button
      type="button"
      class="primary"
      data-testid="resume-start"
      disabled={!plan || live.length > 0 || !current?.ok || busy || (mode === 'brief' && briefLoading)}
      onclick={start}>{busy ? 'Starting…' : LABELS[mode]}</button
    >
  </div>
</Modal>

<style>
  .title {
    margin: 0 0 0.6rem;
    font-size: 1rem;
  }
  .title .t {
    font-weight: normal;
    color: var(--fg-muted, #999);
  }
  .where,
  .live {
    margin: 0 0 0.5rem;
  }
  .host {
    display: flex;
    gap: 0.5rem;
    align-items: center;
    margin-bottom: 0.5rem;
  }
  .modes {
    list-style: none;
    padding: 0;
    margin: 0 0 0.6rem;
  }
  .modes label {
    display: grid;
    grid-template-columns: auto 1fr;
    column-gap: 0.4rem;
    padding: 0.25rem 0;
    cursor: pointer;
  }
  .modes label.off {
    cursor: not-allowed;
    opacity: 0.6;
  }
  .modes .hint {
    grid-column: 2;
    font-size: 0.85em;
    color: var(--fg-muted, #999);
  }
  .brief-lbl {
    display: block;
    font-size: 0.85em;
    margin-bottom: 0.25rem;
  }
  textarea {
    width: 100%;
    box-sizing: border-box;
    font-family: var(--mono, monospace);
    font-size: 0.8em;
  }
  .err {
    color: var(--danger, #e5534b);
  }
  .muted {
    color: var(--fg-muted, #999);
  }
  .linkish {
    background: none;
    border: none;
    padding: 0;
    color: var(--accent, #4c8dff);
    cursor: pointer;
    text-decoration: underline;
  }
  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 0.5rem;
    margin-top: 0.8rem;
  }
</style>
