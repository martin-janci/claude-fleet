<script lang="ts">
  import { untrack } from 'svelte';
  import Modal from './Modal.svelte';
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
    | { mode: 'name'; sessions: { id: number; label: string }[] }
    | { mode: 'rename'; itemId: number; title: string; key?: string | null };

  let {
    target,
    onclose,
    ondone,
  }: {
    target: Target;
    onclose: () => void;
    /** After a successful write, before the dialog closes. */
    ondone?: () => void;
  } = $props();

  const naming = untrack(() => target.mode === 'name');
  let title = $state(untrack(() => (target.mode === 'rename' ? target.title : '')));
  let key = $state('');
  // Every session starts chosen; a group header can leave some out.
  let chosen = $state<Set<number>>(
    untrack(() => new Set(target.mode === 'name' ? target.sessions.map((s) => s.id) : [])),
  );
  let busy = $state(false);
  let failure = $state<string | null>(null);

  const titleError = $derived(title.trim() === '' ? null : workTitleError(title));
  const canSubmit = $derived(
    !busy && title.trim() !== '' && titleError === null && (!naming || chosen.size > 0),
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
            t.sessions.map((s) => s.id).filter((id) => chosen.has(id)),
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
    {:else if target.key}
      <p class="note">Key {target.key} stays; only the title changes.</p>
    {/if}
    {#if failure}
      <p class="err" role="alert" data-testid="name-work-error">{failure}</p>
    {/if}
    <div class="actions">
      <button type="button" onclick={onclose}>Cancel</button>
      <button type="submit" class="primary" disabled={!canSubmit} data-testid="name-work-submit"
        >{naming ? 'Name' : 'Rename'}</button
      >
    </div>
  </form>
</Modal>

<style>
  .form { display: flex; flex-direction: column; gap: 0.6rem; }
  .field { display: flex; flex-direction: column; gap: 0.25rem; }
  .field span, legend { font-size: 0.7rem; color: var(--fg-muted); text-transform: uppercase; letter-spacing: 0.04em; }
  .field input {
    font: inherit;
    padding: 0.35rem 0.5rem;
    border: 1px solid var(--border);
    background: var(--bg-pane);
    color: var(--fg);
    border-radius: 4px;
  }
  .field input[aria-invalid='true'] { border-color: #e64a4a; }
  .sessions { border: 1px solid var(--border); border-radius: 4px; padding: 0.3rem 0.5rem; margin: 0; }
  .sess { display: flex; gap: 0.4rem; align-items: center; font-size: 0.85rem; }
  .note { font-size: 0.8rem; color: var(--fg-muted); margin: 0; }
  .err { color: #e64a4a; font-size: 0.8rem; margin: 0; }
  .actions { display: flex; gap: 0.4rem; justify-content: flex-end; }
  .actions button {
    font-size: 0.85rem;
    padding: 0.3rem 0.8rem;
    border: 1px solid var(--border);
    background: transparent;
    color: var(--fg);
    border-radius: 4px;
    cursor: pointer;
  }
  .actions button:disabled { opacity: 0.5; cursor: not-allowed; }
  .actions button.primary { border-color: var(--accent); }
</style>
