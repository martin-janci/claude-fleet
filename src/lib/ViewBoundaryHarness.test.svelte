<script lang="ts">
  // Test harness for ViewBoundary.test.ts: renders a child that throws
  // while `fail.throwing` is true.
  import ViewBoundary from './ViewBoundary.svelte';
  import { fail } from './view_boundary_harness.test-state';

  function content(): string {
    if (fail.throwing) throw new Error('boom in render');
    return 'hosts content';
  }
</script>

<ViewBoundary name="Hosts">
  <p data-testid="child">{content()}</p>
</ViewBoundary>
