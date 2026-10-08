<script lang="ts">
  // `pair_device`'s answer (result view `pairing`): the one-time code, the
  // URL a phone opens and its QR, until the code expires. The QR comes as
  // rows of 1 / 0 from the hub, drawn here as squares — no QR library.
  import { onDestroy } from 'svelte';
  import { copyText } from '../clipboard';
  import { qrRects, type Pairing } from '../devices';
  import Loader from '../Loader.svelte';

  let { pairing, onclose }: { pairing: Pairing; onclose: () => void } = $props();

  const started = Date.now();
  let now = $state(Date.now());
  const tick = setInterval(() => (now = Date.now()), 1000);
  onDestroy(() => clearInterval(tick));

  const left = $derived(Math.max(0, pairing.expires_in_s - Math.floor((now - started) / 1000)));
  const size = $derived((pairing.qr[0]?.length ?? 0) + 8);
  const rects = $derived(qrRects(pairing.qr));
  const clock = (s: number) => `${Math.floor(s / 60)}:${String(s % 60).padStart(2, '0')}`;
</script>

<section class="pairing" data-testid="pairing-result" aria-label={`Pairing code for ${pairing.name}`}>
  <header>
    <h5>Pair {pairing.name}</h5>
    <button type="button" class="btn btn--quiet" data-testid="pairing-close" onclick={onclose}>Done</button>
  </header>
  {#if left > 0}
    <p class="lead">
      Scan it with the claude-fleet app on the device, or open the link there. The code works once and expires in
      <strong data-testid="pairing-left">{clock(left)}</strong>; a hub restart voids it.
    </p>
    {#if rects.length}
      <svg
        class="qr"
        viewBox={`0 0 ${size} ${size}`}
        role="img"
        aria-label="Pairing QR code"
        data-testid="pairing-qr"
        shape-rendering="crispEdges">
        <rect width={size} height={size} fill="#fff" />
        {#each rects as r, i (i)}<rect x={r.x} y={r.y} width={r.w} height="1" fill="#000" />{/each}
      </svg>
    {/if}
    <div class="row">
      <code data-testid="pairing-url">{pairing.url}</code>
      <button type="button" class="btn" data-testid="pairing-copy" onclick={() => void copyText(pairing.url)}>Copy link</button>
    </div>
    <p class="meta">
      <!-- 11.12: a Halo round the code while it waits for the device. -->
      <span class="halo" data-testid="pairing-halo"
        ><Loader name="halo" size={24} delay={0} label={`Waiting for ${pairing.name}, ${clock(left)} left`} /></span>
      Code <code data-testid="pairing-code">{pairing.code}</code> · {pairing.mode === 'readonly' ? 'read-only' : 'full'}{pairing.person
        ? ` · ${pairing.person}'s device`
        : ''}
    </p>
  {:else}
    <p class="lead" data-testid="pairing-expired">The code expired. Pair the device again for a new one.</p>
  {/if}
</section>

<style>
  .pairing {
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    padding: 0.6rem 0.75rem;
    margin-bottom: 0.75rem;
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
  }
  header {
    display: flex;
    align-items: center;
    justify-content: space-between;
  }
  h5 {
    margin: 0;
    font-size: 0.95rem;
  }
  .lead,
  .meta {
    margin: 0;
    font-size: 0.8rem;
    color: var(--fg-muted);
    line-height: 1.4;
  }
  .qr {
    width: 12rem;
    height: 12rem;
    align-self: flex-start;
  }
  .row {
    display: flex;
    gap: 0.5rem;
    align-items: center;
    flex-wrap: wrap;
  }
  code {
    font-size: 11px;
    word-break: break-all;
  }
  .meta {
    display: flex;
    align-items: center;
    gap: 0.35rem;
    flex-wrap: wrap;
  }
  .halo {
    display: inline-flex;
  }
</style>
