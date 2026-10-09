<script lang="ts">
  // "Attach a running session to <task>" (task → session spec §2.2, J3): a
  // filterable list of the running sessions not already on the task — the
  // same repository first, then those with no task, then idle ones. A
  // session already on another task offers Switch (that link ends, this
  // task becomes the primary) or Add (this task is a secondary link); Add is
  // the default while it is mid-turn or when its checkout is named after
  // the other key. A task already open in another session, or one in
  // another organisation, is said first and needs a second, explicit click.
  import { onMount, tick } from 'svelte';
  import { sessions, type SessionRow } from './sessions';
  import { orgs } from './orgs';
  import { hubStatus, hubActionBlocked } from './hub';
  import { hubConnection } from './hub_connection';
  import { sessionBlocked } from './share';
  import { crossOrgOf, crossOrgSentence, liveElsewhereOf, type LiveElsewhere } from './work';
  import { readErrorText } from './work_view';
  import { shortAge } from './session_status';
  import { claudeStatusLabel } from './attention';
  import { attachCandidates, attachSession, defaultMode, otherWork, type AttachMode, type AttachTarget, type Attached } from './attach';
  import type { IpcError } from './result';

  let {
    target,
    heading,
    onclose,
    onattached,
  }: {
    target: AttachTarget;
    heading: string;
    onclose: (refocus: boolean) => void;
    onattached: (a: Attached, how: string) => void;
  } = $props();

  let filter = $state('');
  let picked = $state<number | null>(null);
  let mode = $state<AttachMode>('switch');
  let busy = $state(false);
  let error = $state<string | null>(null);
  /** The second click's reason: P-3 or another organisation. */
  let confirm = $state<{ kind: 'live'; live: LiveElsewhere[] } | { kind: 'cross_org'; sentence: string } | null>(null);
  /** What the person already went ahead past, sticky until another pick:
   *  a task open elsewhere AND in another org takes both, one per click. */
  let acked = false;
  let forced = false;
  let input: HTMLInputElement | undefined = $state();

  const rows = $derived(attachCandidates($sessions, target, filter));
  const row = $derived(rows.find((r) => r.id === picked) ?? null);
  const other = $derived(row ? otherWork(row, target) : null);
  const cmd = $derived(other && mode === 'switch' ? 'switch_session_work' : 'link_session_work');
  const blocked = $derived(row ? (hubActionBlocked(cmd, $hubStatus, $hubConnection) ?? $sessionBlocked(row, cmd)) : null);

  function pick(r: SessionRow) {
    picked = r.id;
    mode = defaultMode(r);
    confirm = null;
    acked = false;
    forced = false;
    error = null;
  }

  const name = (r: SessionRow) => r.friendly_name ?? r.tmux_name;
  const orgName = (id: number) => $orgs.find((o) => o.id === id)?.name;

  async function attach() {
    if (!row || busy || blocked) return;
    busy = true;
    error = null;
    if (confirm?.kind === 'live') acked = true;
    if (confirm?.kind === 'cross_org') forced = true;
    const ackLive = acked;
    const forceCrossOrg = forced;
    // Said from what was picked: the row the write answers is on this task.
    const how = other
      ? mode === 'switch'
        ? `Switched ${name(row)} from ${other.key ?? 'its task'}`
        : `Added to ${name(row)}`
      : `Attached ${name(row)}`;
    const r = await attachSession(row, target, { mode, ackLive, forceCrossOrg });
    busy = false;
    if (r.ok) {
      onattached(r.value, how);
      return;
    }
    const live = liveElsewhereOf(r.error as IpcError);
    if (live && live.length > 0 && !ackLive) {
      confirm = { kind: 'live', live };
      return;
    }
    const c = crossOrgOf(r.error as IpcError);
    if (c && !forceCrossOrg) {
      confirm = { kind: 'cross_org', sentence: crossOrgSentence(target.key ?? 'This task', c, orgName) };
      return;
    }
    error = readErrorText(r.error);
  }

  onMount(async () => {
    await tick();
    input?.focus();
  });
</script>

<div
  class="attach-pop"
  role="dialog"
  tabindex="-1"
  aria-label={heading}
  data-testid="attach-picker"
  onkeydown={(e) => {
    if (e.key === 'Escape') {
      e.stopPropagation();
      onclose(true);
    } else if (e.key === 'Enter' && !(e.target instanceof HTMLButtonElement)) {
      e.preventDefault();
      if (row) void attach();
      else if (rows[0]) pick(rows[0]);
    }
  }}
