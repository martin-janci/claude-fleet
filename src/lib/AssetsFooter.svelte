<script lang="ts">
  import { untrack } from 'svelte';
  import Badge from './Badge.svelte';
  import CatalogChip from './CatalogChip.svelte';
  import JobChip from './JobChip.svelte';
  import { catalogConfig, lastSyncRun, repoStatusStore, syncProgress, type AssetListing, type RepoStatus } from './assets';
  import { blockedSecretKeys, catalogStatuses, PERSONAL, repoStatusOf, summarizeRun } from './assets_workspace';
  import { fleetSettings, settingBool, SETTING_KEYS } from './fleet_settings';

  /** The workspace footer (spec, Footer): a chip per catalog, `auto`, and
   *  either the work in progress (`JobChip`) or the last sync (R10, R24, R25).
   *  The last sync is what the hub answers: the latest a person made, else
   *  the latest SB6 made, marked `auto`. */
  let {
    readOnly,
    listing,
    busy,
    onpull,
    oncommit,
    onpush,
  }: {
    readOnly: boolean;
    listing: AssetListing | null;
    busy: string;
    onpull: () => void;
    oncommit: () => void;
    onpush: () => void;
  } = $props();

  type Chip = {
    name: string; head: string | null; state: 'loaded' | 'problem' | 'not_loaded'; problem: string | null;
    path: string | null; remote: string | null;
  };
  const chips = $derived.by((): Chip[] => {
    const rows = $catalogStatuses;
    if (rows && rows.length) {
      return rows.map((c) => ({
        name: c.name, head: c.head_commit, state: c.state, problem: c.problem ?? null, path: c.repo_path, remote: c.remote_url,
      }));
    }
    const head = $repoStatusStore?.head ?? $catalogConfig?.head_commit ?? listing?.head ?? null;
    return [{
      name: PERSONAL, head, state: 'loaded', problem: null,
      path: $catalogConfig?.repo_path ?? null, remote: $catalogConfig?.remote_url ?? null,
    }];
  });

  // R24: an org catalog's dirty/ahead, read again with each listing the
  // panel loads (mount, pull, push, every write) — without re-creating the
  // chips, so an open popover stays open; refused (no grant) leaves the
  // chip at its HEAD. A late answer of an older read never wins.
  let orgRepo = $state<Record<string, RepoStatus | null>>({});
  const orgNames = $derived(chips.filter((c) => c.name !== PERSONAL).map((c) => c.name).join('\n'));
  let readSeq = 0;
  $effect(() => {
    void listing;
    const names = orgNames ? orgNames.split('\n') : [];
    if (readOnly || names.length === 0) return;
    const seq = ++readSeq;
    untrack(() => {
      for (const name of names) {
        void repoStatusOf(name).then((r) => {
          if (r.ok && seq === readSeq) orgRepo[name] = r.value;
        });
      }
    });
  });

  const JOB: Record<string, string> = {
    scan: 'Scanning hosts', plan: 'Planning a sync', apply: 'Syncing', pull: 'Pulling', commit: 'Committing', push: 'Pushing',
  };
  // The persistent live region (R16): it is always in the DOM, so a screen
  // reader hears its text change. It names the job and its progress, says
  // "Finished." when the work ends (neutral: a failure is announced by its own alert), and clears when the next job starts.
  const working = $derived(busy !== '' && !!JOB[busy]);
  const progressText = $derived(
    busy === 'apply' && $syncProgress && $syncProgress.total > 0 ? `Syncing ${$syncProgress.done} of ${$syncProgress.total} hosts…` : null,
  );
  let finished = $state(false);
  let wasWorking = untrack(() => working);
  $effect(() => {
    const on = working;
    untrack(() => {
      if (on) finished = false;
      else if (wasWorking) finished = true;
      wasWorking = on;
    });
  });
  const liveText = $derived(working ? (progressText ?? `${JOB[busy]}…`) : finished ? 'Finished.' : '');
  const auto = $derived(settingBool($fleetSettings, SETTING_KEYS.catalogAuto));
  const blockedCount = $derived(blockedSecretKeys($lastSyncRun).length);
</script>

<footer class="foot" data-testid="assets-footer">
  <span class="sr-only" role="status" aria-live="polite" data-testid="assets-live">{liveText}</span>
  {#each chips as c (c.name)}
    <CatalogChip
      name={c.name}
      head={c.head}
      state={c.state}
      problem={c.problem}
      path={c.path}
      remote={c.remote}
      repo={c.name === PERSONAL ? (readOnly ? null : $repoStatusStore) : (orgRepo[c.name] ?? null)}
      writable={!readOnly && c.name === PERSONAL}
      busy={busy !== ''}
      {onpull}
      {oncommit}
      {onpush}
    />
  {/each}
  <Badge
    tone={auto ? 'ok' : 'muted'}
    label={`auto: ${auto ? 'on' : 'off'}`}
    title="catalog.auto — Settings → Automation → Assets: hide internals, prepare cards, sync additively on rolled-out layers"
    testid="assets-auto"
  />
  <span class="grow"></span>
  {#if busy && JOB[busy]}
    <JobChip
      label={JOB[busy]}
      done={busy === 'apply' ? ($syncProgress?.done ?? null) : null}
      total={busy === 'apply' ? ($syncProgress?.total ?? null) : null}
      transfer={busy === 'apply'}
    />
  {:else if $lastSyncRun}
    <span class="last" data-testid="assets-last-sync">
      {summarizeRun($lastSyncRun)}
      {#if $lastSyncRun.auto}<Badge tone="muted" label="auto" title="SB6 ran this sync, not a person" />{/if}
      {#if blockedCount > 0}
        <Badge tone="warn" label={`${blockedCount} blocked on a secret`} title="The last sync could not apply these for want of a secret" testid="assets-blocked" />
      {/if}
    </span>
  {/if}
</footer>

<style>
  .foot {
    display: flex; align-items: center; gap: 10px; min-height: 26px; padding: 0 12px;
    border-top: 1px solid var(--border); background: var(--bg-pane); color: var(--fg-muted); font-size: var(--text-2xs);
  }
  .grow { flex: 1; }
  .sr-only { position: absolute; width: 1px; height: 1px; padding: 0; margin: -1px; border: 0; overflow: hidden; clip-path: inset(50%); white-space: nowrap; }
  .last { display: inline-flex; align-items: center; gap: 6px; white-space: nowrap; }
</style>
