<script lang="ts">
  // The debug devices a session holds or waits for (docs/debug-devices.md),
  // as a strip under the Conversation header: one line per device, with a
  // screenshot, a mirror and Release for a held one. Read from the person's
  // own device list on mount and every 30 s while the window is visible.
  // Nothing at all when the session holds and waits for nothing, and
  // nothing when the list cannot be read (a hub without the command): it is
  // a glance, never a banner of errors.
  //
  // Leave line is not offered: the desktop asks as `DESKTOP_HOLDER`, so a
  // release from here would leave the person's place, not the session's.
  import { untrack } from 'svelte';
  import Icon from './kit/Icon.svelte';
  import ImageShot from './ImageShot.svelte';
  import ConfirmDialog from './ConfirmDialog.svelte';
  import { errorText } from './error_copy';
  import type { IpcError } from './result';
  import {
    canMirror,
    claimLeft,
    debugDeviceScreenshot,
    devicesOf,
    holderName,
    listDebugDevices,
    placeName,
    releaseDebugDevice,
    sessionHolder,
    startDeviceMirror,
    stateLabel,
    type DebugDevice,
    type Screenshot,
  } from './debug_devices';

  let { session }: { session: { id: number; host_alias: string } } = $props();

  /** How often the list is read again while mounted. */
  const POLL_MS = 30_000;
  /** Above this many lines the strip folds to its count. */
  const FOLD_OVER = 2;
  /** Refusals no retry can change: stop asking. */
  const GIVE_UP = ['E_LOCAL_ONLY', 'E_HUB_CONTRACT', 'E_UNKNOWN_COMMAND'];

  let devices = $state<DebugDevice[]>([]);
  let nowSecs = $state(Date.now() / 1000);
  let stopped = false;
  let open = $state(false);
  /** The device whose screenshot panel is open, and what it holds. */
  let shotFor = $state<number | null>(null);
  let shot = $state<Screenshot | null>(null);
  let busy = $state<{ id: number; what: 'shot' | 'mirror' | 'release' } | null>(null);
  /** An action's failure or note, under that device's line. */
  let said = $state<{ id: number; text: string; error: boolean } | null>(null);
  let releasing = $state<DebugDevice | null>(null);

  const holder = $derived(sessionHolder(session));
  const mine = $derived(devicesOf(devices, holder));
  const count = $derived(mine.held.length + mine.waiting.length);
  const folded = $derived(count > FOLD_OVER && !open);

  const message = (e: IpcError) => e.message?.trim() || errorText(e);

  async function load() {
    if (stopped) return;
    const r = await listDebugDevices();
    nowSecs = Date.now() / 1000;
    if (r.ok) devices = Array.isArray(r.value) ? r.value : [];
    else if (GIVE_UP.includes(r.error.code)) {
      stopped = true;
      devices = [];
    }
    // Any other failure keeps the last list: the next poll tries again.
  }

  // Read on mount and when the strip moves to another session, then poll.
  $effect(() => {
    void holder;
    untrack(() => {
      devices = [];
      stopped = false;
      shotFor = null;
      shot = null;
      said = null;
      open = false;
      void load();
    });
    const timer = setInterval(() => {
      if (typeof document !== 'undefined' && document.hidden) return;
      void load();
    }, POLL_MS);
    return () => clearInterval(timer);
  });

  async function screenshot(d: DebugDevice) {
    shotFor = d.id;
    shot = null;
    busy = { id: d.id, what: 'shot' };
    said = null;
    const r = await debugDeviceScreenshot(d.id);
    busy = null;
    if (shotFor !== d.id) return;
    if (r.ok) shot = r.value;
    else {
      shotFor = null;
      said = { id: d.id, text: message(r.error), error: true };
    }
  }

  async function mirror(d: DebugDevice) {
    busy = { id: d.id, what: 'mirror' };
    said = null;
    const r = await startDeviceMirror(d.id);
    busy = null;
    said = r.ok ? { id: d.id, text: 'Mirroring in a scrcpy window.', error: false } : { id: d.id, text: message(r.error), error: true };
  }

  async function release() {
    const d = releasing;
    if (!d) return;
    busy = { id: d.id, what: 'release' };
    const r = await releaseDebugDevice(d.id);
    busy = null;
    releasing = null;
    if (r.ok) {
      if (shotFor === d.id) shotFor = null;
      await load();
    } else said = { id: d.id, text: message(r.error), error: true };
  }

  const isBusy = (d: DebugDevice, what?: 'shot' | 'mirror' | 'release') =>
    busy !== null && busy.id === d.id && (what === undefined || busy.what === what);
</script>

