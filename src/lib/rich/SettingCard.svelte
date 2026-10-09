<script lang="ts" module>
  // What a person did on a settings card, by proposal id, so a re-drawn
  // transcript keeps it. Per window, never saved: "Not now" leaves the
  // proposal waiting in Settings › Proposed changes, it only folds this card.
  const outcome = new Map<number, 'applied' | 'later'>();
</script>

<script lang="ts">
  // A `setting` block (docs/chat-blocks.md, step 10.3): an agent proposed a
  // settings change (`set_setting { propose: true }`) and a person decides
  // it here. The key and both values come from the proposal in the store,
  // never from the block's text. Apply always asks first, then decides the
  // one proposal through `decide_setting_proposals`, the same write and
  // guard Settings › Proposed changes uses; Not now writes nothing.
  import { onMount } from 'svelte';
  import ConfirmDialog from '../ConfirmDialog.svelte';
  import { openSettingsAt } from '../app_views';
  import { push } from '../toasts';
  import { descriptors, homeOf, loadDescriptors, loadPages, pagesBundle } from '../pages/pages';
  import {
    decideProposals,
    loadProposals,
    settingProposals,
    settingsWritable,
    valueInWords,
    whoWords,
  } from '../pages/review';
  import type { UiBlock } from '../rich_blocks';

  let { block }: { block: Extract<UiBlock, { kind: 'setting' }> } = $props();

  let loaded = $state(false);
  let busy = $state(false);
  let confirming = $state(false);
  let failure = $state<string | null>(null);
  let done = $state<'applied' | 'later' | null>(null);
  /** The proposal as it was when this card applied it, for the receipt. */
  let applied = $state<{ key: string; value: string } | null>(null);

  $effect(() => {
    done = outcome.get(block.proposal) ?? null;
  });

  onMount(() => {
    const waits: Promise<unknown>[] = [loadProposals()];
    if ($descriptors.size === 0) waits.push(loadDescriptors());
    if ($pagesBundle.pages.length === 0) waits.push(loadPages());
    void Promise.all(waits).then(() => (loaded = true));
  });

  const p = $derived($settingProposals.find((x) => x.id === block.proposal) ?? null);
  const key = $derived(p?.key ?? applied?.key ?? null);
  const d = $derived(key ? $descriptors.get(key) : undefined);
  const home = $derived(key ? homeOf($pagesBundle.pages, key) : null);
  const pageTitle = $derived(home ? ($pagesBundle.pages.find((pg) => pg.id === home.page)?.title ?? home.page) : 'Settings');
  const moved = $derived(p !== null && p.current !== p.before);

  /** What Apply asks: the setting's own warning when it has one. */
  const question = $derived.by(() => {
    if (!p) return '';
    const change = `${d?.label ?? p.key}: ${valueInWords(d, p.current)} → ${valueInWords(d, p.value)}.`;
    return d?.danger.level === 'confirm' ? `${change} ${d.danger.message}` : change;
  });

  function settle(o: 'applied' | 'later') {
    outcome.set(block.proposal, o);
    done = o;
  }

  async function apply() {
    confirming = false;
    if (!p || busy) return;
    busy = true;
    failure = null;
    const was = { key: p.key, value: p.value };
    const r = await decideProposals([p.id], []);
    busy = false;
    if (!r.ok) {
      failure = r.error.message;
      return;
    }
    const failed = r.value.failed.find((f) => f.id === block.proposal);
    if (failed) {
      failure = failed.error;
      return;
    }
    applied = was;
    settle('applied');
    push({ kind: 'success', message: `Applied: ${d?.label ?? was.key}` });
  }

  function openInSettings() {
    if (home && key) openSettingsAt(home.page, key);
    else openSettingsAt('settings.review');
  }
</script>

