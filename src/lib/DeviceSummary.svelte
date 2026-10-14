<script lang="ts">
  // One debug device as the chat shows it (`docs/debug-devices.md`): its
  // title and host, its state, what kind of device it is, and who holds or
  // waits for it. Shared by a `debug_devices` call's result (`ToolLine`)
  // and the `device` chat block (`rich/DeviceCard.svelte`); it reads only
  // the row it is given.
  import StatusChip from './kit/StatusChip.svelte';
  import type { OfState } from './kit/status';
  import { claimLeft, deviceSubtitle, holderName, placeName, stateHint, stateLabel, type DebugDevice } from './debug_devices';

  let {
    device,
    nowSecs,
    heading,
    compact = false,
  }: {
    device: DebugDevice;
    nowSecs: number;
    /** Shown over the device's own title (a block's `title`). */
    heading?: string;
    /** One line: title, host, state; for a list. */
    compact?: boolean;
  } = $props();

  function tone(d: DebugDevice): OfState {
    if (d.ready) return 'done';
    if (d.state === 'booting') return 'working';
    if (d.state === 'shutdown') return 'idle';
    return 'waiting';
  }

  const subtitle = $derived(deviceSubtitle(device));
  const left = $derived(claimLeft(device, nowSecs));
  const hint = $derived(stateHint(device));
  const waiting = $derived(device.waiting ?? device.queue?.length ?? 0);
  const queueLine = $derived(
    [device.queue_place !== undefined ? `In line: ${placeName(device.queue_place)}` : null, waiting > 0 ? `${waiting} waiting` : null]
      .filter((x) => x !== null)
      .join(' · '),
  );
</script>

<div class="device" class:compact data-testid="device-summary" data-device={device.id}>
  <div class="top">
    <strong class="title">{heading ?? device.title}</strong>
    {#if heading && heading !== device.title}<span class="own">{device.title}</span>{/if}
    <span class="host">on {device.host}</span>
    <StatusChip state={tone(device)} label={stateLabel(device)} testid="device-state" />
  </div>
  {#if !compact}
    {#if subtitle}<div class="sub">{subtitle}</div>{/if}
    {#if hint}<div class="sub hint">{hint}</div>{/if}
    {#if device.claimed_by}
      <div class="claim" data-testid="device-holder">
        Held by {holderName(device.claimed_by)}<span class="muted"
          >{[left, device.claim_note ? `“${device.claim_note}”` : null].map((x) => (x ? ` · ${x}` : '')).join('')}</span
        >
      </div>
    {/if}
    {#if queueLine}<div class="claim muted" data-testid="device-queue">{queueLine}</div>{/if}
  {/if}
</div>

<style>
  .device {
    display: flex;
    flex-direction: column;
    gap: 0.15rem;
    min-width: 0;
    font-size: var(--text-xs);
  }
  .top {
    display: flex;
    align-items: baseline;
    flex-wrap: wrap;
    gap: 0.25rem 0.45rem;
    min-width: 0;
  }
  .title {
    color: var(--fg);
    font-size: var(--text-sm);
  }
  .compact .title {
    font-size: var(--text-xs);
    font-weight: 600;
  }
  .own,
  .host,
  .sub,
  .muted {
    color: var(--fg-muted);
  }
  .hint {
    font-style: italic;
  }
  .sub,
  .claim {
    font-size: var(--text-2xs);
  }
  .claim {
    color: var(--fg-2);
  }
  .top :global(.of-chip) {
    align-self: center;
  }
</style>
