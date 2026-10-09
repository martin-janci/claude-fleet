<script lang="ts">
  // A hub is configured (`hub.remote_url` is set) but this launch could not
  // use it. The backend owns nothing in that state — no reconcile tick, no
  // usage poll, no control API — and refuses every fleet command, because
  // falling back to standalone would make this app a second brain for the
  // hub's fleet. That is only safe if a person can see it, so it is said at
  // the top of the window rather than in a log.
  import { hubUnavailableWords } from './error_copy';

  let {
    reason,
    hubUrl,
    onsettings,
  }: { reason: string; hubUrl: string | null; onsettings: () => void } = $props();
</script>

<div class="hub-unavailable" role="alert" data-testid="hub-unavailable">
  <p>
    <strong>Not managing any fleet.</strong> This app is set to use the hub
    {#if hubUrl}<code>{hubUrl}</code>{/if}, but cannot. <span data-testid="hub-unavailable-why"
      >{hubUnavailableWords(reason)}.</span
    > Until that is fixed it runs no reconcile tick and no control API, and refuses fleet actions, so that it never
    manages the hub's fleet behind the hub's back.
  </p>
  <!-- Review r13: the backend's reason names settings keys and keychain
       errors; it is kept, under Details. -->
  <details class="details"><summary>Details</summary><code data-testid="hub-unavailable-details">{reason}</code></details>
  <button type="button" onclick={onsettings} data-testid="hub-unavailable-settings"
    >Settings → Hub &amp; sync</button
  >
</div>

<style>
  .hub-unavailable {
    display: flex;
    gap: 0.8rem;
    align-items: center;
    padding: 0.5rem 0.8rem;
    background: var(--failed-soft);
    color: var(--fg);
    font-size: var(--text-xs);
    border-bottom: 1px solid var(--failed-line);
  }
  .hub-unavailable p {
    margin: 0;
    flex: 1;
  }
  .details {
    flex: none;
    max-width: 30%;
    font-size: var(--text-2xs);
  }
  .details summary {
    cursor: pointer;
  }
  .details code {
    display: block;
    overflow-wrap: anywhere;
  }
  .hub-unavailable code {
    font-size: var(--text-2xs);
  }
  .hub-unavailable button {
    flex: none;
    background: transparent;
    color: inherit;
    border: 1px solid currentColor;
    border-radius: var(--radius-sm);
    padding: 0.2rem 0.6rem;
    cursor: pointer;
  }
</style>
