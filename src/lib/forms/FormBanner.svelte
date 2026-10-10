<!-- A form's failure, as a banner at the top of its body (FormsAnatomy:
     "Server errors go in a banner at the top of the body; the person's
     input is kept"). A hub refusal reads "The hub refused this: …" and
     asks an admin; the fields under it keep what was typed. -->
<script lang="ts">
  import Banner from '../kit/Banner.svelte';
  import type { IpcError } from '../result';
  import { formFailure } from './form_frame';

  let { error, testid }: { error: string | IpcError | null | undefined; testid?: string } = $props();
  const failure = $derived(formFailure(error));
</script>

{#if failure}
  <div class="form-banner" data-kind={failure.kind} data-testid={testid}>
    <Banner tone="failed" headline={failure.headline} meta={failure.meta ?? undefined} />
  </div>
{/if}

<style>
  .form-banner :global(.of-banner) {
    border-radius: var(--radius-sm);
  }
</style>
