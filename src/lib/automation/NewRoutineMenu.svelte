<script lang="ts">
  // + New on Automation's head (board Automation): a routine from a template
  // (Morning PR sweep first) or a blank one. It asks the Routines to open
  // the editor (`openRoutines`), so it works from every Automation tab.
  import Button from '../kit/Button.svelte';
  import { TEMPLATES } from '../routines';

  let { onpick }: { onpick: (template: string) => void } = $props();
  let open = $state(false);

  function pick(id: string) {
    open = false;
    onpick(id);
  }
</script>

<svelte:window onkeydown={(e) => open && e.key === 'Escape' && (open = false)} />

<div class="new">
  <Button testid="routine-new" onclick={() => (open = !open)}>+ New</Button>
  {#if open}
    <div class="menu" role="menu" data-testid="routine-new-menu">
      {#each TEMPLATES as t (t.id)}
        <button type="button" role="menuitem" data-testid={`routine-template-${t.id}`} onclick={() => pick(t.id)}>
          <span>{t.label}</span><span class="muted">{t.description}</span>
        </button>
      {/each}
      <button type="button" role="menuitem" data-testid="routine-template-blank" onclick={() => pick('blank')}>
        <span>Blank routine</span><span class="muted">Your own prompt and schedule</span>
      </button>
    </div>
  {/if}
</div>

<style>
  .new { position: relative; }
  .menu { position: absolute; right: 0; top: calc(100% + 4px); z-index: 5; min-width: 260px; display: flex; flex-direction: column; background: var(--bg-pane); border: 1px solid var(--border); border-radius: var(--radius-md); padding: 4px; box-shadow: var(--shadow-pop, 0 8px 24px rgb(0 0 0 / 0.25)); }
  .menu button { display: flex; flex-direction: column; align-items: flex-start; gap: 2px; text-align: left; padding: 6px 8px; border: 0; background: none; color: var(--fg); border-radius: var(--radius-sm); cursor: pointer; font: inherit; }
  .menu button:hover, .menu button:focus-visible { background: var(--bg-hover); }
  .muted { color: var(--fg-muted); font-size: var(--text-xs, 11.5px); }
</style>
