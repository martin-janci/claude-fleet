<script lang="ts">
  // Resume past work (roadmap M2.5): where the session would land, the three
  // modes with the hub's reason for any that is not possible, and — for
  // "Fresh with brief" — the brief itself, editable before anything starts
  // (no invisible prompts). Live work offers Jump instead of a second
  // session. Every decision is the hub's (`work_resume_plan`); this only
  // renders it.
  import { onMount, untrack } from 'svelte';
  import { get } from 'svelte/store';
  import DialogSheet from './DialogSheet.svelte';
  import DraftField from './DraftField.svelte';
  import { resumeWork, summarizePastWork, workResumePlan, type ResumeMode, type ResumePlan, type SummaryOutcome } from './work';
  import { plainUntrusted } from './tracker_health';
  import { spliceSummary } from './resume_brief';
  import { hubActionBlocked, hubStatus } from './hub';
  import { hubConnection } from './hub_connection';
  import { sessions } from './sessions';
  import { sessionIdBlocked } from './share';
  import { selectSessionExplicitly } from './selection';

  type Mode = 'last' | 'brief' | 'fresh';

  let {
    workKey,
    linkId = null,
    sessionId = null,
    initialMode = 'last',
    onclose,
    onresumed,
  }: {
    workKey: string;
    linkId?: number | null;
    /** The SOURCE session the resume would re-open, where the caller knows it
     *  (ResumeButton resolves it from the link's snapshot). */
    sessionId?: number | null;
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

  // "What changed" (redesign step 5.12): the past session's
  // summary, written on its own host by `summarize_past_work`, shown as a
  // draft. The next brief includes it, so a brief already built is rebuilt.
  let changed = $state('');
  let changedBy = $state<SummaryOutcome | null>(null);
  let changedBusy = $state(false);
  /** The "What changed" text the brief was built with: an edit or a Clear
   *  of the field is carried into the brief from it (review r15 F22). */
  let briefSummary: string | null = null;
  /** The brief no longer holds that text as written, so it was left alone. */
  let changedStale = $state(false);
  $effect(() => {
    const next = changed;
    untrack(() => {
      if (briefSummary === null || next === briefSummary || mode !== 'brief') return;
      const out = spliceSummary(brief, briefSummary, next);
      if (out === null) {
        changedStale = true;
        return;
      }
      brief = out;
      briefSummary = next;
      changedStale = false;
    });
  });
  const changedBlocked = $derived(
    hubActionBlocked('summarize_past_work', $hubStatus, $hubConnection) ??
      $sessionIdBlocked(sessionId ?? null, 'summarize_past_work'),
  );

  async function whatChanged(): Promise<void> {
    const id = plan?.link_id ?? linkId;
    if (id == null || changedBusy || changedBlocked !== null) return;
    changedBusy = true;
    const r = await summarizePastWork(workKey, id);
    changedBusy = false;
    if (!r.ok) {
      error = r.error.message;
      return;
    }
    changedBy = r.value;
    changed = plainUntrusted(r.value.summary);
    if (mode === 'brief') {
      briefFor = null;
      void loadBrief();
    }
  }

  /**
   * Multi-user M1 (F2c). `resume_work` is `share.ts`'s `own` tier whatever the
   * mode: `last` re-opens the source conversation, `brief` carries a handover
   * written from it, and even `fresh` starts in the source's own worktree and
   * branch. All three act on somebody's past session, so all three are judged
   * against that session's row — and the gate fails closed when this client
   * cannot see it, rather than treating "no row" as "nobody to ask about".
   */
  const shareBlocked = $derived($sessionIdBlocked(sessionId ?? null, 'resume_work'));

  const modes = $derived((plan?.modes ?? []) as ResumeMode[]);
  const current = $derived(modes.find((m) => m.mode === mode) ?? null);
  const live = $derived(plan?.live ?? []);
  const candidates = $derived(plan?.candidates ?? []);
  const lead = $derived(
    candidates.length > 1
      ? `This work has ${candidates.length} earlier sessions. Pick one to continue.`
      : 'Pick up where the last session left off, or start again with a brief.',
  );

  /** "3 d ago" from unix seconds. */
  function ago(ts: number): string {
    const s = Math.max(0, Math.floor(Date.now() / 1000) - ts);
    if (s < 3600) return `${Math.max(1, Math.floor(s / 60))} min ago`;
    if (s < 86400) return `${Math.floor(s / 3600)} h ago`;
    return `${Math.floor(s / 86400)} d ago`;
  }

  async function pickCandidate(id: number) {
    linkId = id;
    briefFor = null;
    briefSummary = null;
    changed = '';
    changedBy = null;
    await loadPlan();
  }

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
      briefSummary = changedBy && changed.trim() !== '' ? changed : null;
      changedStale = false;
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
    // Re-asked at the write: the dialog stays open across a revoke, and the
    // plan it renders carries no owner of its own.
    if (shareBlocked !== null) {
      error = shareBlocked;
      return;
    }
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

<DialogSheet
  title="Resume work {workKey}{plan?.title ? ` · ${plan.title}` : ''}"
  {lead}
  verb={LABELS[mode]}
  busyVerb="Starting…"
  {busy}
  canConfirm={!!plan && live.length === 0 && !!current?.ok && shareBlocked === null && !(mode === 'brief' && briefLoading)}
  onconfirm={() => void start()}
  {onclose}
  {error}
  errorTestid="resume-error"
  confirmTitle={shareBlocked}
  confirmTestid="resume-start"
  width="560px"
  testid="resume-dialog"
>
  {#snippet secondary()}
    {#if plan && live.length === 0 && mode !== 'fresh' && modes.some((m) => m.mode === 'fresh' && m.ok)}
      <button type="button" class="btn btn--quiet" data-testid="resume-fresh-instead" onclick={() => pick('fresh')}
        >Start fresh instead</button
      >
    {/if}
  {/snippet}

  {#if plan}
    {#if live.length > 0}
      <p class="live" data-testid="resume-live">
        Already running as <b>{live[0].friendly_name ?? live[0].tmux_name}</b> on {live[0].host_alias}.
        <button type="button" class="linkish" data-testid="resume-jump" onclick={() => jump(live[0].session_id)}
          >Jump to it</button
        >
      </p>
    {:else}
      {#if candidates.length > 1}
        <ul class="candidates" role="radiogroup" aria-label="Earlier sessions">
          {#each candidates as c (c.link_id)}
            <li>
              <label class:off={!c.resumable}>
                <input
                  type="radio"
                  name="resume-candidate"
                  checked={(plan.link_id ?? linkId) === c.link_id}
                  disabled={busy}
                  data-testid="resume-candidate-{c.link_id}"
                  onchange={() => void pickCandidate(c.link_id)}
                />
                <span class="lbl">{c.name ?? c.branch ?? `session ${c.link_id}`}</span>
                <span class="hint"
                  >{[c.host_alias, c.ended_at ? ago(c.ended_at) : null, c.conversations ? `${c.conversations} conversation${c.conversations === 1 ? '' : 's'}` : null, c.resumable ? null : 'conversation gone']
                    .filter(Boolean)
                    .join(' · ')}</span
                >
              </label>
            </li>
          {/each}
        </ul>
      {/if}
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

    {#if live.length === 0 && (plan.link_id ?? linkId) != null}
      <div class="changed" data-testid="resume-changed">
        {#if changed.trim() !== '' || changedBusy}
          <DraftField
            bind:value={changed}
            label="What changed"
            model={changedBy?.model}
            host={changedBy?.host_alias}
            from={changedBy ? 'from its last conversation' : null}
            busy={changedBusy}
            rows={6}
            onregenerate={() => void whatChanged()}
            onclear={() => (changedBy = null)}
            testid="resume-changed-draft"
          />
          {#if changedStale}
            <p class="muted" data-testid="resume-changed-stale">
              The brief below was edited, so this change did not reach it. Edit the brief itself.
            </p>
          {/if}
        {:else}
          <button
            type="button"
            class="btn btn--quiet"
            data-testid="resume-changed-run"
            disabled={changedBlocked !== null}
            title={changedBlocked ?? 'Ask Claude what the last session did (one model call on its host); the next brief includes it'}
            onclick={() => void whatChanged()}>✎ What changed</button
          >
        {/if}
      </div>
    {/if}

    {#each plan.warnings ?? [] as w (w)}
      <p class="warn" data-testid="resume-warning">{w}</p>
    {/each}

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
      <label class="brief-lbl" for="resume-brief">Tell it what changed since (sent as context with the first prompt; edit freely)</label>
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
</DialogSheet>

<style>
  .where,
  .live {
    margin: 0 0 0.5rem;
  }
  .changed {
    margin-bottom: 0.6rem;
  }
  .host {
    display: flex;
    gap: 0.5rem;
    align-items: center;
    margin-bottom: 0.5rem;
  }
  .modes,
  .candidates {
    list-style: none;
    padding: 0;
    margin: 0 0 0.6rem;
  }
  .modes label,
  .candidates label {
    display: grid;
    grid-template-columns: auto 1fr;
    column-gap: 0.4rem;
    padding: 0.25rem 0;
    cursor: pointer;
  }
  .modes label.off,
  .candidates label.off {
    cursor: not-allowed;
    opacity: 0.6;
  }
  .modes .hint,
  .candidates .hint {
    grid-column: 2;
    font-size: 0.85em;
    color: var(--fg-muted);
  }
  .brief-lbl {
    display: block;
    font-size: 0.85em;
    margin-bottom: 0.25rem;
  }
  textarea {
    width: 100%;
    box-sizing: border-box;
    font-family: var(--mono);
    font-size: 0.8em;
  }
  .muted {
    color: var(--fg-muted);
  }
  .warn {
    margin: 0 0 0.5rem;
    color: var(--status-waiting);
  }
  .linkish {
    background: none;
    border: none;
    padding: 0;
    color: var(--accent);
    cursor: pointer;
    text-decoration: underline;
  }
</style>