>
  <h3 class="head">{heading}</h3>
  <input
    type="search"
    placeholder="Filter sessions…"
    aria-label="Filter sessions"
    data-testid="attach-picker-filter"
    bind:this={input}
    value={filter}
    oninput={(e) => (filter = (e.currentTarget as HTMLInputElement).value)}
  />
  {#if rows.length === 0}
    <p class="muted" data-testid="attach-picker-empty">No other running session.</p>
  {:else}
    <ul class="list" role="listbox" aria-label="Running sessions">
      {#each rows.slice(0, 12) as r (r.id)}
        <li>
          <button
            type="button"
            role="option"
            aria-selected={picked === r.id}
            class="opt"
            class:picked={picked === r.id}
            data-testid="attach-picker-row"
            data-session-id={r.id}
            onclick={() => pick(r)}
          >
            <span class="dot" class:working={r.claude_status === 'working'} aria-hidden="true">●</span>
            <span class="name">{name(r)}</span>
            <span class="muted">{r.host_alias}</span>
            <span class="muted status-word"
              >{claudeStatusLabel(r.claude_status === 'working' || !r.idle_since ? r.claude_status : 'idle')}{r.claude_status !== 'working' && r.idle_since
                ? ` · ${shortAge(r.idle_since, Math.floor(Date.now() / 1000))}`
                : ''}</span
            >
            <span class="task">{otherWork(r, target) ? `${otherWork(r, target)?.key ?? otherWork(r, target)?.title} ★` : 'no task'}</span>
          </button>
        </li>
      {/each}
    </ul>
  {/if}

  {#if row && other}
    <fieldset class="mode" data-testid="attach-picker-mode">
      <legend>It is on {other.key ?? other.title}:</legend>
      <label
        ><input type="radio" name="attach-mode" value="switch" data-testid="attach-picker-switch" checked={mode === 'switch'} onchange={() => (mode = 'switch')} />
        Switch to {target.key ?? 'this task'}</label
      >
      <label
        ><input type="radio" name="attach-mode" value="add" data-testid="attach-picker-add" checked={mode === 'add'} onchange={() => (mode = 'add')} />
        Add ({other.key ?? 'that task'} stays primary)</label
      >
    </fieldset>
  {/if}

  {#if confirm}
    <p class="warn" role="alert" data-testid="attach-picker-confirm">
      {confirm.kind === 'live' ? confirm.live.map((l) => l.message).join(' ') : confirm.sentence}
    </p>
  {/if}
  {#if error}<p class="err" role="alert" data-testid="attach-picker-error">{error}</p>{/if}

  <div class="acts">
    {#if blocked}<span class="why">{blocked}</span>{/if}
    <button class="btn btn--quiet" type="button" data-testid="attach-picker-cancel" onclick={() => onclose(true)}>Cancel</button>
    <button
      class="btn btn--primary"
      type="button"
      data-testid="attach-picker-go"
      disabled={!row || busy || blocked !== null}
      onclick={() => void attach()}
      >{busy ? 'Attaching…' : confirm?.kind === 'live' ? 'Attach anyway' : confirm?.kind === 'cross_org' ? 'Attach across orgs' : 'Attach ⏎'}</button
    >
  </div>
</div>

<style>
  .attach-pop {
    display: grid;
    gap: 8px;
    width: min(380px, calc(100vw - 32px));
    padding: 10px 12px;
    border: 1px solid var(--control-border);
    border-radius: var(--radius-md);
    background: var(--bg);
    color: var(--fg);
    box-shadow: var(--shadow-pop);
    font-size: var(--text-xs);
  }
  .attach-pop:focus-visible {
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
  input[type='search'] {
    height: 24px;
    font: inherit;
  }
  .list {
    display: grid;
    gap: 1px;
    max-height: 220px;
    overflow: auto;
    margin: 0;
    padding: 0;
    list-style: none;
  }
  .opt {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr) auto auto auto;
    gap: 6px;
    align-items: baseline;
    width: 100%;
    padding: 3px 6px;
    border: 0;
    border-radius: var(--radius-sm);
    background: none;
    color: var(--fg);
    font: inherit;
    text-align: left;
    cursor: pointer;
  }
  .opt:hover,
  .opt:focus-visible,
  .opt.picked {
    background: color-mix(in srgb, var(--accent) 12%, transparent);
  }
  .name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .dot {
    color: var(--fg-muted);
    font-size: var(--text-2xs);
  }
  .dot.working {
    color: var(--accent);
  }
  .task {
    font-family: var(--mono);
    font-size: var(--text-2xs);
  }
  .muted {
    color: var(--fg-muted);
    margin: 0;
  }
  .mode {
    display: grid;
    gap: 2px;
    margin: 0;
    padding: 4px 6px;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
  }
  .mode legend {
    padding: 0 4px;
    color: var(--fg-muted);
  }
  .mode label {
    display: flex;
    gap: 6px;
    align-items: center;
  }
  .warn {
    margin: 0;
    color: var(--usage-warn);
  }
  .err {
    margin: 0;
    color: var(--usage-crit);
  }
  .acts {
    display: flex;
    gap: 6px;
    justify-content: flex-end;
    align-items: center;
  }
  .why {
    margin-right: auto;
    color: var(--fg-muted);
    font-size: var(--text-2xs);
  }
</style>
