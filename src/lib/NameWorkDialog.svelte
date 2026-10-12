<script lang="ts">
  import { untrack } from 'svelte';
  import Modal from './Modal.svelte';
  import { hubActionBlocked, hubStatus } from './hub';
  import { hubConnection } from './hub_connection';
  import { sessionIdBlocked } from './share';
  import { sessions } from './sessions';
  import DraftedLabel from './DraftedLabel.svelte';
  import {
    LOCAL_WORK_TITLE_MAX,
    nameWorkForSessions,
    renameWorkItem,
    workTitleError,
  } from './work';

  // "Name this work…" (work graph M11.1): give work that has no ticket a
  // title (and, optionally, a key), and link the chosen sessions to it — or
  // rename a local item. Both go through Routed commands, so a paired
  // desktop names the work on its hub.
  type Target =
    | {
        mode: 'name';
        sessions: { id: number; label: string }[];
        /** G7.6: the other sessions on the same branch with no work yet,
         *  offered as one "Also name…" box, ticked. */
        branchMates?: { id: number; label: string }[];
      }
    | { mode: 'rename'; itemId: number; title: string; key?: string | null };

  let {
    target,
    onclose,
    ondone,
    accessBlocked = null,
  }: {
    target: Target;
    onclose: () => void;
    /** After a successful write, before the dialog closes. */
    ondone?: () => void;
    /**
     * Why this client may not write, when the dialog cannot work it out for
     * itself (multi-user M1, F2b). `rename` mode is handed an ITEM id and no
     * session at all, so there is no `owner_person_id` here to ask about —
     * the caller holds the row and composes the answer (the `ReplyActions`
     * shape, same reason). `name` mode needs no prop: it is given session ids
     * and narrows them itself below.
     */
    accessBlocked?: string | null;
  } = $props();

  const naming = untrack(() => target.mode === 'name');
  let title = $state(untrack(() => (target.mode === 'rename' ? target.title : '')));
  let key = $state('');
  // Every session starts chosen; a group header can leave some out.
  let chosen = $state<Set<number>>(
    untrack(() => new Set(target.mode === 'name' ? target.sessions.map((s) => s.id) : [])),
  );
  /** "Also name the N other sessions on this branch" (G7.6), ticked. */
  const mates = untrack(() => (target.mode === 'name' ? (target.branchMates ?? []) : []));
  let alsoMates = $state(true);
  let busy = $state(false);
  let failure = $state<string | null>(null);

  /**
   * The drafted name (G2.1, rule 7): the label the session's own agent gave
   * it (`set_friendly_name`). G7.6 prefills it, as the board does, when the
   * title is still empty: it shows "Drafted" until the person edits it, Undo
   * empties the field again, and nothing is written before Name.
   */
  const draft = $derived.by(() => {
    if (!naming || target.mode !== 'name') return null;
    for (const s of target.sessions) {
      const row = $sessions.find((r) => r.id === s.id);
      const name = row?.friendly_name?.trim();
      if (name && name !== row?.tmux_name && workTitleError(name) === null) return name;
    }
    return null;
  });
  /** What the title held before the draft went in; null when it is not in. */
  let beforeDraft = $state<string | null>(null);
  const showingDraft = $derived(beforeDraft !== null && draft !== null && title === draft);
  function useDraft() {
    if (draft === null) return;
    beforeDraft = title;
    title = draft;
  }
  function undoDraft() {
    title = beforeDraft ?? '';
    beforeDraft = null;
  }
  // Once: the sessions store may answer after the dialog opens.
  let prefilled = false;
  $effect(() => {
    if (prefilled || draft === null) return;
    prefilled = true;
    if (untrack(() => title.trim() === '')) untrack(useDraft);
  });

  const titleError = $derived(title.trim() === '' ? null : workTitleError(title));
  /**
   * Both halves, per target (multi-user M1, F2b/F2e). `name_session_work` and
   * `link_session_work` are `drive` in `share.ts::SESSION_TIER` and both
   * ROUTE, so the hub's half applies too. The dialog is given session IDS and
   * never a row, so the answer comes from `share.ts::sessionIdBlocked` — and it
   * re-asks rather than trusting the narrowing its caller did: this dialog stays
   * open, and a grant can be narrowed while it is.
   *
   * An id whose row the store does not hold used to count as writable here, on
   * the argument that this dialog and `$sessions` load independently. That was
   * the escape hatch F2d deleted from `TidyReview`, `WorkReview`, `moves.ts` and
   * `TasksPanel`, and it was no safer in this file: on a fleet this client does
   * not own, the hub fences rows this person may not see off the stream, so "not
   * in `$sessions`" reads as *someone else's, or gone* — and naming work for it
   * is a write onto a session we cannot tell the owner of. `$sessionIdBlocked`
   * holds the one rule instead: `UNKNOWN_SESSION_REASON` on a paired desktop,
   * `null` on a standalone one, where the master owns every row and a store that
   * lags this dialog therefore costs the owner nothing.
   *
   * Asked per id rather than once over the batch, so one unresolvable session
   * narrows the write instead of refusing the whole dialog.
   */
  const writableIds = $derived(
    new Set(
      [...chosen, ...(alsoMates ? mates.map((m) => m.id) : [])].filter(
        (id) => $sessionIdBlocked(id, 'name_session_work') === null,
      ),
    ),
  );
  /** Sessions the person ticked that are somebody else's — named below rather
   *  than dropped in silence. */
  const notMine = $derived(
    [...chosen, ...(alsoMates ? mates.map((m) => m.id) : [])].filter((id) => !writableIds.has(id)).length,
  );
  const hubBlocked = $derived(
    hubActionBlocked(naming ? 'name_session_work' : 'rename_work_item', $hubStatus, $hubConnection),
  );
  const blocked = $derived(
    hubBlocked ??
      accessBlocked ??
      (naming && chosen.size > 0 && writableIds.size === 0
        ? ($sessionIdBlocked([...chosen][0] ?? null, 'name_session_work') ??
          'None of these sessions are yours to name work for.')
        : null),
  );

  const canSubmit = $derived(
    !busy &&
      blocked === null &&
      title.trim() !== '' &&
      titleError === null &&
      (!naming || writableIds.size > 0),
  );

  function toggle(id: number) {
    const next = new Set(chosen);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    chosen = next;
  }

  async function submit(e?: Event) {
    e?.preventDefault();
    if (!canSubmit) return;
    busy = true;
    failure = null;
    const t = target;
    const r =
      t.mode === 'name'
        ? await nameWorkForSessions(
            [...t.sessions, ...(alsoMates ? mates : [])].map((s) => s.id).filter((id) => writableIds.has(id)),
            title,
            key,
          )
        : await renameWorkItem(t.itemId, title);
    busy = false;
    if (!r.ok) {
      failure = r.error.message;
      return;
    }
    ondone?.();
    onclose();
  }