<section class="card" data-testid="rich-setting" aria-label="Settings change">
  <header>
    <span class="crumb">{pageTitle} › <code>{key ?? `proposal ${block.proposal}`}</code></span>
    <span class="tag">setting</span>
  </header>

  {#if done === 'applied'}
    <p class="receipt" data-testid="rich-setting-applied">
      Applied{#if applied}: {d?.label ?? applied.key} is now {valueInWords(d, applied.value)}{/if}.
      <button type="button" class="link" data-testid="rich-setting-open" onclick={openInSettings}>Undo in Settings</button>
    </p>
  {:else if p}
    <div class="diff" data-testid="rich-setting-diff">
      <span class="label">{d?.label ?? p.key}</span>
      <span class="minus">− {valueInWords(d, p.current)}</span>
      <span class="plus">+ {valueInWords(d, p.value)}</span>
    </div>
    {#if block.note}<p class="note">{block.note}</p>{/if}
    {#if moved}
      <p class="warn" data-testid="rich-setting-moved">Changed since it was proposed (it was {valueInWords(d, p.before)}).</p>
    {/if}
    {#if p.why}<p class="why">“{p.why}”</p>{/if}
    <p class="meta">✦ suggested by {whoWords(p.source, p.source_detail)}</p>
    {#if failure}<p class="warn" role="alert" data-testid="rich-setting-failure">{failure}</p>{/if}
    {#if done === 'later'}
      <p class="meta" data-testid="rich-setting-later">
        Not applied. It still waits in
        <button type="button" class="link" onclick={() => openSettingsAt('settings.review')}>Settings › Proposed changes</button>.
      </p>
    {:else if !$settingsWritable}
      <p class="meta" data-testid="rich-setting-readonly">This device cannot change the fleet's settings; the hub's operator decides it.</p>
    {:else}
      <div class="actions">
        <button
          type="button"
          class="btn btn--primary"
          disabled={busy}
          data-testid="rich-setting-apply"
          onclick={() => (confirming = true)}>Apply…</button>
        <button type="button" class="btn" disabled={busy} data-testid="rich-setting-later-btn" onclick={() => settle('later')}
          >Not now</button>
        <span class="meta">undo in Settings</span>
      </div>
    {/if}
  {:else if loaded}
    <p class="meta" data-testid="rich-setting-gone">
      This change no longer waits for review: it was applied or rejected.
      <button type="button" class="link" onclick={() => openSettingsAt('settings.review')}>Open Settings › Proposed changes</button>
    </p>
  {:else}
    <p class="meta" data-testid="rich-setting-loading">Reading the proposal…</p>
  {/if}
</section>

{#if confirming && p}
  <ConfirmDialog
    title="Apply this settings change?"
    message={question}
    confirmLabel="Apply"
    danger={d?.danger.level === 'confirm'}
    confirmTestId="rich-setting-confirm"
    onconfirm={() => void apply()}
    oncancel={() => (confirming = false)} />
{/if}

<style>
  .card {
    display: flex;
    flex-direction: column;
    gap: 0.4rem;
    margin: 0.4em 0 0.7em;
    padding: 0.65rem 0.8rem;
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    background: var(--bg-pane);
  }
  header {
    display: flex;
    justify-content: space-between;
    align-items: baseline;
    gap: 0.5rem;
    font-size: var(--text-2xs);
    color: var(--fg-muted);
  }
  .tag {
    font-size: var(--text-2xs);
    text-transform: uppercase;
    letter-spacing: 0.06em;
  }
  .diff {
    display: flex;
    flex-direction: column;
    gap: 0.1rem;
    padding: 0.4rem 0.55rem;
    border-radius: var(--radius-sm);
    background: var(--bg);
    font-family: var(--font-mono);
    font-size: var(--text-2xs);
  }
  .label {
    font-family: inherit;
    font-weight: 600;
    color: var(--fg);
  }
  .minus { color: var(--usage-crit); }
  .plus { color: var(--usage-ok); }
  .note, .why, .receipt {
    margin: 0;
    font-size: var(--text-xs);
  }
  .why { color: var(--fg-muted); font-style: italic; }
  .warn {
    margin: 0;
    font-size: var(--text-2xs);
    color: var(--usage-warn);
  }
  .meta {
    margin: 0;
    font-size: var(--text-2xs);
    color: var(--fg-muted);
  }
  .actions {
    display: flex;
    align-items: center;
    gap: 0.5rem;
  }
  .link {
    padding: 0;
    border: none;
    background: none;
    color: var(--accent);
    font: inherit;
    cursor: pointer;
    text-decoration: underline;
  }
</style>
