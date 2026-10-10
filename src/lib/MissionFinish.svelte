<!--
  A finished mission (Orbit Fleet G3.7, the Finish board): one summary line,
  Reopen and Archive N sessions, the finish checks, the pull requests, the
  sessions it leaves running with their worktree state, and its waves.
  Archive runs each session's own Clean up (`mission_finish.ts`); Reopen is
  the parent's `mission_state` move back to paused.
-->
<script lang="ts">
  import { onDestroy } from 'svelte';
  import { sessions as sessionRows } from './sessions';
  import { bulkTargets, sessionBlocked } from './share';
  import { sizeText } from './hosts_table';
  import { prChecksLabel, prRef, prStateLabel } from './prs';
  import { push } from './toasts';
  import { checkGlyph, dollars, type MissionDetail } from './missions';
  import type { WorkCheck } from './kill_check';
  import {
    afterArchiveLine,
    archiveLabel,
    archiveMissionSessions,
    archiveOutcomeLine,
    checkByLine,
    checkSessions,
    checkWords,
    finishChecks,
    finishFreedKb,
    finishSessions,
    finishSummary,
    waveSummary,
  } from './mission_finish';

  let {
    detail,
    completed,
    mayChange,
    reopenBlocked = false,
    nameOf = () => null,
    onreopen,
    onarchived,
  }: {
    detail: MissionDetail;
    /** How many missions the Completed group holds once this one is archived. */
    completed: number;
    mayChange: boolean;
    reopenBlocked?: boolean;
    nameOf?: (personId: number) => string | null;
    onreopen: () => Promise<void>;
    onarchived: () => Promise<void>;
  } = $props();

  const live = $derived(finishSessions(detail));
  const checks = $derived(finishChecks(detail));
  const prs = $derived(detail.finish?.prs ?? []);
  const waves = $derived(waveSummary(detail));
  const freed = $derived(finishFreedKb(live));
  const titleOf = $derived(new Map((detail.items ?? []).map((i) => [i.id, i.title] as const)));

  let busy = $state(false);
  let confirming = $state(false);
  let workOf = $state.raw<Map<number, WorkCheck>>(new Map());
  let gone = false;
  onDestroy(() => (gone = true));

  const rowOf = (id: number) => $sessionRows.find((r) => r.id === id);

  // Read each live session's worktree once per set of sessions, as the Kill
  // dialog does: "clean · pushed" or what would be left behind.
  let readKey = '';
  $effect(() => {
    // Only the sessions this person may clean up are read (and archived).
    const rows = bulkTargets(
      live.map((s) => rowOf(s.session_id)).filter((r) => r != null),
      'safe_kill_session',
      $sessionBlocked,
    );
    const key = rows.map((r) => r.id).join(',');
    if (key === readKey) return;
    readKey = key;
    workOf = new Map(rows.map((r) => [r.id, { state: 'checking' } as WorkCheck]));
    if (rows.length === 0) return;
    void checkSessions(rows).then((m) => {
      if (!gone && readKey === key) workOf = m;
    });
  });

  async function reopen() {
    busy = true;
    try {
      await onreopen();
    } finally {
      busy = false;
    }
  }

  async function archive() {
    confirming = false;
    busy = true;
    try {
      // Per target: a session this person may not clean up is left as it was.
      const mine = bulkTargets($sessionRows, 'safe_kill_session', $sessionBlocked);
      const out = await archiveMissionSessions(live, (id) => mine.find((r) => r.id === id), workOf);
      push({ message: archiveOutcomeLine(out), kind: out.removed.length + out.asked.length > 0 ? 'success' : 'info' });
      await onarchived();
    } finally {
      busy = false;
    }
  }
</script>

