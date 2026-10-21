<script lang="ts">
  // A saved routine's notebooks (migration 204; fleet-core
  // `routines::memory`): what its runs read under their prompt and rewrite
  // with a `<routine-memory>` block in their reply, kept by the fleet
  // outside any git repo. One box per notebook it uses, as saved: Save
  // writes it back, Clear empties it. While the routine form has unsaved
  // changes they stay, read-only, with what is typed in them kept; and an
  // unsaved notebook edit makes leaving the editor ask, like the form.
  import { onDestroy, untrack } from 'svelte';
  import Button from '../kit/Button.svelte';
  import { errorText } from '../error_copy';
  import { routineMemory, setRoutineMemory, type RoutineMemory, type RoutineNotebook } from '../routines';

  let {
    routineId,
    locked = false,
    onpending,
  }: {
    routineId: number;
    /** The routine form has unsaved changes: the boxes stay, with their
     *  edits, but read-only until it is saved or let go. */
    locked?: boolean;
    /** Whether a notebook edit is still unsaved, so leaving the editor asks. */
    onpending?: (pending: boolean) => void;
  } = $props();

  let mem: RoutineMemory | null = $state(null);
  let error: string | null = $state(null);
  let busy = $state(false);
  /** The text being edited per notebook; absent = as saved. */
  let edits: Partial<Record<RoutineNotebook, string>> = $state({});
  /** The newest load or write: an older reply, or one for another routine,
   *  is dropped rather than shown (or saved over) here. */
  let seq = 0;

  /** Read the notebooks; `keepEdits` keeps the unsaved text of the ones the
   *  routine still uses (a reload after the form was saved). */
  async function load(id: number, keepEdits = false) {
    const mine = ++seq;
    error = null;
    const r = await routineMemory(id);
    if (mine !== seq || id !== routineId) return;
    if (r.ok) {
      mem = r.value;
      const m = r.value.memory;
      const uses = (nb: RoutineNotebook) => m === nb || m === 'both';
      edits = keepEdits ? Object.fromEntries(Object.entries(edits).filter(([nb]) => uses(nb as RoutineNotebook))) : {};
    } else {
      mem = null;
      error = errorText(r.error);
    }
  }

  $effect(() => {
    const id = routineId;
    untrack(() => {
      mem = null;
      edits = {};
      void load(id);
    });
  });

  // The form saved (or let go of) its change: the routine may now use other
  // notebooks, so read them again, keeping what is being typed.
  let wasLocked = untrack(() => locked);
  $effect(() => {
    const l = locked;
    if (wasLocked && !l) untrack(() => void load(routineId, true));
    wasLocked = l;
  });

  const pending = $derived(Object.keys(edits).length > 0);
  $effect(() => {
    onpending?.(pending);
  });
  onDestroy(() => onpending?.(false));

  const notebooks = $derived.by((): { id: RoutineNotebook; label: string; help: string }[] => {
    const m = mem?.memory;
    const out: { id: RoutineNotebook; label: string; help: string }[] = [];
    if (m === 'routine' || m === 'both') out.push({ id: 'routine', label: "This routine's notebook", help: 'Only its own runs read it.' });
    if (m === 'project' || m === 'both')
      out.push({ id: 'project', label: "The project's notebook", help: 'Every routine of this project in your organisation reads it.' });
    return out;
  });

  const saved = (id: RoutineNotebook) => (id === 'routine' ? mem?.routine : mem?.project);
  const textOf = (id: RoutineNotebook) => edits[id] ?? saved(id)?.body ?? '';
  /** Characters as the fleet counts them (Rust `chars()`: code points), not
   *  UTF-16 units: an emoji is one, so a notebook the fleet took fits here. */
  const charCount = (text: string) => {
    let n = 0;
    for (const _ of text) n++;
    return n;
  };

  function byWords(id: RoutineNotebook): string {
    const row = saved(id);
    if (!row) return 'Nothing written yet.';
    const who = row.updated_by === 'run' ? `run ${row.updated_run_id ?? ''}`.trim() : row.updated_by === 'fleet' ? 'the fleet' : 'a person';
    return `Last written by ${who}, ${new Date(row.updated_at * 1000).toLocaleString()}.`;
  }

  async function write(id: RoutineNotebook, text: string) {
    const mine = ++seq;
    const routine = routineId;
    busy = true;
    error = null;
    const r = await setRoutineMemory(routine, id, text);
    busy = false;
    if (mine !== seq || routine !== routineId) return;
    if (r.ok) {
      mem = r.value;
      const { [id]: _, ...rest } = edits;
      edits = rest;
    } else {
      error = errorText(r.error);
    }
  }
</script>

<div class="mem" data-testid="routine-memory">
  {#if error}<p class="errt" data-testid="routine-memory-error">{error}</p>{/if}
  {#if locked && notebooks.length && mem?.may_change}
    <p class="help" data-testid="routine-memory-locked">Save or undo the routine's changes to change its notebooks; what you typed here is kept.</p>
  {/if}
  {#each notebooks as nb (nb.id)}
    {@const text = textOf(nb.id)}
    {@const count = charCount(text)}
    {@const over = mem ? count > mem.max_chars : false}
    <label class="f">
      <span class="fl">{nb.label}<span class="opt-l">{nb.help}</span></span>
      <textarea
        class="inp area"
        data-testid={`routine-memory-${nb.id}`}
        value={text}
        oninput={(e) => (edits = { ...edits, [nb.id]: (e.currentTarget as HTMLTextAreaElement).value })}
        readonly={!mem?.may_change || locked}
        rows="5"
        placeholder="Empty: the next run starts with no notes."
        aria-invalid={over ? 'true' : undefined}></textarea>
      <span class="help"
        >{byWords(nb.id)}
        {#if mem}{count} / {mem.max_chars} characters{/if}</span
      >
    </label>
    {#if mem?.may_change}
      <div class="row">
        <Button
          size="sm"
          testid={`routine-memory-save-${nb.id}`}
          disabled={busy || locked || over || edits[nb.id] === undefined}
          onclick={() => write(nb.id, textOf(nb.id))}>Save notebook</Button
        >
        <Button
          size="sm"
          variant="quiet"
          testid={`routine-memory-clear-${nb.id}`}
          disabled={busy || locked || !saved(nb.id)?.body}
          onclick={() => write(nb.id, '')}>Clear</Button
        >
      </div>
    {/if}
  {/each}
</div>

<style>
  .mem { display: flex; flex-direction: column; gap: 8px; }
  .f { display: flex; flex-direction: column; gap: 4px; min-width: 0; font-size: var(--text-xs); line-height: 16px; color: var(--fg-2); }
  .fl { display: flex; align-items: baseline; gap: 6px; }
  .opt-l { color: var(--fg-muted); font-size: var(--text-2xs); }
  .help { margin: 0; font-size: var(--text-xs); line-height: 16px; color: var(--fg-muted); }
  .errt { margin: 0; font-size: var(--text-xs); line-height: 16px; color: var(--status-failed); }
  .row { display: flex; gap: var(--space-2); }
  .inp { box-sizing: border-box; width: 100%; min-width: 0; padding: 6px 8px; border: 1px solid var(--control-border-strong, var(--border)); border-radius: var(--radius-sm); background: var(--bg-pane); color: var(--fg); font: inherit; font-size: var(--text-sm); }
  .inp:focus { border-color: var(--accent); outline: var(--ring-w) solid var(--ring); outline-offset: 1px; }
  .inp[aria-invalid='true'] { border-color: var(--status-failed); }
  .area { font-family: var(--font-mono); font-size: var(--text-xs); resize: vertical; }
</style>
