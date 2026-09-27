<script lang="ts">
  // Session detail's *Tasks* section (work graph M14.2, read only): every
  // link of the session — primary ★, secondary, suggested, past, rejected —
  // from `work { session_tasks }`, and *Show in Work view*. Make primary,
  // Remove and Add task… are M14.3. Re-read only when the row's links move
  // (`linkSignature`), never on every row update. An older hub (no Work
  // view) shows nothing here; the work chip above still works.
  import type { SessionRow } from './sessions';
  import { linkSignature, occurrenceKind, showInWorkView } from './work_tree';
  import { needsNewerHub, workSessionTasks, type SessionTasks } from './work_view';

  let { session }: { session: SessionRow } = $props();

  const key = $derived(`${session.id}|${linkSignature(session)}`);
  let data = $state<SessionTasks | null>(null);
  let error = $state<string | null>(null);
  let hidden = $state(false);
  let seq = 0;

  $effect(() => {
    const k = key;
    const id = Number(k.split('|')[0]);
    const mine = ++seq;
    void workSessionTasks(id).then((r) => {
      if (mine !== seq) return;
      if (r.ok) {
        data = r.value && Array.isArray(r.value.links) ? r.value : null;
        error = null;
        hidden = false;
      } else if (needsNewerHub(r.error)) {
        hidden = true;
      } else {
        error = r.error.message;
      }
    });
  });

  const LABEL: Record<string, string> = {
    primary: 'primary',
    secondary: 'secondary',
    suggested: 'suggested',
    past: 'past',
    rejected: 'rejected',
  };
  const ORDER: Record<string, number> = { primary: 0, secondary: 1, suggested: 2, past: 3, rejected: 4 };
  const links = $derived(
    (data?.links ?? []).slice().sort((a, b) => ORDER[occurrenceKind(a)] - ORDER[occurrenceKind(b)]),
  );
</script>

{#if !hidden && (links.length > 0 || error)}
  <section class="tasks" data-testid="session-tasks">
    <h3>Tasks ({links.length})</h3>
    {#if error}
      <p class="err">Could not load tasks — {error}</p>
    {/if}
    <ul>
      {#each links as l (l.link_id)}
        {@const kind = occurrenceKind(l)}
        <li class="link {kind}" data-testid="session-task" data-kind={kind}>
          <span class="mark">{kind === 'primary' ? '★' : kind === 'suggested' ? '?' : '·'}</span>
          <span class="what">
            {#if l.task.key}<span class="key" class:unavailable={l.task.unavailable}>{l.task.key}</span>{/if}
            <span class="title" class:unavailable={l.task.unavailable}>{l.task.title || l.task.key || l.task.task_id}</span>
          </span>
          <span class="kind">{LABEL[kind]}</span>
          {#if l.why}<span class="why" title={l.why}>{l.why}</span>{/if}
          <button
            type="button"
            class="show"
            data-testid="show-in-work-view"
            title="Show this task in the Work view"
            onclick={() => showInWorkView(l.task.task_id)}>Show in Work view</button
          >
        </li>
      {/each}
    </ul>
  </section>
{/if}

<style>
  .tasks {
    border-top: 1px solid var(--border);
    padding-top: 0.6rem;
    margin-top: 0.6rem;
  }
  h3 {
    font-size: 0.7rem;
    color: var(--fg-muted);
    text-transform: uppercase;
    letter-spacing: 0.05em;
    margin: 0 0 0.4rem 0;
  }
  ul { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: 0.2rem; }
  .link {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 0.4rem;
    border: 1px solid var(--border);
    border-radius: 4px;
    padding: 0.3rem 0.5rem;
    font-size: 0.8rem;
  }
  .link.suggested { border-style: dashed; }
  .link.past, .link.rejected { opacity: 0.6; }
  .mark { width: 0.8rem; text-align: center; color: var(--fg-muted); }
  .link.primary .mark { color: var(--usage-warn); }
  .what { flex: 1 1 10rem; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .key { font-family: var(--mono); font-size: 0.75rem; color: var(--accent); margin-right: 0.3rem; }
  .unavailable { text-decoration: line-through; color: var(--fg-muted); }
  .kind, .why { font-size: 0.7rem; color: var(--fg-muted); }
  .why { max-width: 14rem; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .show {
    background: transparent;
    border: none;
    color: var(--accent);
    cursor: pointer;
    font-size: 0.72rem;
    padding: 0;
  }
  .err { color: var(--usage-crit); font-size: 0.75rem; margin: 0 0 0.3rem; }
</style>
