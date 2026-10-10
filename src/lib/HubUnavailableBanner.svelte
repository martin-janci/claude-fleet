<script lang="ts">
  // A hub is configured (`hub.remote_url` is set) but this launch could not
  // use it. The backend owns nothing in that state — no reconcile tick, no
  // usage poll, no control API — and refuses every fleet command, because
  // falling back to standalone would make this app a second brain for the
  // hub's fleet. That is only safe if a person can see it, so it is said at
  // the top of the window rather than in a log, in the kit's failed Banner.
  import { hubUnavailableWords } from './error_copy';
  import Banner from './kit/Banner.svelte';
  import Button from './kit/Button.svelte';

  let {
    reason,
    hubUrl,
    onsettings,
  }: { reason: string; hubUrl: string | null; onsettings: () => void } = $props();
</script>

<div class="strip">
  <Banner tone="failed" headline="Not managing any fleet" testid="hub-unavailable">
    {#snippet evidence()}
      This app is set to use the hub {#if hubUrl}<code>{hubUrl}</code>{/if}, but cannot.
      <span data-testid="hub-unavailable-why">{hubUnavailableWords(reason)}.</span>
      Until that is fixed it runs no reconcile tick and no control API, and refuses fleet actions, so that it never manages
      the hub's fleet behind the hub's back.
      <!-- Review r13: the backend's reason names settings keys and keychain
           errors; it is kept, under Details. -->
      <details class="details"><summary>Details</summary><code data-testid="hub-unavailable-details">{reason}</code></details>
    {/snippet}
    {#snippet action()}
      <Button size="sm" onclick={onsettings} testid="hub-unavailable-settings">Settings → Hub &amp; sync</Button>
    {/snippet}
  </Banner>
</div>

<style>
  .strip {
    padding: var(--space-2) var(--space-3) 0;
  }
  .details summary {
    cursor: pointer;
  }
  .details code {
    display: block;
    overflow-wrap: anywhere;
  }
</style>
