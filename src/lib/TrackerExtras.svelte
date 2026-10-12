<script lang="ts">
  // One tracker's extras on the generated Trackers page (declarative pages
  // P4b, a registered custom item): its last sync pass (work graph M11.4),
  // Asana's section map (M6), and Jev's section proposals (J3, assist). Only
  // for the process that owns the fleet; a paired desktop never mounts it.
  // The proposals move to the review_apply layout in P5, and the section map
  // to a choice-map field.
  import { onMount } from 'svelte';
  import {
    categoryLabel,
    decideStatusMapProposal,
    describeSyncMetrics,
    formatConfidence,
    loadTrackers,
    pendingProposals,
    proposalWhy,
    sectionMapRows,
    SECTION_CATEGORIES,
    shadowAgreement,
    statusMapProposals,
    trackerSyncMetrics,
    updateTracker,
    type ProposalAction,
    type SectionProposal,
    type SyncMetrics,
    type TrackerProposals,
    type TrackerRow,
  } from './trackers';
  import { push, pushError } from './toasts';

  let { tracker, onchanged = () => {} }: { tracker: TrackerRow; onchanged?: () => void } = $props();

  let metrics = $state<SyncMetrics | null>(null);
  let proposals = $state<TrackerProposals | null>(null);
  let deciding = $state<number | null>(null);
  let sectionEdits = $state<Record<string, string>>({});
  /** The confirmed map, opened again from "Column map" (M15 G7.14). */
  let editingMap = $state(false);

  const pass = $derived(describeSyncMetrics(metrics));
  const rows = $derived(sectionMapRows(tracker));
  const pending = $derived(pendingProposals(proposals));
  const mapped = $derived(rows.filter((r) => r.confirmed).length);
  const agreement = $derived(shadowAgreement(proposals));

  /** Quiet when this process cannot say (an older build has no such command). */
  async function loadMetrics() {
    const r = await trackerSyncMetrics();
    if (r.ok && Array.isArray(r.value)) metrics = r.value.find((m) => m.tracker_id === tracker.id) ?? null;
  }

  async function loadProposals() {
    if (tracker.provider !== 'asana') return;
    const r = await statusMapProposals(tracker.id);
    if (r.ok && Array.isArray(r.value)) proposals = r.value.find((p) => p.tracker_id === tracker.id) ?? null;
  }

  onMount(() => {
    void loadMetrics();
    void loadProposals();
  });

  const sectionValue = (section: string, fallback: string) => sectionEdits[section] ?? fallback;

  async function confirmSections() {
    const map: Record<string, string> = {};
    for (const r of rows) map[r.section] = sectionValue(r.section, r.category);
    const u = await updateTracker(tracker.id, {
      settings: { ...(tracker.settings ?? {}), section_map: map, section_map_confirmed: true },
    });
    if (!u.ok) {
      pushError(u.error, 'Saving the section map failed');
      return;
    }
    sectionEdits = {};
    editingMap = false;
    await loadTrackers();
    onchanged();
    push({ kind: 'success', message: `${tracker.name}: statuses follow your section map from the next sync.` });
  }

  async function decide(p: SectionProposal, action: ProposalAction, category?: string) {
    if (deciding !== null) return;
    deciding = p.run_id;
    const r = await decideStatusMapProposal(p.run_id, action, category);
    deciding = null;
    if (!r.ok) pushError(r.error, 'Deciding the proposal failed');
    else if (action !== 'reject')
      push({
        kind: 'success',
        message: `${tracker.name}: “${p.section}” is ${categoryLabel(r.value.category)} in your section map from the next sync.`,
      });
    await loadTrackers();
    await loadProposals();
    onchanged();
  }
</script>

