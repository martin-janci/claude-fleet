<!--
  The "Drafted" pill (design-system/ai.md): every LLM-written text a person
  reads carries it, beside who wrote it ("by haiku on mercury"). DraftField
  draws it on an editable draft; a read-only draft (the "Since" summary, a
  Summarise result) draws the same pill through this component.
-->
<script lang="ts">
  let { testid = 'drafted-label' }: { testid?: string } = $props();
</script>

<span class="pill" data-testid={testid}>Drafted</span>

<style>
  .pill {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    font-size: var(--text-2xs);
    line-height: 16px;
    font-weight: 500;
    color: var(--fg-2);
    padding: 0 6px;
    border-radius: var(--radius-sm);
    background: var(--chip-bg);
    white-space: nowrap;
  }
  .pill::before {
    content: '\270E';
    font-size: var(--text-2xs);
  }
</style>
