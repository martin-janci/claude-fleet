<script lang="ts">
  // A hub is configured (`hub.remote_url` is set) but this launch could not
  // use it. The backend owns nothing in that state — no reconcile tick, no
  // usage poll, no control API — and refuses every fleet command, because
  // falling back to standalone would make this app a second brain for the
  // hub's fleet. That is only safe if a person can see it, so it is said at
  // the top of the window rather than in a log.
  let {
    reason,
    hubUrl,
    onsettings,
  }: { reason: string; hubUrl: string | null; onsettings: () => void } = $props();
</script>

<div class="hub-unavailable" role="alert" data-testid="hub-unavailable">
  <p>
    <strong>Not managing any fleet.</strong> This app is set to use the hub
    {#if hubUrl}<code>{hubUrl}</code>{/if}, but cannot: {reason}. Until that
    is fixed it runs no reconcile tick and no control API, and refuses fleet
    actions, so that it never manages the hub's fleet behind the hub's back.
  </p>
  <button type="button" onclick={onsettings} data-testid="hub-unavailable-settings"
    >Settings → Hub</button
  >
</div>

<style>
  .hub-unavailable {
    display: flex;
    gap: 0.8rem;
    align-items: center;
    padding: 0.5rem 0.8rem;
    background: #5a1f1a;
    color: #ffd9d2;
    font-size: 0.85rem;
    border-bottom: 1px solid #8a2f24;
  }
  .hub-unavailable p {
    margin: 0;
    flex: 1;
  }
  .hub-unavailable code {
    font-size: 0.8rem;
  }
  .hub-unavailable button {
    flex: none;
    background: transparent;
    color: inherit;
    border: 1px solid currentColor;
    border-radius: 4px;
    padding: 0.2rem 0.6rem;
    cursor: pointer;
  }
</style>
