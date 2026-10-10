<!--
  "Rename and label" (Orbit Fleet gap plan step G2.7, the FormsSession
  board): the session's name (its display name; empty falls back to the
  tmux name) and a Label, a short word that groups sessions ("release").
  The label is the session's tags (`set_session_tags`), so the phone and
  `list_sessions` show the same words. One Save writes what changed; Undo
  puts both back.
-->
<script lang="ts">
  import { untrack } from 'svelte';
  import DialogSheet from './DialogSheet.svelte';
  import { setFriendlyName, setSessionTags, type SessionRow } from './sessions';
  import { hubActionBlocked, hubStatus } from './hub';
  import { hubConnection } from './hub_connection';
  import { sessionBlocked } from './share';
  import { labelText, parseLabel, sameTags } from './session_label';
  import { savedWithUndo } from './forms/form_frame';
  import type { IpcError } from './result';
  import { pushError } from './toasts';

  let { session, onclose }: { session: SessionRow; onclose: () => void } = $props();

  // A snapshot of what the row held when the sheet opened: Save compares
  // against it, and Undo puts it back.
  const before = untrack(() => ({ name: (session.friendly_name ?? '').trim(), tags: [...(session.tags ?? [])] }));
  let name = $state(before.name);
  let label = $state(labelText(before.tags));
  /** Check on blur (G1.2): the label's problem shows once the person left
   *  the field, or pressed Save. */
  let labelChecked = $state(false);
  let busy = $state(false);
  let error = $state<string | IpcError | null>(null);

  const nameBlocked = $derived(
    hubActionBlocked('set_friendly_name', $hubStatus, $hubConnection) ?? $sessionBlocked(session, 'set_friendly_name'),
  );
  const labelBlocked = $derived(
    hubActionBlocked('set_session_tags', $hubStatus, $hubConnection) ?? $sessionBlocked(session, 'set_session_tags'),
  );
  const parsed = $derived(parseLabel(label));
  const nameChanged = $derived(name.trim() !== before.name);
  const tagsChanged = $derived(!sameTags(parsed.tags, before.tags));
  const dirty = $derived(nameChanged || tagsChanged);
  const why = $derived(
    parsed.error !== null
      ? 'Fix the label first.'
      : !dirty
        ? 'Nothing changed.'
        : nameChanged && nameBlocked !== null
          ? nameBlocked
          : tagsChanged && labelBlocked !== null
            ? labelBlocked
            : null,
  );

  async function write(n: string | null, tags: readonly string[] | null): Promise<IpcError | null> {
    // Asked again at the write: Undo runs later, after a grant may have
    // narrowed (multi-user M1).
    if (n !== null && nameBlocked !== null) return { code: 'E_FORBIDDEN', message: nameBlocked };
    if (tags !== null && labelBlocked !== null) return { code: 'E_FORBIDDEN', message: labelBlocked };
    if (n !== null) {
      const r = await setFriendlyName(session.host_alias, session.tmux_name, n);
      if (!r.ok) return r.error;
    }
    if (tags !== null) {
      const r = await setSessionTags(session.id, tags);
      if (!r.ok) return r.error;
    }
    return null;
  }

  async function save() {
    labelChecked = true;
    if (why !== null || busy) return;
    busy = true;
    error = null;
    const n = nameChanged ? name.trim() : null;
    const tags = tagsChanged ? parsed.tags : null;
    const failed = await write(n, tags);
    busy = false;
    if (failed) {
      // The hub's refusal or a bad value: the banner says so, the input stays.
      error = failed;
      return;
    }
    onclose();
    savedWithUndo('Name and label saved', async () => {
      const undone = await write(n !== null ? before.name : null, tags !== null ? before.tags : null);
      if (undone) pushError(undone, 'Undo failed');
    });
  }
</script>

<DialogSheet
  title="Rename and label"
  lead="Shown in every list and on the phone."
  verb="Save"
  busyVerb="Saving…"
  {busy}
  {error}
  {dirty}
  canConfirm={why === null}
  confirmTitle={why}
  onconfirm={() => void save()}
  oninvalid={() => (labelChecked = true)}
  {onclose}
  testid="rename-label-sheet"
  confirmTestid="rename-label-save"
  errorTestid="rename-label-error"
>
  <label class="field">
    <span class="field-label">Name</span>
    <input
      type="text"
      data-testid="rename-label-name"
      bind:value={name}
      placeholder={session.tmux_name}
      disabled={nameBlocked !== null}
      title={nameBlocked ?? ''}
      autocomplete="off"
      spellcheck="false"
    />
    <p class="field-note">Empty shows the tmux name, {session.tmux_name}.</p>
  </label>
  <label class="field">
    <span class="field-label">Label <span class="opt">optional</span></span>
    <input
      type="text"
      data-testid="rename-label-label"
      bind:value={label}
      placeholder="release"
      disabled={labelBlocked !== null}
      title={labelBlocked ?? ''}
      aria-invalid={labelChecked && parsed.error !== null}
      onblur={() => (labelChecked = true)}
      autocomplete="off"
      spellcheck="false"
    />
    {#if labelChecked && parsed.error}
      <p class="field-note err" role="alert" data-testid="rename-label-problem">{parsed.error}</p>
    {:else if labelBlocked}
      <p class="field-note" data-testid="rename-label-blocked">{labelBlocked}</p>
    {:else}
      <p class="field-note">A short word that groups sessions, e.g. "release". Several are separated by spaces.</p>
    {/if}
  </label>
</DialogSheet>

<style>
  .opt {
    color: var(--fg-muted);
    font-style: italic;
    margin-left: 0.3em;
  }
  .err {
    color: var(--danger) !important;
  }
</style>