<div class="extras" data-testid="tracker-extras">
  {#if pass}
    <p class="hint" data-testid="tracker-metrics">
      {pass}{#if metrics?.last_error}<span class="err" data-testid="tracker-metrics-error"> · {metrics.last_error}</span
        >{/if}{#if (metrics?.items_failed ?? 0) > 0 && metrics?.last_item_error}<span
          class="err"
          data-testid="tracker-metrics-skipped"
          title="The sync skips an item it cannot store and retries it every pass; the hub's log names the view."
          > · skipped: {metrics.last_item_error}</span
        >{/if}
    </p>
  {:else}
    <p class="hint">No sync pass since this app started.</p>
  {/if}

  {#if tracker.provider === 'asana' && rows.length > 0 && tracker.settings?.section_map_confirmed && !editingMap}
    <p class="hint" data-testid="tracker-column-map">
      {mapped}
      {mapped === 1 ? 'column' : 'columns'} mapped ·
      <button type="button" class="btn btn--quiet" data-testid="tracker-column-map-open" onclick={() => (editingMap = true)}
        >Column map</button>
    </p>
  {/if}

  {#if tracker.provider === 'asana' && rows.length > 0 && (!tracker.settings?.section_map_confirmed || editingMap)}
    <div class="sections" data-testid="asana-sections">
      <span class="hint">Which Asana sections mean <em>in progress</em>? A completed task is always done.</span>
      {#each rows as r (r.section)}
        <label class="section-row">
          <span>{r.section}</span>
          <select
            data-testid="asana-section-{r.section}"
            value={sectionValue(r.section, r.category)}
            onchange={(e) => (sectionEdits = { ...sectionEdits, [r.section]: (e.currentTarget as HTMLSelectElement).value })}>
            <option value="todo">to do</option>
            <option value="in_progress">in progress</option>
            <option value="done">done</option>
          </select>
        </label>
      {/each}
      <button class="btn" data-testid="asana-sections-confirm" onclick={() => void confirmSections()}>Confirm</button>
    </div>
  {/if}

  {#if tracker.provider === 'asana' && pending.length > 0}
    <div class="sections" data-testid="jev-proposals">
      <span class="hint"
        >Proposed by Jev (assist) for sections the keyword rule could not classify. Nothing is applied until you
        choose; applying one confirms your section map.</span
      >
      {#if agreement}
        <span class="hint" data-testid="jev-shadow-agreement"
          >In shadow, Jev agreed with the keyword rule on {agreement.agreed} of {agreement.compared} sections the rule
          classified.</span
        >
      {/if}
      {#each pending as p (p.run_id)}
        <div class="section-row" data-testid="jev-proposal">
          <span data-testid="jev-proposal-section">{p.section}</span>
          <span data-testid="jev-proposal-category"
            >→ {categoryLabel(p.answer)}{p.applies_as && p.applies_as !== p.answer
              ? ` (applies as ${categoryLabel(p.applies_as)})`
              : ''}</span
          >
          <span class="conf" data-testid="jev-proposal-confidence" title="confidence">{formatConfidence(p.confidence)}</span>
          {#if proposalWhy(p)}<span class="why" data-testid="jev-proposal-why">why: {proposalWhy(p)}</span>{/if}
          {#if p.applies_as}
            <button class="btn" data-testid="jev-apply" disabled={deciding !== null} onclick={() => void decide(p, 'apply')}
              >Apply</button
            >
          {/if}
          <select
            data-testid="jev-apply-as"
            aria-label="Apply as…"
            disabled={deciding !== null}
            value=""
            onchange={(e) => {
              const sel = e.currentTarget as HTMLSelectElement;
              const c = sel.value;
              sel.value = '';
              if (c) void decide(p, 'apply_as', c);
            }}>
            <option value="">Apply as…</option>
            {#each SECTION_CATEGORIES as c (c)}<option value={c}>{categoryLabel(c)}</option>{/each}
          </select>
          <button class="btn" data-testid="jev-reject" disabled={deciding !== null} onclick={() => void decide(p, 'reject')}
            >Not this</button
          >
        </div>
      {/each}
    </div>
  {/if}
</div>

<style>
  .extras {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
    font-size: var(--text-2xs);
  }
  .hint {
    margin: 0;
    font-size: var(--text-2xs);
    color: var(--fg-muted);
  }
  .err {
    color: var(--usage-crit);
  }
  .sections {
    display: flex;
    flex-direction: column;
    gap: 0.3rem;
  }
  .section-row {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 0.4rem;
  }
  .conf,
  .why {
    color: var(--fg-muted);
    font-size: var(--text-2xs);
  }
</style>
