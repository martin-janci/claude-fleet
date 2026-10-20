<script lang="ts">
  // A saved routine's notebooks (migration 204; fleet-core
  // `routines::memory`): what its runs read under their prompt and rewrite
  // with a `<routine-memory>` block in their reply, kept by the fleet
  // outside any git repo. One box per notebook it uses, as saved: Save
  // writes it back, Clear empties it. A routine whose memory changed in
  // the form shows them once it is saved.
  import Button from '../kit/Button.svelte';
  import { errorText } from '../error_copy';
  import { routineMemory, setRoutineMemory, type RoutineMemory, type RoutineNotebook } from '../routines';

  let { routineId }: { routineId: number } = $props();

  let mem: RoutineMemory | null = $state(null);
  let error: string | null = $state(null);
  let busy = $state(false);
  /** The text being edited per notebook; absent = as saved. */
  let edits: Partial<Record<RoutineNotebook, string>> = $state({});

  async function load(id: number) {
    error = null;
    const r = await routineMemory(id);
    if (r.ok) {
      mem = r.value;
      edits = {};
    } else {
      mem = null;
      error = errorText(r.error);
    }
  }

  $effect(() => {
    void load(routineId);
  });

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

  function byWords(id: RoutineNotebook): string {
    const row = saved(id);
    if (!row) return 'Nothing written yet.';
    const who = row.updated_by === 'run' ? `run ${row.updated_run_id ?? ''}`.trim() : row.updated_by === 'fleet' ? 'the fleet' : 'a person';
    return `Last written by ${who}, ${new Date(row.updated_at * 1000).toLocaleString()}.`;
  }

  async function write(id: RoutineNotebook, text: string) {
    busy = true;
    error = null;
    const r = await setRoutineMemory(routineId, id, text);
    busy = false;
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
  {#each notebooks as nb (nb.id)}
    {@const text = textOf(nb.id)}
    {@const over = mem ? text.length > mem.max_chars : false}
    <label class="f">
      <span class="fl">{nb.label}<span class="opt-l">{nb.help}</span></span>
      <textarea
        class="inp area"
        data-testid={`routine-memory-${nb.id}`}
        value={text}
        oninput={(e) => (edits = { ...edits, [nb.id]: (e.currentTarget as HTMLTextAreaElement).value })}
        readonly={!mem?.may_change}
        rows="5"
        placeholder="Empty: the next run starts with no notes."
        aria-invalid={over ? 'true' : undefined}></textarea>
      <span class="help"
        >{byWords(nb.id)}
        {#if mem}{text.length} / {mem.max_chars} characters{/if}</span
      >
    </label>
    {#if mem?.may_change}
      <div class="row">
        <Button
          size="sm"
          testid={`routine-memory-save-${nb.id}`}
          disabled={busy || over || edits[nb.id] === undefined}
          onclick={() => write(nb.id, textOf(nb.id))}>Save notebook</Button
        >
        <Button
          size="sm"
          variant="quiet"
          testid={`routine-memory-clear-${nb.id}`}
          disabled={busy || !saved(nb.id)?.body}
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
  .inp:focus { outline: none; border-color: var(--accent); box-shadow: 0 0 0 2px var(--accent-soft); }
  .inp[aria-invalid='true'] { border-color: var(--status-failed); }
  .area { font-family: var(--font-mono); font-size: var(--text-xs); resize: vertical; }
</style>
