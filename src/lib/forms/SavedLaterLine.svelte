<script lang="ts">
  // What a form folds to after "Save and finish later" (fleet.form/1's
  // `save_later`): one line with its title and Resume. The answers stay on
  // this device (saved_answers.ts); Resume opens the form again from them.
  // One piece for an agent's form (FormCard) and the app's (ChatForm).
  let {
    title,
    meta = 'saved to finish later',
    onresume,
  }: {
    title: string;
    /** After the title: what was kept, and when the form expires if it does. */
    meta?: string;
    onresume: () => void;
  } = $props();
</script>

<div class="later" data-testid="form-saved-later" role="status">
  <span class="mark" aria-hidden="true">◷</span>
  <strong>{title}</strong>
  <span class="meta">{meta}</span>
  <button type="button" class="link" data-testid="form-resume" onclick={onresume}>Resume</button>
</div>

<style>
  .later { display: flex; gap: 0.5rem; align-items: baseline; padding: 0.55rem 0.8rem; border: 1px solid var(--border); border-radius: var(--radius-md); font-size: var(--text-2xs); }
  strong { flex: 1; }
  .mark, .meta { color: var(--fg-muted); }
  .link { min-block-size: var(--control-h); background: none; border: none; padding: 0; color: var(--accent); cursor: pointer; font: inherit; font-size: var(--text-2xs); }
  .link:hover { text-decoration: underline; }
</style>
