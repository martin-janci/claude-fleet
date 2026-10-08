<script lang="ts">
  // The `?` sheet (redesign step 3.8): every live chord in the shortcut
  // registry, grouped by where it works, in this platform's spelling. It
  // reads the table, so a chord added there is listed here with no edit.
  // `?` opens it from anywhere outside a text field, the terminal and a
  // dialog; a view with its own `?` (the Hosts legend) keeps it.
  import Modal from './Modal.svelte';
  import { shortcutSheetOpen } from './app_views';
  import {
    SCOPE_TITLES,
    SHORTCUTS,
    bindingsFor,
    formatBinding,
    matchShortcut,
    type Binding,
    type Scope,
    type Shortcut,
  } from './shortcuts';
  import { detectMac, isEditable } from './terminal_keys';

  let { isMac = detectMac(typeof navigator === 'undefined' ? undefined : navigator) }: { isMac?: boolean } =
    $props();

  const open = shortcutSheetOpen;

  /** One row's chords as a label. Global rows show their advertised chord
   *  only (the rest are kept, not advertised); a run of nine reads as a
   *  range. */
  function chordText(s: Shortcut): string {
    const bs: readonly Binding[] = bindingsFor(s, isMac);
    if (bs.length >= 9) return `${formatBinding(bs[0], isMac)}–${formatBinding(bs[8], isMac)}`;
    const shown = s.scope === 'global' ? bs.slice(0, 1) : bs;
    return shown.map((b) => formatBinding(b, isMac)).join(' or ');
  }

  const sections = $derived.by(() => {
    const out: { scope: Scope; title: string; rows: { id: string; action: string; chord: string }[] }[] = [];
    for (const scope of Object.keys(SCOPE_TITLES) as Scope[]) {
      const rows = SHORTCUTS.filter((s) => s.scope === scope && s.status === 'live' && bindingsFor(s, isMac).length > 0)
        .map((s) => ({ id: s.id, action: s.action, chord: chordText(s) }));
      if (rows.length > 0) out.push({ scope, title: SCOPE_TITLES[scope], rows });
    }
    return out;
  });

  function onWindowKeydown(e: KeyboardEvent) {
    if ($open || e.defaultPrevented) return;
    if (matchShortcut('global', e, isMac) !== 'shortcut-sheet') return;
    const target = e.target as HTMLElement | null;
    if (isEditable(target) || target?.dataset?.imeProxy !== undefined || target?.closest?.('dialog')) return;
    e.preventDefault();
    open.set(true);
  }
</script>

<svelte:window onkeydown={onWindowKeydown} />

{#if $open}
  <Modal title="Keyboard shortcuts" onclose={() => open.set(false)} width="560px" testid="shortcut-sheet">
    <div class="sheet">
      {#each sections as sec (sec.scope)}
        <section data-testid="shortcut-section" data-scope={sec.scope}>
          <h4>{sec.title}</h4>
          <dl>
            {#each sec.rows as r (r.id)}
              <div class="row" data-testid="shortcut-row" data-id={r.id}>
                <dt>{r.action}</dt>
                <dd><kbd>{r.chord}</kbd></dd>
              </div>
            {/each}
          </dl>
        </section>
      {/each}
    </div>
  </Modal>
{/if}

<style>
  .sheet {
    display: flex;
    flex-direction: column;
    gap: 0.9rem;
    max-height: 70vh;
    overflow-y: auto;
  }
  h4 {
    margin: 0 0 0.3rem;
    font-size: 11px;
    color: var(--fg-muted);
    font-weight: 600;
  }
  dl { margin: 0; }
  .row {
    display: flex;
    justify-content: space-between;
    gap: 1rem;
    padding: 0.15rem 0;
    font-size: 0.8rem;
  }
  dt { color: var(--fg); }
  dd { margin: 0; flex-shrink: 0; }
  kbd {
    font-family: var(--font-mono);
    font-size: 11px;
    padding: 0.05rem 0.35rem;
    border: 1px solid var(--border);
    border-radius: 4px;
    color: var(--fg-muted);
  }
</style>