</script>

<Modal
  title={naming ? 'Name this work' : 'Rename work'}
  {onclose}
  width="420px"
  testid="name-work-dialog"
>
  <form class="form" onsubmit={submit}>
    <label class="field">
      <span>Title</span>
      <input
        type="text"
        bind:value={title}
        maxlength={LOCAL_WORK_TITLE_MAX * 2}
        placeholder="What is this work?"
        data-autofocus=""
        autocomplete="off"
        data-testid="name-work-title"
        aria-invalid={titleError !== null}
      />
    </label>
    {#if titleError}
      <p class="err" data-testid="name-work-title-error">{titleError}</p>
    {/if}
    {#if showingDraft}
      <p class="draft" data-testid="name-work-drafted">
        <DraftedLabel testid="name-work-drafted-label" />
        <span class="note">by the session's agent</span>
        <button type="button" class="link" data-testid="name-work-draft-undo" onclick={undoDraft}>Undo</button>
      </p>
    {:else if draft !== null && title.trim() !== draft}
      <p class="draft">
        <button type="button" class="link" data-testid="name-work-use-draft" onclick={useDraft}
          >Use the drafted name “{draft}”</button
        >
      </p>
    {/if}
    {#if target.mode === 'name'}
      <label class="field">
        <span>Key (optional)</span>
        <input
          type="text"
          bind:value={key}
          placeholder="e.g. OPS-1"
          autocomplete="off"
          spellcheck="false"
          data-testid="name-work-key"
        />
      </label>
      {#if target.sessions.length > 1}
        <fieldset class="sessions" data-testid="name-work-sessions">
          <legend>Sessions</legend>
          {#each target.sessions as s (s.id)}
            <label class="sess">
              <input
                type="checkbox"
                checked={chosen.has(s.id)}
                onchange={() => toggle(s.id)}
                data-testid="name-work-session"
              />
              {s.label}
            </label>
          {/each}
        </fieldset>
      {/if}
      {#if mates.length > 0}
        <label class="sess" title={mates.map((m) => m.label).join(', ')}>
          <input type="checkbox" bind:checked={alsoMates} data-testid="name-work-branch-mates" />
          Also name the {mates.length === 1 ? 'other session' : `${mates.length} other sessions`} on this branch
        </label>
      {/if}
    {:else if target.key}
      <p class="note">Key {target.key} stays; only the title changes.</p>
    {/if}
    {#if blocked}
      <p class="err" data-testid="name-work-blocked">{blocked}</p>
    {:else if notMine > 0}
      <p class="note" data-testid="name-work-not-mine">
        {notMine} of the ticked sessions {notMine === 1 ? 'is' : 'are'} somebody else's and will be
        left out — only the session's owner can name work for it.
      </p>
    {/if}
    {#if failure}
      <p class="err" role="alert" data-testid="name-work-error">{failure}</p>
    {/if}
    <div class="actions">
      <button type="button" onclick={onclose}>Cancel</button>
      <button
        type="submit"
        class="primary"
        disabled={!canSubmit}
        title={blocked ?? ''}
        data-testid="name-work-submit">{naming ? 'Name' : 'Rename'}</button
      >
    </div>
  </form>
</Modal>

<style>
  .form { display: flex; flex-direction: column; gap: 0.6rem; }
  .field { display: flex; flex-direction: column; gap: 0.25rem; }
  .field span, legend { font-size: var(--text-2xs); color: var(--fg-muted); text-transform: uppercase; letter-spacing: 0.04em; }
  .field input {
    font: inherit;
    padding: 0.35rem 0.5rem;
    border: 1px solid var(--border);
    background: var(--bg-pane);
    color: var(--fg);
    border-radius: var(--radius-sm);
  }
  .field input[aria-invalid='true'] { border-color: var(--danger); }
  .sessions { border: 1px solid var(--border); border-radius: var(--radius-sm); padding: 0.3rem 0.5rem; margin: 0; }
  .sess { display: flex; gap: 0.4rem; align-items: center; font-size: var(--text-xs); }
  .note { font-size: var(--text-2xs); color: var(--fg-muted); margin: 0; }
  .err { color: var(--danger); font-size: var(--text-2xs); margin: 0; }
  .draft { display: flex; gap: 0.4rem; align-items: center; margin: 0; font-size: var(--text-2xs); }
  .link {
    background: none;
    border: 0;
    padding: 0;
    color: var(--accent);
    font: inherit;
    cursor: pointer;
  }
  .actions { display: flex; gap: 0.4rem; justify-content: flex-end; }
  .actions button {
    font-size: var(--text-xs);
    padding: 0.3rem 0.8rem;
    border: 1px solid var(--border);
    background: transparent;
    color: var(--fg);
    border-radius: var(--radius-sm);
    cursor: pointer;
  }
  .actions button:disabled { opacity: 0.5; cursor: not-allowed; }
  .actions button.primary { border-color: var(--accent); }
</style>
