<script lang="ts">
  // Redesign step 3.13: a list or a table that is still loading shows the
  // kit's Dot wave beside what it waits for ("Loading…", "Loading 48 rows
  // from 4 hosts"), where the rows will appear, instead of a bare line of
  // text. The words stay: they are what a screen reader and a test read, and
  // they are all there is for the first 400 ms, when the Loader shows nothing.
  import Loader from './Loader.svelte';

  let { text = 'Loading…', testid = 'list-loading' }: { text?: string; testid?: string } = $props();
</script>

<div class="list-loading" data-testid={testid} role="status">
  <Loader name="dot-wave" size={72} stage={false} />
  <span class="text">{text}</span>
</div>

<style>
  .list-loading {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: var(--space-2);
    padding: var(--space-4) var(--space-2);
    color: var(--fg-muted);
    font-size: var(--text-2xs);
  }
</style>