{#if count > 0}
  <section class="devices" data-testid="session-devices" aria-label="Debug devices of this session">
    {#if count > FOLD_OVER}
      <button type="button" class="fold" aria-expanded={open} data-testid="session-devices-toggle" onclick={() => (open = !open)}>
        <Icon name="hosts" size={12} />
        <span>{['Debug devices', `${mine.held.length} held`, mine.waiting.length ? `${mine.waiting.length} waiting` : null].filter(Boolean).join(' · ')}</span>
        <span class="chev" aria-hidden="true">{open ? '▾' : '▸'}</span>
      </button>
    {/if}
    {#if !folded}
      {#each mine.held as d (d.id)}
        {@const left = claimLeft(d, nowSecs)}
        <div class="line" data-testid="session-device-held" data-device={d.id}>
          <span class="glyph" title={d.platform === 'ios' ? 'iOS' : d.platform === 'android' ? 'Android' : d.platform}
            ><Icon name="hosts" size={12} /></span
          >
          <strong class="title">{d.title}</strong>
          <span class="muted">on {d.host}</span>
          <span class="state" class:ready={d.ready}>{stateLabel(d)}</span>
          <span class="muted">{['held', left, d.claim_note ? `“${d.claim_note}”` : null].filter(Boolean).join(' · ')}</span>
          <span class="acts">
            <button type="button" class="act" disabled={isBusy(d)} data-testid="session-device-screenshot" onclick={() => void screenshot(d)}
              >{isBusy(d, 'shot') ? 'Taking…' : 'Screenshot'}</button
            >
            {#if canMirror(d)}
              <button type="button" class="act" disabled={isBusy(d)} data-testid="session-device-mirror" onclick={() => void mirror(d)}
                >{isBusy(d, 'mirror') ? 'Starting…' : 'Mirror'}</button
              >
            {/if}
            <button type="button" class="act" disabled={isBusy(d)} data-testid="session-device-release" onclick={() => (releasing = d)}>Release…</button>
          </span>
        </div>
        {#if said?.id === d.id}
          <p class="said" class:error={said.error} role={said.error ? 'alert' : 'status'}>{said.text}</p>
        {/if}
        {#if shotFor === d.id && shot}
          <div class="shot" data-testid="session-device-shot">
            <ImageShot mime={shot.mime} data={shot.data} alt={shot.caption} testid="session-device-image" />
            <div class="shot-acts">
              <button type="button" class="act" disabled={isBusy(d)} onclick={() => void screenshot(d)}>Refresh</button>
              <button type="button" class="act" onclick={() => (shotFor = null)}>Close</button>
            </div>
          </div>
        {/if}
      {/each}
      {#each mine.waiting as w (w.device.id)}
        <div class="line waiting" data-testid="session-device-waiting" data-device={w.device.id}>
          <span class="glyph"><Icon name="clock" size={12} /></span>
          <span
            >Waiting for <strong class="title">{w.device.title}</strong> <span class="muted">on {w.device.host}</span>
            {[w.place <= 1 ? '· next in line' : `· ${placeName(w.place)}`, w.device.claimed_by ? `· held by ${holderName(w.device.claimed_by)}` : null]
              .filter(Boolean)
              .join(' ')}</span
          >
        </div>
      {/each}
    {/if}
  </section>
{/if}

{#if releasing}
  <ConfirmDialog
    title="Release {releasing.title}?"
    confirmLabel="Release"
    danger
    busy={busy?.what === 'release'}
    onconfirm={() => void release()}
    oncancel={() => (releasing = null)}
    confirmTestId="session-device-release-confirm"
  >
    This takes the session's claim on {releasing.title} away while it may still be using it.
    {#if releasing.waiting}The next in line gets it.{/if}
  </ConfirmDialog>
{/if}

<style>
  .devices {
    display: flex;
    flex-direction: column;
    gap: 0.15rem;
    padding: 0.25rem var(--space-3);
    border-bottom: 1px solid var(--border);
    background: var(--bg-pane);
    font-size: var(--text-2xs);
  }
  .fold {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    padding: 0;
    border: none;
    background: none;
    color: var(--fg-muted);
    font: inherit;
    cursor: pointer;
    text-align: left;
  }
  .fold:hover {
    color: var(--fg);
  }
  .line {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    min-width: 0;
    white-space: nowrap;
    overflow: hidden;
  }
  .line > span:not(.acts) {
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .glyph {
    display: inline-flex;
    flex: 0 0 auto;
    color: var(--fg-muted);
  }
  .title {
    color: var(--fg);
    font-weight: 600;
  }
  .muted {
    color: var(--fg-muted);
  }
  .state {
    flex: 0 0 auto;
    padding: 0 6px;
    border: 1px solid var(--border);
    border-radius: var(--radius-lg);
    color: var(--fg-muted);
  }
  .state.ready {
    color: var(--status-done);
    border-color: color-mix(in srgb, var(--status-done) 40%, transparent);
  }
  .acts {
    display: inline-flex;
    gap: 0.3rem;
    margin-left: auto;
    flex: 0 0 auto;
  }
  .act {
    padding: 0 0.45rem;
    border: 1px solid var(--control-border);
    border-radius: var(--radius-sm);
    background: var(--control-bg);
    color: var(--control-fg);
    font: inherit;
    line-height: 1.6;
    cursor: pointer;
  }
  .act:hover:not(:disabled) {
    background: var(--control-bg-hover);
  }
  .act:disabled {
    opacity: 0.6;
    cursor: default;
  }
  .waiting {
    color: var(--fg-2);
  }
  .said {
    margin: 0 0 0.1rem 1.2rem;
    color: var(--fg-2);
  }
  .said.error {
    color: var(--status-failed);
  }
  .shot {
    display: flex;
    align-items: flex-start;
    gap: 0.5rem;
    padding: 0.2rem 0 0.3rem 1.2rem;
  }
  .shot-acts {
    display: flex;
    flex-direction: column;
    gap: 0.3rem;
  }
</style>