<section class="finish" data-testid="mission-finish">
  <p class="summary" data-testid="mission-finish-summary">
    {finishSummary(detail)}
    {#if live.length > 0}Archiving ends {live.length === 1 ? 'its pane' : `the ${live.length} panes`} and keeps every transcript; a clean, pushed worktree is removed.{/if}
  </p>

  {#if mayChange}
    <div class="row">
      <button class="btn btn--quiet" type="button" disabled={busy || reopenBlocked} data-testid="mission-reopen" onclick={() => void reopen()}
        >Reopen</button
      >
      {#if live.length > 0}
        <button class="btn btn--primary" type="button" disabled={busy} data-testid="mission-archive" onclick={() => (confirming = true)}
          >{archiveLabel(live.length)}</button
        >
      {/if}
    </div>
    {#if confirming}
      <div class="row confirm" role="alertdialog" aria-label="Confirm" data-testid="mission-archive-confirm-row">
        <span
          >{archiveLabel(live.length)}? Their panes end{#if freed != null}, frees about {sizeText(freed)}{/if}. Work not yet pushed is committed and
          pushed by its agent first.</span
        >
        <button class="btn btn--primary" type="button" disabled={busy} data-testid="mission-archive-confirm" onclick={() => void archive()}
          >Archive</button
        >
        <button class="btn btn--quiet" type="button" data-testid="mission-archive-keep" onclick={() => (confirming = false)}>Keep</button>
      </div>
    {/if}
  {/if}

  {#if checks.checks.length > 0}
    <div class="card" data-testid="mission-finish-checks">
      <h4>Finish checks <span class="count" data-testid="mission-finish-checks-count">{checks.passed} of {checks.checks.length}</span></h4>
      <ul>
        {#each checks.checks as c, i (i)}
          <li>
            <span class="glyph" class:ok={c.state === 'pass'}>{checkGlyph(c.state)}</span>
            <span class="title">{c.line}{#if c.detail} <span class="muted">{c.detail}</span>{/if}</span>
            <span class="muted small">{titleOf.get(c.item_id) ?? ''}{#if checkByLine(c, nameOf)} · {checkByLine(c, nameOf)}{/if}</span>
          </li>
        {/each}
      </ul>
    </div>
  {/if}

  {#if prs.length > 0}
    <div class="card" data-testid="mission-finish-prs">
      <h4>Pull request{prs.length === 1 ? '' : 's'}</h4>
      <ul>
        {#each prs as pr (pr.url)}
          <li data-testid="mission-finish-pr">
            <span class="badge">{prStateLabel(pr)}</span>
            <a href={pr.url} target="_blank" rel="noreferrer noopener">{prRef(pr)}</a>
            {#if pr.title}<span class="title">{pr.title}</span>{/if}
            {#if pr.head_ref}<span class="muted small">{pr.head_ref}</span>{/if}
            {#if prChecksLabel(pr)}<span class="muted small">{prChecksLabel(pr)}</span>{/if}
          </li>
        {/each}
      </ul>
    </div>
  {/if}

  {#if live.length > 0}
    <div class="card" data-testid="mission-finish-sessions">
      <h4>Sessions to archive <span class="muted">transcripts are kept</span></h4>
      <ul>
        {#each live as s (s.session_id)}
          <li data-testid="mission-finish-session">
            <span class="title">{titleOf.get(s.item_id) ?? s.tmux_name}</span>
            <span class="muted small">{s.host_alias}</span>
            <span class="small" data-testid="mission-finish-session-state">{checkWords(workOf.get(s.session_id) ?? (rowOf(s.session_id) ? undefined : { state: 'unknown', why: 'not in your session list' }))}</span>
            {#if s.worktree_kb != null}<span class="muted small">{sizeText(s.worktree_kb)}</span>{/if}
          </li>
        {/each}
      </ul>
      <p class="muted small" data-testid="mission-finish-after">{afterArchiveLine(detail, completed, dollars)}</p>
    </div>
  {/if}

  {#if waves.length > 0}
    <div class="card" data-testid="mission-finish-waves">
      <h4>Waves</h4>
      <ul>
        {#each waves as w (w.wave)}
          <li>
            <span class="glyph" class:ok={w.done}>{w.done ? '✓' : '○'}</span>
            <span>Wave {w.wave} · {w.count} task{w.count === 1 ? '' : 's'}</span>
            <span class="muted small title">{w.titles.join(', ')}</span>
          </li>
        {/each}
      </ul>
    </div>
  {/if}
</section>

<style>
  .finish { display: flex; flex-direction: column; gap: 0.5rem; }
  .summary { margin: 0; }
  .row { display: flex; gap: 0.4rem; align-items: center; flex-wrap: wrap; }
  .confirm { font-size: var(--text-xs); }
  .card { border: 1px solid var(--border); border-radius: var(--radius-md); padding: 0.4rem 0.6rem; }
  h4 { margin: 0 0 0.3rem; font-size: var(--text-2xs); text-transform: uppercase; letter-spacing: 0.04em; color: var(--fg-muted); display: flex; gap: 0.4rem; align-items: baseline; }
  .count { text-transform: none; letter-spacing: 0; }
  ul { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: 0.2rem; }
  li { display: flex; gap: 0.4rem; align-items: baseline; flex-wrap: wrap; }
  .title { flex: 1 1 auto; min-width: 0; overflow-wrap: anywhere; }
  .glyph { width: 1.1rem; text-align: center; flex: 0 0 auto; color: var(--fg-muted); }
  .glyph.ok { color: var(--status-done); }
  .badge { font-size: var(--text-2xs); padding: 0.05rem 0.4rem; border: 1px solid var(--border); border-radius: var(--radius-pill); white-space: nowrap; }
  .muted { color: var(--fg-muted); }
  .small { font-size: var(--text-2xs); }
</style>
